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
pub const DIRECTIVE_SLIP_TO_SYSTEM: u64 = 0;
pub const DIRECTIVE_DOCK_AT: u64 = 1;
pub const GUIDANCE_ALIGN: u64 = 0;
pub const GUIDANCE_APPROACH: u64 = 1;
pub const GUIDANCE_KEEP_RANGE: u64 = 2;
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
    observer: [u8; 16],
    contact: u64
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
record!(Guidance {
    present: u64,
    mode: u64,
    target: Target,
    range_m: f64
});
record!(Directive {
    kind: u64,
    entity: [u8; 16],
});
record!(ItineraryEntry {
    label: Text<256>,
    directive: Directive,
    max_loss_ppm: f64,
    fuel_allowance_kg: f64,
    duration_present: u64,
    duration_ticks: u64,
});
record!(Preferences {
    fuel_fraction: f64,
    max_loss_ppm: f64,
    allow_slipdrive: u64
});
record!(OrreryQuery {
    reference: Position
});
record!(OrrerySystemQuery {
    system: [u8; 16],
    after_seconds: f64
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
    navigation_beacon_present: u64,
    navigation_beacon: [u8; 16],
    arrival_velocity_present: u64,
    arrival_velocity: [f64; 3],
});
record!(SlipEligibilityReply {
    ready: u64,
    preparation_s: f64,
    duration_s: f64
});
record!(SlipProbeReply { result: SlipEligibilityReply, error: Text<256> });
record!(ResolveQuery {
    destination: Destination,
    after_seconds: f64
});
pub const MAX_STATUS_MARKERS: usize = 8;
record!(CelestialRef {
    system: [u8; 16],
    body: [u8; 16]
});
record!(PlanMarker { position: Position, label: Text<64> });
record!(FirmwareStatus {
    phase: u64, waiting_until_present: u64, waiting_until: u64, waiting_reason: Text<256>,
    summary: Text<256>, arrival_present: u64, arrival_tick: u64,
    capture_present: u64, capture_body: CelestialRef,
    aim_present: u64, aim_offset_m: [f64; 3], departure_present: u64, departure_tick: u64,
    planned_delta_v_m_s: f64, planned_loss_ppm: f64, spent_loss_ppm: f64,
    planned_exotic_fuel_kg: f64, spent_exotic_fuel_kg: f64,
    marker_count: u64, markers: [PlanMarker; MAX_STATUS_MARKERS],
});
record!(TravelReply {
    enabled: u64, preferences: Preferences, directive_revision: u64,
    itinerary_count: u64, fuel_present: u64, fuel_count: u64, fuel_complete: u64,
    max_log_loss: f64, spent_log_loss: f64, status: FirmwareStatus,
    failure_present: u64, failure: Text<256>, pose: Pose, slip_ready: u64, slip_axis: [f64; 3],
    presence: u64, host: [u8; 16], bay: u64,
    region: u64, system_present: u64, system: [u8; 16], primary_present: u64,
    primary: CelestialRef, hierarchy_count: u64, sample_tick: u64, tick: u64,
    exotic_fuel_kg: f64,
});
record!(RouteRequest {
    id: u64,
    preferences: Preferences
});
record!(RoutePoll { id: u64 });
record!(FuelRequirement { resource: Text<64>, required_kg: f64, available_kg: f64 });
record!(RouteReply {
    id: u64, status: u64, stage: u64, completed: u64, total_present: u64, total: u64,
    planned_tick: u64, directive_revision: u64, topology_revision: u64,
    itinerary_count: u64, fuel_count: u64, fuel_complete: u64, reason: Text<256>,
    estimated_loss_ppm: f64, exotic_fuel_kg: f64,
});
record!(UseRoute {
    id: u64,
    directive_revision: u64,
    engage: u64
});
record!(Fail { directive_revision: u64, reason: Text<256> });
record!(PublishStatus {
    directive_revision: u64,
    status: FirmwareStatus
});
record!(Complete {
    directive_revision: u64
});
record!(Slip {
    destination: Position,
    navigation_beacon_present: u64,
    navigation_beacon: [u8; 16],
    arrival_velocity_present: u64,
    arrival_velocity: [f64; 3],
    not_before_present: u64,
    not_before_tick: u64,
});
record!(ReserveBay {
    station: [u8; 16],
    bay: u64
});
record!(Dock {
    station: [u8; 16],
    bay: u64
});
record!(Undock { reserved: u64 });

#[cfg(target_arch = "wasm32")]
pub mod raw {
    use super::*;

    #[link(wasm_import_module = "ship")]
    unsafe extern "C" {
        pub fn orrery_read(
            query: *const OrreryQuery,
            output: *mut LocalObstacle,
            capacity: u32,
            reply: *mut OrreryReply,
        ) -> i32;
        pub fn orrery_system_read(
            query: *const OrrerySystemQuery,
            output: *mut LocalObstacle,
            capacity: u32,
            reply: *mut OrreryReply,
        ) -> i32;
        pub fn contact_get(query: *const ContactRef, reply: *mut ContactReply) -> i32;
        pub fn slip_eligibility(
            query: *const SlipEligibilityQuery,
            reply: *mut SlipEligibilityReply,
        ) -> i32;
        pub fn slip_eligibility_batch(
            queries: *const SlipEligibilityQuery,
            count: u32,
            output: *mut SlipProbeReply,
        ) -> i32;
        pub fn travel_read(
            reply: *mut TravelReply,
            itinerary: *mut ItineraryEntry,
            capacity: u32,
            fuels: *mut FuelRequirement,
            fuel_capacity: u32,
            hierarchy: *mut CelestialRef,
            hierarchy_capacity: u32,
        ) -> i32;
        pub fn destination_resolve(query: *const ResolveQuery, reply: *mut Pose) -> i32;
        pub fn route_request(
            query: *const RouteRequest,
            directives: *const Directive,
            count: u32,
            reply: *mut RouteReply,
            output: *mut ItineraryEntry,
            capacity: u32,
            fuels: *mut FuelRequirement,
            fuel_capacity: u32,
        ) -> i32;
        pub fn route_poll(
            id: u64,
            reply: *mut RouteReply,
            output: *mut ItineraryEntry,
            capacity: u32,
            fuels: *mut FuelRequirement,
            fuel_capacity: u32,
        ) -> i32;
        pub fn travel_use_route(action: *const UseRoute) -> i32;
        pub fn travel_fail(action: *const Fail) -> i32;
        pub fn travel_publish_status(action: *const PublishStatus) -> i32;
        pub fn travel_complete(action: *const Complete) -> i32;
        pub fn travel_slip(action: *const Slip) -> i32;
        pub fn travel_cancel_slip() -> i32;
        pub fn travel_reserve_bay(action: *const ReserveBay) -> i32;
        pub fn travel_dock(action: *const Dock) -> i32;
        pub fn travel_undock(action: *const Undock) -> i32;
    }
}
