pub mod calendar;
pub mod drawing;
pub mod navigation;
pub mod optical;
pub mod ownership;
pub mod presentation;
pub mod transfer;
pub mod travel;
pub use presentation::*;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use toy_sim_space::GalacticPosition;

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
pub type TrackId = Id;
pub type GroupId = Id;
pub const PUBLIC_GROUP: GroupId = Id([255; 16]);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InfoGroupKey(pub [u8; 32]);

impl InfoGroupKey {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self(rand::random())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for InfoGroupKey {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for InfoGroupKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InfoGroupKey([redacted])")
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tag {
    Kind(String),
    IffOwner(AccountId),
    IffFaction(Id),
    Advertised(String),
    Annotation(String),
}

impl Tag {
    pub fn valid(&self) -> bool {
        match self {
            Self::Kind(s) | Self::Advertised(s) | Self::Annotation(s) => {
                !s.is_empty() && s.len() <= 64 && !s.chars().any(char::is_control)
            }
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IffIdentity {
    pub owner: AccountId,
    pub faction: Option<Id>,
    pub labels: BTreeSet<String>,
    pub enabled: bool,
    pub range_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provenance {
    Sensor,
    Transponder,
    GroupMember,
    Beacon,
    Extrapolated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub spatial_instance: Id,
    pub id: TrackId,
    pub entity: Option<EntityId>,
    pub pose: Pose,
    pub position_sigma_m: f64,
    pub velocity_sigma_m_s: f64,
    pub observed_tick: u64,
    pub estimate_tick: u64,
    pub tags: BTreeSet<Tag>,
    pub provenance: Provenance,
    pub radius_m: Option<f64>,
    pub appearance: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackQuery {
    pub track: Option<TrackId>,
    pub sphere: Option<(GalacticPosition, f64)>,
    pub all: BTreeSet<Tag>,
    pub any: BTreeSet<Tag>,
    pub exclude: BTreeSet<Tag>,
    pub max_age_ticks: Option<u64>,
    pub limit: u16,
    pub work: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Completion {
    Complete,
    ResultLimit,
    WorkLimit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryPage {
    pub revision: u64,
    pub tracks: Vec<Track>,
    pub completion: Completion,
    pub continuation: Option<Id>,
    pub gas_used: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSubscription {
    pub id: u64,
    pub revision: u64,
    pub group: GroupId,
    pub focused_ship: Option<EntityId>,
    pub query: TrackQuery,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    pub focused_ship: Option<EntityId>,
    pub origin: GalacticPosition,
    pub id: u64,
    pub revision: u64,
    pub group: GroupId,
    pub tracks: Vec<TrackId>,
    pub completion: Completion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DockServiceSettings {
    pub cargo: bool,
    pub power: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShipTelemetry {
    pub appearance: Option<[u8; 32]>,
    pub radius_m: f64,
    pub dock_services: DockServiceSettings,
    pub spatial_instance: Id,
    pub info_group: InfoGroupKey,
    pub iff: IffIdentity,
    pub ship: EntityId,
    pub authority_revision: u64,
    pub presence: travel::Presence,
    pub pose: Option<Pose>,
    pub battery_j: u64,
    pub hull_heat_j: f64,
    pub shield_temperature_k: f64,
    pub coolant_reserve_kg: f64,
    pub travel: travel::TravelState,
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
pub enum Reply {
    JoinedGroup(GroupId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub reply: Option<Reply>,
    pub id: Id,
    pub effective_tick: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub optical: Vec<optical::OpticalObservation>,
    pub calendar_unix_ms: i64,
    pub society: ownership::SocietySnapshot,
    pub presentation: PresentationFrame,
    pub world: Id,
    pub sequence: u64,
    pub tick: u64,
    pub sim_time_ns: u64,
    pub rate: f64,
    pub views: Vec<ViewState>,
    pub tracks: BTreeMap<GroupId, Vec<Track>>,
    pub ships: Vec<ShipTelemetry>,
    pub screens: Vec<ScreenUpdate>,
    pub events: Vec<Event>,
    pub results: Vec<CommandResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Society(ownership::SocietyCommand),
    InstrumentSubscribe {
        ship: EntityId,
    },
    InstrumentUnsubscribe {
        ship: EntityId,
    },
    Debug(DebugCommand),
    JoinGroup(InfoGroupKey),
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
        group: GroupId,
        track: TrackId,
        maximum_flight_time_s: f64,
    },
    StopFiring,
    UnmarkTarget,
    StartFiring,
    Aim {
        group: GroupId,
        track: TrackId,
    },
    SetGroup(InfoGroupKey),
    SetIff(IffIdentity),
    SetTravel {
        preferences: travel::PlanningPreferences,
        engage: bool,
        expected_revision: u64,
        orders: Vec<travel::Order>,
    },
    SetAutopilot(bool),
    SetThrottle(f64),
    TransferCargo {
        target: EntityId,
        resource: String,
        quantity: u64,
    },
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
    SlipEligibility {
        origin: GalacticPosition,
        destination: GalacticPosition,
    },
    Travel,
    Contact(ContactRef),
    Beacon(EntityId),
    Resolve(travel::Destination),
    Tracks(TrackQuery),
    Continue {
        cursor: Id,
        work: u64,
    },
    Beacons {
        after: Option<EntityId>,
        limit: u16,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Beacon {
    pub radius_m: f64,
    pub entity: EntityId,
    pub pose: Pose,
    pub iff: IffIdentity,
    pub bays: BTreeMap<u32, Pose>,
    pub gate_exit: Option<EntityId>,
    pub exclusion_m: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProgramReply {
    Contact {
        pose: Pose,
        handle: u64,
        radius_m: f64,
    },
    SlipEligibility {
        ready: bool,
        duration_s: f64,
    },
    Travel {
        state: travel::TravelState,
        pose: Pose,
        slip_ready: bool,
    },
    Pose(Pose),
    Tracks(QueryPage),
    Beacons(Vec<Beacon>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProgramAction {
    Block {
        revision: u64,
        reason: String,
    },
    Route {
        revision: u64,
        orders: Vec<travel::QueuedOrder>,
        fuel_budget: travel::FuelBudget,
    },
    Estimate {
        revision: u64,
        order: usize,
        remaining_ticks: Option<u64>,
        fuel_budget: travel::FuelBudget,
    },
    CompleteOrder {
        revision: u64,
        order: usize,
    },
    Slip(GalacticPosition),
    ReserveBay {
        station: EntityId,
        bay: u32,
    },
    Dock {
        station: EntityId,
        bay: u32,
    },
    Undock,
}
