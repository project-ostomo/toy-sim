use crate::world_graph::{Gate, Graph};
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

const BEACONS_PER_TICK: u16 = 16;
const MAX_BEACONS: usize = 4096;
const MAX_GATES: usize = 256;
const RETRY_TICKS: u64 = 50;

struct Search {
    destination: Destination,
    target: Pose,
    origin: Pose,
    after: Option<EntityId>,
    seen: usize,
    gates: Vec<Gate>,
    graph: Option<Graph>,
    slip_endpoints: [bool; 2],
    slip_base_s: f64,
}

#[derive(Default)]
pub struct Planner {
    revision: Option<(u64, usize)>,
    pub active: bool,
    state_revision: u64,
    pub aim_direction: Option<[f64; 3]>,
    search: Option<Search>,
    retry_at: u64,
    bay: Option<(EntityId, u32)>,
    reserve_at: u64,
    acceleration: f64,
    flow: f64,
    mass: f64,
    next_estimate: u64,
    turn_s: f64,
    pub preferences: PlanningPreferences,
    fuel_rates: Vec<(u64, f64)>,
}

impl Planner {
    pub fn update(
        &mut self,
        tick: u64,
        sample: &crate::hardware::Sample,
        hardware: &crate::hardware::Hardware,
    ) -> Result<Option<abi::Contact>, i32> {
        let bindings = crate::Bindings::new(hardware);
        self.acceleration = (bindings.thrust / sample.mass_kg.max(1.)).max(1e-6);
        self.flow = bindings.propellant_rate;
        self.fuel_rates.clear();
        for column in &bindings.layout.columns {
            let projection = column.force.dot(bindings.engine_axis);
            if (column.axis.is_none() && projection <= 0.) || projection.abs() < 1e-8 {
                continue;
            }
            let resource = match hardware.get(column.handle).map(|device| &device.capability) {
                Some(crate::hardware::Capability::Engine(spec)) => spec.propellant_resource,
                Some(crate::hardware::Capability::Rcs(spec)) => spec.propellant_resource,
                _ => continue,
            };
            if resource == 0 || column.propellant_rate <= 0. {
                continue;
            }
            if let Some((_, rate)) = self.fuel_rates.iter_mut().find(|(id, _)| *id == resource) {
                *rate += column.propellant_rate;
            } else {
                self.fuel_rates.push((resource, column.propellant_rate));
            }
        }
        if self.flow <= 0. {
            self.flow = self.fuel_rates.iter().map(|(_, rate)| rate).sum();
        }
        self.mass = sample.mass_kg;
        self.turn_s = crate::attitude::turn_allowance(
            glam::DMat3::from_cols_array(&sample.inertia),
            &bindings,
        );
        let result = self.step(tick);
        if let Err(error) = result {
            if self.active {
                self.search = None;
                self.retry_at = tick.saturating_add(RETRY_TICKS);
                self.active = false;
                command(ProgramAction::Block {
                    revision: self.state_revision,
                    reason: format!("Routing query failed ({error}); retrying"),
                })?;
            }
        }
        result
    }

    fn plan(&mut self, state: &TravelState, pose: Pose) -> Result<Option<Vec<QueuedOrder>>, i32> {
        let Some(order) = state.orders.get(state.order).map(|stage| &stage.action) else {
            return Ok(Some(Vec::new()));
        };
        let destination = match order {
            Order::TravelTo(destination) => destination.clone(),
            Order::Dock(station) => Destination::Beacon(*station),
            Order::Undock => {
                return Ok(Some(vec![
                    QueuedOrder::estimated(Order::Undock, 0.1).with_propellant(0.),
                ]));
            }
            _ => return Ok(Some(vec![state.orders[state.order].clone()])),
        };
        if self.search.is_none() {
            let destination = if let Destination::Beacon(id) = destination {
                let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(id))? else {
                    return Err(abi::ERR_ARGUMENT);
                };
                let beacon = beacons
                    .iter()
                    .find(|beacon| beacon.entity == id)
                    .ok_or(abi::ERR_UNAVAILABLE)?;
                if matches!(order, Order::Dock(_))
                    && beacon.pose.position.relative_to(pose.position).length() < 1e7
                {
                    return Ok(Some(vec![
                        QueuedOrder::estimated(
                            Order::Dock(id),
                            self.transfer_estimate(&contact(&pose, &beacon.pose)).0,
                        )
                        .with_propellant(self.transfer_estimate(&contact(&pose, &beacon.pose)).1),
                    ]));
                }
                let clearance = beacon.radius_m
                    + sdk::flight()?.radius_m
                    + if beacon.gate_exit.is_some() {
                        beacon.exclusion_m + 1000.
                    } else {
                        100.
                    };
                Destination::Relative {
                    reference: Reference::Beacon(id),
                    offset: GalacticPosition::from_meters(
                        if matches!(order, Order::Dock(_)) {
                            pose.position
                                .relative_to(beacon.pose.position)
                                .try_normalize()
                                .unwrap_or(DVec3::Z)
                        } else {
                            DVec3::NEG_Z
                        } * clearance,
                    ),
                    axes: if matches!(order, Order::Dock(_)) {
                        Axes::Galactic
                    } else {
                        Axes::BodyFixed
                    },
                }
            } else {
                destination
            };
            let ProgramReply::Pose(target) = query(&ProgramQuery::Resolve(destination.clone()))?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            if target.position.relative_to(pose.position).length() < 100. {
                return Ok(Some(vec![
                    QueuedOrder::estimated(
                        if let Order::Dock(station) = order {
                            Order::Dock(*station)
                        } else {
                            Order::Sublight(destination)
                        },
                        self.transfer_estimate(&contact(&pose, &target)).0,
                    )
                    .with_propellant(self.transfer_estimate(&contact(&pose, &target)).1),
                ]));
            }
            let endpoint = |position| -> Result<(bool, f64), i32> {
                let ProgramReply::SlipEligibility { ready, duration_s } =
                    query(&ProgramQuery::SlipEligibility {
                        origin: position,
                        destination: position,
                    })?
                else {
                    return Err(abi::ERR_ARGUMENT);
                };
                Ok((ready, duration_s))
            };
            let (origin_slip, slip_base_s) = endpoint(pose.position)?;
            let (target_slip, _) = endpoint(target.position)?;
            self.search = Some(Search {
                destination,
                target,
                origin: pose.clone(),
                after: None,
                seen: 0,
                gates: Vec::new(),
                graph: None,
                slip_endpoints: [origin_slip, target_slip],
                slip_base_s,
            });
        }
        let search = self.search.as_mut().unwrap();
        if search.graph.is_none() {
            let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacons {
                after: search.after,
                limit: BEACONS_PER_TICK,
            })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            let complete = beacons.len() < BEACONS_PER_TICK as usize;
            for beacon in beacons {
                if search.after.is_some_and(|after| beacon.entity <= after) {
                    return Err(abi::ERR_ARGUMENT);
                }
                search.after = Some(beacon.entity);
                search.seen += 1;
                if search.seen > MAX_BEACONS {
                    return Err(abi::ERR_UNAVAILABLE);
                }
                if let Some(exit) = beacon.gate_exit {
                    if search.gates.len() == MAX_GATES {
                        return Err(abi::ERR_UNAVAILABLE);
                    }
                    let direction = beacon
                        .pose
                        .position
                        .relative_to(search.origin.position)
                        .try_normalize()
                        .unwrap_or(DVec3::X);
                    let staging = beacon.pose.position.offset_by(
                        direction * (beacon.exclusion_m + sdk::flight()?.radius_m + 1000.),
                    );
                    let ProgramReply::SlipEligibility {
                        ready: slip_allowed,
                        ..
                    } = query(&ProgramQuery::SlipEligibility {
                        origin: staging,
                        destination: staging,
                    })?
                    else {
                        return Err(abi::ERR_ARGUMENT);
                    };
                    search.gates.push(Gate {
                        entity: beacon.entity,
                        position: beacon.pose.position,
                        velocity: DVec3::from_array(beacon.pose.velocity),
                        exit,
                        staging,
                        slip_allowed,
                    });
                }
            }
            if !complete {
                return Ok(None);
            }
            search.graph = Some(Graph::new(
                search.origin.position,
                search.target.position,
                &search.gates,
                self.acceleration,
                self.flow,
                search.slip_base_s,
                search.slip_endpoints,
                [
                    DVec3::from_array(search.origin.velocity),
                    DVec3::from_array(search.target.velocity),
                ],
            ));
            search.graph.as_mut().unwrap().weights = self.preferences.cost(self.mass);
        }
        let graph = search.graph.as_mut().unwrap();
        let Some(path) = graph.advance() else {
            return Ok(None);
        };
        let mut orders: Vec<QueuedOrder> = Vec::new();
        let mut transfer_s = 0.;
        let mut transfer_kg = 0.;
        for pair in path.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            if graph.exits[from] == Some(to) {
                let entry = &search.gates[from - 2];
                orders.push(
                    QueuedOrder::estimated(Order::Jump(entry.entity), transfer_s + 0.1)
                        .with_propellant(transfer_kg),
                );
                transfer_s = 0.;
                transfer_kg = 0.;
            } else if graph.is_slip(from, to) {
                orders.push(
                    QueuedOrder::estimated(
                        Order::Slip {
                            destination: graph.positions[to],
                        },
                        graph.costs(from, to).1,
                    )
                    .with_propellant(0.),
                );
                (transfer_s, transfer_kg) = graph.arrival_adjustment(from, to);
                if to != 1 {
                    orders.push(
                        QueuedOrder::estimated(
                            Order::Sublight(graph.destination(to, &search.gates)),
                            transfer_s,
                        )
                        .with_propellant(transfer_kg),
                    );
                    transfer_s = 0.;
                    transfer_kg = 0.;
                }
            } else {
                let (time, fuel) = graph.transfer(from, to);
                transfer_s += time;
                transfer_kg += fuel;
                if to != 1 && graph.exits[to].is_none() {
                    orders.push(
                        QueuedOrder::estimated(
                            Order::Sublight(graph.destination(to, &search.gates)),
                            transfer_s,
                        )
                        .with_propellant(transfer_kg),
                    );
                    transfer_s = 0.;
                    transfer_kg = 0.;
                }
            }
        }
        let final_order = if let Order::Dock(station) = order {
            Order::Dock(*station)
        } else {
            Order::Sublight(search.destination.clone())
        };
        orders.push(QueuedOrder::estimated(final_order, transfer_s).with_propellant(transfer_kg));
        if orders.len() > 256 {
            return Err(abi::ERR_UNAVAILABLE);
        }
        self.search = None;
        Ok(Some(orders))
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

    fn fuel_budget(&self, estimates: impl Iterator<Item = Option<f64>>) -> Result<FuelBudget, i32> {
        let mut required = 0.;
        let mut complete = true;
        for estimate in estimates {
            match estimate.filter(|kg| kg.is_finite() && *kg >= 0.) {
                Some(kg) => required += kg,
                None => complete = false,
            }
        }
        let flow: f64 = self.fuel_rates.iter().map(|(_, rate)| rate).sum();
        let mut resources = Vec::new();
        for &(id, rate) in &self.fuel_rates {
            let info = sdk::resource_info((id - 1) as u32)?;
            let amount = sdk::resource(id)?;
            resources.push(FuelRequirement {
                resource: info.key.as_str().ok_or(abi::ERR_ARGUMENT)?.into(),
                required_kg: required * rate / flow,
                available_kg: amount.units as f64 * info.unit_mass_kg,
            });
        }
        Ok(FuelBudget {
            resources,
            complete: complete && (flow > 0. || required == 0.),
        })
    }

    fn estimate(
        &mut self,
        tick: u64,
        state: &TravelState,
        estimate: Option<(f64, f64)>,
    ) -> Result<(), i32> {
        if tick < self.next_estimate {
            return Ok(());
        }
        self.next_estimate = tick.saturating_add(10);
        let future = state
            .orders
            .iter()
            .skip(state.order + 1)
            .map(|stage| stage.estimated_propellant_kg);
        let fuel_budget =
            self.fuel_budget(std::iter::once(estimate.map(|(_, fuel)| fuel)).chain(future))?;
        command(ProgramAction::Estimate {
            revision: state.revision,
            order: state.order,
            remaining_ticks: estimate
                .map(|(seconds, _)| seconds)
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.)
                .map(|seconds| (seconds * 10.).ceil() as u64),
            fuel_budget,
        })
    }

    fn step(&mut self, tick: u64) -> Result<Option<abi::Contact>, i32> {
        self.aim_direction = None;
        let ProgramReply::Travel {
            state,
            pose,
            slip_ready,
        } = query(&ProgramQuery::Travel)?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        self.state_revision = state.revision;
        self.preferences = state.preferences;
        if self.revision != Some((state.revision, state.order)) {
            self.revision = Some((state.revision, state.order));
            self.search = None;
            self.bay = None;
            self.retry_at = tick;
            self.next_estimate = tick;
        }
        self.active = state.autopilot_enabled
            && matches!(
                state.status,
                Status::Planning | Status::Active | Status::Blocked(_)
            );
        if !self.active {
            self.search = None;
            return Ok(None);
        }
        if state.status == Status::Planning || matches!(state.status, Status::Blocked(_)) {
            if tick < self.retry_at {
                return Ok(None);
            }
            if let Some(orders) = self.plan(&state, pose)? {
                let fuel_budget = self.fuel_budget(
                    orders
                        .iter()
                        .chain(state.orders.iter().skip(state.order + 1))
                        .map(|stage| stage.estimated_propellant_kg),
                )?;
                command(ProgramAction::Route {
                    revision: state.revision,
                    orders,
                    fuel_budget,
                })?;
            }
            return Ok(None);
        }
        let complete = || {
            command(ProgramAction::CompleteOrder {
                revision: state.revision,
                order: state.order,
            })
        };
        let Some(order) = state.orders.get(state.order).map(|stage| &stage.action) else {
            complete()?;
            return Ok(None);
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
                    Target::Destination(destination) => {
                        let ProgramReply::Pose(pose) =
                            query(&ProgramQuery::Resolve(destination.clone()))?
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
                let range = guidance.range_m.max(radius + sdk::flight()?.radius_m + 2.);
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
                Ok(Some(relative))
            }
            Order::Sublight(destination) => {
                let ProgramReply::Pose(target) =
                    query(&ProgramQuery::Resolve(destination.clone()))?
                else {
                    return Err(abi::ERR_ARGUMENT);
                };
                let contact = contact(&pose, &target);
                if DVec3::from_array(contact.position_m).length() <= 2.
                    && DVec3::from_array(contact.velocity_m_s).length() <= 0.5
                {
                    complete()?;
                    return Ok(None);
                }
                self.estimate(tick, &state, Some(self.transfer_estimate(&contact)))?;
                Ok(Some(contact))
            }
            Order::Slip { destination } => {
                let seconds = state
                    .estimated_arrival_tick
                    .map_or(f64::INFINITY, |arrival| {
                        arrival.saturating_sub(tick) as f64 * 0.1
                    });
                self.estimate(tick, &state, Some((seconds, 0.)))?;
                if slip_ready {
                    command(ProgramAction::Slip(*destination))?;
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
                let own_radius = sdk::flight()?.radius_m;
                let clearance = beacon.radius_m - own_radius - 2.;
                if clearance <= 0. {
                    return Err(abi::ERR_UNAVAILABLE);
                }
                let relative = contact(&pose, &beacon.pose);
                let (time, fuel) = self.transfer_estimate(&relative);
                self.estimate(tick, &state, Some((time + 0.1, fuel)))?;
                Ok(Some(relative))
            }
            Order::Undock => {
                command(ProgramAction::Undock)?;
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
                        station: *station,
                        bay,
                    })?;
                    self.bay = Some((*station, bay));
                    self.reserve_at = tick.saturating_add(100);
                }
                let ship_radius = sdk::flight()?.radius_m;
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
                    return Ok(Some(relative));
                }
                command(ProgramAction::Dock {
                    station: *station,
                    bay,
                })?;
                Ok(None)
            }
        }
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
