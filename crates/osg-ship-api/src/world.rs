use crate::abi::{Record, Text, private};

pub const DESTINATION_BEACON: u64 = 0;
pub const DESTINATION_GALACTIC: u64 = 1;
pub const DESTINATION_CELESTIAL_RELATIVE: u64 = 2;
pub const DESTINATION_BEACON_RELATIVE: u64 = 3;
pub const AXES_GALACTIC: u64 = 0;
pub const AXES_BODY_FIXED: u64 = 1;
pub const TARGET_DIRECTION: u64 = 0;
pub const TARGET_DESTINATION: u64 = 1;
pub const TARGET_CONTACT: u64 = 2;
pub const ORDER_GUIDANCE: u64 = 1;
pub const ORDER_TRAVEL: u64 = 2;
pub const ORDER_SUBLIGHT: u64 = 3;
pub const ORDER_SLIP: u64 = 4;
pub const ORDER_DOCK: u64 = 5;
pub const ORDER_UNDOCK: u64 = 6;
pub const ORDER_WAIT: u64 = 7;
pub const GUIDANCE_ALIGN: u64 = 0;
pub const GUIDANCE_APPROACH: u64 = 1;
pub const GUIDANCE_KEEP_RANGE: u64 = 2;
pub const TRAVEL_IDLE: u64 = 0;
pub const TRAVEL_PLANNING: u64 = 1;
pub const TRAVEL_ACTIVE: u64 = 2;
pub const TRAVEL_PAUSED: u64 = 3;
pub const TRAVEL_BLOCKED: u64 = 4;
pub const TRAVEL_COMPLETED: u64 = 5;
pub const ROUTE_UNKNOWN: u64 = 0;
pub const ROUTE_PENDING: u64 = 1;
pub const ROUTE_READY: u64 = 2;
pub const ROUTE_FAILED: u64 = 3;
pub const PLANNING_LOADING_CATALOGUE: u64 = 0;
pub const PLANNING_BUILDING_GRAPH: u64 = 1;
pub const PLANNING_SEARCHING_ROUTES: u64 = 2;

macro_rules! record {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name { $(pub $field: $ty,)* }
        impl private::Sealed for $name {}
        impl Record for $name {}
        const _: () = assert!(core::mem::size_of::<$name>() == 0 $(+ core::mem::size_of::<$ty>())*);
    };
}

record!(Position { words: [u64; 6] });
record!(Pose {
    position: Position,
    velocity: [f64; 3],
    rotation: [f64; 4],
    angular_velocity: [f64; 3],
});
record!(ContactRef {
    group: [u8; 16],
    track: [u8; 16]
});
record!(Destination {
    kind: u64,
    entity: [u8; 16],
    system: [u8; 16],
    position: Position,
    axes: u64
});
record!(Target {
    kind: u64,
    direction: [f64; 3],
    destination: Destination,
    contact: ContactRef
});
record!(Order {
    kind: u64,
    entity: [u8; 16],
    destination: Destination,
    target: Target,
    mode: u64,
    range_m: f64,
    tick: u64,
    speed_ly_s: f64,
    navigation_beacon_present: u64,
    navigation_beacon: [u8; 16],
});
record!(QueuedOrder {
    action: Order,
    seconds_per_kg: f64,
    duration_present: u64,
    duration_ticks: u64,
    propellant_present: u64,
    propellant_kg: f64,
});
record!(Preferences {
    fuel_fraction: f64,
    max_loss_ppm: f64,
    allow_slipdrive: u64
});
record!(OrreryQuery {
    reference: Position
});
record!(LocalObstacle {
    reference: Target,
    pose: Pose,
    radius_m: f64,
    slip_exclusion_m: f64
});
record!(OrreryReply { count: u64 });
record!(ContactReply {
    pose: Pose,
    handle: u64,
    radius_m: f64
});
record!(SlipEligibilityQuery {
    origin: Position,
    destination: Position,
    departure_after_seconds: f64,
    arrival_after_seconds: f64,
    speed_ly_s: f64,
    navigation_beacon_present: u64,
    navigation_beacon: [u8; 16],
});
record!(SlipEligibilityReply {
    ready: u64,
    preparation_s: f64,
    duration_s: f64
});
record!(ResolveQuery {
    destination: Destination,
    after_seconds: f64
});
record!(TravelReply {
    autopilot_enabled: u64, preferences: Preferences, revision: u64, index: u64,
    order_present: u64, order: QueuedOrder, status: u64, reason: Text<256>,
    arrival_present: u64, arrival_tick: u64, pose: Pose, slip_ready: u64,
});
record!(RouteRequest {
    id: u64,
    preferences: Preferences
});
record!(RoutePoll { id: u64 });
record!(FuelRequirement { resource: Text<64>, required_kg: f64, available_kg: f64 });
record!(RouteReply {
    id: u64, status: u64, stage: u64, completed: u64, total_present: u64, total: u64,
    planned_tick: u64, travel_revision: u64, topology_revision: u64,
    order_count: u64, fuel_count: u64, fuel_complete: u64, reason: Text<256>,
    estimated_loss_ppm: f64, exotic_fuel_kg: f64,
});
record!(UseRoute {
    id: u64,
    revision: u64,
    engage: u64
});
record!(Block { revision: u64, order: u64, reason: Text<256> });
record!(Estimate {
    revision: u64,
    order: u64,
    ticks_present: u64,
    remaining_ticks: u64,
    propellant_present: u64,
    remaining_propellant_kg: f64,
});
record!(CompleteOrder {
    revision: u64,
    order: u64
});
record!(Slip {
    revision: u64,
    order: u64,
    destination: Position,
    speed_ly_s: f64,
    navigation_beacon_present: u64,
    navigation_beacon: [u8; 16],
});
record!(ReserveBay {
    revision: u64,
    order: u64,
    station: [u8; 16],
    bay: u64
});
record!(Dock {
    revision: u64,
    order: u64,
    station: [u8; 16],
    bay: u64
});
record!(Undock {
    revision: u64,
    order: u64
});

#[cfg(target_arch = "wasm32")]
pub mod raw {
    use super::*;

    #[link(wasm_import_module = "ship_v32")]
    unsafe extern "C" {
        pub fn orrery_read(
            query: *const OrreryQuery,
            output: *mut LocalObstacle,
            capacity: u32,
            reply: *mut OrreryReply,
        ) -> i32;
        pub fn contact_get(query: *const ContactRef, reply: *mut ContactReply) -> i32;
        pub fn slip_eligibility(
            query: *const SlipEligibilityQuery,
            reply: *mut SlipEligibilityReply,
        ) -> i32;
        pub fn travel_read(reply: *mut TravelReply) -> i32;
        pub fn destination_resolve(query: *const ResolveQuery, reply: *mut Pose) -> i32;
        pub fn route_request(
            query: *const RouteRequest,
            orders: *const Order,
            count: u32,
            reply: *mut RouteReply,
            output: *mut QueuedOrder,
            capacity: u32,
            fuels: *mut FuelRequirement,
            fuel_capacity: u32,
        ) -> i32;
        pub fn route_poll(
            id: u64,
            reply: *mut RouteReply,
            output: *mut QueuedOrder,
            capacity: u32,
            fuels: *mut FuelRequirement,
            fuel_capacity: u32,
        ) -> i32;
        pub fn travel_use_route(action: *const UseRoute) -> i32;
        pub fn travel_block(action: *const Block) -> i32;
        pub fn travel_estimate(action: *const Estimate) -> i32;
        pub fn travel_complete(action: *const CompleteOrder) -> i32;
        pub fn travel_slip(action: *const Slip) -> i32;
        pub fn travel_reserve_bay(action: *const ReserveBay) -> i32;
        pub fn travel_dock(action: *const Dock) -> i32;
        pub fn travel_undock(action: *const Undock) -> i32;
    }
}
