pub mod assets;
pub mod calendar;
pub mod chat;
pub mod diplomacy;
pub mod drawing;
pub mod economy;
pub mod industry;
pub mod local_space;
pub mod location;
pub mod market;
pub mod optical;
pub mod ownership;
pub mod presentation;
pub mod serial;
pub mod slip_visual;
pub mod society;
pub mod transfer;
pub mod travel;
pub mod wasm_beacons;
pub mod wasm_world;
pub use local_space::{LocalObstacle, LocalSpace};
pub use presentation::*;

pub use osg_ship_api::GAME_VERSION;
pub mod rpc;

/// Duration of one authoritative simulation tick.
pub const TICK_NS: u64 = 100_000_000;
pub const TICK_DURATION: std::time::Duration = std::time::Duration::from_nanos(TICK_NS);
pub const TICK_SECONDS: f64 = TICK_NS as f64 / 1_000_000_000.0;
pub const TICK_RATE_HZ: f64 = 1.0 / TICK_SECONDS;

pub use osg_space::GalacticPosition;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Id(pub [u8; 16]);

impl Id {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        uuid::Uuid::from_bytes(self.0).fmt(f)
    }
}

impl std::str::FromStr for Id {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(*uuid::Uuid::parse_str(s)?.as_bytes()))
    }
}

pub type EntityId = Id;
pub type AccountId = Id;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub position: GalacticPosition,
    pub velocity: [f64; 3],
    pub rotation: [f64; 4],
    pub angular_velocity: [f64; 3],
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            position: GalacticPosition::ZERO,
            velocity: [0.; 3],
            rotation: [0., 0., 0., 1.],
            angular_velocity: [0.; 3],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IffIdentity {
    pub owner: AccountId,
    pub faction: Option<Id>,
    pub labels: BTreeSet<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorObservation {
    pub spatial_instance: Id,
    pub id: u64,
    pub entity: Option<EntityId>,
    pub pose: Pose,
    pub radius_m: f64,
    pub iff: Option<IffIdentity>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSubscription {
    pub id: u64,
    pub revision: u64,
    pub focused_ship: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    pub focused_ship: Option<EntityId>,
    pub origin: GalacticPosition,
    pub id: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DockServiceSettings {
    pub cargo: bool,
    pub power: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShipTelemetry {
    pub can_control: bool,
    pub appearance: Option<[u8; 32]>,
    pub radius_m: f64,
    pub dock_services: DockServiceSettings,
    pub spatial_instance: Id,
    pub iff: IffIdentity,
    pub ship: EntityId,
    pub authority_revision: u64,
    pub presence: travel::Presence,
    pub pose: Option<Pose>,
    pub battery_j: u64,
    pub hull_heat_j: f64,
    pub shield_temperature_k: f64,
    pub coolant_reserve_kg: f64,
    pub location: location::LocationContext,
    pub travel: travel::AutopilotState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenUpdate {
    pub ship: EntityId,
    pub slot: u8,
    pub revision: u64,
    pub tick: u64,
    pub frame: Option<drawing::ScreenImage>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub sequence: u64,
    pub tick: u64,
    pub subject: Option<EntityId>,
    pub kind: String,
    pub position: Option<GalacticPosition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub id: Id,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub chat: Option<chat::ChatUpdate>,
    pub optical: Vec<optical::OpticalObservation>,
    pub calendar_unix_ms: i64,
    pub presentation: PresentationFrame,
    pub world: Id,
    pub sequence: u64,
    pub tick: u64,
    pub sim_time_ns: u64,
    pub rate: f64,
    pub views: Vec<ViewState>,
    pub contacts: BTreeMap<EntityId, Vec<SensorObservation>>,
    pub ships: Vec<ShipTelemetry>,
    pub screens: Vec<ScreenUpdate>,
    pub events: Vec<Event>,
    pub results: Vec<CommandResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    ChatSubscribe(chat::ChatSubscription),
    ChatUnsubscribe,
    ChatSend {
        subscription_revision: u64,
        text: String,
    },
    InstrumentSubscribe {
        ship: EntityId,
    },
    InstrumentUnsubscribe {
        ship: EntityId,
    },
    Debug(DebugCommand),
    Subscribe(ViewSubscription),
    Unsubscribe(u64),
    ScreenSubscribe {
        ship: EntityId,
        slot: u8,
        hz: u8,
    },
    ScreenUnsubscribe {
        ship: EntityId,
        slot: u8,
    },
    Ship {
        ship: EntityId,
        authority_revision: u64,
        command: ShipCommand,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShipCommand {
    Flight(FlightCommand),
    SetTransponderEnabled(bool),
    MarkTarget {
        target: ContactRef,
        maximum_flight_time_s: f64,
    },
    StopFiring,
    UnmarkTarget,
    StartFiring,
    Aim {
        target: ContactRef,
    },
    SetIff(IffIdentity),
    SetItinerary {
        preferences: travel::PlanningPreferences,
        engage: bool,
        expected_revision: u64,
        itinerary: Vec<travel::Directive>,
    },
    SetGuidance(Option<travel::Guidance>),
    SetAutopilot(bool),
    SetThrottle(f64),
    SetDockServices {
        cargo: bool,
        power: bool,
    },
    Undock,
    Dock {
        station: EntityId,
        bay: u32,
    },
    ScreenInput {
        slot: u8,
        revision: u64,
        kind: u8,
        code: u64,
        modifiers: u64,
        xy: [f64; 2],
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputFrame {
    pub world: Id,
    pub sequence: u64,
    pub actions: Vec<(Id, Action)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProgramQuery {
    Orrery {
        reference: GalacticPosition,
    },
    OrrerySystem {
        system: Id,
        after_seconds: f64,
    },
    SlipEligibilityBatch(Vec<SlipProbe>),
    SlipEligibility {
        origin: GalacticPosition,
        destination: GalacticPosition,
        departure_after_seconds: f64,
        arrival_after_seconds: f64,
        navigation_beacon: Option<EntityId>,
    },
    Travel,
    Contact(ContactRef),
    Beacon(EntityId),
    Resolve {
        destination: travel::Destination,
        after_seconds: f64,
    },
    Beacons {
        after: Option<EntityId>,
        limit: u16,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlipProbe {
    pub origin: GalacticPosition,
    pub destination: GalacticPosition,
    pub departure_after_seconds: f64,
    pub arrival_after_seconds: f64,
    pub navigation_beacon: Option<EntityId>,
    pub arrival_velocity: Option<[f64; 3]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlipProbeResult {
    pub ready: bool,
    pub preparation_s: f64,
    pub duration_s: f64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Beacon {
    pub system: Option<Id>,
    pub radius_m: f64,
    pub entity: EntityId,
    pub pose: Pose,
    pub iff: IffIdentity,
    pub bays: BTreeMap<u32, Pose>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProgramReply {
    Orrery(Vec<LocalObstacle>),
    SlipEligibilityBatch(Vec<SlipProbeResult>),
    Contact {
        pose: Pose,
        handle: u64,
        radius_m: f64,
    },
    SlipEligibility {
        ready: bool,
        preparation_s: f64,
        duration_s: f64,
    },
    Travel {
        state: travel::AutopilotState,
        pose: Pose,
        presence: travel::Presence,
        location: location::LocationContext,
        tick: u64,
        exotic_fuel_kg: f64,
        slip_ready: bool,
        slip_axis: [f64; 3],
    },
    Pose(Pose),
    Beacons(Vec<Beacon>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProgramAction {
    SetAutopilot {
        directive_revision: u64,
        enabled: bool,
    },
    ClearItinerary {
        directive_revision: u64,
    },
    Fail {
        directive_revision: u64,
        reason: String,
    },
    PublishStatus {
        directive_revision: u64,
        status: travel::FirmwareStatus,
    },
    Complete {
        directive_revision: u64,
    },
    Slip {
        destination: GalacticPosition,
        navigation_beacon: Option<EntityId>,
        arrival_velocity: Option<[f64; 3]>,
        not_before_tick: Option<u64>,
    },
    ReserveBay {
        station: EntityId,
        bay: u32,
    },
    Dock {
        station: EntityId,
        bay: u32,
    },
    Undock,
    CancelSlip,
}
