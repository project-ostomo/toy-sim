use crate::{EntityId, GalacticPosition, Pose};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContactRef {
    pub observer: EntityId,
    pub contact: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresentationFrame {
    pub slip: crate::slip_visual::SlipPresentation,
    pub navigation: std::sync::Arc<NavigationSnapshot>,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelestialPresentation {
    pub reference: crate::travel::CelestialRef,
    pub entity: EntityId,
    pub name: String,
    pub pose: Pose,
    pub radius_m: f64,
    pub gravitational_parameter: f64,
    pub luminosity_lumens: f64,
    pub temperature_k: f64,
    pub color: [f32; 3],
    pub atmosphere: Option<AtmospherePresentation>,
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

impl DebugCommand {
    pub fn capability(&self) -> DebugCapability {
        match self {
            Self::ConfigureSensor { .. } => DebugCapability::ConfigureSensor,
            Self::InspectBody { .. } => DebugCapability::Inspect,
            Self::RelocateToBody { .. } => DebugCapability::Relocate,
            Self::InjectShieldHeat { .. } => DebugCapability::InjectHeat,
            Self::SetRate(_) => DebugCapability::Clock,
            Self::Reset => DebugCapability::Reset,
            Self::Relocate { .. } => DebugCapability::Relocate,
            Self::Recover { .. } => DebugCapability::Recover,
            Self::InjectHeat { .. } => DebugCapability::InjectHeat,
            Self::Inspect(_) => DebugCapability::Inspect,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
pub struct AtmospherePresentation {
    pub height_m: f64,
    pub scale_height_m: f64,
    pub rayleigh_scattering: [f32; 3],
    pub mie_scattering: f32,
    pub mie_absorption: f32,
    pub mie_scale_height_m: f64,
    pub mie_asymmetry: f32,
    pub ground_albedo: [f32; 3],
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationSnapshot {
    pub directory: Option<[u8; 32]>,
    pub beacons: Vec<NavigationBeacon>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationCatalogue {
    pub topology_revision: u64,
    pub systems: Vec<NavigationSystem>,
    pub beacons: Vec<NavigationBeacon>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationSystem {
    pub sovereignty: Option<EntityId>,
    pub id: EntityId,
    pub name: String,
    pub position: GalacticPosition,
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
