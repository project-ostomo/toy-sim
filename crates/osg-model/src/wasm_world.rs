use crate::{GalacticPosition, Id, ProgramAction, ProgramQuery, ProgramReply, routing, travel};
use osg_ship_api::{abi::Text, world as abi};

#[derive(Clone, Copy, Debug, Default)]
pub struct ReplyCapacity {
    pub records: usize,
    pub auxiliary: usize,
    pub bytes: usize,
}

impl ReplyCapacity {
    pub const UNLIMITED: Self = Self {
        records: usize::MAX,
        auxiliary: usize::MAX,
        bytes: usize::MAX,
    };
}

fn flag(value: u64) -> Result<bool, ()> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(()),
    }
}

fn optional<T>(present: u64, value: T) -> Result<Option<T>, ()> {
    Ok(flag(present)?.then_some(value))
}

pub fn text<const N: usize>(value: &Text<N>) -> Result<String, ()> {
    let len = usize::try_from(value.len).map_err(|_| ())?;
    Ok(std::str::from_utf8(value.bytes.get(..len).ok_or(())?)
        .map_err(|_| ())?
        .to_owned())
}

pub fn position_record(value: &GalacticPosition) -> abi::Position {
    let mut words = [0; 6];
    for (index, coordinate) in [value.x, value.y, value.z].into_iter().enumerate() {
        words[index * 2] = coordinate as u64;
        words[index * 2 + 1] = (coordinate >> 64) as u64;
    }
    abi::Position { words }
}

pub fn position_value(value: &abi::Position) -> GalacticPosition {
    let coordinate = |index: usize| {
        ((value.words[index * 2 + 1] as i128) << 64) | value.words[index * 2] as i128
    };
    GalacticPosition::new(coordinate(0), coordinate(1), coordinate(2))
}

impl From<&crate::Pose> for abi::Pose {
    fn from(value: &crate::Pose) -> Self {
        Self {
            position: position_record(&value.position),
            velocity: value.velocity,
            rotation: value.rotation,
            angular_velocity: value.angular_velocity,
        }
    }
}

impl From<&abi::Pose> for crate::Pose {
    fn from(value: &abi::Pose) -> Self {
        Self {
            position: position_value(&value.position),
            velocity: value.velocity,
            rotation: value.rotation,
            angular_velocity: value.angular_velocity,
        }
    }
}

impl From<&crate::ContactRef> for abi::ContactRef {
    fn from(value: &crate::ContactRef) -> Self {
        Self {
            observer: value.observer.0,
            contact: value.contact,
        }
    }
}

impl From<&abi::ContactRef> for crate::ContactRef {
    fn from(value: &abi::ContactRef) -> Self {
        Self {
            observer: Id(value.observer),
            contact: value.contact,
        }
    }
}

impl From<&travel::Destination> for abi::Destination {
    fn from(value: &travel::Destination) -> Self {
        let mut output = Self::default();
        match value {
            travel::Destination::Beacon(id) => output.entity = id.0,
            travel::Destination::Galactic(position) => {
                output.kind = 1;
                output.position = position_record(position);
            }
            travel::Destination::Relative {
                reference,
                offset,
                axes,
            } => {
                let (kind, id) = match reference {
                    travel::Reference::Celestial(reference) => {
                        output.system = reference.system.0;
                        (2, &reference.body)
                    }
                    travel::Reference::Beacon(id) => (3, id),
                };
                output.kind = kind;
                output.entity = id.0;
                output.position = position_record(offset);
                output.axes = matches!(axes, travel::Axes::BodyFixed) as u64;
            }
        }
        output
    }
}

impl TryFrom<&abi::Destination> for travel::Destination {
    type Error = ();

    fn try_from(value: &abi::Destination) -> Result<Self, ()> {
        Ok(match value.kind {
            0 => Self::Beacon(Id(value.entity)),
            1 => Self::Galactic(position_value(&value.position)),
            2 | 3 => Self::Relative {
                reference: if value.kind == 2 {
                    travel::Reference::Celestial(travel::CelestialRef {
                        system: Id(value.system),
                        body: Id(value.entity),
                    })
                } else {
                    travel::Reference::Beacon(Id(value.entity))
                },
                offset: position_value(&value.position),
                axes: if flag(value.axes)? {
                    travel::Axes::BodyFixed
                } else {
                    travel::Axes::Galactic
                },
            },
            _ => return Err(()),
        })
    }
}

impl From<&travel::Target> for abi::Target {
    fn from(value: &travel::Target) -> Self {
        let mut output = Self::default();
        match value {
            travel::Target::Direction(direction) => output.direction = *direction,
            travel::Target::Destination(destination) => {
                output.kind = 1;
                output.destination = destination.into();
            }
            travel::Target::Contact(contact) => {
                output.kind = 2;
                output.contact = contact.into();
            }
        }
        output
    }
}

impl TryFrom<&abi::Target> for travel::Target {
    type Error = ();

    fn try_from(value: &abi::Target) -> Result<Self, ()> {
        Ok(match value.kind {
            0 => Self::Direction(value.direction),
            1 => Self::Destination((&value.destination).try_into()?),
            2 => Self::Contact((&value.contact).into()),
            _ => return Err(()),
        })
    }
}

impl From<&travel::Order> for abi::Order {
    fn from(value: &travel::Order) -> Self {
        let mut output = Self::default();
        match value {
            travel::Order::Guidance(guidance) => {
                output.kind = 1;
                output.target = (&guidance.target).into();
                output.mode = match guidance.mode {
                    travel::GuidanceMode::Align => 0,
                    travel::GuidanceMode::Approach => 1,
                    travel::GuidanceMode::KeepRange => 2,
                };
                output.range_m = guidance.range_m;
            }
            travel::Order::TravelTo(destination) => {
                output.kind = 2;
                output.destination = destination.into();
            }
            travel::Order::TravelToSystem(id) => {
                output.kind = 8;
                output.entity = id.0;
            }
            travel::Order::Sublight(destination) => {
                output.kind = 3;
                output.destination = destination.into();
            }
            travel::Order::Slip {
                destination,
                navigation_beacon,
            } => {
                output.kind = 4;
                output.destination = destination.into();
                output.navigation_beacon_present = navigation_beacon.is_some() as u64;
                output.navigation_beacon = navigation_beacon.unwrap_or_default().0;
            }
            travel::Order::Dock(id) => {
                output.kind = 5;
                output.entity = id.0;
            }
            travel::Order::Undock => output.kind = 6,
            travel::Order::WaitUntil(tick) => {
                output.kind = 7;
                output.tick = *tick;
            }
        }
        output
    }
}

impl TryFrom<&abi::Order> for travel::Order {
    type Error = ();

    fn try_from(value: &abi::Order) -> Result<Self, ()> {
        Ok(match value.kind {
            1 => Self::Guidance(travel::Guidance {
                mode: match value.mode {
                    0 => travel::GuidanceMode::Align,
                    1 => travel::GuidanceMode::Approach,
                    2 => travel::GuidanceMode::KeepRange,
                    _ => return Err(()),
                },
                target: (&value.target).try_into()?,
                range_m: value.range_m,
            }),
            2 => Self::TravelTo((&value.destination).try_into()?),
            3 => Self::Sublight((&value.destination).try_into()?),
            4 => Self::Slip {
                destination: (&value.destination).try_into()?,
                navigation_beacon: optional(
                    value.navigation_beacon_present,
                    Id(value.navigation_beacon),
                )?,
            },
            5 => Self::Dock(Id(value.entity)),
            6 => Self::Undock,
            7 => Self::WaitUntil(value.tick),
            8 => Self::TravelToSystem(Id(value.entity)),
            _ => return Err(()),
        })
    }
}

impl From<&travel::QueuedOrder> for abi::QueuedOrder {
    fn from(value: &travel::QueuedOrder) -> Self {
        Self {
            label: Text::new(&value.label),
            action: (&value.action).into(),
            seconds_per_kg: value.transfer_cost.seconds_per_kg,
            duration_present: value.estimated_duration_ticks.is_some() as u64,
            duration_ticks: value.estimated_duration_ticks.unwrap_or_default(),
            propellant_present: value.estimated_propellant_kg.is_some() as u64,
            propellant_kg: value.estimated_propellant_kg.unwrap_or_default(),
            loss_present: value.estimated_loss_ppm.is_some() as u64,
            loss_ppm: value.estimated_loss_ppm.unwrap_or_default(),
        }
    }
}

impl TryFrom<&abi::QueuedOrder> for travel::QueuedOrder {
    type Error = ();

    fn try_from(value: &abi::QueuedOrder) -> Result<Self, ()> {
        Ok(Self {
            label: text(&value.label)?,
            action: (&value.action).try_into()?,
            transfer_cost: crate::transfer::TransferCost {
                seconds_per_kg: value.seconds_per_kg,
            },
            estimated_duration_ticks: optional(value.duration_present, value.duration_ticks)?,
            estimated_propellant_kg: optional(value.propellant_present, value.propellant_kg)?,
            estimated_loss_ppm: optional(value.loss_present, value.loss_ppm)?,
        })
    }
}

impl From<&travel::PlanningPreferences> for abi::Preferences {
    fn from(value: &travel::PlanningPreferences) -> Self {
        Self {
            fuel_fraction: value.fuel_fraction,
            max_loss_ppm: value.max_loss_ppm,
            allow_slipdrive: value.allow_slipdrive as u64,
        }
    }
}

impl TryFrom<&abi::Preferences> for travel::PlanningPreferences {
    type Error = ();

    fn try_from(value: &abi::Preferences) -> Result<Self, ()> {
        let preferences = Self {
            fuel_fraction: value.fuel_fraction,
            max_loss_ppm: value.max_loss_ppm,
            allow_slipdrive: flag(value.allow_slipdrive)?,
        };
        preferences.valid().then_some(preferences).ok_or(())
    }
}

impl From<&crate::LocalObstacle> for abi::LocalObstacle {
    fn from(value: &crate::LocalObstacle) -> Self {
        Self {
            reference: (&value.reference).into(),
            pose: (&value.pose).into(),
            radius_m: value.radius_m,
            slip_exclusion_m: value.slip_exclusion_m,
        }
    }
}

impl TryFrom<&abi::LocalObstacle> for crate::LocalObstacle {
    type Error = ();

    fn try_from(value: &abi::LocalObstacle) -> Result<Self, ()> {
        Ok(Self {
            reference: (&value.reference).try_into()?,
            pose: (&value.pose).into(),
            radius_m: value.radius_m,
            slip_exclusion_m: value.slip_exclusion_m,
        })
    }
}

impl From<&travel::FuelRequirement> for abi::FuelRequirement {
    fn from(value: &travel::FuelRequirement) -> Self {
        Self {
            resource: Text::new(&value.resource),
            required_kg: value.required_kg,
            available_kg: value.available_kg,
        }
    }
}

impl TryFrom<&abi::FuelRequirement> for travel::FuelRequirement {
    type Error = ();

    fn try_from(value: &abi::FuelRequirement) -> Result<Self, ()> {
        Ok(Self {
            resource: text(&value.resource)?,
            required_kg: value.required_kg,
            available_kg: value.available_kg,
        })
    }
}

pub fn travel_record(
    state: &travel::CurrentOrder,
    pose: &crate::Pose,
    slip_ready: bool,
    slip_axis: [f64; 3],
) -> abi::TravelReply {
    let (status, reason) = match &state.status {
        travel::Status::Idle => (0, ""),
        travel::Status::Planning => (1, ""),
        travel::Status::Active => (2, ""),
        travel::Status::Paused => (3, ""),
        travel::Status::Blocked(reason) => (4, reason.as_str()),
        travel::Status::Completed => (5, ""),
    };
    abi::TravelReply {
        autopilot_enabled: state.autopilot_enabled as u64,
        preferences: (&state.preferences).into(),
        revision: state.revision,
        index: state.index as u64,
        order_present: state.order.is_some() as u64,
        order: state.order.as_ref().map(Into::into).unwrap_or_default(),
        status,
        reason: Text::new(reason),
        arrival_present: state.estimated_arrival_tick.is_some() as u64,
        arrival_tick: state.estimated_arrival_tick.unwrap_or_default(),
        pose: pose.into(),
        slip_ready: slip_ready as u64,
        slip_axis,
    }
}

pub fn travel_reply(value: &abi::TravelReply) -> Result<ProgramReply, ()> {
    let status = match value.status {
        0 => travel::Status::Idle,
        1 => travel::Status::Planning,
        2 => travel::Status::Active,
        3 => travel::Status::Paused,
        4 => travel::Status::Blocked(text(&value.reason)?),
        5 => travel::Status::Completed,
        _ => return Err(()),
    };
    Ok(ProgramReply::Travel {
        state: travel::CurrentOrder {
            autopilot_enabled: flag(value.autopilot_enabled)?,
            preferences: (&value.preferences).try_into()?,
            revision: value.revision,
            index: usize::try_from(value.index).map_err(|_| ())?,
            order: if flag(value.order_present)? {
                Some((&value.order).try_into()?)
            } else {
                None
            },
            status,
            estimated_arrival_tick: optional(value.arrival_present, value.arrival_tick)?,
        },
        pose: (&value.pose).into(),
        slip_ready: flag(value.slip_ready)?,
        slip_axis: value.slip_axis,
    })
}

pub fn route_records(
    id: u64,
    status: &routing::Status,
) -> (
    abi::RouteReply,
    Vec<abi::QueuedOrder>,
    Vec<abi::FuelRequirement>,
) {
    let mut header = abi::RouteReply {
        id,
        ..Default::default()
    };
    let mut orders = Vec::new();
    let mut fuels = Vec::new();
    match status {
        routing::Status::Unknown => {}
        routing::Status::Pending { progress } => {
            header.status = 1;
            header.stage = match progress.stage {
                travel::PlanningStage::LoadingCatalogue => 0,
                travel::PlanningStage::BuildingGraph => 1,
                travel::PlanningStage::SearchingRoutes => 2,
            };
            header.completed = progress.completed as u64;
            header.total_present = progress.total.is_some() as u64;
            header.total = progress.total.unwrap_or_default() as u64;
        }
        routing::Status::Ready { plan } => {
            header.status = 2;
            header.planned_tick = plan.planned_tick;
            header.travel_revision = plan.travel_revision;
            header.topology_revision = plan.topology_revision;
            header.order_count = plan.orders.len() as u64;
            header.fuel_count = plan.fuel_budget.resources.len() as u64;
            header.fuel_complete = plan.fuel_budget.complete as u64;
            header.estimated_loss_ppm = plan.estimated_loss_ppm;
            header.exotic_fuel_kg = plan.exotic_fuel_kg;
            orders.extend(plan.orders.iter().map(abi::QueuedOrder::from));
            fuels.extend(
                plan.fuel_budget
                    .resources
                    .iter()
                    .map(abi::FuelRequirement::from),
            );
        }
        routing::Status::Failed { reason } => {
            header.status = 3;
            header.reason = Text::new(reason);
        }
    }
    (header, orders, fuels)
}

pub fn route_reply(
    header: &abi::RouteReply,
    orders: &[abi::QueuedOrder],
    fuels: &[abi::FuelRequirement],
) -> Result<ProgramReply, ()> {
    let status = match header.status {
        0 => routing::Status::Unknown,
        1 => routing::Status::Pending {
            progress: travel::PlanningProgress {
                stage: match header.stage {
                    0 => travel::PlanningStage::LoadingCatalogue,
                    1 => travel::PlanningStage::BuildingGraph,
                    2 => travel::PlanningStage::SearchingRoutes,
                    _ => return Err(()),
                },
                completed: u32::try_from(header.completed).map_err(|_| ())?,
                total: optional(
                    header.total_present,
                    u32::try_from(header.total).map_err(|_| ())?,
                )?,
            },
        },
        2 => routing::Status::Ready {
            plan: routing::Plan {
                estimated_loss_ppm: header.estimated_loss_ppm,
                exotic_fuel_kg: header.exotic_fuel_kg,
                beacon_assumptions: counted(orders, header.order_count)?
                    .iter()
                    .filter(|order| {
                        order.action.kind == abi::ORDER_SLIP
                            && order.action.navigation_beacon_present == 1
                    })
                    .map(|order| Id(order.action.navigation_beacon))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                planned_tick: header.planned_tick,
                travel_revision: header.travel_revision,
                topology_revision: header.topology_revision,
                orders: counted(orders, header.order_count)?
                    .iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                fuel_budget: travel::FuelBudget {
                    resources: counted(fuels, header.fuel_count)?
                        .iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    complete: flag(header.fuel_complete)?,
                },
            },
        },
        3 => routing::Status::Failed {
            reason: text(&header.reason)?,
        },
        _ => return Err(()),
    };
    Ok(ProgramReply::Route {
        id: header.id,
        status,
    })
}

fn counted<T>(values: &[T], count: u64) -> Result<&[T], ()> {
    values
        .get(..usize::try_from(count).map_err(|_| ())?)
        .ok_or(())
}

impl From<&abi::OrreryQuery> for ProgramQuery {
    fn from(value: &abi::OrreryQuery) -> Self {
        Self::Orrery {
            reference: position_value(&value.reference),
        }
    }
}

impl TryFrom<&abi::SlipEligibilityQuery> for ProgramQuery {
    type Error = ();

    fn try_from(value: &abi::SlipEligibilityQuery) -> Result<Self, ()> {
        Ok(Self::SlipEligibility {
            origin: position_value(&value.origin),
            destination: position_value(&value.destination),
            departure_after_seconds: value.departure_after_seconds,
            arrival_after_seconds: value.arrival_after_seconds,
            navigation_beacon: optional(
                value.navigation_beacon_present,
                Id(value.navigation_beacon),
            )?,
        })
    }
}

impl TryFrom<&abi::ResolveQuery> for ProgramQuery {
    type Error = ();

    fn try_from(value: &abi::ResolveQuery) -> Result<Self, ()> {
        Ok(Self::Resolve {
            destination: (&value.destination).try_into()?,
            after_seconds: value.after_seconds,
        })
    }
}

pub fn route_request(
    header: &abi::RouteRequest,
    orders: &[abi::Order],
) -> Result<ProgramQuery, ()> {
    if orders.len() > routing::MAX_ORDERS {
        return Err(());
    }
    Ok(ProgramQuery::RouteRequest(routing::Request {
        id: header.id,
        preferences: (&header.preferences).try_into()?,
        orders: orders
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
    }))
}

macro_rules! action {
    ($ty:ident, $value:ident, $body:expr) => {
        impl TryFrom<&abi::$ty> for ProgramAction {
            type Error = ();

            fn try_from($value: &abi::$ty) -> Result<Self, ()> {
                Ok($body)
            }
        }
    };
}

action!(
    UseRoute,
    value,
    Self::UseRoute {
        id: value.id,
        revision: value.revision,
        engage: flag(value.engage)?
    }
);
action!(
    Block,
    value,
    Self::Block {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?,
        reason: text(&value.reason)?
    }
);
action!(
    Estimate,
    value,
    Self::Estimate {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?,
        remaining_ticks: optional(value.ticks_present, value.remaining_ticks)?,
        remaining_propellant_kg: optional(value.propellant_present, value.remaining_propellant_kg)?,
    }
);
action!(
    CompleteOrder,
    value,
    Self::CompleteOrder {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?
    }
);
action!(
    Slip,
    value,
    Self::Slip {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?,
        destination: position_value(&value.destination),
        navigation_beacon: optional(value.navigation_beacon_present, Id(value.navigation_beacon))?,
    }
);
action!(
    ReserveBay,
    value,
    Self::ReserveBay {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?,
        station: Id(value.station),
        bay: u32::try_from(value.bay).map_err(|_| ())?,
    }
);
action!(
    Dock,
    value,
    Self::Dock {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?,
        station: Id(value.station),
        bay: u32::try_from(value.bay).map_err(|_| ())?,
    }
);
action!(
    Undock,
    value,
    Self::Undock {
        revision: value.revision,
        order: usize::try_from(value.order).map_err(|_| ())?
    }
);

impl From<&abi::ContactRef> for ProgramQuery {
    fn from(value: &abi::ContactRef) -> Self {
        Self::Contact(value.into())
    }
}

impl TryFrom<&ProgramReply> for abi::TravelReply {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::Travel {
                state,
                pose,
                slip_ready,
                slip_axis,
            } => Ok(travel_record(state, pose, *slip_ready, *slip_axis)),
            _ => Err(()),
        }
    }
}

impl TryFrom<&ProgramReply> for abi::ContactReply {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::Contact {
                pose,
                handle,
                radius_m,
            } => Ok(Self {
                pose: pose.into(),
                handle: *handle,
                radius_m: *radius_m,
            }),
            _ => Err(()),
        }
    }
}

impl TryFrom<&ProgramReply> for abi::SlipEligibilityReply {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::SlipEligibility {
                ready,
                preparation_s,
                duration_s,
            } => Ok(Self {
                ready: *ready as u64,
                preparation_s: *preparation_s,
                duration_s: *duration_s,
            }),
            _ => Err(()),
        }
    }
}

impl TryFrom<&ProgramReply> for abi::Pose {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::Pose(pose) => Ok(pose.into()),
            _ => Err(()),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn syscall(status: i32) -> Result<(), i32> {
    if status < 0 { Err(status) } else { Ok(()) }
}

#[cfg(target_arch = "wasm32")]
pub fn query(query: &ProgramQuery) -> Result<ProgramReply, i32> {
    use abi::raw;
    use osg_ship_api::abi::ERR_ARGUMENT;

    match query {
        ProgramQuery::Orrery { reference } => {
            let input = abi::OrreryQuery {
                reference: position_record(reference),
            };
            let mut values =
                vec![abi::LocalObstacle::default(); crate::local_space::MAX_LOCAL_OBSTACLES];
            let mut header = abi::OrreryReply::default();
            syscall(unsafe {
                raw::orrery_read(
                    &input,
                    values.as_mut_ptr(),
                    values.len() as u32,
                    &mut header,
                )
            })?;
            let obstacles = counted(&values, header.count)
                .map_err(|_| ERR_ARGUMENT)?
                .iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()
                .map_err(|_| ERR_ARGUMENT)?;
            Ok(ProgramReply::Orrery(obstacles))
        }
        ProgramQuery::Contact(contact) => {
            let mut output = abi::ContactReply::default();
            syscall(unsafe { raw::contact_get(&contact.into(), &mut output) })?;
            Ok(ProgramReply::Contact {
                pose: (&output.pose).into(),
                handle: output.handle,
                radius_m: output.radius_m,
            })
        }
        ProgramQuery::SlipEligibility {
            origin,
            destination,
            departure_after_seconds,
            arrival_after_seconds,
            navigation_beacon,
        } => {
            let input = abi::SlipEligibilityQuery {
                origin: position_record(origin),
                destination: position_record(destination),
                departure_after_seconds: *departure_after_seconds,
                arrival_after_seconds: *arrival_after_seconds,
                navigation_beacon_present: navigation_beacon.is_some() as u64,
                navigation_beacon: navigation_beacon.unwrap_or_default().0,
            };
            let mut output = abi::SlipEligibilityReply::default();
            syscall(unsafe { raw::slip_eligibility(&input, &mut output) })?;
            Ok(ProgramReply::SlipEligibility {
                ready: flag(output.ready).map_err(|_| ERR_ARGUMENT)?,
                preparation_s: output.preparation_s,
                duration_s: output.duration_s,
            })
        }
        ProgramQuery::Travel => {
            let mut output = abi::TravelReply::default();
            syscall(unsafe { raw::travel_read(&mut output) })?;
            travel_reply(&output).map_err(|_| ERR_ARGUMENT)
        }
        ProgramQuery::Resolve {
            destination,
            after_seconds,
        } => {
            let input = abi::ResolveQuery {
                destination: destination.into(),
                after_seconds: *after_seconds,
            };
            let mut output = abi::Pose::default();
            syscall(unsafe { raw::destination_resolve(&input, &mut output) })?;
            Ok(ProgramReply::Pose((&output).into()))
        }
        ProgramQuery::RouteRequest(_) | ProgramQuery::RoutePoll { .. } => {
            let mut output = abi::RouteReply::default();
            let mut orders = vec![abi::QueuedOrder::default(); routing::MAX_ORDERS];
            let mut fuels = vec![abi::FuelRequirement::default(); 256];
            let status = match query {
                ProgramQuery::RouteRequest(request) => {
                    let input = abi::RouteRequest {
                        id: request.id,
                        preferences: (&request.preferences).into(),
                    };
                    let inputs: Vec<abi::Order> = request.orders.iter().map(Into::into).collect();
                    unsafe {
                        raw::route_request(
                            &input,
                            inputs.as_ptr(),
                            inputs.len() as u32,
                            &mut output,
                            orders.as_mut_ptr(),
                            orders.len() as u32,
                            fuels.as_mut_ptr(),
                            fuels.len() as u32,
                        )
                    }
                }
                ProgramQuery::RoutePoll { id } => unsafe {
                    raw::route_poll(
                        *id,
                        &mut output,
                        orders.as_mut_ptr(),
                        orders.len() as u32,
                        fuels.as_mut_ptr(),
                        fuels.len() as u32,
                    )
                },
                _ => unreachable!(),
            };
            syscall(status)?;
            route_reply(&output, &orders, &fuels).map_err(|_| ERR_ARGUMENT)
        }
        _ => crate::wasm_beacons::query(query),
    }
}

#[cfg(target_arch = "wasm32")]
pub fn command(action: ProgramAction) -> Result<(), i32> {
    use abi::raw;

    let result = match action {
        ProgramAction::UseRoute {
            id,
            revision,
            engage,
        } => unsafe {
            raw::travel_use_route(&abi::UseRoute {
                id,
                revision,
                engage: engage as u64,
            })
        },
        ProgramAction::Block {
            revision,
            order,
            reason,
        } => {
            if reason.len() > 256 {
                return Err(osg_ship_api::abi::ERR_ARGUMENT);
            }
            unsafe {
                raw::travel_block(&abi::Block {
                    revision,
                    order: order as u64,
                    reason: Text::new(&reason),
                })
            }
        }
        ProgramAction::Estimate {
            revision,
            order,
            remaining_ticks,
            remaining_propellant_kg,
        } => unsafe {
            raw::travel_estimate(&abi::Estimate {
                revision,
                order: order as u64,
                ticks_present: remaining_ticks.is_some() as u64,
                remaining_ticks: remaining_ticks.unwrap_or_default(),
                propellant_present: remaining_propellant_kg.is_some() as u64,
                remaining_propellant_kg: remaining_propellant_kg.unwrap_or_default(),
            })
        },
        ProgramAction::CompleteOrder { revision, order } => unsafe {
            raw::travel_complete(&abi::CompleteOrder {
                revision,
                order: order as u64,
            })
        },
        ProgramAction::Slip {
            revision,
            order,
            destination,
            navigation_beacon,
        } => unsafe {
            raw::travel_slip(&abi::Slip {
                revision,
                order: order as u64,
                destination: position_record(&destination),
                navigation_beacon_present: navigation_beacon.is_some() as u64,
                navigation_beacon: navigation_beacon.unwrap_or_default().0,
            })
        },
        ProgramAction::ReserveBay {
            revision,
            order,
            station,
            bay,
        } => unsafe {
            raw::travel_reserve_bay(&abi::ReserveBay {
                revision,
                order: order as u64,
                station: station.0,
                bay: bay as u64,
            })
        },
        ProgramAction::Dock {
            revision,
            order,
            station,
            bay,
        } => unsafe {
            raw::travel_dock(&abi::Dock {
                revision,
                order: order as u64,
                station: station.0,
                bay: bay as u64,
            })
        },
        ProgramAction::Undock { revision, order } => unsafe {
            raw::travel_undock(&abi::Undock {
                revision,
                order: order as u64,
            })
        },
    };
    syscall(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_ship_api::abi::Record;

    #[test]
    fn wide_coordinates_survive_record_memory_roundtrip() {
        let position = GalacticPosition::new(i128::MIN, i128::MAX, -(1_i128 << 90) + 17);
        let record = position_record(&position);
        let restored = abi::Position::read(record.bytes()).unwrap();
        assert_eq!(position_value(&restored), position);
    }

    #[test]
    fn nested_route_records_preserve_optional_values_and_references() {
        let system_order = travel::Order::TravelToSystem(Id([42; 16]));
        assert_eq!(
            travel::Order::try_from(&abi::Order::from(&system_order)).unwrap(),
            system_order
        );
        let position = GalacticPosition::new(1_i128 << 92, -127, 101);
        let action = travel::Order::Guidance(travel::Guidance {
            mode: travel::GuidanceMode::KeepRange,
            target: travel::Target::Destination(travel::Destination::Relative {
                reference: travel::Reference::Celestial(travel::CelestialRef {
                    system: Id([12; 16]),
                    body: Id([13; 16]),
                }),
                offset: position,
                axes: travel::Axes::BodyFixed,
            }),
            range_m: 1050.,
        });
        let plan = routing::Plan {
            estimated_loss_ppm: 31.5,
            beacon_assumptions: vec![Id([14; 16])],
            exotic_fuel_kg: 14.0,
            planned_tick: 42,
            travel_revision: 17,
            topology_revision: 8,
            orders: vec![
                travel::QueuedOrder {
                    label: "Authored waypoint".into(),
                    transfer_cost: crate::transfer::TransferCost { seconds_per_kg: 8. },
                    action,
                    estimated_duration_ticks: Some(0),
                    estimated_propellant_kg: None,
                    estimated_loss_ppm: Some(12.5),
                },
                travel::Order::Slip {
                    destination: travel::Destination::Galactic(position),
                    navigation_beacon: Some(Id([14; 16])),
                }
                .into(),
            ],
            fuel_budget: travel::FuelBudget {
                resources: vec![travel::FuelRequirement {
                    resource: "hydrogen".into(),
                    required_kg: 72.,
                    available_kg: 1300.,
                }],
                complete: true,
            },
        };
        let status = routing::Status::Ready { plan };
        let (header, orders, fuels) = route_records(19, &status);
        let ProgramReply::Route {
            id,
            status: restored,
        } = route_reply(&header, &orders, &fuels).unwrap()
        else {
            panic!("unexpected reply");
        };
        assert_eq!(id, 19);
        assert_eq!(restored, status);
        assert!(route_reply(&header, &[], &fuels).is_err());
    }

    #[test]
    fn malformed_discriminants_and_text_are_rejected() {
        let destination = abi::Destination {
            kind: 999,
            ..Default::default()
        };
        assert!(travel::Destination::try_from(&destination).is_err());
        let preferences = abi::Preferences {
            fuel_fraction: 0.5,
            max_loss_ppm: f64::NAN,
            allow_slipdrive: 1,
        };
        assert!(travel::PlanningPreferences::try_from(&preferences).is_err());
        let mut reason = Text::<256>::new("valid");
        reason.len = 257;
        assert!(text(&reason).is_err());
        reason.len = 1;
        reason.bytes[0] = 255;
        assert!(text(&reason).is_err());
    }
}
