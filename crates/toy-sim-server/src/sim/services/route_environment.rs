use super::*;
use crate::sim::routing::{RouteEnvironment, RouteGate, SlipEstimate};
use anyhow::Context;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Environment {
    source: Arc<FusedScan>,
    gates: Vec<RouteGate>,
    cancel: Arc<AtomicBool>,
    ready_tick: u64,
    wanted: Option<BTreeSet<Id>>,
    eligibility: std::sync::Mutex<std::collections::HashMap<GalacticPosition, bool>>,
}

impl Environment {
    pub(crate) fn capture(
        world: &mut World,
        ship: Entity,
        cancel: Arc<AtomicBool>,
        input: &crate::sim::routing::RouteRequest,
    ) -> Result<Self> {
        let mut source = super::current_fused_source(world, ship)
            .ok_or_else(|| anyhow::anyhow!("route observations unavailable"))?;
        let drive = world.get::<crate::sim::travel::SlipDrive>(ship);
        let ready_tick = drive.map_or(0, |drive| drive.ready_tick);

        let captured = Arc::get_mut(&mut source).expect("new route observation snapshot");
        captured.radius = input.performance.radius_m;
        captured.pose = input.origin.clone();

        // A docked ship may plan a future undock and slip leg. Its installed
        // drive remains known hardware even though it cannot actuate in port.
        if let Some(drive) = drive {
            Arc::get_mut(&mut source)
                .expect("new route observation snapshot")
                .slip_power_w = drive.power_w;
        }
        let wanted = if input
            .orders
            .iter()
            .any(|order| matches!(order, Order::TravelTo(_) | Order::Dock(_)))
        {
            None
        } else {
            let mut ids = BTreeSet::new();
            for order in &input.orders {
                if let Order::Jump(id) = order {
                    ids.insert(*id);
                    if let Some(gate) = source
                        .gates
                        .get(id)
                        .or_else(|| source.orbital.gates.get(id))
                    {
                        ids.insert(gate.exit);
                    }
                }
            }
            Some(ids)
        };

        Ok(Self {
            source,
            gates: Vec::new(),
            cancel,
            ready_tick,
            wanted,
            eligibility: Default::default(),
        })
    }

    pub(crate) fn gate_count(&self) -> usize {
        self.wanted.as_ref().map_or(
            self.source.gates.len() + self.source.orbital.gates.len(),
            BTreeSet::len,
        )
    }

    pub(crate) fn prepare(&mut self) -> Result<()> {
        let source = &self.source;
        let mut gates = BTreeMap::new();
        let records = match &self.wanted {
            Some(ids) => ids
                .iter()
                .filter_map(|id| {
                    source
                        .gates
                        .get(id)
                        .or_else(|| source.orbital.gates.get(id))
                        .map(|gate| (id, gate))
                })
                .collect::<Vec<_>>(),
            None => source
                .orbital
                .gates
                .iter()
                .chain(source.gates.iter())
                .collect(),
        };

        for (&id, gate) in records {
            ensure!(!self.cancelled(), "route computation cancelled");
            let pose = source.orbital_pose(&gate.pose, gate.orbit.as_deref());
            let outward = source
                .pose
                .position
                .relative_to(pose.position)
                .try_normalize()
                .unwrap_or(DVec3::X);
            let staging = pose
                .position
                .offset_by(outward * (gate.exclusion_m + source.radius + 1000.0));
            let beacon = source
                .beacons
                .get(&id)
                .or_else(|| source.orbital.beacons.get(&id))
                .ok_or_else(|| anyhow::anyhow!("public gate beacon unavailable"))?;
            gates.insert(
                id,
                RouteGate {
                    navigation: NavigationGate {
                        entity: id,
                        system: gate.system,
                        pose,
                        exit: gate.exit,
                        staging,
                        slip_ready: source.slip_power_w > 0.0,
                    },
                    aperture_radius_m: beacon.beacon.radius_m,
                    exclusion_m: gate.exclusion_m,
                },
            );
        }

        self.gates = gates.into_values().collect();
        Ok(())
    }
}

impl RouteEnvironment for Environment {
    fn gates(&self) -> &[RouteGate] {
        &self.gates
    }

    fn resolve(&self, destination: &Destination, after_s: f64) -> Result<Pose> {
        self.source
            .resolve_at(destination.clone(), self.source.prediction_epoch(after_s)?)
    }

    fn contact(&self, reference: ContactRef) -> Result<(Pose, f64)> {
        let snapshot = if reference.group == self.source.group {
            &self.source.snapshot
        } else if reference.group == PUBLIC_GROUP {
            &self.source.public
        } else {
            anyhow::bail!("contact group unavailable");
        };
        let track = snapshot
            .tracks
            .get(&reference.track)
            .context("contact unavailable")?;
        Ok((track.pose.clone(), track.radius_m.unwrap_or(1.0)))
    }

    fn beacon(&self, id: Id) -> Result<Beacon> {
        let beacon = self
            .source
            .beacons
            .get(&id)
            .or_else(|| self.source.orbital.beacons.get(&id))
            .ok_or_else(|| anyhow::anyhow!("public beacon unavailable"))?;
        ensure!(
            beacon.bays.len() <= MAX_QUERY_BEACON_BAYS,
            "too many docking apertures"
        );
        let mut visible = self.source.beacon(beacon);
        if visible.gate_exit.is_none() && !beacon.bays.is_empty() {
            visible.radius_m =
                toy_sim_ships::thermal::shield_radius(visible.radius_m + 3.0_f64.sqrt());
        }
        Ok(visible)
    }

    fn slip(
        &self,
        origin: GalacticPosition,
        destination: GalacticPosition,
        departure_after_s: f64,
        arrival_after_s: f64,
    ) -> Result<SlipEstimate> {
        if origin == destination && departure_after_s == 0.0 && arrival_after_s == 0.0 {
            let mut cache = self.eligibility.lock().unwrap();
            let ready = *cache.entry(origin).or_insert_with(|| {
                self.source.slip_power_w > 0.0
                    && self.source.admissible_at(origin, self.source.epoch)
            });
            return Ok(SlipEstimate {
                ready,
                preparation_s: 0.0,
                duration_s: 0.0,
            });
        }
        let departure = self.source.prediction_epoch(departure_after_s)?;
        let arrival = self.source.prediction_epoch(arrival_after_s)?;
        ensure!(arrival >= departure, "arrival precedes departure");
        let (preparation_s, duration_s) = crate::sim::travel::slip_times(
            origin,
            destination,
            self.source.mass,
            self.source.slip_power_w,
            self.source.slip_preparation.as_ref(),
            self.source.tick,
        );
        let cooldown = (self.ready_tick.saturating_sub(self.source.tick) as f64 * 0.1
            - departure_after_s)
            .max(0.0);
        Ok(SlipEstimate {
            ready: self.source.slip_power_w > 0.0 && self.source.admissible_at(origin, departure),
            preparation_s: preparation_s.max(cooldown),
            duration_s,
        })
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
