# Protocol payload schemas — autopilot updated for version 56

This reference retains historical protocol sections; current definitions are in
`crates/osg-model/src` and `crates/osg-protocol/src`. It covers main-stream messages, blueprint upload
acknowledgements, and inhabited-directory assets. Field order is wire order;
enum numbers below are the Postcard discriminants. See [the protocol](protocol.md)
for primitive encodings, units, constraints, and session behavior.

[protocol-schema.rs](protocol-schema.rs) supplies the same declarations as a
standalone Rust library requiring only Serde with its `derive` feature. Using
this library is optional; the declarations define the bytes for any language.
It contains no simulation or rendering behavior. `GalacticPosition` is three
consecutive signed 128-bit coordinates. Ownership wrappers have no wire encoding;
sequence indexes are expressed as `u64` and must obey their documented bounds.

The three entry points are `Message`, `industry::BlueprintUploadAck`, and
`presentation::InhabitedDirectory`. All referenced types are defined below.

## Messages and common types

### Id

```rust
pub struct Id(pub [u8; 16]);
```

### EntityId

```rust
pub type EntityId = Id;
```

### AccountId

```rust
pub type AccountId = Id;
```

### Pose

```rust
pub struct Pose {
    pub position: GalacticPosition,
    pub velocity: [f64; 3],
    pub rotation: [f64; 4],
    pub angular_velocity: [f64; 3],
}
```

### IffIdentity

```rust
pub struct IffIdentity {
    pub owner: AccountId,
    pub faction: Option<Id>,
    pub labels: BTreeSet<String>,
    pub enabled: bool,
}
```

### SensorObservation

```rust
pub struct SensorObservation {
    pub spatial_instance: Id,
    pub id: u64,
    pub entity: Option<EntityId>,
    pub pose: Pose,
    pub radius_m: f64,
    pub iff: Option<IffIdentity>,
}
```

### ViewSubscription

```rust
pub struct ViewSubscription {
    pub id: u64,
    pub revision: u64,
    pub focused_ship: Option<EntityId>,
}
```

### ViewState

```rust
pub struct ViewState {
    pub focused_ship: Option<EntityId>,
    pub origin: GalacticPosition,
    pub id: u64,
    pub revision: u64,
}
```

### DockServiceSettings

```rust
pub struct DockServiceSettings {
    pub cargo: bool,
    pub power: bool,
}
```

### ShipTelemetry

```rust
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
```

### ScreenUpdate

```rust
pub struct ScreenUpdate {
    pub ship: EntityId,
    pub slot: u8,
    pub revision: u64,
    pub tick: u64,
    pub frame: Option<drawing::ScreenImage>,
    pub error: Option<String>,
}
```

### Event

```rust
pub struct Event {
    pub sequence: u64,
    pub tick: u64,
    pub subject: Option<EntityId>,
    pub kind: String,
    pub position: Option<GalacticPosition>,
}
```

### Reply

Tags: `0` Route.

```rust
pub enum Reply {
    Route { id: u64, status: routing::Status },
}
```

### CommandResult

```rust
pub struct CommandResult {
    pub reply: Option<Reply>,
    pub id: Id,
    pub effective_tick: u64,
    pub error: Option<String>,
}
```

### Frame

```rust
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
```

### Action

Tags: `0` RouteCancel; `1` RouteRequest; `2` RoutePoll; `3` ChatSubscribe; `4` ChatUnsubscribe; `5` ChatSend; `6` Industry; `7` IndustrySubscribe; `8` IndustryUnsubscribe; `9` Society; `10` InstrumentSubscribe; `11` InstrumentUnsubscribe; `12` Debug; `13` Subscribe; `14` Unsubscribe; `15` ScreenSubscribe; `16` ScreenUnsubscribe; `17` Ship.

```rust
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
```

### ShipCommand

Tags: `0` UseRoute; `1` Flight; `2` SetTransponderEnabled; `3` MarkTarget; `4` StopFiring; `5` UnmarkTarget; `6` StartFiring; `7` Aim; `8` SetIff; `9` SetItinerary; `10` SetGuidance; `11` SetAutopilot; `12` SetThrottle; `13` SetDockServices; `14` Undock; `15` Dock; `16` ScreenInput.

```rust
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
```

### InputFrame

```rust
pub struct InputFrame {
    pub world: Id,
    pub sequence: u64,
    pub actions: Vec<(Id, Action)>,
}
```

### GalacticPosition

```rust
pub struct GalacticPosition {
    pub x: i128,
    pub y: i128,
    pub z: i128,
}
```

### Message

Tags: `0` State; `1` Input; `2` Session.

```rust
pub enum Message {
    State(Frame),
    Input(InputFrame),
    Session { world: Id, universe: UniverseDescriptor },
}
```

## presentation

### ContactRef

```rust
pub struct ContactRef {
    pub observer: EntityId,
    pub contact: u64,
}
```

### PresentationFrame

```rust
pub struct PresentationFrame {
    pub slip: slip_visual::SlipPresentation,
    pub navigation: NavigationSnapshot,
    pub ships: Vec<ShipPresentation>,
    pub combat: Vec<CombatEvent>,
    pub capabilities: Vec<DebugCapability>,
    pub diagnostics: Option<Diagnostics>,
}
```

### ShipPresentation

```rust
pub struct ShipPresentation {
    pub serial: serial::Screen,
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
    pub cargo: Vec<industry::CargoStack>,
    pub cargo_capacity_m3: f64,
    pub cargo_used_m3: f64,
    pub computer: ComputerStatus,
    pub instruments: Option<Instruments>,
    pub screens: Vec<ScreenDefinition>,
}
```

### ReactorTelemetry

```rust
pub struct ReactorTelemetry {
    pub name: String,
    pub status: ReactorStatus,
    pub temperature_k: f64,
    pub coolant_temperature_k: f64,
    pub operating_temperature_k: f64,
    pub shutdown_temperature_k: f64,
}
```

### ReactorStatus

Tags: `0` Running; `1` Standby; `2` Shutdown; `3` Damaged.

```rust
pub enum ReactorStatus {
    Running,
    Standby,
    Shutdown,
    Damaged,
}
```

### SlipChargeTelemetry

```rust
pub struct SlipChargeTelemetry {
    pub stored_j: u64,
    pub required_j: u64,
    pub input_w: f64,
    pub remaining_s: Option<f64>,
}
```

### SlipTransitTelemetry

```rust
pub struct SlipTransitTelemetry {
    pub departed_ns: u64,
    pub destination: GalacticPosition,
    pub failure_ppm: f64,
    pub direction: [f64; 3],
}
```

### PropulsionTelemetry

```rust
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
```

### DriveReserve

```rust
pub struct DriveReserve {
    pub name: String,
    pub resource: String,
    pub delta_v_m_s: f64,
    pub full_delta_v_m_s: f64,
    pub flow_kg_s: f64,
}
```

### ResourceAmount

```rust
pub struct ResourceAmount {
    pub resource: String,
    pub quantity: u64,
    pub unit_mass_kg: f64,
    pub unit_volume_m3: f64,
    pub name: String,
    pub amount_kg: f64,
    pub capacity_kg: f64,
}
```

### ComputerStatus

Tags: `0` Unpowered; `1` Booting; `2` Running; `3` Paused; `4` Fault.

```rust
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
```

### ExecutionStatus

Tags: `0` Ready; `1` Suspended; `2` WaitingForGas.

```rust
pub enum ExecutionStatus {
    Ready,
    Suspended,
    WaitingForGas,
}
```

### Instruments

```rust
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
```

### AttitudeInstrument

```rust
pub struct AttitudeInstrument {
    pub mode: u64,
    pub reference: Option<[f64; 4]>,
    pub control_error_rad: f64,
}
```

### NavigationInstrument

```rust
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
```

### WeaponInstrument

```rust
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
```

### Trajectory

```rust
pub struct Trajectory {
    pub id: u64,
    pub revision: u64,
    pub published_at_ns: u64,
    pub valid_until_ns: u64,
    pub timed: bool,
    pub vertices: Vec<TrajectoryVertex>,
}
```

### TrajectoryVertex

```rust
pub struct TrajectoryVertex {
    pub sim_time_ns: u64,
    pub position: GalacticPosition,
}
```

### NavigationMarker

```rust
pub struct NavigationMarker {
    pub id: u64,
    pub kind: u64,
    pub position: GalacticPosition,
    pub sim_time_ns: u64,
    pub label: String,
}
```

### ScreenDefinition

```rust
pub struct ScreenDefinition {
    pub slot: u8,
    pub width: u16,
    pub height: u16,
    pub title: String,
}
```

### ShipVisual

```rust
pub struct ShipVisual {
    pub slip_readiness: f64,
    pub engines: Vec<EngineVisual>,
    pub turrets: Vec<TurretVisual>,
    pub shield: Option<ShieldVisual>,
}
```

### EngineVisual

```rust
pub struct EngineVisual {
    pub part: u64,
    pub thrust_n: [f64; 3],
    pub thrust_fraction: f64,
}
```

### TurretVisual

```rust
pub struct TurretVisual {
    pub part: u64,
    pub yaw_rad: f64,
    pub pitch_rad: f64,
}
```

### ShieldVisual

```rust
pub struct ShieldVisual {
    pub temperature_k: f64,
    pub coverage: f64,
}
```

### CombatEvent

```rust
pub struct CombatEvent {
    pub sequence: u64,
    pub sim_time_ns: u64,
    pub kind: CombatEventKind,
}
```

### CombatEventKind

Tags: `0` Beam; `1` Projectile; `2` Fired; `3` Impact; `4` Destroyed.

```rust
pub enum CombatEventKind {
    Beam {
        source: Id,
        start: GalacticPosition,
        end: GalacticPosition,
        velocity_m_s: [f64; 3],
        end_time_ns: u64,
    },
    Projectile {
        id: u64,
        source: Option<Id>,
        start: GalacticPosition,
        end: GalacticPosition,
        end_time_ns: u64,
        radius_m: f64,
    },
    Fired {
        source: Id,
        position: GalacticPosition,
        energy_j: f64,
    },
    Impact {
        normal: [f64; 3],
        target: Option<Id>,
        position: GalacticPosition,
        velocity_m_s: [f64; 3],
        energy_j: f64,
        shield: bool,
    },
    Destroyed {
        target: Id,
        pose: Pose,
        appearance: Option<[u8; 32]>,
        energy_j: f64,
        mass_kg: f64,
        radius_m: f64,
    },
}
```

### DebugCapability

Tags: `0` Clock; `1` Reset; `2` Relocate; `3` Recover; `4` InjectHeat; `5` Inspect; `6` ConfigureSensor.

```rust
pub enum DebugCapability {
    Clock,
    Reset,
    Relocate,
    Recover,
    InjectHeat,
    Inspect,
    ConfigureSensor,
}
```

### DebugCommand

Tags: `0` ConfigureSensor; `1` InspectBody; `2` RelocateToBody; `3` InjectShieldHeat; `4` SetRate; `5` Reset; `6` Relocate; `7` Recover; `8` InjectHeat; `9` Inspect.

```rust
pub enum DebugCommand {
    ConfigureSensor {
        ship: EntityId,
        range_m: f64,
        occlusion: bool,
    },
    InspectBody {
        body: Option<travel::CelestialRef>,
    },
    RelocateToBody {
        ship: EntityId,
        body: travel::CelestialRef,
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
```

### Diagnostics

```rust
pub struct Diagnostics {
    pub collision: Option<CollisionDiagnostics>,
    pub entity_count: u64,
    pub active_ships: u64,
    pub dormant_ships: u64,
    pub tick_duration_ms: f64,
    pub systems: Vec<(String, f64)>,
}
```

### CollisionDiagnostics

```rust
pub struct CollisionDiagnostics {
    pub bodies: u64,
    pub candidates: u64,
    pub detailed_queries: u64,
    pub impacts: u64,
    pub dissipated_j: f64,
}
```

### FlightCommand

Tags: `0` HoldAttitude; `1` StopGuidance; `2` AimDirection; `3` SelectTarget; `4` EngageNavigation.

```rust
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
```

### FlightEnvironment

```rust
pub struct FlightEnvironment {
    pub altitude_m: f64,
    pub airspeed_m_s: [f64; 3],
    pub density_kg_m3: f64,
    pub pressure_pa: f64,
}
```

### ShipHealth

```rust
pub struct ShipHealth {
    pub crew_people: u32,
    pub crew_capacity: u32,
    pub life_support_fraction: f64,
    pub hull_hp: f64,
    pub hull_max_hp: f64,
    pub shield_reserve_capacity_kg: f64,
    pub shield_strength: f64,
}
```

### ExecutionMetrics

```rust
pub struct ExecutionMetrics {
    pub memory_bytes: u64,
    pub step_us: f64,
    pub prepare_us: f64,
    pub callback_us: f64,
    pub publish_us: f64,
    pub hardware_us: f64,
    pub scan_us: f64,
}
```

### WeaponsInstrument

```rust
pub struct WeaponsInstrument {
    pub firing: bool,
    pub target: Option<ContactRef>,
    pub reason: String,
}
```

### NavigationSnapshot

```rust
pub struct NavigationSnapshot {
    pub directory: Option<[u8; 32]>,
    pub beacons: Vec<NavigationBeacon>,
}
```

### InhabitedDirectory

```rust
pub struct InhabitedDirectory {
    pub systems: Vec<EntityId>,
    pub ownership: BTreeMap<EntityId, EntityId>,
    pub sovereignties: BTreeMap<EntityId, PublicSovereignty>,
}
```

### PublicSovereignty

```rust
pub struct PublicSovereignty {
    pub id: EntityId,
    pub name: String,
    pub bloc: ownership::Bloc,
}
```

### NavigationBeacon

```rust
pub struct NavigationBeacon {
    pub id: EntityId,
    pub systems: Vec<EntityId>,
    pub name: String,
    pub pose: Pose,
    pub radius_m: f64,
    pub docking: bool,
    pub navigation: bool,
}
```

### UniverseDescriptor

```rust
pub struct UniverseDescriptor {
    pub fingerprint: [u8; 32],
    pub epoch_mjd_utc: f64,
    pub sim_time_origin_ns: u64,
}
```

## travel

### Presence

Tags: `0` Space; `1` Docked; `2` SlipTransit; `3` StoredInWreck; `4` Destroyed.

```rust
pub enum Presence {
    Space,
    Docked { host: EntityId, bay: u32 },
    SlipTransit(Id),
    StoredInWreck(EntityId),
    Destroyed,
}
```

### Axes

Tags: `0` Galactic; `1` BodyFixed.

```rust
pub enum Axes {
    Galactic,
    BodyFixed,
}
```

### Reference

Tags: `0` Celestial; `1` Beacon.

```rust
pub enum Reference {
    Celestial(CelestialRef),
    Beacon(EntityId),
}
```

### CelestialRef

```rust
pub struct CelestialRef {
    pub system: Id,
    pub body: Id,
}
```

### Destination

Tags: `0` Beacon; `1` Galactic; `2` Relative.

```rust
pub enum Destination {
    Beacon(EntityId),
    Galactic(GalacticPosition),
    Relative {
        reference: Reference,
        offset: GalacticPosition,
        axes: Axes,
    },
}
```

### Directive

```rust
pub enum Directive {
    SlipToSystem(Id),
    DockAt(EntityId),
}
```

### PlanningPreferences

```rust
pub struct PlanningPreferences {
    pub fuel_fraction: f64,
    pub max_loss_ppm: f64,
    pub allow_slipdrive: bool,
}
```

### FuelRequirement

```rust
pub struct FuelRequirement {
    pub resource: String,
    pub required_kg: f64,
    pub available_kg: f64,
}
```

### FuelBudget

```rust
pub struct FuelBudget {
    pub resources: Vec<FuelRequirement>,
    pub complete: bool,
}
```

### ItineraryEntry

```rust
pub struct ItineraryEntry {
    pub directive: Directive,
    pub label: String,
    pub max_loss_ppm: f64,
    pub fuel_allowance_kg: f64,
    pub estimated_duration_ticks: Option<u64>,
}
```

### PlanMarker

```rust
pub struct PlanMarker {
    pub position: GalacticPosition,
    pub label: String,
}
```

### FirmwareStatus

```rust
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
```

### FirmwarePhase

```rust
pub enum FirmwarePhase {
    #[default]
    Idle,
    Planning,
    Waiting { until: Option<u64>, why: String },
    Charging,
    Transit,
    Maneuvering,
    Docking,
    Completed,
}
```

### PlanningStage

Tags: `0` LoadingCatalogue; `1` BuildingGraph; `2` SearchingRoutes.

```rust
pub enum PlanningStage {
    LoadingCatalogue,
    BuildingGraph,
    SearchingRoutes,
}
```

### PlanningProgress

```rust
pub struct PlanningProgress {
    pub stage: PlanningStage,
    pub completed: u32,
    pub total: Option<u32>,
}
```

### AutopilotState

```rust
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
```

### RiskBudget

```rust
pub struct RiskBudget {
    pub max_log_loss: f64,
    pub spent_log_loss: f64,
}
```

### Target

Tags: `0` Direction; `1` Destination; `2` Contact.

```rust
pub enum Target {
    Direction([f64; 3]),
    Destination(Destination),
    Contact(ContactRef),
}
```

### GuidanceMode

Tags: `0` Align; `1` Approach; `2` KeepRange.

```rust
pub enum GuidanceMode {
    Align,
    Approach,
    KeepRange,
}
```

### Guidance

```rust
pub struct Guidance {
    pub mode: GuidanceMode,
    pub target: Target,
    pub range_m: f64,
}
```

## location

### LocationRegion

```rust
pub enum LocationRegion {
    System,
    Interstellar,
    SlipTransit,
}
```

### LocationContext

```rust
pub struct LocationContext {
    pub region: LocationRegion,
    pub system: Option<Id>,
    pub primary: Option<CelestialRef>,
    /// Ancestors followed by the primary, from the system root inward.
    pub hierarchy: Vec<CelestialRef>,
    pub sample_tick: u64,
}
```

## routing

### Request

```rust
pub struct Request {
    pub id: u64,
    pub directives: Vec<Directive>,
    pub preferences: PlanningPreferences,
}
```

### Plan

```rust
pub struct Plan {
    pub planned_tick: u64,
    pub directive_revision: u64,
    pub topology_revision: u64,
    pub itinerary: Vec<ItineraryEntry>,
    pub fuel_budget: FuelBudget,
    pub estimated_loss_ppm: f64,
    pub exotic_fuel_kg: f64,
}
```

### Status

Tags: `0` Unknown; `1` Pending; `2` Ready; `3` Failed.

```rust
pub enum Status {
    Unknown,
    Pending { progress: PlanningProgress },
    Ready { plan: Plan },
    Failed { reason: String },
}
```

## ownership

### Principal

Tags: `0` Sovereignty; `1` Organization; `2` Player.

```rust
pub enum Principal {
    Sovereignty(Id),
    Organization(Id),
    Player(AccountId),
}
```

### Bloc

Tags: `0` Union; `1` League; `2` NonAligned.

```rust
pub enum Bloc {
    Union,
    League,
    NonAligned,
}
```

### Sovereignty

```rust
pub struct Sovereignty {
    pub id: Id,
    pub name: String,
    pub bloc: Bloc,
    pub officers: BTreeSet<AccountId>,
}
```

### Organization

```rust
pub struct Organization {
    pub id: Id,
    pub name: String,
    pub sovereignty: Id,
    pub open_membership: bool,
    pub officers: BTreeSet<AccountId>,
}
```

### PlayerAffiliation

```rust
pub struct PlayerAffiliation {
    pub account: AccountId,
    pub name: String,
    pub organization: Option<Id>,
}
```

### Standing

Tags: `0` Friendly; `1` Neutral; `2` Hostile.

```rust
pub enum Standing {
    Friendly,
    Neutral,
    Hostile,
}
```

### OwnershipDirectory

```rust
pub struct OwnershipDirectory {
    pub sovereignties: BTreeMap<Id, Sovereignty>,
    pub organizations: BTreeMap<Id, Organization>,
    pub players: BTreeMap<AccountId, PlayerAffiliation>,
    pub standings: BTreeMap<(Principal, Principal), Standing>,
}
```

### Permission

Tags: `0` Navigate; `1` Dock; `2` View; `3` Control; `4` Configure; `5` TransferCargo; `6` Industry; `7` ManageAccess.

```rust
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
```

### AccessGrant

```rust
pub struct AccessGrant {
    pub principal: Principal,
    pub permissions: BTreeSet<Permission>,
}
```

### AccessPolicy

```rust
pub struct AccessPolicy {
    pub public: BTreeSet<Permission>,
    pub grants: Vec<AccessGrant>,
}
```

### SocietySnapshot

```rust
pub struct SocietySnapshot {
    pub account: AccountId,
    pub directory: OwnershipDirectory,
    pub assets: Vec<AssetAffiliation>,
    pub gas_accounts: Vec<GasAccountSnapshot>,
}
```

### GasAccountSnapshot

```rust
pub struct GasAccountSnapshot {
    pub owner: Principal,
    pub available: u64,
    pub spent: u64,
}
```

### AssetAffiliation

```rust
pub struct AssetAffiliation {
    pub entity: Id,
    pub name: String,
    pub owner: Principal,
    pub access: AccessPolicy,
    pub can_manage: bool,
}
```

### SocietyCommand

Tags: `0` CreateOrganization; `1` SetOfficer; `2` SetStanding; `3` SetMembership; `4` SetAssetAccess; `5` TransferAsset.

```rust
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
```

## industry

### BlueprintUploadAck

Tags: `0` Ready; `1` Rejected.

```rust
pub enum BlueprintUploadAck {
    Ready { hash: [u8; 32] },
    Rejected { reason: String },
}
```

### CargoItem

Tags: `0` Resource; `1` Part.

```rust
pub enum CargoItem {
    Resource(String),
    Part(String),
}
```

### ItemStack

```rust
pub struct ItemStack {
    pub item: CargoItem,
    pub quantity: u64,
}
```

### CargoStack

```rust
pub struct CargoStack {
    pub item: CargoItem,
    pub quantity: u64,
    pub reserved: u64,
    pub name: String,
    pub unit_mass_kg: f64,
    pub unit_volume_m3: f64,
}
```

### IndustryCapability

Tags: `0` Refinery; `1` FuelPlant; `2` Fabricator; `3` Shipyard.

```rust
pub enum IndustryCapability {
    Refinery,
    FuelPlant,
    Fabricator,
    Shipyard,
}
```

### Recipe

```rust
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
```

### JobStatus

Tags: `0` Queued; `1` Running; `2` AwaitingPower; `3` AwaitingCargoSpace; `4` AwaitingBerth; `5` ModuleUnavailable.

```rust
pub enum JobStatus {
    Queued,
    Running,
    AwaitingPower,
    AwaitingCargoSpace,
    AwaitingBerth,
    ModuleUnavailable,
}
```

### JobView

```rust
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
```

### FacilityCapability

```rust
pub struct FacilityCapability {
    pub part: u64,
    pub capability: IndustryCapability,
    pub lanes: u32,
    pub power_per_lane_w: u64,
    pub max_radius_m: Option<f64>,
    pub operational: bool,
}
```

### FacilityView

```rust
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
```

### BlueprintView

```rust
pub struct BlueprintView {
    pub name: String,
    pub blueprint: Vec<u8>,
    pub inputs: Vec<ItemStack>,
    pub duration_ticks: u64,
    pub energy_j: u64,
}
```

### IndustryCommand

Tags: `0` UnloadProduct; `1` Refill; `2` StartRecipe; `3` BuildShip; `4` CancelJob; `5` Transfer.

```rust
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
```

### IndustrySubscription

```rust
pub struct IndustrySubscription {
    pub revision: u64,
    pub directory: bool,
    pub directory_after: Option<Id>,
    pub hangar: Option<HangarSubscription>,
    pub inventories: Vec<EntityId>,
    pub catalogue: bool,
}
```

### HangarSubscription

```rust
pub struct HangarSubscription {
    pub ship: Id,
    pub after: Option<Id>,
}
```

### HangarView

```rust
pub struct HangarView {
    pub ship: Id,
    pub host: Id,
    pub host_name: String,
    pub host_inventory: Option<FacilitySummary>,
    pub ships: Vec<HangarEntry>,
    pub next: Option<Id>,
}
```

### HangarEntry

```rust
pub struct HangarEntry {
    pub inventory: FacilitySummary,
    pub can_focus: bool,
    pub can_open_inventory: bool,
    pub can_control: bool,
}
```

### FacilitySummary

```rust
pub struct FacilitySummary {
    pub entity: EntityId,
    pub owner: Principal,
    pub name: String,
    pub location: Option<EntityId>,
    pub capabilities: Vec<IndustryCapability>,
    pub can_manage: bool,
    pub can_transfer: bool,
}
```

### IndustryCatalogue

```rust
pub struct IndustryCatalogue {
    pub revision: [u8; 32],
    pub recipes: Vec<Recipe>,
    pub blueprints: Vec<BlueprintView>,
}
```

### IndustrySnapshot

```rust
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
```

## chat

### ChatMessage

```rust
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
```

### ChatPage

```rust
pub struct ChatPage {
    pub messages: Vec<ChatMessage>,
    pub next_sequence: u64,
    pub missed: u64,
}
```

### ChatSubscription

```rust
pub struct ChatSubscription {
    pub revision: u64,
    pub view: u64,
}
```

### ChatUpdate

```rust
pub struct ChatUpdate {
    pub subscription_revision: u64,
    pub view: u64,
    pub view_revision: u64,
    pub unavailable: bool,
    pub page: ChatPage,
}
```

## drawing

### ScreenId

```rust
pub type ScreenId = u8;
```

### Ink

```rust
pub type Ink = [u8; 3];
```

### Draw

Tags: `0` Pixel; `1` Text; `2` Line; `3` Polyline; `4` Rect; `5` Ellipse.

```rust
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
```

### ScreenImage

```rust
pub struct ScreenImage {
    pub screen_id: ScreenId,
    pub background: Ink,
    pub width: u16,
    pub height: u16,
    pub draws: Vec<Draw>,
    pub buttons: [Option<String>; 12],
}
```

## serial

### Cell

```rust
pub struct Cell {
    pub character: char,
    pub foreground: [u8; 3],
    pub background: Option<[u8; 3]>,
    pub bold: bool,
}
```

### Screen

```rust
pub struct Screen {
    pub cells: Vec<Cell>,
}
```

## optical

### OpticalObservation

```rust
pub struct OpticalObservation {
    pub view: u64,
    pub id: Id,
    pub spatial_instance: Id,
    pub known_entity: Option<EntityId>,
    pub iff: Option<IffIdentity>,
    pub contact: Option<ContactRef>,
    pub pose: Pose,
    pub radius_m: f64,
    pub luminosity_w: f64,
    pub appearance: Option<[u8; 32]>,
    pub visual: ShipVisual,
}
```

## slip_visual

### SlipPresentation

```rust
pub struct SlipPresentation {
    pub wakes: Vec<SlipWake>,
    pub transitions: Vec<SlipTransition>,
}
```

### SlipWake

```rust
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
```

### SlipTransition

```rust
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
```

## transfer

### TransferCost

```rust
pub struct TransferCost {
    pub seconds_per_kg: f64,
}
```
