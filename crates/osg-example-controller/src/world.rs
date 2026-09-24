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
    mass: f64,
    fuel_available: f64,
    capture_radius: f64,
    max_loss_ppm: f64,
    fuel_limit: f64,
    station: Option<EntityId>,
}

struct DirectTransfer {
    relative_position: DVec3,
    mass: f64,
    fuel_available: f64,
}

impl DirectTransfer {
    fn remains_valid(&self, relative_position: DVec3, mass: f64, fuel: f64) -> bool {
        !planner::changed_materially(self.mass, mass)
            && !planner::changed_materially(self.fuel_available, fuel)
            && (relative_position - self.relative_position).length()
                < (self.relative_position.length() * 0.2).max(100.)
    }
}

#[derive(Default)]
pub struct Executor {
    revision: Option<u64>,
    pub active: bool,
    pub aim_direction: Option<[f64; 3]>,
    pub aim_attitude: Option<[f64; 4]>,
    pub reference_changed: bool,
    pub speed_limit: f64,
    pub preferences: osg_model::transfer::TransferCost,
    plan: Option<Plan>,
    direct_transfer: Option<DirectTransfer>,
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
    fn physical(&self, action: ProgramAction) -> Result<(), i32> {
        if osg_ship_api::sdk::tick()?.tick != self.callback_tick {
            return Err(abi::ERR_UNAVAILABLE);
        }
        command(action)
    }

    pub fn set_guidance(&mut self, guidance: Option<Guidance>) {
        self.manual_guidance = guidance;
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
        self.direct_transfer = None;
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
            self.direct_transfer = None;
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
            self.direct_transfer = None;
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
        let direct = target_station
            .as_ref()
            .map(|station| self.transfer_estimate(&contact(&pose, &station.pose)).0);

        if self.tick >= self.next_track {
            let direct_valid = self
                .direct_transfer
                .as_ref()
                .zip(target_station.as_ref())
                .is_some_and(|(transfer, station)| {
                    transfer.remains_valid(
                        station.pose.position.relative_to(pose.position),
                        self.mass,
                        exotic_fuel_kg,
                    )
                });
            if !direct_valid {
                self.direct_transfer = None;
            }
            let valid = self.plan.as_ref().is_some_and(|plan| {
                !planner::changed_materially(plan.mass, self.mass)
                    && !planner::changed_materially(plan.fuel_available, exotic_fuel_kg)
            });
            let feasible = if valid { self.track(&pose)? } else { false };
            if !feasible {
                if self.plan.is_some() {
                    command(ProgramAction::CancelSlip)?;
                }
                self.plan = None;
            }
            // Close rendezvous uses the ordinary controller. Far transfers
            // also evaluate capture geometries before choosing a maneuver.
            if self.plan.is_none()
                && self.direct_transfer.is_none()
                && direct.is_none_or(|seconds| seconds > 300.)
            {
                if !state.preferences.allow_slipdrive {
                    if target_station.is_none() {
                        return self.fail("This itinerary requires an enabled slipdrive");
                    }
                } else if !slip_ready {
                    if target_station.is_none() {
                        return self.fail("No operational slipdrive is available");
                    }
                } else if exotic_fuel_kg <= 0. {
                    if target_station.is_none() {
                        return self.fail("No exotic fuel remains for this hop");
                    }
                } else {
                    self.status.phase = FirmwarePhase::Planning;
                    self.status.summary = "Comparing capture bodies and departure windows".into();
                    self.next_publish = 0;
                    self.publish()?;
                    let (candidate, fuel_blocked) =
                        self.search(&pose, system, final_station, entry, exotic_fuel_kg)?;
                    // Queries can suspend across ticks. Never execute a plan
                    // computed for a replaced directive or a moved ship.
                    let ProgramReply::Travel {
                        state: fresh,
                        pose: current,
                        tick: now,
                        exotic_fuel_kg: fresh_fuel,
                        ..
                    } = query(&ProgramQuery::Travel)?
                    else {
                        return Err(abi::ERR_ARGUMENT);
                    };
                    if fresh.directive_revision != state.directive_revision || !fresh.enabled {
                        command(ProgramAction::CancelSlip)?;
                        self.active = false;
                        return Ok(None);
                    }
                    let query_callback_tick = osg_ship_api::sdk::tick()?.tick;
                    let fresh_mass = osg_ship_api::sdk::flight()?.mass_kg;
                    let sample_tick = osg_ship_api::sdk::tick()?.tick;
                    let resources_changed = planner::changed_materially(exotic_fuel_kg, fresh_fuel)
                        || planner::changed_materially(self.mass, fresh_mass)
                        || (fuel_blocked
                            && (fresh_fuel > exotic_fuel_kg || fresh_mass < self.mass));
                    if resources_changed || sample_tick != query_callback_tick {
                        command(ProgramAction::CancelSlip)?;
                        self.next_track = 0;
                        self.tick = now;
                        self.wait("Resources changed during planning; refreshing".into(), 0.);
                        self.publish()?;
                        return Ok(None);
                    }
                    if fuel_blocked && target_station.is_none() {
                        return self
                            .fail("The remaining exotic fuel allowance cannot reach this system");
                    }
                    let elapsed = now.saturating_sub(self.tick) as f64 * TICK_SECONDS;
                    let expected = pose
                        .position
                        .offset_by(DVec3::from_array(pose.velocity) * elapsed);
                    if current.position.relative_to(expected).length() > self.own_radius.max(100.) {
                        command(ProgramAction::CancelSlip)?;
                        self.next_track = 0;
                        self.wait("Geometry changed during planning; refreshing".into(), 0.);
                        self.publish()?;
                        return Ok(None);
                    }
                    self.plan = candidate.filter(|plan| {
                        direct
                            .is_none_or(|seconds| planner::meaningfully_better(plan.score, seconds))
                    });
                    if self.plan.is_none() {
                        // Choosing an ordinary transfer is a completed planning
                        // decision. Retain it while relative geometry and
                        // resources remain suitable, including across slices.
                        self.direct_transfer =
                            target_station.as_ref().map(|station| DirectTransfer {
                                relative_position: station
                                    .pose
                                    .position
                                    .offset_by(DVec3::from_array(station.pose.velocity) * elapsed)
                                    .relative_to(current.position),
                                mass: fresh_mass,
                                fuel_available: fresh_fuel,
                            });
                    }
                    self.tick = now;
                    if self.plan.is_some() && !self.track(&current)? {
                        self.plan = None;
                        command(ProgramAction::CancelSlip)?;
                    }
                    // The caller refreshes its control sample after a
                    // suspended search. Do the physical action next cycle.
                    if now != tick {
                        if self.plan.is_none() && target_station.is_none() {
                            self.wait_horizon =
                                (self.wait_horizon * 2.).min(MAX_PREDICTION_SECONDS * 0.5);
                            self.next_track = now.saturating_add(300);
                            self.wait(
                                "No feasible capture in this window; waiting for new geometry"
                                    .into(),
                                30.,
                            );
                        } else {
                            self.next_track = now;
                        }
                        self.publish()?;
                        return Ok(None);
                    }
                }
            }
            if self.plan.is_none() && target_station.is_none() {
                self.wait_horizon = (self.wait_horizon * 2.).min(MAX_PREDICTION_SECONDS * 0.5);
                self.next_track = self.tick.saturating_add(300);
            } else {
                self.next_track = self.tick.saturating_add(30);
            }
        }

        if let Some(plan) = self.plan.clone() {
            self.fly_plan(&pose, slip_axis, plan)
        } else if let Some(station) = target_station.as_ref() {
            self.dock(&pose, station)
        } else {
            self.wait(
                "No feasible capture in this window; waiting for new geometry".into(),
                self.next_track.saturating_sub(self.tick) as f64 * TICK_SECONDS,
            );
            self.publish()?;
            Ok(None)
        }
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
        let (time, fuel) =
            self.preferences
                .remaining(offset.length(), closing, self.acceleration, self.flow);
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
