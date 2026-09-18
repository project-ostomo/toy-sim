use crate::{EntityId, GalacticPosition, GroupId, Pose, TrackId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRef {
    pub group: GroupId,
    pub track: TrackId,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresentationFrame {
    pub navigation: NavigationCatalogue,
    pub ships: Vec<ShipPresentation>,
    pub visuals: Vec<TrackVisual>,
    pub combat: Vec<CombatEvent>,
    pub celestial_systems: Vec<CelestialSystemRef>,
    pub capabilities: Vec<DebugCapability>,
    pub diagnostics: Option<Diagnostics>,
    pub universe: Option<UniverseStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShipPresentation {
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
    pub battery_capacity_j: f64,
    pub power_generated_w: f64,
    pub power_consumed_w: f64,
    pub inventory: Vec<ResourceAmount>,
    pub cargo_capacity_m3: f64,
    pub cargo_used_m3: f64,
    pub devices: Vec<DeviceTelemetry>,
    pub computer: ComputerStatus,
    pub instruments: Option<Instruments>,
    pub screens: Vec<ScreenDefinition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceAmount {
    pub resource: String,
    pub quantity: u64,
    pub cargo_quantity: u64,
    pub unit_mass_kg: f64,
    pub unit_volume_m3: f64,
    pub name: String,
    pub amount_kg: f64,
    pub capacity_kg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceTelemetry {
    pub part: u64,
    pub name: String,
    pub enabled: bool,
    pub power_requested_w: f64,
    pub power_delivered_w: f64,
    pub reading: DeviceReading,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeviceReading {
    Rcs {
        thrust_n: [f64; 3],
    },
    Accelerometer {
        acceleration_m_s2: Option<[f64; 3]>,
    },
    Engine {
        throttle: f64,
        thrust_n: f64,
    },
    Torquer {
        torque_nm: [f64; 3],
    },
    Generator {
        output_w: f64,
    },
    Battery {
        energy_j: f64,
        capacity_j: f64,
    },
    Shield {
        temperature_k: f64,
        area_m2: f64,
        reserve_kg: f64,
        feed_kg_s: f64,
        ablation_kg_s: f64,
    },
    Weapon {
        yaw_rad: f64,
        pitch_rad: f64,
        loaded: bool,
        firing: bool,
        progress: f64,
    },
    Sensor {
        range_m: f64,
    },
    Storage {
        contents: Vec<ResourceAmount>,
    },
    Avionics,
    Structure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ComputerStatus {
    Unpowered,
    Booting { progress: f64 },
    Running { gas_used: u64, gas_limit: u64 },
    Paused,
    Fault(String),
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
    pub battery_energy_j: f64,
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
pub struct TrackVisual {
    pub contact: ContactRef,
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
    Projectile {
        id: u64,
        source: Option<ContactRef>,
        start: GalacticPosition,
        end: GalacticPosition,
        end_time_ns: u64,
        radius_m: f64,
    },
    Fired {
        source: ContactRef,
        position: GalacticPosition,
        energy_j: f64,
    },
    Impact {
        normal: [f64; 3],
        target: Option<ContactRef>,
        position: GalacticPosition,
        velocity_m_s: [f64; 3],
        energy_j: f64,
        shield: bool,
    },
    Destroyed {
        target: ContactRef,
        pose: Pose,
        appearance: Option<[u8; 32]>,
        energy_j: f64,
        mass_kg: f64,
        radius_m: f64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelestialPresentation {
    pub entity: EntityId,
    pub name: String,
    pub pose: Pose,
    pub radius_m: f64,
    pub gravitational_parameter: f64,
    pub luminosity_lumens: f64,
    pub temperature_k: f64,
    pub color: [f32; 3],
    pub atmosphere: Option<AtmospherePresentation>,
    pub ephemeris: Option<[u8; 32]>,
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
        body: Option<EntityId>,
    },
    RelocateToBody {
        ship: EntityId,
        body: EntityId,
    },
    InjectShieldHeat {
        ship: EntityId,
        joules: f64,
    },
    SetRate(f64),
    Step,
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
            Self::SetRate(_) | Self::Step => DebugCapability::Clock,
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
    pub contact_reviews: u64,
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
    pub mode: u64,
    pub target: Option<ContactRef>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UniverseStatus {
    pub catalogue: [u8; 32],
    pub active_systems: Vec<ActiveSystem>,
    pub inspected_body: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveSystem {
    pub system: EntityId,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UniverseCatalogue {
    pub systems: Vec<UniverseSystem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UniverseSystem {
    pub id: EntityId,
    pub name: String,
    pub position: GalacticPosition,
    pub influence_radius_m: f64,
    pub bodies: Vec<UniverseBody>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UniverseBody {
    pub id: EntityId,
    pub name: String,
    pub kind: String,
    pub radius_m: f64,
    pub mass_kg: f64,
    pub parent: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelestialSystemRef {
    pub view: u64,
    pub system: EntityId,
    pub definition: [u8; 32],
    pub epoch_mjd_utc: f64,
    pub sim_time_origin_ns: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationCatalogue {
    pub systems: Vec<NavigationSystem>,
    pub beacons: Vec<NavigationBeacon>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationSystem {
    pub id: EntityId,
    pub name: String,
    pub position: GalacticPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationBeacon {
    pub id: EntityId,
    pub system: EntityId,
    pub name: String,
    pub pose: Pose,
    pub radius_m: f64,
    pub gate_exit: Option<EntityId>,
    pub docking: bool,
}
