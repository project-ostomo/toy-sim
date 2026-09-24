//! Weapon mechanisms shared by the simulator and hardware tests.
use glam::{DQuat, DVec3};
use osg_ship_api::abi;
use serde::{Deserialize, Serialize};

use crate::Catalogue;

/// Prototype counter-exhaust: ten percent of projectile mass, in kg of propellant.
/// Exhaust energy and momentum are deliberately not simulated.
pub fn shot_propellant_kg(spec: &WeaponSpec) -> f64 {
    match spec {
        WeaponSpec::Gun(gun) if gun.chemical == 0 => gun.projectile_mass_kg * 0.1,
        _ => 0.0,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponDrive {
    #[default]
    Electric,
    Chemical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponDef {
    pub mechanism: WeaponMechanism,
    pub cycle_interval_s: f64,
    pub efficiency: f64,
    pub dispersion_half_angle_rad: f64,
    pub muzzle_offset_m: [f64; 3],
    pub slew_rate_rad_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WeaponMechanism {
    Gun {
        #[serde(default)]
        drive: WeaponDrive,
        ammunition: String,
        projectile_radius_m: f64,
        muzzle_speed_m_s: f64,
    },
    Laser {
        pulse_energy_j: f64,
        range_m: f64,
        beam_waist_m: f64,
        divergence_half_angle_rad: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WeaponSpec {
    Gun(abi::GunSpec),
    Laser(abi::LaserSpec),
}

impl std::ops::Deref for WeaponSpec {
    type Target = abi::WeaponSpec;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Gun(spec) => &spec.weapon,
            Self::Laser(spec) => &spec.weapon,
        }
    }
}

impl WeaponSpec {
    pub fn gun(&self) -> Option<&abi::GunSpec> {
        match self {
            Self::Gun(spec) => Some(spec),
            Self::Laser(_) => None,
        }
    }

    pub fn laser(&self) -> Option<&abi::LaserSpec> {
        match self {
            Self::Laser(spec) => Some(spec),
            Self::Gun(_) => None,
        }
    }

    pub fn kind(&self) -> u64 {
        match self {
            Self::Gun(_) => abi::DEVICE_GUN,
            Self::Laser(_) => abi::DEVICE_LASER,
        }
    }
}

impl WeaponDef {
    pub fn spec(&self, catalogue: &Catalogue) -> Option<WeaponSpec> {
        let turret = self.slew_rate_rad_s > 0.0;
        let weapon = abi::WeaponSpec {
            cycle_interval_s: self.cycle_interval_s,
            efficiency: self.efficiency,
            dispersion_half_angle_rad: self.dispersion_half_angle_rad,
            pivot_device_m: [0.0; 3],
            muzzle_offset_m: self.muzzle_offset_m,
            yaw_min_rad: if turret { -std::f64::consts::PI } else { 0.0 },
            yaw_max_rad: if turret { std::f64::consts::PI } else { 0.0 },
            pitch_min_rad: if turret { -80.0_f64.to_radians() } else { 0.0 },
            pitch_max_rad: if turret { 80.0_f64.to_radians() } else { 0.0 },
            yaw_rate_rad_s: self.slew_rate_rad_s,
            pitch_rate_rad_s: self.slew_rate_rad_s,
        };
        Some(match &self.mechanism {
            WeaponMechanism::Gun {
                drive,
                ammunition,
                projectile_radius_m,
                muzzle_speed_m_s,
            } => {
                let resource = catalogue
                    .resources
                    .iter()
                    .position(|r| &r.id == ammunition)?;
                WeaponSpec::Gun(abi::GunSpec {
                    weapon,
                    chemical: u64::from(*drive == WeaponDrive::Chemical),
                    ammunition_resource: resource as u64 + 1,
                    projectile_mass_kg: catalogue.resources[resource].mass_kg,
                    projectile_radius_m: *projectile_radius_m,
                    muzzle_speed_m_s: *muzzle_speed_m_s,
                })
            }
            WeaponMechanism::Laser {
                pulse_energy_j,
                range_m,
                beam_waist_m,
                divergence_half_angle_rad,
            } => WeaponSpec::Laser(abi::LaserSpec {
                weapon,
                pulse_energy_j: *pulse_energy_j,
                range_m: *range_m,
                beam_waist_m: *beam_waist_m,
                divergence_half_angle_rad: *divergence_half_angle_rad,
            }),
        })
    }

    pub fn valid(&self) -> bool {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        let valid_mechanism = match &self.mechanism {
            WeaponMechanism::Gun {
                ammunition,
                projectile_radius_m,
                muzzle_speed_m_s,
                ..
            } => {
                !ammunition.is_empty()
                    && positive(*projectile_radius_m)
                    && positive(*muzzle_speed_m_s)
                    && DVec3::from_array(self.muzzle_offset_m).length() > *projectile_radius_m
            }
            WeaponMechanism::Laser {
                pulse_energy_j,
                range_m,
                beam_waist_m,
                divergence_half_angle_rad,
            } => {
                positive(*pulse_energy_j)
                    && positive(*range_m)
                    && positive(*beam_waist_m)
                    && positive(*divergence_half_angle_rad)
                    && *divergence_half_angle_rad < 0.1
            }
        };
        if !valid_mechanism {
            return false;
        }
        [self.cycle_interval_s, self.efficiency]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
            && self.efficiency <= 1.0
            && [self.slew_rate_rad_s, self.dispersion_half_angle_rad]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
            && self.dispersion_half_angle_rad < 0.1
            && self.muzzle_offset_m.iter().all(|v| v.is_finite())
            && DVec3::from_array(self.muzzle_offset_m).length() > 0.0
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WeaponCommand {
    #[serde(with = "SavedWeaponSetting")]
    pub setting: abi::WeaponSetting,
    pub epoch_s: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WeaponState {
    pub previous_yaw_rad: f64,
    pub previous_pitch_rad: f64,
    pub yaw_rad: f64,
    pub pitch_rad: f64,
    pub next_fire_s: f64,
    pub shots_fired: u64,
    pub command: Option<WeaponCommand>,
    pub inhibit_flags: u64,
    pub advanced_s: f64,
    pub powered: bool,
}

pub fn shot_energy(spec: &WeaponSpec) -> f64 {
    match spec {
        WeaponSpec::Gun(gun) if gun.chemical != 0 => 0.0,
        WeaponSpec::Gun(gun) => {
            0.5 * gun.projectile_mass_kg * gun.muzzle_speed_m_s.powi(2) / gun.efficiency
        }
        WeaponSpec::Laser(laser) => laser.pulse_energy_j / laser.efficiency,
    }
}

/// Fraction of a Gaussian pulse intercepted by a centered circular silhouette.
pub fn beam_interception(waist_m: f64, divergence_rad: f64, distance_m: f64, radius_m: f64) -> f64 {
    let width_squared = waist_m.powi(2) + (divergence_rad * distance_m).powi(2);
    -(-2.0 * radius_m.powi(2) / width_squared).exp_m1()
}

pub fn valid_setting(value: &abi::WeaponSetting) -> bool {
    let direction = DVec3::from_array(value.aim_direction);
    let rate = DVec3::from_array(value.aim_angular_velocity_rad_s);
    direction.is_finite()
        && (direction.length_squared() - 1.0).abs() <= 1e-6
        && rate.is_finite()
        && rate.length() <= 100.0
        && value.maximum_pointing_error_rad.is_finite()
        && (0.0..=std::f64::consts::PI).contains(&value.maximum_pointing_error_rad)
        && value.valid_until_s.is_finite()
        && value.trigger <= 1
}

impl WeaponState {
    pub fn desired(&self, now: f64) -> DVec3 {
        self.command.map_or(DVec3::NEG_Z, |command| {
            DQuat::from_scaled_axis(
                DVec3::from_array(command.setting.aim_angular_velocity_rad_s)
                    * (now - command.epoch_s).max(0.0),
            ) * DVec3::from_array(command.setting.aim_direction)
        })
    }

    pub fn barrel_rotation(&self) -> DQuat {
        DQuat::from_rotation_y(self.yaw_rad) * DQuat::from_rotation_x(self.pitch_rad)
    }

    pub fn advance(
        &mut self,
        spec: &abi::WeaponSpec,
        now: f64,
        mut mount: impl FnMut(f64) -> DQuat,
    ) {
        let mut t = self.advanced_s;
        while t < now && self.powered && self.command.is_some() {
            let step = (now - t).min(0.005);
            t += step;
            let direction = mount(t).inverse() * self.desired(t);
            let yaw = (-direction.x).atan2(-direction.z);
            let pitch = direction.y.clamp(-1.0, 1.0).asin();
            let full_yaw = spec.yaw_max_rad - spec.yaw_min_rad >= std::f64::consts::TAU - 1e-8;
            let wrap = |angle: f64| {
                (angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI
            };
            let yaw_error = if full_yaw {
                wrap(yaw - self.yaw_rad)
            } else {
                yaw.clamp(spec.yaw_min_rad, spec.yaw_max_rad) - self.yaw_rad
            };
            let next_yaw = self.yaw_rad
                + yaw_error.clamp(-spec.yaw_rate_rad_s * step, spec.yaw_rate_rad_s * step);
            self.yaw_rad = if full_yaw {
                wrap(next_yaw)
            } else {
                next_yaw.clamp(spec.yaw_min_rad, spec.yaw_max_rad)
            };
            self.pitch_rad += (pitch.clamp(spec.pitch_min_rad, spec.pitch_max_rad)
                - self.pitch_rad)
                .clamp(-spec.pitch_rate_rad_s * step, spec.pitch_rate_rad_s * step);
        }
        self.advanced_s = now;
    }

    pub fn candidate(&self, start: f64, end: f64) -> Option<f64> {
        let command = self.command?;
        if command.setting.trigger == 0 || !self.powered {
            return None;
        }
        let at = start.max(self.next_fire_s);
        (at < end - 1e-9 && at < command.setting.valid_until_s - 1e-9).then_some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_beam_has_finite_near_field_and_inverse_area_far_field() {
        let near = beam_interception(0.1, 0.00002, 0.0, 0.05);
        assert!((near - (1.0 - (-0.5_f64).exp())).abs() < 1e-12);
        let far = beam_interception(0.1, 0.00002, 1e7, 1.0);
        let twice_as_far = beam_interception(0.1, 0.00002, 2e7, 1.0);
        assert!((far / twice_as_far - 4.0).abs() < 0.001);
        assert_eq!(beam_interception(0.1, 0.00002, 0.0, 10.0), 1.0);
        assert_eq!(beam_interception(0.1, 0.00002, 100.0, 0.0), 0.0);
    }

    #[test]
    fn laser_configuration_has_no_ballistic_fields() {
        let catalogue = Catalogue::builtin();
        for part in &catalogue.parts {
            let crate::Equipment::Weapon { weapon } = &part.equipment else {
                continue;
            };
            let encoded = toml::to_string(weapon).unwrap();
            let parsed: WeaponDef = toml::from_str(&encoded).unwrap();
            assert!(parsed.valid());
            let spec = parsed.spec(&catalogue).unwrap();
            if let WeaponSpec::Laser(laser) = spec {
                assert!(!encoded.contains("ammunition"));
                assert!(!encoded.contains("muzzle_speed"));
                assert_eq!(shot_propellant_kg(&spec), 0.0);
                assert_eq!(shot_energy(&spec), laser.pulse_energy_j / laser.efficiency);
            }
        }
    }

    #[test]
    fn full_rotation_turret_crosses_yaw_seam_in_both_directions() {
        let catalogue = Catalogue::builtin();
        let design = crate::armed_starter().compile(&catalogue).unwrap();
        let spec = &design.weapon_specs[0];
        for sign in [-1.0, 1.0] {
            let desired = DQuat::from_rotation_y(-sign * 3.0) * DVec3::NEG_Z;
            let mut state = WeaponState {
                powered: true,
                yaw_rad: sign * 3.0,
                command: Some(WeaponCommand {
                    epoch_s: 0.0,
                    setting: abi::WeaponSetting {
                        aim_direction: desired.to_array(),
                        valid_until_s: 1.0,
                        ..Default::default()
                    },
                }),
                ..Default::default()
            };
            state.advance(spec, 0.2, |_| DQuat::IDENTITY);
            assert!((state.barrel_rotation() * DVec3::NEG_Z).angle_between(desired) < 1e-8);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "abi::WeaponSetting")]
pub(crate) struct SavedWeaponSetting {
    pub aim_direction: [f64; 3],
    pub aim_angular_velocity_rad_s: [f64; 3],
    pub maximum_pointing_error_rad: f64,
    pub valid_until_s: f64,
    pub trigger: u64,
}
