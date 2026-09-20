use glam::DVec3;
use osg_model::{travel::*, *};
use osg_ship_api::abi;

fn query(query: &ProgramQuery) -> Result<ProgramReply, i32> {
    osg_model::wasm_world::query(query)
}

fn command(action: ProgramAction) -> Result<(), i32> {
    osg_model::wasm_world::command(action)
}

#[derive(Default)]
pub struct Executor {
    revision: Option<(u64, usize)>,
    pub active: bool,
    state_revision: u64,
    state_index: usize,
    pub aim_direction: Option<[f64; 3]>,
    pub reference_changed: bool,
    bay: Option<(EntityId, u32)>,
    reserve_at: u64,
    acceleration: f64,
    flow: f64,
    mass: f64,
    next_estimate: u64,
    turn_s: f64,
    own_radius: f64,
    avoidance: crate::local_guidance::Avoidance,
    environment: Option<(u64, GalacticPosition, LocalSpace)>,
    sensor_targets: Vec<(usize, TrackId)>,
    tick: u64,
    pub speed_limit: f64,
    pub preferences: osg_model::transfer::TransferCost,
}

impl Executor {
    pub fn update(
        &mut self,
        tick: u64,
        sample: &crate::hardware::Sample,
        hardware: &crate::hardware::Hardware,
    ) -> Result<Option<abi::Contact>, i32> {
        self.reference_changed = false;
        let bindings = crate::Bindings::new(hardware);
        self.acceleration = (bindings.thrust / sample.mass_kg.max(1.)).max(1e-6);
        self.flow = bindings.propellant_rate;
        self.mass = sample.mass_kg;
        self.own_radius = sample.radius_m;
        self.turn_s = crate::attitude::turn_allowance(
            glam::DMat3::from_cols_array(&sample.inertia),
            &bindings,
        );
        let result = self.step(tick);
        if let Err(error) = result {
            if self.active {
                self.active = false;
                command(ProgramAction::Block {
                    revision: self.state_revision,
                    order: self.state_index,
                    reason: format!("Command execution failed ({error})"),
                })?;
            }
        }
        result
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

    fn estimate(
        &mut self,
        tick: u64,
        state: &CurrentOrder,
        estimate: Option<(f64, f64)>,
    ) -> Result<(), i32> {
        if tick < self.next_estimate {
            return Ok(());
        }
        self.next_estimate = tick.saturating_add(10);

        command(ProgramAction::Estimate {
            revision: state.revision,
            order: state.index,
            remaining_ticks: estimate
                .map(|(seconds, _)| seconds)
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.)
                .map(|seconds| (seconds * 10.).ceil() as u64),
            remaining_propellant_kg: estimate
                .map(|(_, kg)| kg)
                .filter(|kg| kg.is_finite() && *kg >= 0.),
        })
    }

    fn step(&mut self, tick: u64) -> Result<Option<abi::Contact>, i32> {
        self.tick = tick;
        self.aim_direction = None;
        self.active = false;
        self.speed_limit = f64::INFINITY;
        let ProgramReply::Travel {
            state,
            pose,
            slip_ready,
        } = query(&ProgramQuery::Travel)?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        self.state_revision = state.revision;
        self.state_index = state.index;
        self.preferences = state
            .order
            .as_ref()
            .map_or_else(Default::default, |order| order.transfer_cost);
        if self.revision != Some((state.revision, state.index)) {
            self.revision = Some((state.revision, state.index));
            self.reference_changed = true;
            self.bay = None;
            self.next_estimate = tick;
            self.avoidance.reset();
            self.environment = None;
        }

        self.active =
            state.autopilot_enabled && state.status == Status::Active && state.order.is_some();
        if !self.active {
            return Ok(None);
        }
        let Some(order) = state.order.as_ref().map(|queued| &queued.action) else {
            return Ok(None);
        };
        let complete = || {
            command(ProgramAction::CompleteOrder {
                revision: state.revision,
                order: state.index,
            })
        };
        match order {
            Order::TravelTo(_) => Err(abi::ERR_ARGUMENT),
            Order::Guidance(guidance) => {
                if let Target::Direction(direction) = guidance.target {
                    if guidance.mode != GuidanceMode::Align {
                        return Err(abi::ERR_ARGUMENT);
                    }
                    let direction = DVec3::from_array(direction)
                        .try_normalize()
                        .ok_or(abi::ERR_ARGUMENT)?;
                    self.aim_direction = Some(direction.to_array());
                    let forward = glam::DQuat::from_array(pose.rotation) * DVec3::NEG_Z;
                    self.estimate(
                        tick,
                        &state,
                        Some((
                            self.turn_s
                                * (forward.angle_between(direction) / std::f64::consts::PI).sqrt(),
                            0.,
                        )),
                    )?;
                    if forward.angle_between(direction) < 0.02
                        && DVec3::from_array(pose.angular_velocity).length() < 0.05
                    {
                        complete()?;
                    }
                    return Ok(None);
                }
                let (target, radius) = match &guidance.target {
                    Target::Direction(_) => unreachable!(),
                    Target::Destination(Destination::Beacon(id)) => {
                        let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(*id))?
                        else {
                            return Err(abi::ERR_ARGUMENT);
                        };
                        let beacon = beacons
                            .into_iter()
                            .find(|beacon| beacon.entity == *id)
                            .ok_or(abi::ERR_UNAVAILABLE)?;
                        (beacon.pose, beacon.radius_m)
                    }
                    Target::Destination(destination) => {
                        let ProgramReply::Pose(pose) = query(&ProgramQuery::Resolve {
                            destination: destination.clone(),
                            after_seconds: 0.,
                        })?
                        else {
                            return Err(abi::ERR_ARGUMENT);
                        };
                        (pose, 0.)
                    }
                    Target::Contact(reference) => {
                        let ProgramReply::Contact {
                            pose,
                            handle: _,
                            radius_m,
                        } = query(&ProgramQuery::Contact(*reference))?
                        else {
                            return Err(abi::ERR_ARGUMENT);
                        };
                        (pose, radius_m)
                    }
                };
                let mut relative = contact(&pose, &target);
                let offset = DVec3::from_array(relative.position_m);
                if guidance.mode == GuidanceMode::Align {
                    let direction = offset.try_normalize().ok_or(abi::ERR_UNAVAILABLE)?;
                    self.aim_direction = Some(direction.to_array());
                    let forward = glam::DQuat::from_array(pose.rotation) * DVec3::NEG_Z;
                    self.estimate(
                        tick,
                        &state,
                        Some((
                            self.turn_s
                                * (forward.angle_between(direction) / std::f64::consts::PI).sqrt(),
                            0.,
                        )),
                    )?;
                    if forward.angle_between(direction) < 0.02
                        && DVec3::from_array(pose.angular_velocity).length() < 0.05
                    {
                        complete()?;
                    }
                    return Ok(None);
                }
                let range = guidance.range_m.max(radius + self.own_radius + 10.);
                relative.position_m = (offset - offset.normalize_or_zero() * range).to_array();
                if guidance.mode == GuidanceMode::Approach
                    && DVec3::from_array(relative.position_m).length() < 5.
                    && DVec3::from_array(relative.velocity_m_s).length() < 0.5
                {
                    complete()?;
                    return Ok(None);
                }
                self.estimate(
                    tick,
                    &state,
                    if guidance.mode == GuidanceMode::KeepRange {
                        None
                    } else {
                        Some(self.transfer_estimate(&relative))
                    },
                )?;
                let mut destination = target;
                destination.position = pose
                    .position
                    .offset_by(DVec3::from_array(relative.position_m));
                self.steer(&pose, &destination, None).map(Some)
            }
            Order::Sublight(destination) => {
                let target = self.sublight_target(destination, &pose)?;
                let contact = contact(&pose, &target);
                if DVec3::from_array(contact.position_m).length() <= 2.
                    && DVec3::from_array(contact.velocity_m_s).length() <= 0.5
                {
                    complete()?;
                    return Ok(None);
                }
                self.estimate(tick, &state, Some(self.transfer_estimate(&contact)))?;
                self.steer(&pose, &target, None).map(Some)
            }
            Order::Slip {
                destination,
                speed_ly_s,
                navigation_beacon,
            } => {
                let target = resolve_at(destination, 0.)?;
                let space = self.navigation_environment(&pose)?;
                let departure = crate::local_guidance::outside_exclusions(
                    pose.position,
                    target.position.relative_to(pose.position),
                    &space,
                    self.own_radius,
                );
                let Some(departure) = departure else {
                    return Err(abi::ERR_UNAVAILABLE);
                };
                if departure.relative_to(pose.position).length() > 1. {
                    let mut local = pose.clone();
                    local.position = departure;
                    if let Some(obstacle) = space.obstacles.iter().find(|obstacle| {
                        obstacle.slip_exclusion_m > 0.
                            && pose.position.relative_to(obstacle.pose.position).length()
                                < obstacle.slip_exclusion_m + self.own_radius + 10.
                    }) {
                        local.velocity = obstacle.pose.velocity;
                    }
                    let outward = departure.relative_to(pose.position).normalize_or_zero();
                    let relative_velocity =
                        DVec3::from_array(pose.velocity) - DVec3::from_array(local.velocity);
                    local.velocity = (DVec3::from_array(local.velocity)
                        + outward * relative_velocity.dot(outward).max(50.))
                    .to_array();
                    let relative = contact(&pose, &local);
                    self.estimate(tick, &state, Some(self.transfer_estimate(&relative)))?;
                    return self.steer(&pose, &local, None).map(Some);
                }

                if slip_ready {
                    let solution = solve_slip(
                        *speed_ly_s,
                        *navigation_beacon,
                        |after| {
                            Ok(pose
                                .position
                                .offset_by(DVec3::from_array(pose.velocity) * after))
                        },
                        |after| {
                            let target = resolve_at(destination, after)?;
                            Ok(target.position)
                        },
                    )?;
                    self.estimate(tick, &state, Some((solution.seconds, 0.)))?;
                    command(ProgramAction::Slip {
                        revision: state.revision,
                        order: state.index,
                        destination: solution.destination,
                        speed_ly_s: *speed_ly_s,
                        navigation_beacon: *navigation_beacon,
                    })?;
                } else {
                    let seconds = state
                        .estimated_arrival_tick
                        .map_or(f64::INFINITY, |arrival| {
                            arrival.saturating_sub(tick) as f64 * 0.1
                        });
                    self.estimate(tick, &state, Some((seconds, 0.)))?;
                }
                Ok(None)
            }
            Order::Undock => {
                command(ProgramAction::Undock {
                    revision: state.revision,
                    order: state.index,
                })?;
                Ok(None)
            }
            Order::WaitUntil(until) => {
                self.estimate(
                    tick,
                    &state,
                    Some((until.saturating_sub(tick) as f64 * 0.1, 0.)),
                )?;
                if tick >= *until {
                    complete()?;
                }
                Ok(None)
            }
            Order::Dock(station) => {
                let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(*station))? else {
                    return Err(abi::ERR_ARGUMENT);
                };
                let beacon = beacons
                    .iter()
                    .find(|beacon| beacon.entity == *station)
                    .ok_or(abi::ERR_UNAVAILABLE)?;
                let selected = self
                    .bay
                    .filter(|(host, bay)| host == station && beacon.bays.contains_key(bay));
                let bay = selected
                    .map(|(_, bay)| bay)
                    .or_else(|| beacon.bays.first_key_value().map(|(&bay, _)| bay))
                    .ok_or(abi::ERR_UNAVAILABLE)?;
                if selected.is_none() || tick >= self.reserve_at {
                    command(ProgramAction::ReserveBay {
                        revision: state.revision,
                        order: state.index,
                        station: *station,
                        bay,
                    })?;
                    self.bay = Some((*station, bay));
                    self.reserve_at = tick.saturating_add(100);
                }
                let ship_radius = self.own_radius;
                let delta = pose.position.relative_to(beacon.pose.position);
                let surface_gap = delta.length() - beacon.radius_m - ship_radius;
                let relative_speed = (DVec3::from_array(pose.velocity)
                    - DVec3::from_array(beacon.pose.velocity))
                .length();
                if surface_gap > DOCKING_CLEARANCE_M || relative_speed > DOCKING_SPEED_M_S {
                    let direction = delta.try_normalize().unwrap_or(DVec3::Z);
                    let mut target = beacon.pose.clone();
                    target.position = target.position.offset_by(
                        direction * (beacon.radius_m + ship_radius + DOCKING_CLEARANCE_M * 0.5),
                    );
                    let relative = contact(&pose, &target);
                    self.estimate(tick, &state, Some(self.transfer_estimate(&relative)))?;
                    return self.steer(&pose, &target, None).map(Some);
                }
                command(ProgramAction::Dock {
                    revision: state.revision,
                    order: state.index,
                    station: *station,
                    bay,
                })?;
                Ok(None)
            }
        }
    }

    fn sublight_target(&self, destination: &Destination, pose: &Pose) -> Result<Pose, i32> {
        if let Destination::Beacon(id) = destination {
            let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(*id))? else {
                return Err(abi::ERR_ARGUMENT);
            };
            let beacon = beacons
                .into_iter()
                .find(|beacon| beacon.entity == *id)
                .ok_or(abi::ERR_UNAVAILABLE)?;
            let direction = pose
                .position
                .relative_to(beacon.pose.position)
                .try_normalize()
                .unwrap_or(DVec3::Z);
            return Ok(crate::local_guidance::offset_pose(
                &beacon.pose,
                direction * (beacon.radius_m + self.own_radius + 50.),
            ));
        }
        resolve_at(destination, 0.)
    }

    fn navigation_environment(&mut self, pose: &Pose) -> Result<LocalSpace, i32> {
        const SURVEY_RADIUS: f64 = 1e8;
        let needs_survey = self.environment.as_ref().is_none_or(|(_, origin, _)| {
            pose.position.relative_to(*origin).length() > SURVEY_RADIUS * 0.5
        });
        if needs_survey {
            self.sensor_targets.clear();
            let ProgramReply::Orrery(mut obstacles) = query(&ProgramQuery::Orrery {
                reference: pose.position,
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            let ProgramReply::Tracks(mut page) = query(&ProgramQuery::Tracks(TrackQuery {
                sphere: Some((pose.position, SURVEY_RADIUS)),
                limit: 256,
                work: 65536,
                ..Default::default()
            }))?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            page.tracks.sort_by(|a, b| {
                a.pose
                    .position
                    .relative_to(pose.position)
                    .length_squared()
                    .total_cmp(&b.pose.position.relative_to(pose.position).length_squared())
            });
            page.tracks.truncate(32);
            for track in page.tracks {
                if track.pose.position.relative_to(pose.position).length() < self.own_radius {
                    continue;
                }
                if obstacles.iter().any(|obstacle| {
                    matches!(&obstacle.reference,
                        Target::Destination(Destination::Beacon(id))
                            if Some(*id) == track.entity)
                }) {
                    continue;
                }
                self.sensor_targets.push((obstacles.len(), track.id));
                obstacles.push(LocalObstacle {
                    reference: Target::Destination(Destination::Galactic(track.pose.position)),
                    radius_m: track.radius_m.unwrap_or(1.) + 3. * track.position_sigma_m,
                    pose: track.pose,
                    slip_exclusion_m: 0.,
                });
            }
            self.environment = Some((
                self.tick,
                pose.position,
                LocalSpace {
                    obstacles,
                    truncated: false,
                },
            ));
        }
        let (observed_tick, _, cached) = self.environment.as_mut().unwrap();
        if self.tick.saturating_sub(*observed_tick) >= 10 {
            let elapsed = self.tick.saturating_sub(*observed_tick) as f64 * 0.1;
            for obstacle in &mut cached.obstacles {
                obstacle.pose.position = obstacle
                    .pose
                    .position
                    .offset_by(DVec3::from_array(obstacle.pose.velocity) * elapsed);
                if self.tick % 100 < 10 {
                    if let Target::Destination(destination) = &obstacle.reference {
                        if !matches!(destination, Destination::Galactic(_)) {
                            if let Ok(updated) = resolve_at(destination, 0.) {
                                obstacle.pose = updated;
                            }
                        }
                    }
                }
            }
            for &(index, track) in &self.sensor_targets {
                if let Ok(ProgramReply::Tracks(page)) = query(&ProgramQuery::Tracks(TrackQuery {
                    track: Some(track),
                    limit: 1,
                    work: 256,
                    ..Default::default()
                })) {
                    if let Some(observation) = page.tracks.first() {
                        cached.obstacles[index].pose = observation.pose.clone();
                        cached.obstacles[index].radius_m =
                            observation.radius_m.unwrap_or(1.) + 3. * observation.position_sigma_m;
                    }
                }
            }
            *observed_tick = self.tick;
        }
        let (observed_tick, _, cached) = self.environment.as_ref().unwrap();
        let elapsed = self.tick.saturating_sub(*observed_tick) as f64 * 0.1;
        let mut space = cached.clone();
        for obstacle in &mut space.obstacles {
            obstacle.pose.position = obstacle
                .pose
                .position
                .offset_by(DVec3::from_array(obstacle.pose.velocity) * elapsed);
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
            let local_speed =
                crate::navigation::arrival_speed(clearance, self.acceleration, self.turn_s + 0.1)
                    .max(40.);
            self.speed_limit = self.speed_limit.min(local_speed);
        }
        Ok(contact(pose, &steering.target))
    }
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

fn solve_slip(
    speed_ly_s: f64,
    navigation_beacon: Option<EntityId>,
    origin_at: impl FnMut(f64) -> Result<GalacticPosition, i32>,
    destination_at: impl FnMut(f64) -> Result<GalacticPosition, i32>,
) -> Result<crate::slip_guidance::Solution, i32> {
    crate::slip_guidance::intercept(
        origin_at,
        destination_at,
        |origin, destination, departure_after_seconds, arrival_after_seconds| {
            let ProgramReply::SlipEligibility {
                ready,
                preparation_s,
                duration_s,
            } = query(&ProgramQuery::SlipEligibility {
                origin,
                destination,
                departure_after_seconds,
                arrival_after_seconds,
                speed_ly_s,
                navigation_beacon,
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            Ok((ready, preparation_s, duration_s))
        },
    )
}
