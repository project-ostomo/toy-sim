use glam::DVec3;
use toy_sim_model::{travel::*, *};
use toy_sim_ship_api::{abi, sdk};

fn query(query: &ProgramQuery) -> Result<ProgramReply, i32> {
    let bytes = postcard::to_allocvec(query).map_err(|_| abi::ERR_ARGUMENT)?;
    let mut reply = Vec::<u8>::with_capacity(65536);
    let length = unsafe {
        abi::raw::world_query(
            bytes.as_ptr(),
            bytes.len() as u32,
            reply.as_mut_ptr(),
            reply.capacity() as u32,
        )
    };
    sdk::check(length)?;
    let length = length as usize;
    if length > reply.capacity() {
        return Err(abi::ERR_BUFFER);
    }
    // A successful host call initializes exactly the returned byte count.
    unsafe { reply.set_len(length) };
    postcard::from_bytes(&reply).map_err(|_| abi::ERR_ARGUMENT)
}

fn command(action: ProgramAction) -> Result<(), i32> {
    let bytes = postcard::to_allocvec(&action).map_err(|_| abi::ERR_ARGUMENT)?;
    sdk::check(unsafe { abi::raw::world_command(bytes.as_ptr(), bytes.len() as u32) })
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
    gate: crate::local_guidance::GateApproach,
    slip_clearance: f64,
    slip_attempts: u8,
    pub speed_limit: f64,
    pub preferences: PlanningPreferences,
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
        let (time, fuel) = self.preferences.cost(self.mass).remaining(
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
        self.preferences = state.preferences;
        if self.revision != Some((state.revision, state.index)) {
            self.revision = Some((state.revision, state.index));
            self.reference_changed = true;
            self.bay = None;
            self.next_estimate = tick;
            self.avoidance.reset();
            self.gate = Default::default();
            self.slip_clearance = 0.;
            self.slip_attempts = 0;
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
            Order::Slip { destination } => {
                let target = resolve_at(destination, 0.)?;
                let space = self.local_space(&pose, &target, 0.)?;
                let departure = crate::local_guidance::outside_exclusions(
                    pose.position,
                    target.position.relative_to(pose.position),
                    &space,
                    self.own_radius + self.slip_clearance,
                );
                let Some(departure) = departure else {
                    return self.steer(&pose, &pose, None).map(Some);
                };
                if departure.relative_to(pose.position).length() > 1. {
                    let mut local = pose.clone();
                    local.position = departure;
                    if let Some(obstacle) = space.obstacles.iter().find(|obstacle| {
                        obstacle.slip_exclusion_m > 0.
                            && pose.position.relative_to(obstacle.pose.position).length()
                                < obstacle.slip_exclusion_m
                                    + self.own_radius
                                    + self.slip_clearance
                                    + 10.
                    }) {
                        local.velocity = obstacle.pose.velocity;
                    }
                    let relative = contact(&pose, &local);
                    self.estimate(tick, &state, Some(self.transfer_estimate(&relative)))?;
                    return self.steer(&pose, &local, None).map(Some);
                }

                if slip_ready {
                    let anchor = space
                        .obstacles
                        .iter()
                        .find(|obstacle| {
                            obstacle.reference == Target::Destination(destination.clone())
                        })
                        .map_or(target.position, |obstacle| obstacle.pose.position);
                    let arrival = crate::local_guidance::outside_exclusions(
                        anchor,
                        pose.position.relative_to(anchor),
                        &space,
                        self.own_radius + self.slip_clearance,
                    )
                    .ok_or(abi::ERR_UNAVAILABLE)?;
                    let arrival_offset = arrival.relative_to(anchor);
                    let solution = solve_slip(
                        |after| {
                            Ok(pose
                                .position
                                .offset_by(DVec3::from_array(pose.velocity) * after))
                        },
                        |after| {
                            let target = resolve_at(destination, after)?;
                            Ok(target.position.offset_by(arrival_offset))
                        },
                    );
                    let solution = match solution {
                        Ok(solution) => solution,
                        Err(abi::ERR_UNAVAILABLE) if self.slip_attempts < 8 => {
                            self.slip_attempts += 1;
                            let volume_margin = space
                                .obstacles
                                .iter()
                                .map(|obstacle| obstacle.slip_exclusion_m * 0.1)
                                .fold(1000., f64::max);
                            self.slip_clearance = (self.slip_clearance * 2.)
                                .max(volume_margin)
                                .min(toy_sim_model::local_space::MAX_RANGE_M);
                            return Ok(None);
                        }
                        Err(error) => return Err(error),
                    };
                    self.estimate(tick, &state, Some((solution.seconds, 0.)))?;
                    command(ProgramAction::Slip {
                        revision: state.revision,
                        order: state.index,
                        destination: solution.destination,
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
            Order::Jump(entry) => {
                let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(*entry))? else {
                    return Err(abi::ERR_ARGUMENT);
                };
                let beacon = beacons
                    .iter()
                    .find(|beacon| beacon.entity == *entry && beacon.gate_exit.is_some())
                    .ok_or(abi::ERR_UNAVAILABLE)?;
                let own_radius = self.own_radius;
                let clearance = beacon.radius_m - own_radius - 2.;
                if clearance <= 0. {
                    return Err(abi::ERR_UNAVAILABLE);
                }
                let stand_off = beacon.radius_m + own_radius + 100.;
                let target = self.gate.guide(&pose, &beacon.pose, stand_off);
                self.reference_changed |= target.changed;
                let relative = contact(&pose, &target.pose);
                let (time, fuel) = self.transfer_estimate(&relative);
                self.estimate(tick, &state, Some((time + 0.1, fuel)))?;
                let ignored = target
                    .crossing
                    .then_some(Target::Destination(Destination::Beacon(*entry)));
                let result = self.steer(&pose, &target.pose, ignored.as_ref())?;
                self.speed_limit = self.speed_limit.min(target.speed_limit);
                Ok(Some(result))
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

    fn local_space(
        &self,
        pose: &Pose,
        target: &Pose,
        after_seconds: f64,
    ) -> Result<LocalSpace, i32> {
        let relative_speed =
            (DVec3::from_array(pose.velocity) - DVec3::from_array(target.velocity)).length();
        let response = 2. * self.turn_s + 2.;
        let range_m = (relative_speed * relative_speed / self.acceleration
            + relative_speed * response
            + 1000.)
            .clamp(1000., toy_sim_model::local_space::MAX_RANGE_M);
        let ProgramReply::LocalSpace(space) = query(&ProgramQuery::LocalSpace {
            destination: target.position,
            range_m,
            after_seconds,
        })?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        Ok(space)
    }

    fn steer(
        &mut self,
        pose: &Pose,
        target: &Pose,
        ignored: Option<&Target>,
    ) -> Result<abi::Contact, i32> {
        let space = self.local_space(pose, target, 0.)?;
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
            let local_speed = crate::navigation::arrival_speed(
                clearance,
                self.acceleration,
                2. * self.turn_s + 2.,
            )
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
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            Ok((ready, preparation_s, duration_s))
        },
    )
}
