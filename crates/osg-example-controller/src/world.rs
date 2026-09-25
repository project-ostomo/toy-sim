//! The standard firmware owns maneuver plans; the server stores its itinerary.
use glam::{DQuat, DVec3};
use osg_model::{travel::*, *};
use osg_ship_api::abi;

use crate::directive_planner as planner;

fn query(query: &ProgramQuery) -> Result<ProgramReply, i32> {
    osg_model::wasm_world::query(query)
}

fn command(action: ProgramAction) -> Result<(), i32> {
    osg_model::wasm_world::command(action)
}

#[derive(Clone)]
struct Plan {
    epoch: u64,
    body: CelestialRef,
    offset: DVec3,
    destination: GalacticPosition,
    departure: u64,
    arrival: u64,
    beacon: Option<EntityId>,
    arrival_velocity: Option<[f64; 3]>,
    delta_v: f64,
    loss_ppm: f64,
    fuel_kg: f64,
    maneuver: Option<Pose>,
    score: f64,
    capture_radius: f64,
    max_loss_ppm: f64,
    fuel_limit: f64,
    station: Option<EntityId>,
}

#[derive(Default)]
pub struct Executor {
    revision: Option<u64>,
    pub active: bool,
    pub aim_direction: Option<[f64; 3]>,
    pub aim_attitude: Option<[f64; 4]>,
    pub reference_changed: bool,
    pub speed_limit: f64,
    plan: Option<Plan>,
    search_cursor: usize,
    assistance: Vec<Option<EntityId>>,
    beacon_after: Option<EntityId>,
    beacons_loaded: bool,
    search_input: Option<(Pose, Id, Option<Beacon>, ItineraryEntry, f64, f64)>,
    status: FirmwareStatus,
    manual_guidance: Option<Guidance>,
    next_track: u64,
    next_publish: u64,
    wait_horizon: f64,
    transit: bool,
    transit_fuel: Option<f64>,
    bay: Option<(EntityId, u32)>,
    reserve_at: u64,
    acceleration: f64,
    flow: f64,
    mass: f64,
    turn_s: f64,
    own_radius: f64,
    avoidance: crate::local_guidance::Avoidance,
    environment: Option<(u64, GalacticPosition, LocalSpace)>,
    contacts: Vec<abi::Contact>,
    tick: u64,
    callback_tick: u64,
}

impl Executor {
    pub fn console_status(&self) -> Option<String> {
        self.active
            .then(|| crate::display::status_line(&self.status))
    }

    fn physical(&self, action: ProgramAction) -> Result<(), i32> {
        if osg_ship_api::sdk::tick()?.tick != self.callback_tick {
            return Err(abi::ERR_UNAVAILABLE);
        }
        command(action)
    }

    pub fn set_guidance(&mut self, guidance: Option<Guidance>) {
        self.manual_guidance = guidance;
        self.search_input = None;
        self.plan = None;
        self.reference_changed = true;
        self.avoidance.reset();
    }

    pub fn update(
        &mut self,
        tick: u64,
        sample: &crate::hardware::Sample,
        hardware: &crate::hardware::Hardware,
        contacts: &[abi::Contact],
    ) -> Result<Option<abi::Contact>, i32> {
        self.contacts.clear();
        self.contacts.extend_from_slice(contacts);
        self.tick = tick;
        self.callback_tick = tick;
        self.reference_changed = false;
        self.aim_direction = None;
        self.aim_attitude = None;
        self.speed_limit = f64::INFINITY;
        let bindings = crate::Bindings::new(hardware);
        self.acceleration = (bindings.thrust / sample.mass_kg.max(1.)).max(1e-6);
        self.flow = bindings.propellant_rate;
        self.mass = sample.mass_kg;
        self.own_radius = sample.radius_m;
        self.turn_s = crate::attitude::turn_allowance(
            glam::DMat3::from_cols_array(&sample.inertia),
            &bindings,
        );
        let result = self.step();
        if let Err(error) = result {
            if self.active {
                // Unavailable geometry is transient. Keep intent and retry;
                // explicit impossibility is reported at its decision point.
                self.wait(
                    format!("World service unavailable ({error}); retrying"),
                    10.,
                );
                if self.plan.is_some() {
                    command(ProgramAction::CancelSlip)?;
                }
                self.plan = None;
                self.publish()?;
            }
        }
        result
    }

    fn reset(&mut self, state: &AutopilotState) {
        self.revision = Some(state.directive_revision);
        self.reference_changed = true;
        self.plan = None;
        self.search_cursor = 0;
        self.assistance.clear();
        self.beacon_after = None;
        self.beacons_loaded = false;
        self.search_input = None;
        self.bay = None;
        self.next_track = 0;
        self.next_publish = 0;
        self.wait_horizon = 300.;
        self.transit = false;
        self.transit_fuel = None;
        self.status = FirmwareStatus {
            spent_loss_ppm: state.status.spent_loss_ppm,
            spent_exotic_fuel_kg: state.status.spent_exotic_fuel_kg,
            ..Default::default()
        };
        self.avoidance.reset();
        self.environment = None;
    }

    fn step(&mut self) -> Result<Option<abi::Contact>, i32> {
        let ProgramReply::Travel {
            state,
            pose,
            presence,
            slip_ready,
            slip_axis,
            location,
            tick,
            exotic_fuel_kg,
        } = query(&ProgramQuery::Travel)?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        self.tick = tick;
        if self.revision != Some(state.directive_revision) {
            self.reset(&state);
        }
        self.active = state.enabled && !state.itinerary.is_empty();
        if !self.active {
            self.plan = None;
            self.search_cursor = 0;
            self.assistance.clear();
            self.beacon_after = None;
            self.beacons_loaded = false;
            self.search_input = None;
            return self.manual(&pose);
        }
        self.manual_guidance = None;
        let entry = &state.itinerary[0];
        if matches!(presence, Presence::SlipTransit(_)) {
            // Update from the actual inventory on each observation so the
            // published budget also survives a save during transit.
            let previous = self.transit_fuel.unwrap_or(exotic_fuel_kg);
            self.status.spent_exotic_fuel_kg += (previous - exotic_fuel_kg).max(0.);
            self.transit_fuel = Some(exotic_fuel_kg);
            if !self.transit {
                if let Some(plan) = &self.plan {
                    let spent = slip::log_loss_from_ppm(self.status.spent_loss_ppm)
                        + slip::log_loss_from_ppm(plan.loss_ppm);
                    self.status.spent_loss_ppm = slip::ppm_from_log_loss(spent);
                }
            }
            self.transit = true;
            self.status.phase = FirmwarePhase::Transit;
            self.publish()?;
            return Ok(None);
        }
        if self.transit {
            if let Some(previous) = self.transit_fuel.take() {
                self.status.spent_exotic_fuel_kg += (previous - exotic_fuel_kg).max(0.);
            }
            self.plan = None;
            self.search_cursor = 0;
            self.assistance.clear();
            self.beacon_after = None;
            self.beacons_loaded = false;
            self.search_input = None;
            self.next_track = 0;
            self.reference_changed = true;
            self.transit = false;
        }
        self.transit_fuel = Some(exotic_fuel_kg);
        if matches!(presence, Presence::Destroyed | Presence::StoredInWreck(_)) {
            return self.fail("The ship cannot fly in its current state");
        }

        let target_station = match entry.directive {
            Directive::DockAt(station) => {
                if matches!(presence, Presence::Docked { host, .. } if host == station) {
                    self.complete()?;
                    return Ok(None);
                }
                let beacon = beacon(station)?;
                let Some(beacon) = beacon
                    .filter(|beacon| beacon.system.is_some() && beacon.system == location.system)
                else {
                    return self.fail("Docking destination is not known in the current system");
                };
                Some(beacon)
            }
            Directive::SlipToSystem(system) => {
                if location.system == Some(system) {
                    self.complete()?;
                    return Ok(None);
                }
                None
            }
        };
        if matches!(presence, Presence::Docked { .. }) {
            self.status.phase = FirmwarePhase::Maneuvering;
            self.status.summary = "Undocking before departure".into();
            self.physical(ProgramAction::Undock)?;
            self.publish()?;
            return Ok(None);
        }

        let system = match entry.directive {
            Directive::SlipToSystem(system) => system,
            Directive::DockAt(_) => location.system.ok_or(abi::ERR_UNAVAILABLE)?,
        };
        let next_station = if target_station.is_none() {
            match state.itinerary.get(1).map(|entry| &entry.directive) {
                Some(Directive::DockAt(id)) => beacon(*id)?,
                _ => None,
            }
        } else {
            None
        };
        let final_station = target_station.as_ref().or(next_station.as_ref());

        let fuel_limit = state
            .fuel_budget
            .as_ref()
            .and_then(|budget| {
                budget
                    .resources
                    .iter()
                    .find(|resource| resource.resource == slip::EXOTIC_RESOURCE)
            })
            .map_or(
                exotic_fuel_kg + self.status.spent_exotic_fuel_kg,
                |resource| resource.available_kg,
            )
            * state.preferences.fuel_fraction;
        let available = exotic_fuel_kg.min((fuel_limit - self.status.spent_exotic_fuel_kg).max(0.));
        let risk = RiskBudget {
            max_log_loss: slip::log_loss_from_ppm(state.preferences.max_loss_ppm),
            spent_log_loss: slip::log_loss_from_ppm(self.status.spent_loss_ppm),
        }
        .remaining_ppm();
        if self.tick >= self.next_track {
            if self.plan.is_some() && !self.track(&pose)? {
                self.plan = None;
                self.physical(ProgramAction::CancelSlip)?;
            }
            self.next_track = self.tick.saturating_add(30);
        }
        if slip_ready && state.preferences.allow_slipdrive {
            self.search_input = Some((
                pose.clone(),
                system,
                final_station.cloned(),
                entry.clone(),
                available,
                risk,
            ));
        } else {
            self.search_input = None;
        }

        if let Some(plan) = self.plan.clone() {
            self.fly_plan(&pose, slip_axis, plan)
        } else if let Some(station) = target_station.as_ref() {
            self.dock(&pose, station)
        } else {
            self.status = FirmwareStatus {
                spent_loss_ppm: self.status.spent_loss_ppm,
                spent_exotic_fuel_kg: self.status.spent_exotic_fuel_kg,
                ..Default::default()
            };
            if self.search_input.is_some() {
                self.status.phase = FirmwarePhase::Planning;
                self.status.summary = "Searching for a feasible capture".into();
            } else {
                self.wait(
                    if state.preferences.allow_slipdrive {
                        "Waiting for slipdrive availability"
                    } else {
                        "Slipdrive disabled in route preferences"
                    }
                    .into(),
                    1.,
                );
            }
            self.publish()?;
            Ok(None)
        }
    }

    // Called after control and mailbox processing. One candidate per callback;
    // no exhaustive search can hold up initial rendezvous guidance.
    pub fn refine(&mut self) -> Result<(), i32> {
        let Some((pose, system, station, entry, fuel, risk)) = self.search_input.take() else {
            return Ok(());
        };
        let direct = matches!(entry.directive, Directive::DockAt(_))
            .then(|| {
                station
                    .as_ref()
                    .map(|station| self.transfer_estimate(&contact(&pose, &station.pose)).0)
            })
            .flatten();
        // Even an ideal capture cannot beat charging plus the minimum transit.
        // Leave the callback budget to guidance during the final approach.
        if direct.is_some_and(|seconds| {
            !planner::meaningfully_better(
                slip::MIN_CHARGE_SECONDS + slip::MIN_TRANSIT_SECONDS,
                seconds,
            )
        }) {
            return Ok(());
        }
        let candidate = self.search(&pose, system, station.as_ref(), &entry, fuel, risk)?;
        if let Some(candidate) = candidate {
            let incumbent = self.plan.as_ref().map(|plan| {
                plan.score - self.tick.saturating_sub(plan.epoch) as f64 * TICK_SECONDS
            });
            if incumbent
                .into_iter()
                .chain(direct)
                .all(|seconds| planner::meaningfully_better(candidate.score, seconds))
            {
                self.plan = Some(candidate);
            }
        }
        Ok(())
    }

    fn complete(&mut self) -> Result<(), i32> {
        self.physical(ProgramAction::Complete {
            directive_revision: self.revision.unwrap(),
        })?;
        self.plan = None;
        self.active = false;
        Ok(())
    }

    fn manual(&mut self, pose: &Pose) -> Result<Option<abi::Contact>, i32> {
        let Some(guidance) = self.manual_guidance.clone() else {
            return Ok(None);
        };
        let (target, radius) = match guidance.target {
            Target::Direction(direction) => {
                self.aim_direction = Some(direction);
                return Ok(None);
            }
            Target::Destination(Destination::Beacon(id)) => {
                let Some(beacon) = beacon(id)? else {
                    self.manual_guidance = None;
                    return Ok(None);
                };
                (beacon.pose, beacon.radius_m)
            }
            Target::Destination(destination) => (resolve_at(&destination, 0.)?, 0.),
            Target::Contact(reference) => {
                let ProgramReply::Contact { pose, radius_m, .. } =
                    query(&ProgramQuery::Contact(reference))?
                else {
                    return Err(abi::ERR_ARGUMENT);
                };
                (pose, radius_m)
            }
        };
        let offset = target.position.relative_to(pose.position);
        if guidance.mode == GuidanceMode::Align {
            self.aim_direction = Some(offset.normalize_or_zero().to_array());
            return Ok(None);
        }
        let mut target = target;
        let range = guidance.range_m.max(radius + self.own_radius + 10.);
        target.position = target
            .position
            .offset_by(-offset.normalize_or_zero() * range);
        let relative = contact(pose, &target);
        if guidance.mode == GuidanceMode::Approach
            && DVec3::from_array(relative.position_m).length() < 5.
            && DVec3::from_array(relative.velocity_m_s).length() < 0.5
        {
            self.manual_guidance = None;
            return Ok(None);
        }
        self.steer(pose, &target, None).map(Some)
    }

    fn fail(&mut self, reason: &str) -> Result<Option<abi::Contact>, i32> {
        command(ProgramAction::Fail {
            directive_revision: self.revision.unwrap(),
            reason: reason.into(),
        })?;
        self.active = false;
        self.plan = None;
        Ok(None)
    }

    fn wait(&mut self, why: String, seconds: f64) {
        self.status.summary = why.clone();
        self.status.phase = FirmwarePhase::Waiting {
            until: Some(
                self.tick
                    .saturating_add((seconds * TICK_RATE_HZ).ceil() as u64),
            ),
            why,
        };
        self.status.estimated_arrival_tick = None;
        self.status.capture_body = None;
        self.status.aim_offset_m = None;
        self.status.departure_tick = None;
        self.status.planned_delta_v_m_s = 0.;
        self.status.planned_loss_ppm = 0.;
        self.status.planned_exotic_fuel_kg = 0.;
        self.status.markers.clear();
    }

    fn publish(&mut self) -> Result<(), i32> {
        if self.tick < self.next_publish {
            return Ok(());
        }
        self.next_publish = self.tick.saturating_add(10);
        command(ProgramAction::PublishStatus {
            directive_revision: self.revision.unwrap(),
            status: self.status.clone(),
        })
    }

    fn transfer_estimate(&self, relative: &abi::Contact) -> (f64, f64) {
        let offset = DVec3::from_array(relative.position_m);
        let velocity = -DVec3::from_array(relative.velocity_m_s);
        let direction = offset.normalize_or_zero();
        let closing = velocity.dot(direction);
        let lateral = (velocity - direction * closing).length();
        let (time, fuel) = osg_model::transfer::TransferCost { seconds_per_kg: 0. }.remaining(
            offset.length(),
            closing,
            self.acceleration,
            self.flow,
        );
        let correction_s = lateral / self.acceleration;
        (
            time + correction_s + 2. * self.turn_s,
            fuel + correction_s * self.flow,
        )
    }

    fn dock(&mut self, pose: &Pose, station: &Beacon) -> Result<Option<abi::Contact>, i32> {
        let selected = self
            .bay
            .filter(|(host, bay)| *host == station.entity && station.bays.contains_key(bay));
        let Some(bay) = selected
            .map(|(_, bay)| bay)
            .or_else(|| station.bays.first_key_value().map(|(&bay, _)| bay))
        else {
            self.wait("Waiting for an accessible docking bay".into(), 10.);
            self.publish()?;
            return Ok(None);
        };
        if selected.is_none() || self.tick >= self.reserve_at {
            self.physical(ProgramAction::ReserveBay {
                station: station.entity,
                bay,
            })?;
            self.bay = Some((station.entity, bay));
            self.reserve_at = self.tick.saturating_add(100);
        }
        let delta = pose.position.relative_to(station.pose.position);
        let gap = delta.length() - station.radius_m - self.own_radius;
        let relative_speed =
            (DVec3::from_array(pose.velocity) - DVec3::from_array(station.pose.velocity)).length();
        self.status.phase = FirmwarePhase::Docking;
        self.status.summary = "Rendezvous and docking".into();
        self.status.markers = vec![PlanMarker {
            position: station.pose.position,
            label: "Docking destination".into(),
        }];
        if gap <= DOCKING_CLEARANCE_M && relative_speed <= DOCKING_SPEED_M_S {
            self.physical(ProgramAction::Dock {
                station: station.entity,
                bay,
            })?;
            self.publish()?;
            return Ok(None);
        }
        let mut target = station.pose.clone();
        self.speed_limit = crate::navigation::arrival_speed(
            (gap - DOCKING_CLEARANCE_M * 0.5).max(0.),
            self.acceleration,
            self.turn_s + 0.3,
        )
        .max(DOCKING_SPEED_M_S * 0.5);
        target.position = target.position.offset_by(
            delta.try_normalize().unwrap_or(DVec3::Z)
                * (station.radius_m + self.own_radius + DOCKING_CLEARANCE_M * 0.5),
        );
        let seconds = self.transfer_estimate(&contact(pose, &target)).0;
        self.status.estimated_arrival_tick =
            Some(self.tick + (seconds * TICK_RATE_HZ).ceil() as u64);
        self.publish()?;
        self.steer(pose, &target, None).map(Some)
    }

    fn navigation_environment(&mut self, pose: &Pose) -> Result<LocalSpace, i32> {
        const SURVEY_RADIUS: f64 = 1e8;
        let refresh = self.environment.as_ref().is_none_or(|(tick, origin, _)| {
            self.tick.saturating_sub(*tick) >= 10
                || pose.position.relative_to(*origin).length() > SURVEY_RADIUS * 0.5
        });
        if refresh {
            let ProgramReply::Orrery(obstacles) = query(&ProgramQuery::Orrery {
                reference: pose.position,
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            self.environment = Some((
                self.tick,
                pose.position,
                LocalSpace {
                    obstacles,
                    truncated: false,
                },
            ));
        }
        let (tick, _, cached) = self.environment.as_ref().unwrap();
        let mut space = cached.clone();
        let elapsed = self.tick.saturating_sub(*tick) as f64 * TICK_SECONDS;
        for obstacle in &mut space.obstacles {
            obstacle.pose.position = obstacle
                .pose
                .position
                .offset_by(DVec3::from_array(obstacle.pose.velocity) * elapsed);
        }
        for contact in self.contacts.iter().take(32) {
            let position = pose
                .position
                .offset_by(DVec3::from_array(contact.position_m));
            space.obstacles.push(LocalObstacle {
                reference: Target::Destination(Destination::Galactic(position)),
                pose: Pose {
                    position,
                    velocity: (DVec3::from_array(pose.velocity)
                        + DVec3::from_array(contact.velocity_m_s))
                    .to_array(),
                    ..Default::default()
                },
                radius_m: contact.radius_m,
                slip_exclusion_m: 0.,
                hill_radius_m: 0.,
            });
        }
        Ok(space)
    }

    fn steer(
        &mut self,
        pose: &Pose,
        target: &Pose,
        ignored: Option<&Target>,
    ) -> Result<abi::Contact, i32> {
        let space = self.navigation_environment(pose)?;
        let steering = self
            .avoidance
            .steer(pose, target, &space, self.own_radius, ignored);
        self.reference_changed |= steering.changed;
        if steering.detouring {
            let clearance = space
                .obstacles
                .iter()
                .filter(|obstacle| ignored != Some(&obstacle.reference))
                .map(|obstacle| {
                    pose.position.relative_to(obstacle.pose.position).length()
                        - obstacle.radius_m
                        - self.own_radius
                })
                .fold(f64::INFINITY, f64::min)
                .max(0.);
            self.speed_limit = self.speed_limit.min(
                crate::navigation::arrival_speed(clearance, self.acceleration, self.turn_s + 0.1)
                    .max(40.),
            );
        }
        Ok(contact(pose, &steering.target))
    }
}

fn beacon(id: EntityId) -> Result<Option<Beacon>, i32> {
    let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(id))? else {
        return Err(abi::ERR_ARGUMENT);
    };
    Ok(beacons.into_iter().find(|beacon| beacon.entity == id))
}

fn contact(pose: &Pose, target: &Pose) -> abi::Contact {
    abi::Contact {
        id: u64::MAX,
        kind: abi::CONTACT_SHIP,
        position_m: target.position.relative_to(pose.position).to_array(),
        velocity_m_s: (DVec3::from_array(target.velocity) - DVec3::from_array(pose.velocity))
            .to_array(),
        radius_m: 0.,
    }
}

fn resolve_at(destination: &Destination, after_seconds: f64) -> Result<Pose, i32> {
    let ProgramReply::Pose(pose) = query(&ProgramQuery::Resolve {
        destination: destination.clone(),
        after_seconds,
    })?
    else {
        return Err(abi::ERR_ARGUMENT);
    };
    Ok(pose)
}

mod flight;
