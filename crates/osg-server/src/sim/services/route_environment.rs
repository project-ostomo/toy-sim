use super::*;
use crate::sim::routing::{CaptureTarget, RouteEnvironment, SlipEstimate};
use anyhow::Context;
use osg_model::travel::slip;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Environment {
    source: Arc<ShipScan>,
    cancel: Arc<AtomicBool>,
    navigation: BTreeMap<Id, Id>,
    deadline: std::time::Instant,
}

impl Environment {
    fn navigation_for(&self, system: Id) -> Option<Id> {
        self.navigation.get(&system).copied()
    }

    fn check_budget(&self) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "route computation cancelled"
        );
        ensure!(
            std::time::Instant::now() < self.deadline,
            "route search time budget exhausted"
        );
        Ok(())
    }
    pub(crate) fn capture(
        world: &mut World,
        ship: Entity,
        cancel: Arc<AtomicBool>,
        input: &crate::sim::routing::RouteRequest,
    ) -> Result<Self> {
        let mut source =
            super::current_ship_source(world, ship).context("route observations unavailable")?;
        let captured = Arc::get_mut(&mut source).expect("new route observation snapshot");
        captured.radius = input.performance.radius_m;
        captured.mass = input.performance.mass_kg;
        captured.pose = input.origin.clone();
        captured.slip_power_w = input.performance.slip_power_w;
        let mut navigation = BTreeMap::new();
        for (id, beacon) in source.beacons.iter() {
            if beacon.navigation
                && crate::sim::ownership::permits_principal(
                    &source.directory,
                    beacon.owner,
                    Some(&beacon.access),
                    source.owner,
                    ownership::Permission::Navigate,
                )
            {
                for system in &beacon.systems {
                    navigation.entry(*system).or_insert(*id);
                }
            }
        }
        Ok(Self {
            source,
            cancel,
            navigation,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        })
    }

    pub(crate) fn prepare(&mut self) -> Result<()> {
        self.deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        ensure!(!self.cancelled(), "route computation cancelled");
        Ok(())
    }

    fn targets(&self, index: usize, after_s: f64) -> Result<Vec<CaptureTarget>> {
        let registry = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry;
        let system = registry.universe.resolve_index(index)?;
        let epoch = self.source.prediction_epoch(after_s)?;
        let mut targets = Vec::new();
        for body in system.solver.iter() {
            self.check_budget()?;
            if matches!(
                body.class_params,
                osg_universe::orrery_cfg::BodyClass::Barycenter
            ) {
                continue;
            }
            let reference = crate::sim::registry::model_reference(
                system
                    .body_id(&body.name)
                    .context("celestial identity unavailable")?,
            );
            let Some(pose) = registry.pose(reference, epoch) else {
                continue;
            };
            let navigation_beacon = self.navigation_for(reference.system);
            targets.push(CaptureTarget {
                reference,
                pose,
                radius_m: slip::exclusion_radius_m(body.mass),
                surface_radius_m: body.radius,
                navigation_beacon,
            });
        }
        Ok(targets)
    }
}

impl RouteEnvironment for Environment {
    fn system_targets(&self, id: Id, after_s: f64) -> Result<Vec<CaptureTarget>> {
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        let index = universe.system_index(id.0).context("unknown system")?;
        self.targets(index, after_s)
    }

    fn system(&self, id: Id) -> Result<(Pose, f64)> {
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        let index = universe.system_index(id.0).context("unknown system")?;
        let definition = universe.resolve_index(index)?;
        Ok((
            Pose {
                position: universe.systems[index].position,
                ..Default::default()
            },
            definition.influence,
        ))
    }

    fn label(&self, order: &osg_model::travel::Order) -> String {
        use osg_model::travel::{Order, Reference};
        let (action, destination) = match order {
            Order::TravelTo(destination) => ("Travel", destination),
            Order::Sublight(destination) => ("Transfer", destination),
            Order::Slip { destination, .. } => ("Slip", destination),
            Order::Dock(id) => {
                return self
                    .source
                    .beacons
                    .get(id)
                    .and_then(|beacon| beacon.beacon.iff.labels.iter().next())
                    .map_or_else(|| order.label(), |name| format!("Dock · {name}"));
            }
            _ => return order.label(),
        };
        let Some(universe) = self
            .source
            .universe
            .as_ref()
            .map(|source| &source.registry.universe)
        else {
            return order.label();
        };
        let name = match destination {
            Destination::Galactic(position) => universe
                .index
                .nearest(*position)
                .map(|index| universe.systems[index].name.to_string()),
            Destination::Relative {
                reference: Reference::Celestial(reference),
                ..
            } => {
                if matches!(order, Order::Sublight(_)) {
                    universe
                        .body(crate::sim::registry::universe_reference(*reference))
                        .map(|body| body.name.to_string())
                } else {
                    universe
                        .system_index(reference.system.0)
                        .map(|index| universe.systems[index].name.to_string())
                }
            }
            Destination::Beacon(id)
            | Destination::Relative {
                reference: Reference::Beacon(id),
                ..
            } => self
                .source
                .beacons
                .get(id)
                .and_then(|beacon| beacon.beacon.iff.labels.iter().next())
                .cloned(),
        };
        name.map_or_else(|| order.label(), |name| format!("{action} · {name}"))
    }

    fn candidates(
        &self,
        origin: GalacticPosition,
        goal: GalacticPosition,
        limit: usize,
    ) -> Result<Vec<CaptureTarget>> {
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        let mut indices = universe.index.nearest_many(origin, limit / 4 + 1);
        indices.extend(universe.index.nearest_many(goal, limit / 4 + 1));
        let displacement = goal.relative_to(origin);
        for fraction in [0.25, 0.5, 0.75] {
            indices.extend(universe.index.nearest_filtered(
                origin.offset_by(displacement * fraction),
                limit / 8 + 1,
                |index| {
                    self.navigation
                        .contains_key(&Id(universe.systems[index].id))
                },
            ));
            indices.extend(
                universe
                    .index
                    .nearest_many(origin.offset_by(displacement * fraction), limit / 6 + 1),
            );
        }
        indices.extend(
            universe
                .index
                .nearest_filtered(goal, limit / 8 + 1, |index| {
                    self.navigation
                        .contains_key(&Id(universe.systems[index].id))
                }),
        );
        indices.sort_unstable();
        indices.dedup();
        let mut targets = Vec::new();
        for index in indices {
            self.check_budget()?;
            ensure!(!self.cancelled(), "route computation cancelled");
            let summary = &universe.systems[index];
            if slip::exclusion_radius_m(summary.stellar_mass)
                <= summary.star_radius + self.source.radius
            {
                targets.extend(self.targets(index, 0.0)?.into_iter().filter(|target| {
                    target.radius_m > target.surface_radius_m + self.source.radius
                }));
                continue;
            }
            let reference = crate::sim::registry::model_reference(summary.primary);
            targets.push(CaptureTarget {
                reference,
                pose: Pose {
                    position: summary.position,
                    ..Default::default()
                },
                radius_m: slip::exclusion_radius_m(summary.stellar_mass),
                surface_radius_m: summary.star_radius,
                navigation_beacon: self.navigation_for(reference.system),
            });
        }
        Ok(targets)
    }

    fn capture_target(
        &self,
        destination: &Destination,
        after_s: f64,
    ) -> Result<Option<CaptureTarget>> {
        let registry = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry;
        if let Destination::Relative {
            reference: Reference::Celestial(reference),
            ..
        } = destination
        {
            if let Some(index) = registry.universe.system_index(reference.system.0) {
                return Ok(self
                    .targets(index, after_s)?
                    .into_iter()
                    .find(|target| target.reference == *reference));
            }
        }
        let pose = self.resolve(destination, after_s)?;
        let Some(index) = registry.universe.index.nearest(pose.position) else {
            return Ok(None);
        };
        Ok(self
            .targets(index, after_s)?
            .into_iter()
            .filter(|target| {
                target.pose.position.relative_to(pose.position).length() <= target.radius_m
            })
            .max_by(|a, b| a.radius_m.total_cmp(&b.radius_m)))
    }

    fn departure(
        &self,
        origin: &Pose,
        toward: GalacticPosition,
        after_s: f64,
    ) -> Result<Option<Destination>> {
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        for index in universe
            .index
            .containing_segment(origin.position, DVec3::ZERO)
        {
            for target in self.targets(index, after_s)? {
                self.check_budget()?;
                let radius = target.radius_m + self.source.radius + 1000.0;
                let offset = origin.position.relative_to(target.pose.position);
                let outgoing = toward.relative_to(origin.position).normalize_or_zero();
                let closest = offset + outgoing * (-offset.dot(outgoing)).max(0.0);
                if offset.length() <= radius
                    || (offset.dot(outgoing) < 0.0 && closest.length() < radius)
                {
                    let radial = offset.try_normalize().unwrap_or(DVec3::X);
                    let desired = toward
                        .relative_to(target.pose.position)
                        .try_normalize()
                        .unwrap_or(DVec3::X);
                    let direction = if radial.dot(desired) < 0.0 {
                        (radial - desired * desired.dot(radial))
                            .try_normalize()
                            .unwrap_or_else(|| radial.any_orthonormal_vector())
                    } else {
                        radial
                    };
                    // Stay just outside the exclusion on its outgoing side.
                    // The waypoint follows the body, preserving its orbital
                    // velocity throughout the transfer and slip preparation.
                    return Ok(Some(Destination::Relative {
                        reference: Reference::Celestial(target.reference),
                        offset: GalacticPosition::from_meters(direction * (radius * 1.001)),
                        axes: Axes::Galactic,
                    }));
                }
            }
        }
        Ok(None)
    }

    fn resolve(&self, destination: &Destination, after_s: f64) -> Result<Pose> {
        self.source
            .resolve_at(destination.clone(), self.source.prediction_epoch(after_s)?)
    }

    fn contact(&self, reference: ContactRef) -> Result<(Pose, f64)> {
        ensure!(
            reference.observer == self.source.own,
            "contact observer unavailable"
        );
        let contact = self
            .source
            .snapshot
            .contacts
            .get(&reference.contact)
            .context("contact unavailable")?;
        Ok((contact.pose.clone(), contact.radius_m))
    }

    fn beacon(&self, id: Id) -> Result<Beacon> {
        let beacon = self
            .source
            .beacons
            .get(&id)
            .context("public beacon unavailable")?;
        ensure!(
            beacon.bays.len() <= MAX_QUERY_BEACON_BAYS,
            "too many docking apertures"
        );
        let mut visible = self.source.beacon(beacon);
        if !beacon.bays.is_empty() {
            visible.radius_m = osg_ships::thermal::shield_radius(visible.radius_m + 3.0_f64.sqrt());
        }
        Ok(visible)
    }

    fn slip(
        &self,
        origin: GalacticPosition,
        destination: GalacticPosition,
        departure_after_s: f64,
        arrival_after_s: f64,
        assisted: bool,
    ) -> Result<SlipEstimate> {
        let departure = self.source.prediction_epoch(departure_after_s)?;
        ensure!(
            arrival_after_s >= departure_after_s,
            "arrival precedes departure"
        );
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        let displacement = destination.relative_to(origin);
        let distance = displacement.length();
        if distance > 0.0 {
            let direction = displacement / distance;
            for index in universe.capture_candidates(
                origin,
                displacement,
                self.source.radius,
                &mut osg_space::spatial::QueryBudget::new(1_000_000),
            )? {
                self.check_budget()?;
                for body in self.targets(index, arrival_after_s)? {
                    let offset = body.pose.position.relative_to(origin);
                    let along = offset.dot(direction);
                    let perpendicular = (offset - direction * along).length();
                    if along > 0.0
                        && along < distance
                        && perpendicular < body.radius_m
                        && body.pose.position.relative_to(destination).length() > body.radius_m
                    {
                        anyhow::bail!(
                            "slip path encounters another exclusion before its intended target"
                        );
                    }
                }
            }
        }
        Ok(SlipEstimate {
            ready: self.source.slip_power_w > 0.0 && self.source.admissible_at(origin, departure),
            preparation_s: (slip::charging_energy_j(self.source.mass, distance / slip::LY_M)
                / self.source.slip_power_w)
                .max(slip::MIN_CHARGE_SECONDS),
            duration_s: slip::flight_seconds(destination.relative_to(origin).length(), assisted),
        })
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
