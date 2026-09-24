//! Shared mount controls and separate ballistic and optical hardware capabilities.
use super::{Record, private};

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct WeaponSpec {
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
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct GunSpec {
    pub weapon: WeaponSpec,
    pub ammunition_resource: u64,
    pub projectile_mass_kg: f64,
    pub projectile_radius_m: f64,
    pub muzzle_speed_m_s: f64,
    pub chemical: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct LaserSpec {
    pub weapon: WeaponSpec,
    pub pulse_energy_j: f64,
    pub range_m: f64,
    pub beam_waist_m: f64,
    pub divergence_half_angle_rad: f64,
}

impl private::Sealed for WeaponSpec {}
impl Record for WeaponSpec {}
impl private::Sealed for GunSpec {}
impl Record for GunSpec {}
impl private::Sealed for LaserSpec {}
impl Record for LaserSpec {}

impl core::ops::Deref for GunSpec {
    type Target = WeaponSpec;

    fn deref(&self) -> &Self::Target {
        &self.weapon
    }
}

impl core::ops::Deref for LaserSpec {
    type Target = WeaponSpec;

    fn deref(&self) -> &Self::Target {
        &self.weapon
    }
}

const _: () = assert!(core::mem::size_of::<WeaponSpec>() == 120);
const _: () = assert!(core::mem::size_of::<GunSpec>() == 160);
const _: () = assert!(core::mem::size_of::<LaserSpec>() == 152);
const _: () = assert!(core::mem::offset_of!(GunSpec, ammunition_resource) == 120);
const _: () = assert!(core::mem::offset_of!(LaserSpec, pulse_energy_j) == 120);
