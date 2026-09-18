use crate::world_graph::{Gate, Graph};
use glam::DVec3;
use toy_sim_model::{travel::*, *};
use toy_sim_ship_api::{abi, sdk};

fn query(query: &ProgramQuery) -> Result<ProgramReply, i32> {
    let bytes = postcard::to_allocvec(query).map_err(|_| abi::ERR_ARGUMENT)?;
    let mut reply = vec![0; 65536];
    let length = unsafe {
        abi::raw::world_query(
            bytes.as_ptr(),
            bytes.len() as u32,
            reply.as_mut_ptr(),
            reply.len() as u32,
        )
    };
    sdk::check(length)?;
    postcard::from_bytes(&reply[..length as usize]).map_err(|_| abi::ERR_ARGUMENT)
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
    slip_ready: bool,
}

#[derive(Default)]
pub struct Planner {
    revision: Option<(u64, usize)>,
    pub active: bool,
    state_revision: u64,
    pub docking_attitude: Option<[f64; 4]>,
    pub aim_direction: Option<[f64; 3]>,
    pub engagement: Option<abi::Contact>,
    docking_entry: bool,
    search: Option<Search>,
    retry_at: u64,
    bay: Option<(EntityId, u32)>,
    reserve_at: u64,
}

impl Planner {
    pub fn update(&mut self, tick: u64) -> Result<Option<abi::Contact>, i32> {
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

    fn plan(&mut self, state: &TravelState, pose: Pose) -> Result<Option<Vec<Leg>>, i32> {
        let Some(order) = state.orders.get(state.order) else {
            return Ok(Some(Vec::new()));
        };
        let destination = match order {
            Order::Jump(entry) => {
                let ProgramReply::Beacons(beacons) = query(&ProgramQuery::Beacon(*entry))? else {
                    return Err(abi::ERR_ARGUMENT);
                };
                let exit = beacons
                    .first()
                    .and_then(|b| b.gate_exit)
                    .ok_or(abi::ERR_UNAVAILABLE)?;
                return Ok(Some(vec![
                    Leg::Sublight(Destination::Beacon(*entry)),
                    Leg::Gate {
                        entry: *entry,
                        exit,
                    },
                ]));
            }
            Order::Guidance(guidance) => return Ok(Some(vec![Leg::Guidance(guidance.clone())])),
            Order::TravelTo(destination) => destination.clone(),
            Order::Dock(station) => Destination::Beacon(*station),
            Order::Undock => return Ok(Some(vec![Leg::Undock])),
            Order::WaitUntil(tick) => return Ok(Some(vec![Leg::WaitUntil(*tick)])),
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
                let clearance = beacon.radius_m
                    + sdk::flight()?.radius_m
                    + if beacon.gate_exit.is_some() {
                        1e7 + 100.
                    } else {
                        100.
                    };
                Destination::Relative {
                    reference: Reference::Beacon(id),
                    offset: GalacticPosition::from_meters(DVec3::NEG_Z * clearance),
                    axes: Axes::BodyFixed,
                }
            } else {
                destination
            };
            let ProgramReply::Pose(target) = query(&ProgramQuery::Resolve(destination.clone()))?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            if target.position.relative_to(pose.position).length() < 1e7 {
                let mut legs = vec![Leg::Sublight(destination)];
                if let Order::Dock(station) = order {
                    legs.push(Leg::Dock(*station));
                }
                return Ok(Some(legs));
            }
            let ProgramReply::SlipEligibility { ready: slip_ready } =
                query(&ProgramQuery::SlipEligibility {
                    destination: target.position,
                })?
            else {
                return Err(abi::ERR_ARGUMENT);
            };
            self.search = Some(Search {
                destination,
                target,
                origin: pose.clone(),
                after: None,
                seen: 0,
                gates: Vec::new(),
                graph: None,
                slip_ready,
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
                    search.gates.push(Gate {
                        entity: beacon.entity,
                        position: beacon.pose.position,
                        exit,
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
            ));
        }
        let graph = search.graph.as_mut().unwrap();
        let Some(path) = graph.advance() else {
            return Ok(None);
        };
        let direct = search
            .target
            .position
            .relative_to(search.origin.position)
            .length();
        let mut legs = Vec::new();
        if search.slip_ready && direct > 1e9 && graph.distances[1] > direct * 0.5 {
            legs.push(Leg::Sublight(Destination::Galactic(pose.position)));
            legs.push(Leg::Slip {
                destination: search.target.position,
            });
        } else {
            let mut index = 1;
            while index < path.len() {
                let node = path[index];
                if node == 1 {
                    break;
                }
                let entry = &search.gates[node - 2];
                if path.get(index + 1).copied() == graph.exits[node] {
                    legs.push(Leg::Sublight(Destination::Relative {
                        reference: Reference::Beacon(entry.entity),
                        offset: GalacticPosition::ZERO,
                        axes: Axes::BodyFixed,
                    }));
                    legs.push(Leg::Gate {
                        entry: entry.entity,
                        exit: entry.exit,
                    });
                    index += 2;
                } else {
                    index += 1;
                }
            }
        }
        legs.push(Leg::Sublight(search.destination.clone()));
        if let Order::Dock(station) = order {
            legs.push(Leg::Dock(*station));
        }
        if legs.len() > 256 {
            return Err(abi::ERR_UNAVAILABLE);
        }
        self.search = None;
        Ok(Some(legs))
    }

    fn step(&mut self, tick: u64) -> Result<Option<abi::Contact>, i32> {
        self.docking_attitude = None;
        self.aim_direction = None;
        self.engagement = None;
        let ProgramReply::Travel {
            state,
            pose,
            slip_ready,
        } = query(&ProgramQuery::Travel)?
        else {
            return Err(abi::ERR_ARGUMENT);
        };
        self.state_revision = state.revision;
        if self.revision != Some((state.revision, state.order)) {
            self.revision = Some((state.revision, state.order));
            self.search = None;
            self.bay = None;
            self.docking_entry = false;
            self.retry_at = tick;
        }
        self.active = matches!(
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
            if let Some(legs) = self.plan(&state, pose)? {
                command(ProgramAction::Route {
                    revision: state.revision,
                    legs,
                })?;
            }
            return Ok(None);
        }
        let complete = || {
            command(ProgramAction::CompleteLeg {
                revision: state.revision,
                leg: state.leg,
            })
        };
        let Some(leg) = state.legs.get(state.leg) else {
            complete()?;
            return Ok(None);
        };
        match leg {
            Leg::Guidance(guidance) => {
                let (target, handle, radius) = match &guidance.target {
                    Target::Destination(destination) => {
                        let ProgramReply::Pose(pose) =
                            query(&ProgramQuery::Resolve(destination.clone()))?
                        else {
                            return Err(abi::ERR_ARGUMENT);
                        };
                        (pose, u64::MAX, 0.)
                    }
                    Target::Contact(reference) => {
                        let ProgramReply::Contact {
                            pose,
                            handle,
                            radius_m,
                        } = query(&ProgramQuery::Contact(*reference))?
                        else {
                            return Err(abi::ERR_ARGUMENT);
                        };
                        (pose, handle, radius_m)
                    }
                };
                let mut relative = contact(&pose, &target);
                let offset = DVec3::from_array(relative.position_m);
                if guidance.mode == GuidanceMode::Align {
                    let direction = offset.try_normalize().ok_or(abi::ERR_UNAVAILABLE)?;
                    self.aim_direction = Some(direction.to_array());
                    let forward = glam::DQuat::from_array(pose.rotation) * DVec3::NEG_Z;
                    if forward.angle_between(direction) < 0.02
                        && DVec3::from_array(pose.angular_velocity).length() < 0.05
                    {
                        complete()?;
                    }
                    return Ok(None);
                }
                if guidance.mode == GuidanceMode::Engage {
                    let mut enemy = relative;
                    enemy.id = handle;
                    enemy.radius_m = radius;
                    self.engagement = Some(enemy);
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
                Ok(Some(relative))
            }
            Leg::Sublight(destination) => {
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
                Ok(Some(contact))
            }
            Leg::Slip { destination } => {
                if slip_ready {
                    command(ProgramAction::Slip(*destination))?;
                }
                Ok(None)
            }
            Leg::Gate { entry, .. } => {
                command(ProgramAction::Gate(*entry))?;
                Ok(None)
            }
            Leg::Undock => {
                command(ProgramAction::Undock)?;
                Ok(None)
            }
            Leg::WaitUntil(until) => {
                if tick >= *until {
                    complete()?;
                }
                Ok(None)
            }
            Leg::Dock(station) => {
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
                let berth = beacon.bays.get(&bay).ok_or(abi::ERR_UNAVAILABLE)?;
                let mut entry = berth.clone();
                entry.position = entry.position.offset_by(
                    glam::DQuat::from_array(berth.rotation)
                        * DVec3::NEG_Z
                        * (beacon.radius_m + sdk::flight()?.radius_m + 30.),
                );
                if !self.docking_entry
                    && entry.position.relative_to(pose.position).length() < 5.
                    && (DVec3::from_array(entry.velocity) - DVec3::from_array(pose.velocity))
                        .length()
                        < 0.5
                {
                    self.docking_entry = true;
                }
                let target = if self.docking_entry { berth } else { &entry };
                self.docking_attitude = Some(target.rotation);
                let contact = contact(&pose, target);
                if DVec3::from_array(contact.position_m).length() > 2.
                    || DVec3::from_array(contact.velocity_m_s).length() > 0.3
                {
                    return Ok(Some(contact));
                }
                if glam::DQuat::from_array(pose.rotation)
                    .angle_between(glam::DQuat::from_array(target.rotation))
                    .abs()
                    > 5_f64.to_radians()
                {
                    return Ok(None);
                }
                if !self.docking_entry {
                    return Ok(None);
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
