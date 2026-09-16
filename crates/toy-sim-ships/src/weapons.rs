//! Weapon mechanisms shared by the simulator and hardware tests.
use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};
use toy_sim_ship_api::abi;

use crate::Catalogue;

/// Prototype counter-exhaust: ten percent of projectile mass, in kg of propellant.
/// Exhaust energy and momentum are deliberately not simulated.
pub fn shot_propellant_kg(spec: &abi::WeaponSpec) -> f64 {
    spec.projectile_mass_kg * 0.1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponDef {
    pub ammunition: String,
    pub projectile_radius_m: f64,
    pub muzzle_speed_m_s: f64,
    pub cycle_interval_s: f64,
    pub efficiency: f64,
    pub dispersion_half_angle_rad: f64,
    pub muzzle_offset_m: [f64; 3],
    pub slew_rate_rad_s: f64,
}

impl WeaponDef {
    pub fn spec(&self, catalogue: &Catalogue) -> Option<abi::WeaponSpec> {
        let resource = catalogue
            .resources
            .iter()
            .position(|r| r.id == self.ammunition)?;
        let turret = self.slew_rate_rad_s > 0.0;
        Some(abi::WeaponSpec {
            ammunition_resource: resource as u64 + 1,
            projectile_mass_kg: catalogue.resources[resource].mass_kg,
            projectile_radius_m: self.projectile_radius_m,
            muzzle_speed_m_s: self.muzzle_speed_m_s,
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
        })
    }

    pub fn valid(&self) -> bool {
        [
            self.projectile_radius_m,
            self.muzzle_speed_m_s,
            self.cycle_interval_s,
            self.efficiency,
        ]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0)
            && self.efficiency <= 1.0
            && [self.slew_rate_rad_s, self.dispersion_half_angle_rad]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
            && self.dispersion_half_angle_rad < 0.1
            && self.muzzle_offset_m.iter().all(|v| v.is_finite())
            && DVec3::from_array(self.muzzle_offset_m).length() > self.projectile_radius_m
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WeaponCommand {
    pub setting: abi::WeaponSetting,
    pub epoch_s: f64,
}

#[derive(Clone, Debug, Default)]
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

pub fn shot_energy(spec: &abi::WeaponSpec) -> f64 {
    0.5 * spec.projectile_mass_kg * spec.muzzle_speed_m_s.powi(2) / spec.efficiency
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
