//! Ship ABI: fixed little-endian C records and caller-owned output buffers.
use core::mem::{align_of, size_of};
#[cfg(target_endian = "big")]
compile_error!("ship ABI requires little endian");
pub const IMPORT_MODULE: &str = "ship_v32";
pub const VERSION: u32 = 32;
pub const ERR_BUFFER: i32 = -2;
pub const ERR_ARGUMENT: i32 = -3;
pub const ERR_UNAVAILABLE: i32 = -4;
pub const ERR_LIMIT: i32 = -5;
pub const ERR_HANDLE: i32 = -6;
pub const ERR_UNSUPPORTED: i32 = -7;
pub const CALL_GAS: u64 = 100;
pub const SCAN_GAS_PER_OBJECT: u64 = 3000;
pub const MAX_CONTACTS: u32 = 256;
pub const MAX_TRACKS: u32 = 512;
pub const MAX_SNAPSHOTS: u32 = 8;
pub const MAX_MARKERS: u32 = 64;
pub const MAX_PATHS: u32 = 8;
pub const MAX_PATH_VERTICES: u32 = 128;
pub const MAX_TOTAL_VERTICES: u32 = 1024;
pub const MAX_SCREENS: u32 = 8;
pub const MAX_SCREEN_PRIMITIVES: u32 = 256;
pub const MAX_SCREEN_PAYLOAD: u32 = 65536;
pub const MAX_INPUTS: u32 = 256;
pub const INTEREST_MARKERS: u64 = 1;
pub const INTEREST_PATHS: u64 = 2;
pub const FIRST_CALLBACK: u64 = 1;
pub const OPERATIONAL: u64 = 1;
pub const POWERED: u64 = 2;
pub const CONTROL_ENABLED: u64 = 1;
pub const CONTACT_SHIP: u64 = 0;
pub const CONTACT_CELESTIAL: u64 = 1;
pub const CONTACT_OTHER: u64 = 2;
pub const DEVICE_ACCELEROMETER: u64 = 0;
pub const DEVICE_COMPUTER: u64 = 1;
pub const DEVICE_STORAGE: u64 = 2;
pub const DEVICE_BATTERY: u64 = 3;
pub const DEVICE_ENGINE: u64 = 4;
pub const DEVICE_TORQUER: u64 = 5;
pub const DEVICE_GENERATOR: u64 = 6;
pub const DEVICE_SHIELD: u64 = 7;
pub const DEVICE_SENSOR: u64 = 8;
pub const SET_THROTTLE: u64 = 0;
pub const SET_TORQUE: u64 = 1;
pub const SET_GENERATOR_DEMAND: u64 = 2;
pub const SET_SHIELD_ENABLED: u64 = 3;
pub const SET_SENSOR_ENABLED: u64 = 4;
pub const REQUEST_THROTTLE: u64 = 11;
pub const REQUEST_MANUAL: u64 = 0;
pub const REQUEST_HOLD_ATTITUDE: u64 = 1;
pub const REQUEST_STOP_GUIDANCE: u64 = 2;
pub const REQUEST_AIM_DIRECTION: u64 = 3;
pub const REQUEST_AIM_CONTACT: u64 = 4;
pub const REQUEST_SELECT_TARGET: u64 = 5;
pub const REQUEST_ENGAGE_NAVIGATION: u64 = 6;
pub const REPLY_ACCEPTED: u64 = 0;
pub const REPLY_REJECTED: u64 = 1;
pub const REPLY_UNSUPPORTED: u64 = 2;
pub const FRAME_SNAPSHOT: u64 = 0;
pub const FRAME_SHIP: u64 = 1;
pub const FRAME_SHIP_BODY: u64 = 2;
pub const FRAME_CONTACT: u64 = 3;
pub const FRAME_PATH: u64 = 4;
pub const TIME_CURRENT: u64 = 0;
pub const TIME_FIXED: u64 = 1;
pub const PATH_POLYLINE: u64 = 0;
pub const PATH_TIMED: u64 = 1;
pub const MARKER_ANNOTATION: u64 = 0;
pub const MARKER_TARGET: u64 = 1;
pub const MARKER_WAYPOINT: u64 = 2;
pub const MARKER_AIM: u64 = 3;
pub const MARKER_EVENT: u64 = 4;
pub const PATH_REFERENCE: u64 = 0;
pub const PATH_OWN_FORECAST: u64 = 1;
pub const PATH_CONTACT_FORECAST: u64 = 2;
pub const PATH_ROUTE: u64 = 3;
pub const ATTITUDE_MANUAL: u64 = 0;
pub const ATTITUDE_HOLD: u64 = 1;
pub const ATTITUDE_GUIDANCE: u64 = 2;
pub const ATTITUDE_REFERENCE: u64 = 1;
pub const NAV_IDLE: u64 = 0;
pub const NAV_ACTIVE: u64 = 1;
pub const NAV_SUSPENDED: u64 = 2;
pub const NAV_UNAVAILABLE: u64 = 3;
pub const NAV_STAND_OFF: u64 = 1;
pub const NAV_SPEED_LIMIT: u64 = 2;
pub const NAV_BRAKING_DISTANCE: u64 = 4;
pub const NAV_ARRIVAL: u64 = 8;
pub const NAV_FUEL: u64 = 16;
pub const INSTRUMENT_ATTITUDE: u64 = 0;
pub const INSTRUMENT_NAVIGATION: u64 = 1;
pub const INSTRUMENT_CONTACTS: u64 = 2;
pub const DRAW_PIXEL: u64 = 0;
pub const DRAW_TEXT: u64 = 1;
pub const DRAW_LINE: u64 = 2;
pub const DRAW_POLYLINE: u64 = 3;
pub const DRAW_RECTANGLE: u64 = 4;
pub const DRAW_ELLIPSE: u64 = 5;
pub const EVENT_POINTER_MOVE: u64 = 0;
pub const EVENT_POINTER_PRESS: u64 = 1;
pub const EVENT_POINTER_RELEASE: u64 = 2;
pub const EVENT_KEY_PRESS: u64 = 3;
pub const EVENT_TEXT: u64 = 4;
pub const EVENT_KEY_RELEASE: u64 = 5;
pub const EVENT_BEZEL: u64 = 6;
pub const EVENT_RESET: u64 = 7;

pub const DEVICE_WEAPON: u64 = 9;
pub const DEVICE_RCS: u64 = 10;
pub const SET_RCS: u64 = 6;
pub const WEAPON_PROPELLANT: u64 = 4096;
pub const SET_WEAPON: u64 = 5;
pub const REQUEST_MARK_TARGET: u64 = 7;
pub const REQUEST_STOP_FIRING: u64 = 8;
pub const REQUEST_UNMARK_TARGET: u64 = 9;
pub const REQUEST_START_FIRING: u64 = 10;
pub const INSTRUMENT_WEAPONS: u64 = 3;
pub const CONTACT_PROJECTILE: u64 = 3;
pub const WEAPONS_HOLD: u64 = 0;
pub const WEAPONS_FIRING: u64 = 1;
pub const WEAPON_UNAVAILABLE: u64 = 1;
pub const WEAPON_AMMO: u64 = 4;
pub const WEAPON_COOLDOWN: u64 = 16;
pub const WEAPON_BLOCKED: u64 = 128;
pub const WEAPON_TRAVEL: u64 = 256;
pub const WEAPON_POINTING: u64 = 512;
pub const WEAPON_EXPIRED: u64 = 1024;
pub const WEAPON_ENERGY: u64 = 2048;
pub const WEAPON_SOLUTION: u64 = 1;
pub const WEAPON_ROW_GAS: u64 = 100;

pub(crate) mod private {
    pub trait Sealed {}
}

pub trait Record: private::Sealed + Copy + Default {
    fn bytes(&self) -> &[u8] {
        // SAFETY: sealed records have no padding, references, or invalid bit patterns.
        unsafe { core::slice::from_raw_parts((self as *const Self).cast(), size_of::<Self>()) }
    }
    fn read(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        let mut value = Self::default();
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (&mut value as *mut Self).cast(),
                bytes.len(),
            );
        }
        Some(value)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ThrottleRequest {
    pub throttle: f64,
}
impl private::Sealed for ThrottleRequest {}
impl Record for ThrottleRequest {}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Text<const N: usize> {
    pub len: u64,
    pub bytes: [u8; N],
}
impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self {
            len: 0,
            bytes: [0; N],
        }
    }
}
impl<const N: usize> Text<N> {
    pub fn new(text: &str) -> Self {
        let mut n = text.len().min(N);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        let mut out = Self::default();
        out.len = n as u64;
        out.bytes[..n].copy_from_slice(&text.as_bytes()[..n]);
        out
    }
    pub fn as_str(&self) -> Option<&str> {
        let n = usize::try_from(self.len).ok()?;
        if self.bytes.get(n..)?.iter().any(|b| *b != 0) {
            return None;
        }
        core::str::from_utf8(self.bytes.get(..n)?).ok()
    }
}

pub type Text64 = Text<64>;
pub type Text256 = Text<256>;
impl private::Sealed for Text64 {}

impl Record for Text64 {}

impl private::Sealed for Text256 {}

impl Record for Text256 {}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TickContext {
    pub tick: u64,
    pub snapshot: u64,
    pub time_s: f64,
    pub dt_s: f64,
    pub physics_dt_s: f64,
    pub device_count: u64,
    pub resource_count: u64,
    pub request_count: u64,
    pub screen_event_count: u64,
    pub interest: u64,
    pub requested_screens: u64,
    pub flags: u64,
}

impl private::Sealed for TickContext {}

impl Record for TickContext {}
const _: () = assert!(size_of::<TickContext>() == 96 && align_of::<TickContext>() == 8);
const _: () = assert!(core::mem::offset_of!(TickContext, tick) == 0);
const _: () = assert!(core::mem::offset_of!(TickContext, snapshot) == 8);
const _: () = assert!(core::mem::offset_of!(TickContext, time_s) == 16);
const _: () = assert!(core::mem::offset_of!(TickContext, dt_s) == 24);
const _: () = assert!(core::mem::offset_of!(TickContext, physics_dt_s) == 32);
const _: () = assert!(core::mem::offset_of!(TickContext, device_count) == 40);
const _: () = assert!(core::mem::offset_of!(TickContext, resource_count) == 48);
const _: () = assert!(core::mem::offset_of!(TickContext, request_count) == 56);
const _: () = assert!(core::mem::offset_of!(TickContext, screen_event_count) == 64);
const _: () = assert!(core::mem::offset_of!(TickContext, interest) == 72);
const _: () = assert!(core::mem::offset_of!(TickContext, requested_screens) == 80);
const _: () = assert!(core::mem::offset_of!(TickContext, flags) == 88);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BudgetInfo {
    pub gas_remaining: u64,
    pub gas_limit: u64,
    pub gas_per_tick: u64,
}

impl private::Sealed for BudgetInfo {}

impl Record for BudgetInfo {}
const _: () = assert!(size_of::<BudgetInfo>() == 24 && align_of::<BudgetInfo>() == 8);
const _: () = assert!(core::mem::offset_of!(BudgetInfo, gas_remaining) == 0);
const _: () = assert!(core::mem::offset_of!(BudgetInfo, gas_limit) == 8);
const _: () = assert!(core::mem::offset_of!(BudgetInfo, gas_per_tick) == 16);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct FlightState {
    pub rotation: [f64; 4],
    pub angular_velocity: [f64; 3],
    pub velocity: [f64; 3],
    pub mass_kg: f64,
    pub inertia: [f64; 9],
    pub radius_m: f64,
}

impl private::Sealed for FlightState {}

impl Record for FlightState {}
const _: () = assert!(size_of::<FlightState>() == 168 && align_of::<FlightState>() == 8);
const _: () = assert!(core::mem::offset_of!(FlightState, rotation) == 0);
const _: () = assert!(core::mem::offset_of!(FlightState, angular_velocity) == 32);
const _: () = assert!(core::mem::offset_of!(FlightState, velocity) == 56);
const _: () = assert!(core::mem::offset_of!(FlightState, mass_kg) == 80);
const _: () = assert!(core::mem::offset_of!(FlightState, inertia) == 88);
const _: () = assert!(core::mem::offset_of!(FlightState, radius_m) == 160);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ShipResources {
    pub hull_hp: f64,
    pub hull_max_hp: f64,
    pub hull_heat_j: f64,
    pub hull_heat_capacity_j: f64,
    pub shield_temperature_k: f64,
    pub shield_reserve_kg: f64,
    pub shield_reserve_capacity_kg: f64,
    pub shield_strength: f64,
    pub energy_j: u64,
    pub shield_state: u64,
}

impl private::Sealed for ShipResources {}
impl Record for ShipResources {}
const _: () = assert!(core::mem::offset_of!(ShipResources, hull_hp) == 0);
const _: () = assert!(core::mem::offset_of!(ShipResources, hull_max_hp) == 8);
const _: () = assert!(core::mem::offset_of!(ShipResources, hull_heat_j) == 16);
const _: () = assert!(core::mem::offset_of!(ShipResources, hull_heat_capacity_j) == 24);
const _: () = assert!(core::mem::offset_of!(ShipResources, shield_temperature_k) == 32);
const _: () = assert!(core::mem::offset_of!(ShipResources, shield_reserve_kg) == 40);
const _: () = assert!(core::mem::offset_of!(ShipResources, shield_reserve_capacity_kg) == 48);
const _: () = assert!(core::mem::offset_of!(ShipResources, shield_strength) == 56);
const _: () = assert!(core::mem::offset_of!(ShipResources, energy_j) == 64);
const _: () = assert!(core::mem::offset_of!(ShipResources, shield_state) == 72);
const _: () = assert!(size_of::<ShipResources>() == 80 && align_of::<ShipResources>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct DeviceInfo {
    pub id: u64,
    pub part_id: u64,
    pub kind: u64,
    pub flags: u64,
    pub position_m: [f64; 3],
    pub rotation: [f64; 4],
    pub alias: Text64,
    pub group_count: u64,
}

impl private::Sealed for DeviceInfo {}

impl Record for DeviceInfo {}
const _: () = assert!(size_of::<DeviceInfo>() == 168 && align_of::<DeviceInfo>() == 8);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, id) == 0);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, part_id) == 8);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, kind) == 16);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, flags) == 24);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, position_m) == 32);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, rotation) == 56);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, alias) == 88);
const _: () = assert!(core::mem::offset_of!(DeviceInfo, group_count) == 160);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct DeviceStatus {
    pub flags: u64,
}

impl private::Sealed for DeviceStatus {}

impl Record for DeviceStatus {}
const _: () = assert!(size_of::<DeviceStatus>() == 8 && align_of::<DeviceStatus>() == 8);
const _: () = assert!(core::mem::offset_of!(DeviceStatus, flags) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct AccelerometerReading {
    pub status: DeviceStatus,
    pub sample_present: u64,
    pub sample_time_s: f64,
    pub acceleration: [f64; 3],
}

impl private::Sealed for AccelerometerReading {}

impl Record for AccelerometerReading {}
const _: () =
    assert!(size_of::<AccelerometerReading>() == 48 && align_of::<AccelerometerReading>() == 8);
const _: () = assert!(core::mem::offset_of!(AccelerometerReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(AccelerometerReading, sample_present) == 8);
const _: () = assert!(core::mem::offset_of!(AccelerometerReading, sample_time_s) == 16);
const _: () = assert!(core::mem::offset_of!(AccelerometerReading, acceleration) == 24);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct StorageSpec {
    pub capacity_m3: f64,
}

impl private::Sealed for StorageSpec {}

impl Record for StorageSpec {}
const _: () = assert!(size_of::<StorageSpec>() == 8 && align_of::<StorageSpec>() == 8);
const _: () = assert!(core::mem::offset_of!(StorageSpec, capacity_m3) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BatterySpec {
    pub capacity_j: u64,
}

impl private::Sealed for BatterySpec {}

impl Record for BatterySpec {}
const _: () = assert!(size_of::<BatterySpec>() == 8 && align_of::<BatterySpec>() == 8);
const _: () = assert!(core::mem::offset_of!(BatterySpec, capacity_j) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct EngineSpec {
    pub propellant_resource: u64,
    pub max_thrust_n: f64,
    pub propellant_units_s: f64,
    pub max_power_w: f64,
}

impl private::Sealed for EngineSpec {}

impl Record for EngineSpec {}
const _: () = assert!(size_of::<EngineSpec>() == 32 && align_of::<EngineSpec>() == 8);
const _: () = assert!(core::mem::offset_of!(EngineSpec, propellant_resource) == 0);
const _: () = assert!(core::mem::offset_of!(EngineSpec, max_thrust_n) == 8);
const _: () = assert!(core::mem::offset_of!(EngineSpec, propellant_units_s) == 16);
const _: () = assert!(core::mem::offset_of!(EngineSpec, max_power_w) == 24);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct EngineReading {
    pub status: DeviceStatus,
    pub thrust_n: f64,
}

impl private::Sealed for EngineReading {}

impl Record for EngineReading {}
const _: () = assert!(size_of::<EngineReading>() == 16 && align_of::<EngineReading>() == 8);
const _: () = assert!(core::mem::offset_of!(EngineReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(EngineReading, thrust_n) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TorquerSpec {
    pub per_axis_limit_nm: f64,
    pub max_power_w: f64,
}

impl private::Sealed for TorquerSpec {}

impl Record for TorquerSpec {}
const _: () = assert!(size_of::<TorquerSpec>() == 16 && align_of::<TorquerSpec>() == 8);
const _: () = assert!(core::mem::offset_of!(TorquerSpec, per_axis_limit_nm) == 0);
const _: () = assert!(core::mem::offset_of!(TorquerSpec, max_power_w) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TorquerReading {
    pub status: DeviceStatus,
    pub torque_magnitude_nm: f64,
}

impl private::Sealed for TorquerReading {}

impl Record for TorquerReading {}
const _: () = assert!(size_of::<TorquerReading>() == 16 && align_of::<TorquerReading>() == 8);
const _: () = assert!(core::mem::offset_of!(TorquerReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(TorquerReading, torque_magnitude_nm) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct GeneratorSpec {
    pub fuel_resource: u64,
    pub max_power_w: f64,
    pub fuel_units_s: f64,
    pub efficiency: f64,
}

impl private::Sealed for GeneratorSpec {}
impl Record for GeneratorSpec {}
const _: () = assert!(core::mem::offset_of!(GeneratorSpec, fuel_resource) == 0);
const _: () = assert!(core::mem::offset_of!(GeneratorSpec, max_power_w) == 8);
const _: () = assert!(core::mem::offset_of!(GeneratorSpec, fuel_units_s) == 16);
const _: () = assert!(core::mem::offset_of!(GeneratorSpec, efficiency) == 24);
const _: () = assert!(size_of::<GeneratorSpec>() == 32 && align_of::<GeneratorSpec>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct GeneratorReading {
    pub status: DeviceStatus,
    pub power_w: f64,
}

impl private::Sealed for GeneratorReading {}

impl Record for GeneratorReading {}
const _: () = assert!(size_of::<GeneratorReading>() == 16 && align_of::<GeneratorReading>() == 8);
const _: () = assert!(core::mem::offset_of!(GeneratorReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(GeneratorReading, power_w) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ShieldSpec {
    pub deployed_mass_kg: f64,
    pub radiator_area_m2: f64,
    pub feed_rate_kg_s: f64,
    pub emissivity: f64,
    pub sustain_power_w: f64,
}

impl private::Sealed for ShieldSpec {}
impl Record for ShieldSpec {}
const _: () = assert!(core::mem::offset_of!(ShieldSpec, deployed_mass_kg) == 0);
const _: () = assert!(core::mem::offset_of!(ShieldSpec, radiator_area_m2) == 8);
const _: () = assert!(core::mem::offset_of!(ShieldSpec, feed_rate_kg_s) == 16);
const _: () = assert!(core::mem::offset_of!(ShieldSpec, emissivity) == 24);
const _: () = assert!(core::mem::offset_of!(ShieldSpec, sustain_power_w) == 32);
const _: () = assert!(size_of::<ShieldSpec>() == 40 && align_of::<ShieldSpec>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ShieldReading {
    pub status: DeviceStatus,
    pub state: u64,
    pub temperature_k: f64,
    pub reserve_kg: f64,
    pub reserve_capacity_kg: f64,
    pub strength: f64,
    pub ablation_kg_s: f64,
    pub radiated_power_w: f64,
    pub power_w: f64,
}

impl private::Sealed for ShieldReading {}
impl Record for ShieldReading {}
const _: () = assert!(core::mem::offset_of!(ShieldReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(ShieldReading, state) == 8);
const _: () = assert!(core::mem::offset_of!(ShieldReading, temperature_k) == 16);
const _: () = assert!(core::mem::offset_of!(ShieldReading, reserve_kg) == 24);
const _: () = assert!(core::mem::offset_of!(ShieldReading, reserve_capacity_kg) == 32);
const _: () = assert!(core::mem::offset_of!(ShieldReading, strength) == 40);
const _: () = assert!(core::mem::offset_of!(ShieldReading, ablation_kg_s) == 48);
const _: () = assert!(core::mem::offset_of!(ShieldReading, radiated_power_w) == 56);
const _: () = assert!(core::mem::offset_of!(ShieldReading, power_w) == 64);
const _: () = assert!(size_of::<ShieldReading>() == 72 && align_of::<ShieldReading>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct WeaponSpec {
    pub ammunition_resource: u64,
    pub projectile_mass_kg: f64,
    pub projectile_radius_m: f64,
    pub muzzle_speed_m_s: f64,
    pub cycle_interval_s: f64,
    pub efficiency: f64,
    pub dispersion_half_angle_rad: f64,
    pub pivot_device_m: [f64; 3],
    pub muzzle_offset_m: [f64; 3],
    pub yaw_min_rad: f64,
    pub yaw_max_rad: f64,
    pub pitch_min_rad: f64,
    pub pitch_max_rad: f64,
    pub yaw_rate_rad_s: f64,
    pub pitch_rate_rad_s: f64,
    pub beam_power_w: f64,
    pub beam_range_m: f64,
    pub chemical: u64,
}

impl private::Sealed for WeaponSpec {}
impl Record for WeaponSpec {}
const _: () = assert!(core::mem::offset_of!(WeaponSpec, ammunition_resource) == 0);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, projectile_mass_kg) == 8);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, projectile_radius_m) == 16);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, muzzle_speed_m_s) == 24);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, cycle_interval_s) == 32);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, efficiency) == 40);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, dispersion_half_angle_rad) == 48);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, pivot_device_m) == 56);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, muzzle_offset_m) == 80);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, yaw_min_rad) == 104);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, yaw_max_rad) == 112);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, pitch_min_rad) == 120);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, pitch_max_rad) == 128);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, yaw_rate_rad_s) == 136);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, pitch_rate_rad_s) == 144);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, beam_power_w) == 152);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, beam_range_m) == 160);
const _: () = assert!(core::mem::offset_of!(WeaponSpec, chemical) == 168);
const _: () = assert!(size_of::<WeaponSpec>() == 176 && align_of::<WeaponSpec>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct WeaponSetting {
    pub aim_direction: [f64; 3],
    pub aim_angular_velocity_rad_s: [f64; 3],
    pub maximum_pointing_error_rad: f64,
    pub valid_until_s: f64,
    pub trigger: u64,
}

impl private::Sealed for WeaponSetting {}
impl Record for WeaponSetting {}
const _: () = assert!(core::mem::offset_of!(WeaponSetting, aim_direction) == 0);
const _: () = assert!(core::mem::offset_of!(WeaponSetting, aim_angular_velocity_rad_s) == 24);
const _: () = assert!(core::mem::offset_of!(WeaponSetting, maximum_pointing_error_rad) == 48);
const _: () = assert!(core::mem::offset_of!(WeaponSetting, valid_until_s) == 56);
const _: () = assert!(core::mem::offset_of!(WeaponSetting, trigger) == 64);
const _: () = assert!(size_of::<WeaponSetting>() == 72 && align_of::<WeaponSetting>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct WeaponReading {
    pub status: DeviceStatus,
    pub inhibit_flags: u64,
    pub ammunition_units: u64,
    pub shots_fired: u64,
    pub battery_energy_j: u64,
    pub shot_energy_j: f64,
    pub yaw_rad: f64,
    pub pitch_rad: f64,
    pub next_fire_s: f64,
}

impl private::Sealed for WeaponReading {}
impl Record for WeaponReading {}
const _: () = assert!(core::mem::offset_of!(WeaponReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(WeaponReading, inhibit_flags) == 8);
const _: () = assert!(core::mem::offset_of!(WeaponReading, ammunition_units) == 16);
const _: () = assert!(core::mem::offset_of!(WeaponReading, shots_fired) == 24);
const _: () = assert!(core::mem::offset_of!(WeaponReading, battery_energy_j) == 32);
const _: () = assert!(core::mem::offset_of!(WeaponReading, shot_energy_j) == 40);
const _: () = assert!(core::mem::offset_of!(WeaponReading, yaw_rad) == 48);
const _: () = assert!(core::mem::offset_of!(WeaponReading, pitch_rad) == 56);
const _: () = assert!(core::mem::offset_of!(WeaponReading, next_fire_s) == 64);
const _: () = assert!(size_of::<WeaponReading>() == 72 && align_of::<WeaponReading>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct MarkTargetRequest {
    pub contact: u64,
    pub maximum_flight_time_s: f64,
}

impl private::Sealed for MarkTargetRequest {}
impl Record for MarkTargetRequest {}
const _: () = assert!(core::mem::offset_of!(MarkTargetRequest, contact) == 0);
const _: () = assert!(core::mem::offset_of!(MarkTargetRequest, maximum_flight_time_s) == 8);
const _: () = assert!(size_of::<MarkTargetRequest>() == 16 && align_of::<MarkTargetRequest>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct WeaponsState {
    pub valid_until_s: f64,
    pub mode: u64,
    pub target_contact: u64,
    pub reason: Text256,
}

impl private::Sealed for WeaponsState {}
impl Record for WeaponsState {}
const _: () = assert!(core::mem::offset_of!(WeaponsState, valid_until_s) == 0);
const _: () = assert!(core::mem::offset_of!(WeaponsState, mode) == 8);
const _: () = assert!(core::mem::offset_of!(WeaponsState, target_contact) == 16);
const _: () = assert!(core::mem::offset_of!(WeaponsState, reason) == 24);
const _: () = assert!(size_of::<WeaponsState>() == 288 && align_of::<WeaponsState>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct WeaponInstrument {
    pub device: u64,
    pub aim_marker: u64,
    pub solution_flags: u64,
    pub time_of_flight_s: f64,
    pub pointing_error_rad: f64,
    pub reading: WeaponReading,
}

impl private::Sealed for WeaponInstrument {}
impl Record for WeaponInstrument {}
const _: () = assert!(core::mem::offset_of!(WeaponInstrument, device) == 0);
const _: () = assert!(core::mem::offset_of!(WeaponInstrument, aim_marker) == 8);
const _: () = assert!(core::mem::offset_of!(WeaponInstrument, solution_flags) == 16);
const _: () = assert!(core::mem::offset_of!(WeaponInstrument, time_of_flight_s) == 24);
const _: () = assert!(core::mem::offset_of!(WeaponInstrument, pointing_error_rad) == 32);
const _: () = assert!(core::mem::offset_of!(WeaponInstrument, reading) == 40);
const _: () = assert!(size_of::<WeaponInstrument>() == 112 && align_of::<WeaponInstrument>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SensorSpec {
    pub max_range_m: f64,
    pub power_w: f64,
}

impl private::Sealed for SensorSpec {}

impl Record for SensorSpec {}
const _: () = assert!(size_of::<SensorSpec>() == 16 && align_of::<SensorSpec>() == 8);
const _: () = assert!(core::mem::offset_of!(SensorSpec, max_range_m) == 0);
const _: () = assert!(core::mem::offset_of!(SensorSpec, power_w) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SensorReading {
    pub status: DeviceStatus,
    pub range_m: f64,
}

impl private::Sealed for SensorReading {}

impl Record for SensorReading {}
const _: () = assert!(size_of::<SensorReading>() == 16 && align_of::<SensorReading>() == 8);
const _: () = assert!(core::mem::offset_of!(SensorReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(SensorReading, range_m) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ThrottleSetting {
    pub fraction: f64,
}

impl private::Sealed for ThrottleSetting {}

impl Record for ThrottleSetting {}
const _: () = assert!(size_of::<ThrottleSetting>() == 8 && align_of::<ThrottleSetting>() == 8);
const _: () = assert!(core::mem::offset_of!(ThrottleSetting, fraction) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TorqueSetting {
    pub torque_nm: [f64; 3],
}

impl private::Sealed for TorqueSetting {}

impl Record for TorqueSetting {}
const _: () = assert!(size_of::<TorqueSetting>() == 24 && align_of::<TorqueSetting>() == 8);
const _: () = assert!(core::mem::offset_of!(TorqueSetting, torque_nm) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct GeneratorDemandSetting {
    pub fraction: f64,
}

impl private::Sealed for GeneratorDemandSetting {}

impl Record for GeneratorDemandSetting {}
const _: () =
    assert!(size_of::<GeneratorDemandSetting>() == 8 && align_of::<GeneratorDemandSetting>() == 8);
const _: () = assert!(core::mem::offset_of!(GeneratorDemandSetting, fraction) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct EnabledSetting {
    pub enabled: u64,
}

impl private::Sealed for EnabledSetting {}

impl Record for EnabledSetting {}
const _: () = assert!(size_of::<EnabledSetting>() == 8 && align_of::<EnabledSetting>() == 8);
const _: () = assert!(core::mem::offset_of!(EnabledSetting, enabled) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ResourceInfo {
    pub id: u64,
    pub key: Text64,
    pub unit_mass_kg: f64,
    pub unit_volume_m3: f64,
}

impl private::Sealed for ResourceInfo {}

impl Record for ResourceInfo {}
const _: () = assert!(size_of::<ResourceInfo>() == 96 && align_of::<ResourceInfo>() == 8);
const _: () = assert!(core::mem::offset_of!(ResourceInfo, id) == 0);
const _: () = assert!(core::mem::offset_of!(ResourceInfo, key) == 8);
const _: () = assert!(core::mem::offset_of!(ResourceInfo, unit_mass_kg) == 80);
const _: () = assert!(core::mem::offset_of!(ResourceInfo, unit_volume_m3) == 88);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ResourceAmount {
    pub units: u64,
}

impl private::Sealed for ResourceAmount {}

impl Record for ResourceAmount {}
const _: () = assert!(size_of::<ResourceAmount>() == 8 && align_of::<ResourceAmount>() == 8);
const _: () = assert!(core::mem::offset_of!(ResourceAmount, units) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Contact {
    pub id: u64,
    pub kind: u64,
    pub radius_m: f64,
    pub position_m: [f64; 3],
    pub velocity_m_s: [f64; 3],
}

impl private::Sealed for Contact {}

impl Record for Contact {}
const _: () = assert!(size_of::<Contact>() == 72 && align_of::<Contact>() == 8);
const _: () = assert!(core::mem::offset_of!(Contact, id) == 0);
const _: () = assert!(core::mem::offset_of!(Contact, kind) == 8);
const _: () = assert!(core::mem::offset_of!(Contact, radius_m) == 16);
const _: () = assert!(core::mem::offset_of!(Contact, position_m) == 24);
const _: () = assert!(core::mem::offset_of!(Contact, velocity_m_s) == 48);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct RequestInfo {
    pub id: u64,
    pub kind: u64,
    pub payload_bytes: u64,
}

impl private::Sealed for RequestInfo {}

impl Record for RequestInfo {}
const _: () = assert!(size_of::<RequestInfo>() == 24 && align_of::<RequestInfo>() == 8);
const _: () = assert!(core::mem::offset_of!(RequestInfo, id) == 0);
const _: () = assert!(core::mem::offset_of!(RequestInfo, kind) == 8);
const _: () = assert!(core::mem::offset_of!(RequestInfo, payload_bytes) == 16);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ManualRequest {
    pub throttle: f64,
    pub steering: [f64; 3],
}

impl private::Sealed for ManualRequest {}

impl Record for ManualRequest {}
const _: () = assert!(size_of::<ManualRequest>() == 32 && align_of::<ManualRequest>() == 8);
const _: () = assert!(core::mem::offset_of!(ManualRequest, throttle) == 0);
const _: () = assert!(core::mem::offset_of!(ManualRequest, steering) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct DirectionRequest {
    pub direction: [f64; 3],
}

impl private::Sealed for DirectionRequest {}

impl Record for DirectionRequest {}
const _: () = assert!(size_of::<DirectionRequest>() == 24 && align_of::<DirectionRequest>() == 8);
const _: () = assert!(core::mem::offset_of!(DirectionRequest, direction) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ContactRequest {
    pub contact: u64,
}

impl private::Sealed for ContactRequest {}

impl Record for ContactRequest {}
const _: () = assert!(size_of::<ContactRequest>() == 8 && align_of::<ContactRequest>() == 8);
const _: () = assert!(core::mem::offset_of!(ContactRequest, contact) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct NavigationRequest {
    pub throttle_limit: f64,
    pub stand_off_m: f64,
}

impl private::Sealed for NavigationRequest {}

impl Record for NavigationRequest {}
const _: () = assert!(size_of::<NavigationRequest>() == 16 && align_of::<NavigationRequest>() == 8);
const _: () = assert!(core::mem::offset_of!(NavigationRequest, throttle_limit) == 0);
const _: () = assert!(core::mem::offset_of!(NavigationRequest, stand_off_m) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpatialMeta {
    pub id: u64,
    pub role: u64,
    pub valid_until_s: f64,
    pub label: Text64,
}

impl private::Sealed for SpatialMeta {}

impl Record for SpatialMeta {}
const _: () = assert!(size_of::<SpatialMeta>() == 96 && align_of::<SpatialMeta>() == 8);
const _: () = assert!(core::mem::offset_of!(SpatialMeta, id) == 0);
const _: () = assert!(core::mem::offset_of!(SpatialMeta, role) == 8);
const _: () = assert!(core::mem::offset_of!(SpatialMeta, valid_until_s) == 16);
const _: () = assert!(core::mem::offset_of!(SpatialMeta, label) == 24);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpatialFrame {
    pub kind: u64,
    pub reference: u64,
    pub origin_velocity_m_s: [f64; 3],
}

impl private::Sealed for SpatialFrame {}

impl Record for SpatialFrame {}
const _: () = assert!(size_of::<SpatialFrame>() == 40 && align_of::<SpatialFrame>() == 8);
const _: () = assert!(core::mem::offset_of!(SpatialFrame, kind) == 0);
const _: () = assert!(core::mem::offset_of!(SpatialFrame, reference) == 8);
const _: () = assert!(core::mem::offset_of!(SpatialFrame, origin_velocity_m_s) == 16);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpatialMarker {
    pub meta: SpatialMeta,
    pub frame: SpatialFrame,
    pub offset_m: [f64; 3],
    pub time_mode: u64,
    pub time_s: f64,
}

impl private::Sealed for SpatialMarker {}

impl Record for SpatialMarker {}
const _: () = assert!(size_of::<SpatialMarker>() == 176 && align_of::<SpatialMarker>() == 8);
const _: () = assert!(core::mem::offset_of!(SpatialMarker, meta) == 0);
const _: () = assert!(core::mem::offset_of!(SpatialMarker, frame) == 96);
const _: () = assert!(core::mem::offset_of!(SpatialMarker, offset_m) == 136);
const _: () = assert!(core::mem::offset_of!(SpatialMarker, time_mode) == 160);
const _: () = assert!(core::mem::offset_of!(SpatialMarker, time_s) == 168);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpatialPath {
    pub meta: SpatialMeta,
    pub frame: SpatialFrame,
    pub kind: u64,
    pub subject_contact: u64,
}

impl private::Sealed for SpatialPath {}

impl Record for SpatialPath {}
const _: () = assert!(size_of::<SpatialPath>() == 152 && align_of::<SpatialPath>() == 8);
const _: () = assert!(core::mem::offset_of!(SpatialPath, meta) == 0);
const _: () = assert!(core::mem::offset_of!(SpatialPath, frame) == 96);
const _: () = assert!(core::mem::offset_of!(SpatialPath, kind) == 136);
const _: () = assert!(core::mem::offset_of!(SpatialPath, subject_contact) == 144);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpatialVertex {
    pub time_s: f64,
    pub position_m: [f64; 3],
}

impl private::Sealed for SpatialVertex {}

impl Record for SpatialVertex {}
const _: () = assert!(size_of::<SpatialVertex>() == 32 && align_of::<SpatialVertex>() == 8);
const _: () = assert!(core::mem::offset_of!(SpatialVertex, time_s) == 0);
const _: () = assert!(core::mem::offset_of!(SpatialVertex, position_m) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct AttitudeState {
    pub valid_until_s: f64,
    pub mode: u64,
    pub present: u64,
    pub reference: [f64; 4],
    pub control_error: f64,
}

impl private::Sealed for AttitudeState {}

impl Record for AttitudeState {}
const _: () = assert!(size_of::<AttitudeState>() == 64 && align_of::<AttitudeState>() == 8);
const _: () = assert!(core::mem::offset_of!(AttitudeState, valid_until_s) == 0);
const _: () = assert!(core::mem::offset_of!(AttitudeState, mode) == 8);
const _: () = assert!(core::mem::offset_of!(AttitudeState, present) == 16);
const _: () = assert!(core::mem::offset_of!(AttitudeState, reference) == 24);
const _: () = assert!(core::mem::offset_of!(AttitudeState, control_error) == 56);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct NavigationState {
    pub valid_until_s: f64,
    pub status: u64,
    pub target_contact: u64,
    pub own_path: u64,
    pub target_path: u64,
    pub present: u64,
    pub throttle_limit: f64,
    pub throttle: f64,
    pub stand_off_m: f64,
    pub approach_speed_limit_m_s: f64,
    pub braking_distance_m: f64,
    pub arrival_time_s: f64,
    pub predicted_fuel_kg: f64,
    pub reason: Text256,
}

impl private::Sealed for NavigationState {}

impl Record for NavigationState {}
const _: () = assert!(size_of::<NavigationState>() == 368 && align_of::<NavigationState>() == 8);
const _: () = assert!(core::mem::offset_of!(NavigationState, valid_until_s) == 0);
const _: () = assert!(core::mem::offset_of!(NavigationState, status) == 8);
const _: () = assert!(core::mem::offset_of!(NavigationState, target_contact) == 16);
const _: () = assert!(core::mem::offset_of!(NavigationState, own_path) == 24);
const _: () = assert!(core::mem::offset_of!(NavigationState, target_path) == 32);
const _: () = assert!(core::mem::offset_of!(NavigationState, present) == 40);
const _: () = assert!(core::mem::offset_of!(NavigationState, throttle_limit) == 48);
const _: () = assert!(core::mem::offset_of!(NavigationState, throttle) == 56);
const _: () = assert!(core::mem::offset_of!(NavigationState, stand_off_m) == 64);
const _: () = assert!(core::mem::offset_of!(NavigationState, approach_speed_limit_m_s) == 72);
const _: () = assert!(core::mem::offset_of!(NavigationState, braking_distance_m) == 80);
const _: () = assert!(core::mem::offset_of!(NavigationState, arrival_time_s) == 88);
const _: () = assert!(core::mem::offset_of!(NavigationState, predicted_fuel_kg) == 96);
const _: () = assert!(core::mem::offset_of!(NavigationState, reason) == 104);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ContactsState {
    pub valid_until_s: f64,
    pub selected_contact: u64,
}

impl private::Sealed for ContactsState {}

impl Record for ContactsState {}
const _: () = assert!(size_of::<ContactsState>() == 16 && align_of::<ContactsState>() == 8);
const _: () = assert!(core::mem::offset_of!(ContactsState, valid_until_s) == 0);
const _: () = assert!(core::mem::offset_of!(ContactsState, selected_contact) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenDefinition {
    pub id: u64,
    pub width: u64,
    pub height: u64,
    pub title: Text64,
}

impl private::Sealed for ScreenDefinition {}

impl Record for ScreenDefinition {}
const _: () = assert!(size_of::<ScreenDefinition>() == 96 && align_of::<ScreenDefinition>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenDefinition, id) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenDefinition, width) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenDefinition, height) == 16);
const _: () = assert!(core::mem::offset_of!(ScreenDefinition, title) == 24);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenFrame {
    pub id: u64,
    pub background: u64,
}

impl private::Sealed for ScreenFrame {}

impl Record for ScreenFrame {}
const _: () = assert!(size_of::<ScreenFrame>() == 16 && align_of::<ScreenFrame>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenFrame, id) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenFrame, background) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenPixel {
    pub color: u64,
    pub x: i64,
    pub y: i64,
}

impl private::Sealed for ScreenPixel {}

impl Record for ScreenPixel {}
const _: () = assert!(size_of::<ScreenPixel>() == 24 && align_of::<ScreenPixel>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenPixel, color) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenPixel, x) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenPixel, y) == 16);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenText {
    pub color: u64,
    pub x: i64,
    pub y: i64,
}

impl private::Sealed for ScreenText {}

impl Record for ScreenText {}
const _: () = assert!(size_of::<ScreenText>() == 24 && align_of::<ScreenText>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenText, color) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenText, x) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenText, y) == 16);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenLine {
    pub color: u64,
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
}

impl private::Sealed for ScreenLine {}

impl Record for ScreenLine {}
const _: () = assert!(size_of::<ScreenLine>() == 40 && align_of::<ScreenLine>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenLine, color) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenLine, x1) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenLine, y1) == 16);
const _: () = assert!(core::mem::offset_of!(ScreenLine, x2) == 24);
const _: () = assert!(core::mem::offset_of!(ScreenLine, y2) == 32);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenPolyline {
    pub color: u64,
}

impl private::Sealed for ScreenPolyline {}

impl Record for ScreenPolyline {}
const _: () = assert!(size_of::<ScreenPolyline>() == 8 && align_of::<ScreenPolyline>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenPolyline, color) == 0);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenPoint {
    pub x: i64,
    pub y: i64,
}

impl private::Sealed for ScreenPoint {}

impl Record for ScreenPoint {}
const _: () = assert!(size_of::<ScreenPoint>() == 16 && align_of::<ScreenPoint>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenPoint, x) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenPoint, y) == 8);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenRectangle {
    pub color: u64,
    pub filled: u64,
    pub x: i64,
    pub y: i64,
    pub width: u64,
    pub height: u64,
}

impl private::Sealed for ScreenRectangle {}

impl Record for ScreenRectangle {}
const _: () = assert!(size_of::<ScreenRectangle>() == 48 && align_of::<ScreenRectangle>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenRectangle, color) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenRectangle, filled) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenRectangle, x) == 16);
const _: () = assert!(core::mem::offset_of!(ScreenRectangle, y) == 24);
const _: () = assert!(core::mem::offset_of!(ScreenRectangle, width) == 32);
const _: () = assert!(core::mem::offset_of!(ScreenRectangle, height) == 40);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenEllipse {
    pub color: u64,
    pub filled: u64,
    pub cx: i64,
    pub cy: i64,
    pub rx: u64,
    pub ry: u64,
}

impl private::Sealed for ScreenEllipse {}

impl Record for ScreenEllipse {}
const _: () = assert!(size_of::<ScreenEllipse>() == 48 && align_of::<ScreenEllipse>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenEllipse, color) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenEllipse, filled) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenEllipse, cx) == 16);
const _: () = assert!(core::mem::offset_of!(ScreenEllipse, cy) == 24);
const _: () = assert!(core::mem::offset_of!(ScreenEllipse, rx) == 32);
const _: () = assert!(core::mem::offset_of!(ScreenEllipse, ry) == 40);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ScreenEvent {
    pub id: u64,
    pub screen: u64,
    pub kind: u64,
    pub code: u64,
    pub modifiers: u64,
    pub x: f64,
    pub y: f64,
    pub text: Text64,
}

impl private::Sealed for ScreenEvent {}

impl Record for ScreenEvent {}
const _: () = assert!(size_of::<ScreenEvent>() == 128 && align_of::<ScreenEvent>() == 8);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, id) == 0);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, screen) == 8);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, kind) == 16);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, code) == 24);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, modifiers) == 32);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, x) == 40);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, y) == 48);
const _: () = assert!(core::mem::offset_of!(ScreenEvent, text) == 56);
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MissileObservation {
    pub handle: u64,
    pub target_visible: u64,
    pub target_offset_m: [f64; 3],
    pub target_relative_velocity_m_s: [f64; 3],
    pub rotation: [f64; 4],
    pub angular_velocity_rad_s: [f64; 3],
    pub velocity_m_s: [f64; 3],
    pub maximum_acceleration_m_s2: f64,
    pub turn_rate_rad_s: f64,
    pub fuel_units: u64,
    pub dt_s: f64,
    pub time_s: f64,
    pub target_uncertainty_m: f64,
}

impl private::Sealed for MissileObservation {}
impl Record for MissileObservation {}
const _: () =
    assert!(size_of::<MissileObservation>() == 192 && align_of::<MissileObservation>() == 8);
const _: () = assert!(core::mem::offset_of!(MissileObservation, target_offset_m) == 16);
const _: () = assert!(core::mem::offset_of!(MissileObservation, rotation) == 64);
const _: () = assert!(core::mem::offset_of!(MissileObservation, angular_velocity_rad_s) == 96);
const _: () = assert!(core::mem::offset_of!(MissileObservation, maximum_acceleration_m_s2) == 144);
const _: () = assert!(core::mem::offset_of!(MissileObservation, fuel_units) == 160);
const _: () = assert!(core::mem::offset_of!(MissileObservation, target_uncertainty_m) == 184);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MissileControl {
    pub direction: [f64; 3],
    pub throttle: f64,
}

impl private::Sealed for MissileControl {}
impl Record for MissileControl {}
const _: () = assert!(size_of::<MissileControl>() == 32 && align_of::<MissileControl>() == 8);
const _: () = assert!(core::mem::offset_of!(MissileControl, throttle) == 24);

pub const IMPORTS: &[&str] = &[
    "llm_submit",
    "llm_poll",
    "llm_cancel",
    "chat_send",
    "serial_write",
    "chat_read",
    "missile_read",
    "missile_control",
    "persistent_read",
    "persistent_write",
    "orrery_read",
    "navigation_query",
    "contact_get",
    "slip_eligibility",
    "travel_read",
    "destination_resolve",
    "route_request",
    "route_poll",
    "travel_use_route",
    "travel_block",
    "travel_estimate",
    "travel_complete",
    "travel_slip",
    "travel_reserve_bay",
    "travel_dock",
    "travel_undock",
    "intel_tracks",
    "intel_continue",
    "beacons_read",
    "beacon_read",
    "tick_read",
    "budget_read",
    "flight_read",
    "ship_resources_read",
    "tick_set_interval",
    "snapshot_keep",
    "snapshot_drop",
    "device_info",
    "device_group_read",
    "device_spec",
    "device_read",
    "device_write",
    "resource_info",
    "resource_read",
    "sensor_scan",
    "contact_label",
    "request_info",
    "request_read",
    "request_reply",
    "instrument_attitude_put",
    "instrument_navigation_put",
    "instrument_weapons_put",
    "instrument_contacts_put",
    "instrument_clear",
    "spatial_marker_put",
    "spatial_path_put",
    "spatial_remove",
    "spatial_clear",
    "screen_define",
    "screen_remove",
    "screen_begin",
    "screen_draw",
    "screen_button",
    "screen_end",
    "screen_event_read",
    "screen_event_ack",
];
#[cfg(target_arch = "wasm32")]
pub mod raw {
    #[link(wasm_import_module = "ship_v32")]
    unsafe extern "C" {
        pub fn llm_submit(id: u64, input: *const u8, bytes: u32, max_tokens: u32) -> i32;
        pub fn llm_poll(
            id: u64,
            out: *mut u8,
            capacity: u32,
            status: *mut crate::services::LlmPoll,
        ) -> i32;
        pub fn llm_cancel(id: u64) -> i32;
        pub fn serial_write(input: *const u8, bytes: u32) -> i32;
        pub fn chat_send(id: u64, input: *const u8, bytes: u32) -> i32;
        pub fn chat_read(
            after: u64,
            out: *mut crate::services::ChatMessage,
            capacity: u32,
            page: *mut crate::services::ChatPage,
        ) -> i32;
        pub fn missile_read(output: *mut u8, bytes: u32) -> i32;
        pub fn missile_control(input: *const u8, bytes: u32) -> i32;
        pub fn persistent_read(output: *mut u8, capacity: u32) -> i32;
        pub fn persistent_write(input: *const u8, length: u32) -> i32;
        pub fn tick_read(out: *mut u8, bytes: u32) -> i32;
        pub fn budget_read(out: *mut u8, bytes: u32) -> i32;
        pub fn flight_read(out: *mut u8, bytes: u32) -> i32;
        pub fn ship_resources_read(out: *mut u8, bytes: u32) -> i32;
        pub fn tick_set_interval(seconds: f64) -> i32;
        pub fn snapshot_keep(snapshot: u64) -> i32;
        pub fn snapshot_drop(snapshot: u64) -> i32;
        pub fn device_info(index: u32, out: *mut u8, bytes: u32) -> i32;
        pub fn device_group_read(device: u64, index: u32, out: *mut u8, bytes: u32) -> i32;
        pub fn device_spec(device: u64, expected_kind: u64, out: *mut u8, bytes: u32) -> i32;
        pub fn device_read(device: u64, expected_kind: u64, out: *mut u8, bytes: u32) -> i32;
        pub fn device_write(device: u64, setting: u64, input: *const u8, bytes: u32) -> i32;
        pub fn resource_info(index: u32, out: *mut u8, bytes: u32) -> i32;
        pub fn resource_read(resource: u64, out: *mut u8, bytes: u32) -> i32;
        pub fn sensor_scan(
            sensor: u64,
            maximum: u32,
            contacts: *mut u8,
            contacts_bytes: u32,
        ) -> i32;
        pub fn contact_label(contact: u64, out: *mut u8, bytes: u32) -> i32;
        pub fn request_info(index: u32, out: *mut u8, bytes: u32) -> i32;
        pub fn request_read(index: u32, expected_kind: u64, out: *mut u8, bytes: u32) -> i32;
        pub fn request_reply(
            request: u64,
            result: u64,
            message: *const u8,
            message_bytes: u32,
        ) -> i32;
        pub fn instrument_weapons_put(
            state_pointer: *const u8,
            state_bytes: u32,
            weapons_pointer: *const u8,
            weapon_count: u32,
        ) -> i32;
        pub fn instrument_attitude_put(input: *const u8, bytes: u32) -> i32;
        pub fn instrument_navigation_put(input: *const u8, bytes: u32) -> i32;
        pub fn instrument_contacts_put(input: *const u8, bytes: u32) -> i32;
        pub fn instrument_clear(kind: u64) -> i32;
        pub fn spatial_marker_put(marker: *const u8, marker_bytes: u32) -> i32;
        pub fn spatial_path_put(
            path: *const u8,
            path_bytes: u32,
            vertices: *const u8,
            vertex_count: u32,
        ) -> i32;
        pub fn spatial_remove(id: u64) -> i32;
        pub fn spatial_clear() -> i32;
        pub fn screen_define(input: *const u8, bytes: u32) -> i32;
        pub fn screen_remove(screen: u64) -> i32;
        pub fn screen_begin(input: *const u8, bytes: u32) -> i32;
        pub fn screen_draw(
            screen: u64,
            kind: u64,
            parameters: *const u8,
            parameter_bytes: u32,
            payload: *const u8,
            payload_bytes: u32,
        ) -> i32;
        pub fn screen_button(screen: u64, key: u64, label: *const u8, label_bytes: u32) -> i32;
        pub fn screen_end(screen: u64) -> i32;
        pub fn screen_event_read(index: u32, out: *mut u8, bytes: u32) -> i32;
        pub fn screen_event_ack(event: u64) -> i32;
    }
}

pub const SHIELD_ABSENT: u64 = 0;
pub const SHIELD_OFF: u64 = 1;
pub const SHIELD_ACTIVE: u64 = 2;
pub const SHIELD_DEPLETED: u64 = 3;
pub const SHIELD_UNPOWERED: u64 = 4;
pub const SHIELD_BLOCKED: u64 = 5;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RcsSpec {
    pub propellant_resource: u64,
    pub per_axis_thrust_n: f64,
    pub per_axis_propellant_units_s: f64,
    pub per_axis_power_w: f64,
}
impl private::Sealed for RcsSpec {}
impl Record for RcsSpec {}
const _: () = assert!(size_of::<RcsSpec>() == 32 && align_of::<RcsSpec>() == 8);
const _: () = assert!(core::mem::offset_of!(RcsSpec, propellant_resource) == 0);
const _: () = assert!(core::mem::offset_of!(RcsSpec, per_axis_thrust_n) == 8);
const _: () = assert!(core::mem::offset_of!(RcsSpec, per_axis_propellant_units_s) == 16);
const _: () = assert!(core::mem::offset_of!(RcsSpec, per_axis_power_w) == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RcsSetting {
    pub thrust_n: [f64; 3],
}
impl private::Sealed for RcsSetting {}
impl Record for RcsSetting {}
const _: () = assert!(size_of::<RcsSetting>() == 24 && align_of::<RcsSetting>() == 8);
const _: () = assert!(core::mem::offset_of!(RcsSetting, thrust_n) == 0);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RcsReading {
    pub status: DeviceStatus,
    pub thrust_n: [f64; 3],
}
impl private::Sealed for RcsReading {}
impl Record for RcsReading {}
const _: () = assert!(size_of::<RcsReading>() == 32 && align_of::<RcsReading>() == 8);
const _: () = assert!(core::mem::offset_of!(RcsReading, status) == 0);
const _: () = assert!(core::mem::offset_of!(RcsReading, thrust_n) == 8);
