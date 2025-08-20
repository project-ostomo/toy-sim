use std::collections::BTreeMap;

use crate::precision::ToMillimetersExt;
use bevy::{
    ecs::resource::Resource,
    math::{DQuat, DVec3, I64Vec3},
};
use hifitime::Epoch;
use smol_str::SmolStr;
use std::f64::consts::PI;

use crate::orrery::orrery_cfg::{Body, OrreryCfg};

/// A solver for a whole star system
#[derive(Resource)]
pub struct Orrery {
    name: SmolStr,
    bodies: BTreeMap<SmolStr, Body>,
}

impl Orrery {
    /// Create a new star-system solver.
    pub fn init(cfg: OrreryCfg) -> anyhow::Result<Self> {
        let mut bodies: BTreeMap<SmolStr, Body> = BTreeMap::new();
        for mut body in cfg.bodies {
            let name = body.name.clone();
            // ensure parent exists before computing period
            if let Some(parent) = body.parent.as_ref()
                && !bodies.contains_key(parent)
            {
                anyhow::bail!("unidentified parent {parent} of {name}");
            }
            // calculate missing orbital period via Kepler's third law if semi-major axis is non-zero
            if body.orbit.period == 0.0 && body.orbit.semi_major != 0.0 {
                // gravitational constant [m^3 kg^-1 s^-2]
                const G: f64 = 6.674e-11;
                // semi-major axis is in meters
                let a_m = body.orbit.semi_major;
                // parent mass in kg if any
                let parent_mass = if let Some(parent_name) = &body.parent {
                    if let Some(parent) = bodies.get(parent_name) {
                        parent.mass
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                // Kepler's third law: T = 2π * sqrt(a^3 / (G (M_parent + M_body)))
                let mu = G * (parent_mass + body.mass);
                body.orbit.period = 2.0 * std::f64::consts::PI * (a_m.powi(3) / mu).sqrt();
            }
            if bodies.insert(name.clone(), body).is_some() {
                anyhow::bail!("duplicate name in star system: {name}");
            }
        }
        Ok(Self {
            name: cfg.name,
            bodies,
        })
    }

    /// Iterates through the bodies of the system.
    pub fn iter(&self) -> impl Iterator<Item = &Body> {
        self.bodies.values()
    }

    /// Gets a body by name.
    pub fn get_body(&self, name: &str) -> Option<&Body> {
        self.bodies.get(name)
    }

    /// Solves for the position, in millimeters, of a particular body in the system, at a particular time. Returns None if such a body does not exist in the system.
    #[allow(non_snake_case)]
    pub fn solve_position(&self, body: &str, epoch: Epoch) -> Option<I64Vec3> {
        // Lookup body and compute parent position
        let body_cfg = self.bodies.get(body)?;
        let parent_pos = if let Some(parent) = &body_cfg.parent {
            self.solve_position(parent, epoch)?
        } else {
            I64Vec3::ZERO
        };
        // Bodies with zero semi-major axis are fixed relative to their parent
        if body_cfg.orbit.semi_major == 0.0 {
            return Some(parent_pos);
        }

        // Time since reference epoch (config epoch is in MJD)
        let epoch0 = Epoch::from_mjd_utc(body_cfg.orbit.epoch);
        let dt_s = (epoch - epoch0).to_seconds();

        // Mean anomaly at current epoch
        let n = 2.0 * std::f64::consts::PI / body_cfg.orbit.period;
        let m = body_cfg.orbit.mean_anomaly + n * dt_s;

        // Solve Kepler's equation for eccentric anomaly E via Newton's method
        let e = body_cfg.orbit.eccentricity;
        let mut E = m;
        for _ in 0..50 {
            let f = E - e * E.sin() - m;
            let f_prime = 1.0 - e * E.cos();
            E -= f / f_prime;
        }

        // True anomaly
        let cos_E = E.cos();
        let sin_E = E.sin();
        let v = ((1.0 - e * e).sqrt() * sin_E).atan2(cos_E - e);

        // Radius in orbital plane (m)
        let r_m = body_cfg.orbit.semi_major * (1.0 - e * cos_E);

        // Position in orbital plane (m)
        let pos_orb = DVec3::new(r_m * v.cos(), r_m * v.sin(), 0.0);

        // Rotate from orbital plane to inertial frame
        let rot = DQuat::from_rotation_z(body_cfg.orbit.ascending_node)
            * DQuat::from_rotation_x(body_cfg.orbit.inclination)
            * DQuat::from_rotation_z(body_cfg.orbit.arg_of_pericenter);
        let pos_inertial = rot * pos_orb;

        // Convert to millimeters and add parent offset
        Some(parent_pos + pos_inertial.to_millimeters())
    }

    /// Solves for the orbital velocity (m/s) of a body at a given time, in inertial frame.
    /// Returns None if the body is not found or is fixed (zero semi-major axis).
    #[allow(non_snake_case)]
    pub fn solve_velocity(&self, body: &str, epoch: Epoch) -> Option<DVec3> {
        let cfg = self.bodies.get(body)?;
        // Static bodies have no orbital velocity
        if cfg.orbit.semi_major == 0.0 {
            return Some(DVec3::ZERO);
        }
        // Gravitational parameter µ from period: µ = 4π²a³ / T²
        let a = cfg.orbit.semi_major;
        let T = cfg.orbit.period;
        let mu = 4.0 * PI * PI * a.powi(3) / (T * T);
        // Time since reference epoch
        let epoch0 = Epoch::from_mjd_utc(cfg.orbit.epoch);
        let dt = (epoch - epoch0).to_seconds();
        // Mean motion and anomaly
        let n = 2.0 * PI / T;
        let m = cfg.orbit.mean_anomaly + n * dt;
        // Solve Kepler's equation for E
        let e = cfg.orbit.eccentricity;
        let mut E = m;
        for _ in 0..50 {
            let f = E - e * E.sin() - m;
            let f_prime = 1.0 - e * E.cos();
            E -= f / f_prime;
        }
        let cosE = E.cos();
        let sinE = E.sin();
        // True anomaly
        let v = ((1.0 - e * e).sqrt() * sinE).atan2(cosE - e);
        // Radius
        let r = a * (1.0 - e * cosE);
        // Specific angular momentum
        let h = (mu * a * (1.0 - e * e)).sqrt();
        // Radial and transverse velocity in orbital plane
        let vr = mu / h * e * sinE;
        let vtheta = mu / h * (1.0 + e * cosE);
        let vx = vr * v.cos() - vtheta * v.sin();
        let vy = vr * v.sin() + vtheta * v.cos();
        let vel_orb = DVec3::new(vx, vy, 0.0);
        // Rotate into inertial frame
        let rot = DQuat::from_rotation_z(cfg.orbit.ascending_node)
            * DQuat::from_rotation_x(cfg.orbit.inclination)
            * DQuat::from_rotation_z(cfg.orbit.arg_of_pericenter);
        Some(rot * vel_orb)
    }

    /// Solves for the rotation quaternion of a body at a given epoch.
    /// Rotation parameters (eq_ascend_node, obliquity, rotation_epoch) are defined in the body's orbital frame,
    /// so we first orient the equator in inertial space via the orbit plane, then apply the body spin.
    #[allow(non_snake_case)]
    pub fn solve_rotation(&self, body: &str, epoch: Epoch) -> Option<DQuat> {
        let cfg = self.bodies.get(body)?;
        // no rotation period ⇒ identity orientation
        if cfg.rotation.rotation_period == 0.0 {
            return Some(DQuat::IDENTITY);
        }
        // spin phase since reference rotation_epoch
        let epoch0 = Epoch::from_mjd_utc(cfg.rotation.rotation_epoch);
        let spin_angle = 2.0 * std::f64::consts::PI * (epoch - epoch0).to_seconds()
            / cfg.rotation.rotation_period;

        // 1) orbit frame → inertial: apply ascending_node, inclination, arg_of_pericenter
        let orbit_rot = DQuat::from_rotation_z(cfg.orbit.ascending_node)
            * DQuat::from_rotation_x(cfg.orbit.inclination)
            * DQuat::from_rotation_z(cfg.orbit.arg_of_pericenter);

        // 2) body equator within orbital plane: eq_ascend_node, obliquity, then spin
        let eq_rot = DQuat::from_rotation_z(cfg.rotation.eq_ascend_node)
            * DQuat::from_rotation_x(cfg.rotation.obliquity)
            * DQuat::from_rotation_z(spin_angle - cfg.rotation.eq_ascend_node);

        // full spin in inertial space
        Some(orbit_rot * eq_rot)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::orrery::orrery_cfg::OrreryCfg;
    use anyhow::Result;
    use hifitime::Epoch;

    #[test]
    fn sun_earth_system_positions() -> Result<()> {
        let yaml = r#"
name: "sun-earth"
bodies:
  - name: "Sun"
    mass: "1 massSol"
  - name: "Earth"
    parent: "Sun"
    mass: "1 massEarth"
    semi_major: "1 au"
    period: "365.25 d"
    eccentricity: 0.0
    inclination: 0.0
    ascending_node: 0.0
    arg_of_pericenter: 0.0
    mean_anomaly: 0.0
    epoch: 0.0
"#;
        let cfg: OrreryCfg = serde_yml::from_str(yaml)?;
        let ss = Orrery::init(cfg)?;
        for day in 0..365 {
            let epoch = Epoch::from_mjd_utc(day as f64);
            let pos = ss.solve_position("Earth", epoch).unwrap();
            println!("day {day:3}: {pos:?}");
        }
        Ok(())
    }
    #[test]
    fn default_solve_rotation_identity() -> Result<()> {
        let yaml = r#"
name: "test"
bodies:
  - name: "A"
    mass: "1 massEarth"
"#;
        let cfg: OrreryCfg = serde_yml::from_str(yaml)?;
        let ss = Orrery::init(cfg)?;
        let epoch = Epoch::from_mjd_utc(42.0);
        assert_eq!(ss.solve_rotation("A", epoch).unwrap(), DQuat::IDENTITY);
        Ok(())
    }
}
