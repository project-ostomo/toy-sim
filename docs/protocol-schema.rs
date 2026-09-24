// Postcard schema reference; autopilot declarations updated for version 56.
// See protocol.md for encoding and behavior; protocol-schema.md for the index.
// This file is a standalone Serde library. It has no game or engine dependencies.
// Fields and variants are in wire order. Do not alphabetize them.
// Shared ownership wrappers are erased; bounded sequence indexes use u64.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use presentation::*;
pub const GAME_VERSION: u16 = 56;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub [u8; 16]);

pub type EntityId = Id;

pub type AccountId = Id;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub position: GalacticPosition,
    pub velocity: [f64; 3],
    pub rotation: [f64; 4],
    pub angular_velocity: [f64; 3],
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
pub enum Reply {
    Route { id: u64, status: routing::Status },
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
    pub chat: Option<chat::ChatUpdate>,
    pub industry: Option<industry::IndustrySnapshot>,
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
    pub contacts: BTreeMap<EntityId, Vec<SensorObservation>>,
    pub ships: Vec<ShipTelemetry>,
    pub screens: Vec<ScreenUpdate>,
    pub events: Vec<Event>,
    pub results: Vec<CommandResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    RouteCancel {
        ship: EntityId,
        authority_revision: u64,
        id: u64,
    },
    RouteRequest {
        ship: EntityId,
        authority_revision: u64,
        request: routing::Request,
    },
    RoutePoll {
        ship: EntityId,
        authority_revision: u64,
        id: u64,
    },
    ChatSubscribe(chat::ChatSubscription),
    ChatUnsubscribe,
    ChatSend {
        subscription_revision: u64,
        text: String,
    },
    Industry(industry::IndustryCommand),
    IndustrySubscribe(industry::IndustrySubscription),
    IndustryUnsubscribe,
    Society(ownership::SocietyCommand),
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
    UseRoute {
        id: u64,
        expected_revision: u64,
        engage: bool,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GalacticPosition {
    pub x: i128,
    pub y: i128,
    pub z: i128,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Message {
    State(Frame),
    Input(InputFrame),
    Session { world: Id, universe: UniverseDescriptor },
}

pub mod presentation {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    pub struct ContactRef {
        pub observer: EntityId,
        pub contact: u64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct PresentationFrame {
        pub slip: crate::slip_visual::SlipPresentation,
        pub navigation: NavigationSnapshot,
        pub ships: Vec<ShipPresentation>,
        pub combat: Vec<CombatEvent>,
        pub capabilities: Vec<DebugCapability>,
        pub diagnostics: Option<Diagnostics>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ShipPresentation {
        pub serial: crate::serial::Screen,
        pub memory_limit_bytes: u64,
        pub propulsion: PropulsionTelemetry,
        pub ship: EntityId,
        pub revision: u64,
        pub sim_time_ns: u64,
        pub environment: Option<FlightEnvironment>,
        pub health: Option<ShipHealth>,
        pub execution: Option<ExecutionMetrics>,
        pub mass_kg: f64,
        pub inertia_kg_m2: [f64; 9],
        pub control_rotation: [f64; 4],
        pub hull_heat_capacity_j: f64,
        pub battery_capacity_j: u64,
        pub power_generated_w: f64,
        pub generation_capacity_w: f64,
        pub reactors: Vec<ReactorTelemetry>,
        pub slip_available: bool,
        pub slip_exotic_fuel_kg: Option<f64>,
        pub slip_navigation_lock: Option<bool>,
        pub power_consumed_w: f64,
        pub power_requested_w: f64,
        pub slip_charge: Option<SlipChargeTelemetry>,
        pub slip_transit: Option<SlipTransitTelemetry>,
        pub inventory: Vec<ResourceAmount>,
        pub cargo: Vec<crate::industry::CargoStack>,
        pub cargo_capacity_m3: f64,
        pub cargo_used_m3: f64,
        pub computer: ComputerStatus,
        pub instruments: Option<Instruments>,
        pub screens: Vec<ScreenDefinition>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ReactorTelemetry {
        pub name: String,
        pub status: ReactorStatus,
        pub temperature_k: f64,
        pub coolant_temperature_k: f64,
        pub operating_temperature_k: f64,
        pub shutdown_temperature_k: f64,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ReactorStatus {
        Running,
        Standby,
        Shutdown,
        Damaged,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct SlipChargeTelemetry {
        pub stored_j: u64,
        pub required_j: u64,
        pub input_w: f64,
        pub remaining_s: Option<f64>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct SlipTransitTelemetry {
        pub departed_ns: u64,
        pub destination: GalacticPosition,
        pub failure_ppm: f64,
        pub direction: [f64; 3],
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct PropulsionTelemetry {
        pub force_n: [f64; 3],
        pub torque_nm: [f64; 3],
        pub rated_forward_n: f64,
        pub positive_torque_nm: [f64; 3],
        pub negative_torque_nm: [f64; 3],
        pub propellants: Vec<String>,
        pub fuels: Vec<String>,
        pub charges: Vec<String>,
        pub ammunition: Vec<String>,
        pub drives: Vec<DriveReserve>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct DriveReserve {
        pub name: String,
        pub resource: String,
        pub delta_v_m_s: f64,
        pub full_delta_v_m_s: f64,
        pub flow_kg_s: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ResourceAmount {
        pub resource: String,
        pub quantity: u64,
        pub unit_mass_kg: f64,
        pub unit_volume_m3: f64,
        pub name: String,
        pub amount_kg: f64,
        pub capacity_kg: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum ComputerStatus {
        Unpowered,
        Booting {
            progress: f64,
            remaining_s: f64,
        },
        Running {
            gas_used: u64,
            gas_limit: u64,
            execution: ExecutionStatus,
        },
        Paused,
        Fault {
            message: String,
            reboot_remaining_s: Option<f64>,
        },
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ExecutionStatus {
        Ready,
        Suspended,
        WaitingForGas,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Instruments {
        pub valid_until_ns: u64,
        pub selected_contact: Option<ContactRef>,
        pub attitude: Option<AttitudeInstrument>,
        pub navigation: Option<NavigationInstrument>,
        pub weapons_state: Option<WeaponsInstrument>,
        pub weapons: Vec<WeaponInstrument>,
        pub paths: Vec<Trajectory>,
        pub markers: Vec<NavigationMarker>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct AttitudeInstrument {
        pub mode: u64,
        pub reference: Option<[f64; 4]>,
        pub control_error_rad: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct NavigationInstrument {
        pub status: u64,
        pub target: Option<ContactRef>,
        pub own_path: Option<u64>,
        pub target_path: Option<u64>,
        pub throttle_limit: f64,
        pub throttle: f64,
        pub stand_off_m: f64,
        pub approach_speed_limit_m_s: f64,
        pub braking_distance_m: f64,
        pub arrival_time_ns: Option<u64>,
        pub predicted_fuel_kg: Option<f64>,
        pub reason: String,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct WeaponInstrument {
        pub ammunition_units: f64,
        pub battery_energy_j: u64,
        pub shot_energy_j: f64,
        pub pointing_error_rad: f64,
        pub inhibit_flags: u64,
        pub part: u64,
        pub target: Option<ContactRef>,
        pub status: u64,
        pub aim_direction: [f64; 3],
        pub flight_time_s: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Trajectory {
        pub id: u64,
        pub revision: u64,
        pub published_at_ns: u64,
        pub valid_until_ns: u64,
        pub timed: bool,
        pub vertices: Vec<TrajectoryVertex>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct TrajectoryVertex {
        pub sim_time_ns: u64,
        pub position: GalacticPosition,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct NavigationMarker {
        pub id: u64,
        pub kind: u64,
        pub position: GalacticPosition,
        pub sim_time_ns: u64,
        pub label: String,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ScreenDefinition {
        pub slot: u8,
        pub width: u16,
        pub height: u16,
        pub title: String,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ShipVisual {
        pub slip_readiness: f64,
        pub engines: Vec<EngineVisual>,
        pub turrets: Vec<TurretVisual>,
        pub shield: Option<ShieldVisual>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct EngineVisual {
        pub part: u64,
        pub thrust_n: [f64; 3],
        pub thrust_fraction: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct TurretVisual {
        pub part: u64,
        pub yaw_rad: f64,
        pub pitch_rad: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ShieldVisual {
        pub temperature_k: f64,
        pub coverage: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct CombatEvent {
        pub sequence: u64,
        pub sim_time_ns: u64,
        pub kind: CombatEventKind,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum CombatEventKind {
        Beam {
            source: crate::Id,
            start: GalacticPosition,
            end: GalacticPosition,
            velocity_m_s: [f64; 3],
            end_time_ns: u64,
        },
        Projectile {
            id: u64,
            source: Option<crate::Id>,
            start: GalacticPosition,
            end: GalacticPosition,
            end_time_ns: u64,
            radius_m: f64,
        },
        Fired {
            source: crate::Id,
            position: GalacticPosition,
            energy_j: f64,
        },
        Impact {
            normal: [f64; 3],
            target: Option<crate::Id>,
            position: GalacticPosition,
            velocity_m_s: [f64; 3],
            energy_j: f64,
            shield: bool,
        },
        Destroyed {
            target: crate::Id,
            pose: Pose,
            appearance: Option<[u8; 32]>,
            energy_j: f64,
            mass_kg: f64,
            radius_m: f64,
        },
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum DebugCapability {
        Clock,
        Reset,
        Relocate,
        Recover,
        InjectHeat,
        Inspect,
        ConfigureSensor,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum DebugCommand {
        ConfigureSensor {
            ship: EntityId,
            range_m: f64,
            occlusion: bool,
        },
        InspectBody {
            body: Option<crate::travel::CelestialRef>,
        },
        RelocateToBody {
            ship: EntityId,
            body: crate::travel::CelestialRef,
        },
        InjectShieldHeat {
            ship: EntityId,
            joules: f64,
        },
        SetRate(f64),
        Reset,
        Relocate {
            ship: EntityId,
            pose: Pose,
        },
        Recover {
            ship: EntityId,
        },
        InjectHeat {
            ship: EntityId,
            joules: f64,
        },
        Inspect(bool),
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Diagnostics {
        pub collision: Option<CollisionDiagnostics>,
        pub entity_count: u64,
        pub active_ships: u64,
        pub dormant_ships: u64,
        pub tick_duration_ms: f64,
        pub systems: Vec<(String, f64)>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct CollisionDiagnostics {
        pub bodies: u64,
        pub candidates: u64,
        pub detailed_queries: u64,
        pub impacts: u64,
        pub dissipated_j: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum FlightCommand {
        HoldAttitude,
        StopGuidance,
        AimDirection([f64; 3]),
        SelectTarget(ContactRef),
        EngageNavigation {
            throttle_limit: f64,
            stand_off_m: f64,
        },
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FlightEnvironment {
        pub altitude_m: f64,
        pub airspeed_m_s: [f64; 3],
        pub density_kg_m3: f64,
        pub pressure_pa: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ShipHealth {
        pub crew_people: u32,
        pub crew_capacity: u32,
        pub life_support_fraction: f64,
        pub hull_hp: f64,
        pub hull_max_hp: f64,
        pub shield_reserve_capacity_kg: f64,
        pub shield_strength: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ExecutionMetrics {
        pub memory_bytes: u64,
        pub step_us: f64,
        pub prepare_us: f64,
        pub callback_us: f64,
        pub publish_us: f64,
        pub hardware_us: f64,
        pub scan_us: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct WeaponsInstrument {
        pub firing: bool,
        pub target: Option<ContactRef>,
        pub reason: String,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct NavigationSnapshot {
        pub directory: Option<[u8; 32]>,
        pub beacons: Vec<NavigationBeacon>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct InhabitedDirectory {
        pub systems: Vec<EntityId>,
        pub ownership: BTreeMap<EntityId, EntityId>,
        pub sovereignties: BTreeMap<EntityId, PublicSovereignty>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct PublicSovereignty {
        pub id: EntityId,
        pub name: String,
        pub bloc: crate::ownership::Bloc,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct NavigationBeacon {
        pub id: EntityId,
        pub systems: Vec<EntityId>,
        pub name: String,
        pub pose: Pose,
        pub radius_m: f64,
        pub docking: bool,
        pub navigation: bool,
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct UniverseDescriptor {
        pub fingerprint: [u8; 32],
        pub epoch_mjd_utc: f64,
        pub sim_time_origin_ns: u64,
    }
}

pub mod travel {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Presence {
        Space,
        Docked { host: EntityId, bay: u32 },
        SlipTransit(Id),
        StoredInWreck(EntityId),
        Destroyed,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Axes {
        Galactic,
        BodyFixed,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Reference {
        Celestial(CelestialRef),
        Beacon(EntityId),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    pub struct CelestialRef {
        pub system: Id,
        pub body: Id,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Destination {
        Beacon(EntityId),
        Galactic(GalacticPosition),
        Relative {
            reference: Reference,
            offset: GalacticPosition,
            axes: Axes,
        },
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Directive {
        SlipToSystem(Id),
        DockAt(EntityId),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
    pub struct PlanningPreferences {
        pub fuel_fraction: f64,
        pub max_loss_ppm: f64,
        pub allow_slipdrive: bool,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FuelRequirement {
        pub resource: String,
        pub required_kg: f64,
        pub available_kg: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FuelBudget {
        pub resources: Vec<FuelRequirement>,
        pub complete: bool,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ItineraryEntry {
        pub directive: Directive,
        pub label: String,
        pub max_loss_ppm: f64,
        pub fuel_allowance_kg: f64,
        pub estimated_duration_ticks: Option<u64>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct PlanMarker {
        pub position: GalacticPosition,
        pub label: String,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FirmwareStatus {
        pub phase: FirmwarePhase,
        pub summary: String,
        pub estimated_arrival_tick: Option<u64>,
        pub capture_body: Option<CelestialRef>,
        pub aim_offset_m: Option<[f64; 3]>,
        pub departure_tick: Option<u64>,
        pub planned_delta_v_m_s: f64,
        pub planned_loss_ppm: f64,
        pub spent_loss_ppm: f64,
        pub planned_exotic_fuel_kg: f64,
        pub spent_exotic_fuel_kg: f64,
        pub markers: Vec<PlanMarker>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum FirmwarePhase {
        Idle,
        Planning,
        Waiting { until: Option<u64>, why: String },
        Charging,
        Transit,
        Maneuvering,
        Docking,
        Completed,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum PlanningStage {
        LoadingCatalogue,
        BuildingGraph,
        SearchingRoutes,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct PlanningProgress {
        pub stage: PlanningStage,
        pub completed: u32,
        pub total: Option<u32>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct AutopilotState {
        pub enabled: bool,
        pub directive_revision: u64,
        pub itinerary: Vec<ItineraryEntry>,
        pub preferences: PlanningPreferences,
        pub risk_budget: RiskBudget,
        pub fuel_budget: Option<FuelBudget>,
        pub status: FirmwareStatus,
        pub failure: Option<String>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
    pub struct RiskBudget {
        pub max_log_loss: f64,
        pub spent_log_loss: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Target {
        Direction([f64; 3]),
        Destination(Destination),
        Contact(crate::ContactRef),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum GuidanceMode {
        Align,
        Approach,
        KeepRange,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Guidance {
        pub mode: GuidanceMode,
        pub target: Target,
        pub range_m: f64,
    }
}

pub mod location {
    use super::*;
    use super::travel::CelestialRef;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum LocationRegion {
        System,
        Interstellar,
        SlipTransit,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct LocationContext {
        pub region: LocationRegion,
        pub system: Option<Id>,
        pub primary: Option<CelestialRef>,
        pub hierarchy: Vec<CelestialRef>,
        pub sample_tick: u64,
    }
}

pub mod routing {
    use super::*;
    use super::travel::{Directive, FuelBudget, ItineraryEntry, PlanningPreferences, PlanningProgress};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Request {
        pub id: u64,
        pub directives: Vec<Directive>,
        pub preferences: PlanningPreferences,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Plan {
        pub planned_tick: u64,
        pub directive_revision: u64,
        pub topology_revision: u64,
        pub itinerary: Vec<ItineraryEntry>,
        pub fuel_budget: FuelBudget,
        pub estimated_loss_ppm: f64,
        pub exotic_fuel_kg: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Status {
        Unknown,
        Pending { progress: PlanningProgress },
        Ready { plan: Plan },
        Failed { reason: String },
    }
}

pub mod ownership {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    pub enum Principal {
        Sovereignty(Id),
        Organization(Id),
        Player(AccountId),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Bloc {
        Union,
        League,
        NonAligned,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Sovereignty {
        pub id: Id,
        pub name: String,
        pub bloc: Bloc,
        pub officers: BTreeSet<AccountId>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Organization {
        pub id: Id,
        pub name: String,
        pub sovereignty: Id,
        pub open_membership: bool,
        pub officers: BTreeSet<AccountId>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct PlayerAffiliation {
        pub account: AccountId,
        pub name: String,
        pub organization: Option<Id>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Standing {
        Friendly,
        Neutral,
        Hostile,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct OwnershipDirectory {
        pub sovereignties: BTreeMap<Id, Sovereignty>,
        pub organizations: BTreeMap<Id, Organization>,
        pub players: BTreeMap<AccountId, PlayerAffiliation>,
        pub standings: BTreeMap<(Principal, Principal), Standing>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    pub enum Permission {
        Navigate,
        Dock,
        View,
        Control,
        Configure,
        TransferCargo,
        Industry,
        ManageAccess,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AccessGrant {
        pub principal: Principal,
        pub permissions: BTreeSet<Permission>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AccessPolicy {
        pub public: BTreeSet<Permission>,
        pub grants: Vec<AccessGrant>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct SocietySnapshot {
        pub account: AccountId,
        pub directory: OwnershipDirectory,
        pub assets: Vec<AssetAffiliation>,
        pub gas_accounts: Vec<GasAccountSnapshot>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct GasAccountSnapshot {
        pub owner: Principal,
        pub available: u64,
        pub reserved: u64,
        pub spent: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AssetAffiliation {
        pub entity: Id,
        pub name: String,
        pub owner: Principal,
        pub access: AccessPolicy,
        pub can_manage: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum SocietyCommand {
        CreateOrganization {
            name: String,
        },
        SetOfficer {
            organization: Id,
            account: AccountId,
            officer: bool,
        },
        SetStanding {
            target: Principal,
            standing: Option<Standing>,
        },
        SetMembership {
            account: AccountId,
            organization: Option<Id>,
        },
        SetAssetAccess {
            asset: Id,
            policy: AccessPolicy,
        },
        TransferAsset {
            asset: Id,
            owner: Principal,
        },
    }
}

pub mod industry {
    use super::*;
    use super::ownership::Principal;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum BlueprintUploadAck {
        Ready { hash: [u8; 32] },
        Rejected { reason: String },
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    pub enum CargoItem {
        Resource(String),
        Part(String),
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ItemStack {
        pub item: CargoItem,
        pub quantity: u64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct CargoStack {
        pub item: CargoItem,
        pub quantity: u64,
        pub reserved: u64,
        pub name: String,
        pub unit_mass_kg: f64,
        pub unit_volume_m3: f64,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    pub enum IndustryCapability {
        Refinery,
        FuelPlant,
        Fabricator,
        Shipyard,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Recipe {
        pub id: String,
        pub name: String,
        pub capability: IndustryCapability,
        pub inputs: Vec<ItemStack>,
        pub outputs: Vec<ItemStack>,
        pub duration_ticks: u64,
        pub energy_j: u64,
        pub stored_energy_j: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum JobStatus {
        Queued,
        Running,
        AwaitingPower,
        AwaitingCargoSpace,
        AwaitingBerth,
        ModuleUnavailable,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct JobView {
        pub id: Id,
        pub name: String,
        pub capability: IndustryCapability,
        pub progress_ticks: u64,
        pub duration_ticks: u64,
        pub status: JobStatus,
        pub owner: Principal,
        pub created_by: AccountId,
        pub module_part: Option<u64>,
        pub requested_power_w: u64,
        pub supplied_power_w: u64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FacilityCapability {
        pub part: u64,
        pub capability: IndustryCapability,
        pub lanes: u32,
        pub power_per_lane_w: u64,
        pub max_radius_m: Option<f64>,
        pub operational: bool,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FacilityView {
        pub entity: EntityId,
        pub owner: Principal,
        pub name: String,
        pub can_manage: bool,
        pub can_transfer: bool,
        pub cargo_capacity_m3: f64,
        pub cargo_used_m3: f64,
        pub items: Vec<CargoStack>,
        pub products: Vec<CargoStack>,
        pub jobs: Vec<JobView>,
        pub capabilities: Vec<FacilityCapability>,
        pub location: Option<EntityId>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct BlueprintView {
        pub name: String,
        pub blueprint: Vec<u8>,
        pub inputs: Vec<ItemStack>,
        pub duration_ticks: u64,
        pub energy_j: u64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum IndustryCommand {
        UnloadProduct {
            source: EntityId,
            target: EntityId,
            resource: String,
            quantity: u64,
        },
        Refill {
            source: EntityId,
            ship: EntityId,
            resource: String,
            quantity: u64,
        },
        StartRecipe {
            facility: EntityId,
            recipe: String,
            batches: u32,
        },
        BuildShip {
            facility: EntityId,
            owner: Principal,
            blueprint_hash: [u8; 32],
        },
        CancelJob {
            facility: EntityId,
            job: Id,
        },
        Transfer {
            source: EntityId,
            target: EntityId,
            item: CargoItem,
            quantity: u64,
        },
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct IndustrySubscription {
        pub revision: u64,
        pub directory: bool,
        pub directory_after: Option<Id>,
        pub hangar: Option<HangarSubscription>,
        pub inventories: Vec<EntityId>,
        pub catalogue: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct HangarSubscription {
        pub ship: Id,
        pub after: Option<Id>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct HangarView {
        pub ship: Id,
        pub host: Id,
        pub host_name: String,
        pub host_inventory: Option<FacilitySummary>,
        pub ships: Vec<HangarEntry>,
        pub next: Option<Id>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct HangarEntry {
        pub inventory: FacilitySummary,
        pub can_focus: bool,
        pub can_open_inventory: bool,
        pub can_control: bool,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct FacilitySummary {
        pub entity: EntityId,
        pub owner: Principal,
        pub name: String,
        pub location: Option<EntityId>,
        pub capabilities: Vec<IndustryCapability>,
        pub can_manage: bool,
        pub can_transfer: bool,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct IndustryCatalogue {
        pub revision: [u8; 32],
        pub recipes: Vec<Recipe>,
        pub blueprints: Vec<BlueprintView>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct IndustrySnapshot {
        pub subscription_revision: u64,
        pub error: Option<String>,
        pub omitted_inventories: Vec<EntityId>,
        pub directory: Vec<FacilitySummary>,
        pub directory_next: Option<Id>,
        pub hangar: Option<HangarView>,
        pub facilities: Vec<FacilityView>,
        pub catalogue: Option<IndustryCatalogue>,
    }
}

pub mod chat {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ChatMessage {
        pub id: Id,
        pub sequence: u64,
        pub tick: u64,
        pub calendar_unix_ms: i64,
        pub sender_name: String,
        pub advertised_owner: Option<AccountId>,
        pub advertised_organization: Option<Id>,
        pub text: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ChatPage {
        pub messages: Vec<ChatMessage>,
        pub next_sequence: u64,
        pub missed: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ChatSubscription {
        pub revision: u64,
        pub view: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ChatUpdate {
        pub subscription_revision: u64,
        pub view: u64,
        pub view_revision: u64,
        pub unavailable: bool,
        pub page: ChatPage,
    }
}

pub mod drawing {
    use super::*;

    pub type ScreenId = u8;

    pub type Ink = [u8; 3];

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Draw {
        Pixel {
            at: [i16; 2],
            color: Ink,
        },
        Text {
            at: [i16; 2],
            text: String,
            color: Ink,
        },
        Line {
            from: [i16; 2],
            to: [i16; 2],
            color: Ink,
        },
        Polyline {
            points: Vec<[i16; 2]>,
            color: Ink,
        },
        Rect {
            at: [i16; 2],
            size: [u16; 2],
            filled: bool,
            color: Ink,
        },
        Ellipse {
            centre: [i16; 2],
            radii: [u16; 2],
            filled: bool,
            color: Ink,
        },
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ScreenImage {
        pub screen_id: ScreenId,
        pub background: Ink,
        pub width: u16,
        pub height: u16,
        pub draws: Vec<Draw>,
        pub buttons: [Option<String>; 12],
    }
}

pub mod serial {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Cell {
        pub character: char,
        pub foreground: [u8; 3],
        pub background: Option<[u8; 3]>,
        pub bold: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Screen {
        pub cells: Vec<Cell>,
    }
}

pub mod optical {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct OpticalObservation {
        pub view: u64,
        pub id: Id,
        pub spatial_instance: Id,
        pub known_entity: Option<EntityId>,
        pub iff: Option<crate::IffIdentity>,
        pub contact: Option<ContactRef>,
        pub pose: Pose,
        pub radius_m: f64,
        pub luminosity_w: f64,
        pub appearance: Option<[u8; 32]>,
        pub visual: ShipVisual,
    }
}

pub mod slip_visual {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct SlipPresentation {
        pub wakes: Vec<SlipWake>,
        pub transitions: Vec<SlipTransition>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct SlipWake {
        pub view: u64,
        pub id: Id,
        pub start: GalacticPosition,
        pub end: GalacticPosition,
        pub start_ns: u64,
        pub end_ns: u64,
        pub drift_m_s: [f64; 3],
        pub radius_m: f64,
        pub seed: u32,
        pub offset_m: f64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct SlipTransition {
        pub view: u64,
        pub id: Id,
        pub time_ns: u64,
        pub position: GalacticPosition,
        pub drift_m_s: [f64; 3],
        pub direction: [f64; 3],
        pub radius_m: f64,
        pub arriving: bool,
        pub seed: u32,
    }
}

pub mod transfer {

    #[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct TransferCost {
        pub seconds_per_kg: f64,
    }
}
