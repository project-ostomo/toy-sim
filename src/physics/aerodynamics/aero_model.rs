use std::f64::consts::PI;

use bevy::{math::DVec3, prelude::*};
use serde::{Deserialize, Serialize};

use crate::{
    physics::{AccumulatedForce, AccumulatedTorque, AngularVelocity, aerodynamics::AeroEnv},
    precision::{PreciseTransform, ToMetersExt},
};

pub(crate) fn calc_aerodynamics(
    mut planes: Query<(
        &AeroEnv,
        &AngularVelocity,
        &AeroModel,
        &PreciseTransform,
        &mut AccumulatedForce,
        &mut AccumulatedTorque,
    )>,
) {
    for (env, angvel, model, ptf, mut force, mut torque) in planes.iter_mut() {
        let rot_inv = ptf.rotation.inverse();
        let airspeed_local = rot_inv * env.airspeed;
        let angvel_local = rot_inv * angvel.0;
        let out = model.relative_force(airspeed_local, angvel_local, env);
        info!(torque = debug(out.torque), "torque applying");
        force.0 += ptf.rotation * out.force;
        torque.0 += ptf.rotation * out.torque;
    }
}

#[derive(Component)]
pub struct AeroModel {
    pub main: MainBodyModel,
    pub wings: Vec<(PreciseTransform, Wing)>,
}

impl Default for AeroModel {
    fn default() -> Self {
        Self {
            main: MainBodyModel::Sphere(1.0),
            wings: vec![],
        }
    }
}

impl AeroModel {
    /// Computes aerodynamic force and torque, given the body-frame
    /// airspeed and angular velocity.
    ///
    /// Coordinate convention: forward is -Z; hence for forward flight the
    /// relative airspeed points along -Z in body axes.
    pub fn relative_force(
        &self,
        relative_airspeed: DVec3,
        relative_angvel: DVec3,
        env: &AeroEnv,
    ) -> AeroModelOutput {
        if relative_airspeed == DVec3::ZERO {
            return AeroModelOutput {
                torque: DVec3::ZERO,
                force: DVec3::ZERO,
            };
        }

        let make_flow = |speed: f64| -> Flow {
            let mach = (speed / env.speed_of_sound).abs();
            let q = 0.5 * env.density * speed * speed;
            Flow { mach, q }
        };

        let mut total_force = DVec3::ZERO;
        let mut total_torque = DVec3::ZERO;

        // main body drag
        let main_drag = self.main.drag(make_flow(relative_airspeed.length()));
        let vel_dir = relative_airspeed.normalize();
        let drag_dir = -vel_dir;
        total_force += drag_dir * main_drag;

        for (wing_tf, wing) in &self.wings {
            let r = wing_tf.translation_mm.to_meters_64();
            // Rigid-body kinematics: local point velocity = v_com + ω × r
            let v_local = relative_airspeed + relative_angvel.cross(r);
            let v_hat = v_local.normalize();

            // Compute AoA in the wing's LOCAL frame so twist/orientation are respected.
            // In wing-local axes: forward = -Z, lift axis = +Y, span = +X.
            let v_local_wing = wing_tf.rotation.conjugate() * v_local;
            let aoa = (-v_local_wing.y).atan2(-v_local_wing.z);

            let flow = make_flow(v_local.length());
            let wing_force = wing.eval_forces(aoa, flow);

            // Directions in WORLD frame
            let drag_dir = -v_hat;
            // We assume the wing's span is along its local X-axis.
            let wing_span_dir = wing_tf.rotation * DVec3::X;
            let lift_dir = wing_span_dir.cross(v_hat).normalize();

            let projected_force = lift_dir * wing_force.lift + drag_dir * wing_force.drag;
            total_force += projected_force;
            total_torque += r.cross(projected_force);
        }

        AeroModelOutput {
            torque: total_torque,
            force: total_force,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AeroModelOutput {
    pub torque: DVec3,
    pub force: DVec3,
}

pub enum MainBodyModel {
    Sphere(f64),
}

impl MainBodyModel {
    pub fn drag(&self, flow: Flow) -> f64 {
        match self {
            MainBodyModel::Sphere(radius) => {
                // Cross-sectional area of sphere
                let area = PI * radius * radius;

                // Simple drag coefficient for a sphere
                // Could be made more sophisticated with Reynolds number dependence
                let cd = if flow.mach < 0.8 {
                    0.47 // Typical subsonic value for a sphere
                } else {
                    // Simple supersonic increase
                    0.47 * (1.0 + 0.5 * (flow.mach - 0.8))
                };

                cd * flow.q * area
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Wing {
    pub area: f64,
    pub span: f64,

    #[serde(default)]
    pub details: WingDetails,

    #[serde(default)]
    pub control: Option<ControlSurface>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WingDetails {
    pub aoa0: f64,       // default 0.0 rad
    pub efficiency: f64, // default 0.85
    pub cdrag0: f64,     // default 0.012
    pub clift_max: f64,  // default 1.4
    pub clift_min: f64,  // default -1.2
}

impl Default for WingDetails {
    fn default() -> Self {
        Self {
            aoa0: 0.0,
            efficiency: 0.85,
            cdrag0: 0.012,
            clift_max: 1.4,
            clift_min: -1.4,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Flow {
    pub mach: f64,
    pub q: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct WingCoeffs {
    pub cl: f64,
    pub cd: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct WingForces {
    pub lift: f64,
    pub drag: f64,
}

impl Wing {
    /// Combined evaluator: returns CL and CD (useful for UI, logging, stability).
    #[inline]
    pub fn eval_coeffs(&self, aoa: f64, flow: Flow) -> WingCoeffs {
        // Geometry-derived constants
        let ar = (self.span * self.span) / self.area;
        let inv_pi_ar_e = 1.0 / (PI * ar * self.details.efficiency.max(1e-3));

        // Incompressible 3D lift slope (a0 ≈ 2π) with simple subsonic compressibility
        let cla_inc = (2.0 * PI) / (1.0 + (2.0 * PI) * inv_pi_ar_e);
        let beta = (1.0 - flow.mach * flow.mach).max(1e-6).sqrt();
        let comp_gain = (1.0 / beta).min(1.35); // gentle cap near transonic
        let cla = cla_inc * comp_gain;

        // Controls
        let (dcl, dcd0) = if let Some(c) = self.control {
            (c.a_delta * c.delta, c.dcd0_delta * c.delta.abs())
        } else {
            (0.0, 0.0)
        };

        // Linear CL and smooth stall cap (~3° band)
        let cl_lin = cla * (aoa - self.details.aoa0) + dcl;
        let width_cl = (cla * (3.0_f64.to_radians())).max(1e-4);
        let cl = soft_clip(
            cl_lin,
            self.details.clift_min,
            self.details.clift_max,
            width_cl,
        );

        // Drag: parasite + induced + small post-stall + tiny wave-drag rise near M~0.72+
        let cd0 = self.details.cdrag0 + dcd0;
        let k = inv_pi_ar_e;
        let k_stall = 0.02;
        let cd_wave = if flow.mach > 0.72 {
            let t = (flow.mach - 0.72) / (1.0 - 0.72);
            0.02 * (t * t)
        } else {
            0.0
        };
        let cd = cd0 + k * cl * cl + k_stall * (cl_lin - cl).abs() + cd_wave;

        WingCoeffs { cl, cd }
    }

    /// Evaluate the forces on this wing, given the angle of attack and airflow.
    #[inline]
    pub fn eval_forces(&self, aoa: f64, flow: Flow) -> WingForces {
        let c = self.eval_coeffs(aoa, flow);
        let q_s = flow.q * self.area;
        WingForces {
            lift: c.cl * q_s,
            drag: c.cd * q_s,
        }
    }
}

#[inline]
fn soft_clip(x: f64, xmin: f64, xmax: f64, width: f64) -> f64 {
    let s = |t: f64| t * t * (3.0 - 2.0 * t); // smoothstep
    if x < xmin {
        let t = ((x - (xmin - width)) / width).clamp(0.0, 1.0);
        (1.0 - s(t)) * (xmin - width) + s(t) * xmin
    } else if x > xmax {
        let t = (((xmax + width) - x) / width).clamp(0.0, 1.0);
        (1.0 - s(t)) * (xmax + width) + s(t) * xmax
    } else {
        x
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ControlSurface {
    /// Commanded deflection (rad).
    pub delta: f64,
    /// Lift increment per rad of deflection (ΔCL = a_delta * delta).
    pub a_delta: f64,
    /// Extra profile drag from deflection (added to CD0).
    pub dcd0_delta: f64,
    /// Pitching-moment change per rad (ΔCM = m_delta * delta).
    pub m_delta: f64,
}

#[cfg(test)]
mod tests {
    use super::{AeroEnv, AeroModel, Flow, MainBodyModel, Wing, WingDetails};
    use crate::precision::PreciseTransform;
    use crate::precision::ToMillimetersExt;
    use bevy::math::{DQuat, DVec3};

    #[test]
    fn simple_wing() {
        let wing = Wing {
            area: 10.0,
            span: 10.0,
            details: Default::default(),
            control: None,
        };
        for angle in 0..90 {
            let forces = wing.eval_forces(
                (angle as f64).to_radians(),
                Flow {
                    mach: 0.8,
                    q: 1000.0,
                },
            );
            eprintln!("{:.4} / {:.4}", forces.lift, forces.drag)
        }
    }

    #[test]
    fn torque_about_z_from_outboard_wing() {
        // Wing offset along +X; upward lift should create +Z torque (yaw)
        let wing = Wing {
            area: 10.0,
            span: 10.0,
            details: WingDetails {
                aoa0: (-5.0_f64).to_radians(),
                ..Default::default()
            },
            control: None,
        };

        let wing_tf = PreciseTransform {
            translation_mm: DVec3::new(2.0, 0.0, 0.0).to_millimeters(),
            rotation: DQuat::IDENTITY,
        };

        let model = AeroModel {
            main: MainBodyModel::Sphere(0.1),
            wings: vec![(wing_tf, wing)],
        };

        let env = AeroEnv {
            density: 1.225,
            speed_of_sound: 340.0,
            ..Default::default()
        };

        let rel_wind = DVec3::new(0.0, 0.0, 100.0);
        let out = model.relative_force(rel_wind, DVec3::ZERO, &env);

        // Positive yaw torque due to r=(+X) and F_y>0  => τ_z = r_x * F_y > 0
        assert!(
            out.torque.z > 0.0,
            "expected +Z torque (got {:?})",
            out.torque
        );
        // No roll torque expected from symmetric setup
        assert!(
            out.torque.x.abs() < 1e-6,
            "unexpected roll torque (got {:?})",
            out.torque
        );
    }

    #[test]
    fn vertical_fin_produces_yaw_from_sideslip() {
        // A vertical fin (90° twist about Z) at the tail should create a yawing moment
        // under a small positive sideslip (v_x > 0, forward v_z < 0).
        let wing = Wing {
            area: 10.0,
            span: 10.0,
            details: WingDetails {
                aoa0: 0.0,
                ..Default::default()
            },
            control: None,
        };

        // Place the fin 2 m behind the CoM, and rotate it 90° about Z so the span is vertical.
        let wing_tf = PreciseTransform {
            translation_mm: DVec3::new(0.0, 0.0, 2.0).to_millimeters(),
            rotation: DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
        };

        let model = AeroModel {
            main: MainBodyModel::Sphere(0.1),
            wings: vec![(wing_tf, wing)],
        };

        let env = AeroEnv {
            density: 1.225,
            speed_of_sound: 340.0,
            ..Default::default()
        };

        // Forward flight with slight +X sideslip
        let rel_wind = DVec3::new(10.0, 0.0, -100.0);
        let out = model.relative_force(rel_wind, DVec3::ZERO, &env);

        // Expect a yawing moment about +Y (sign depends on force direction). With +X sideslip
        // and rearward lever (r_z>0), lift acts roughly along -X, giving τ_y = r_z * F_x < 0.
        assert!(out.torque.y < 0.0, "expected negative yaw torque, got {:?}", out.torque);

        // No significant roll/pitch in this symmetric setup
        assert!(out.torque.x.abs() < 1e-5, "unexpected pitch torque: {:?}", out.torque);
        assert!(out.torque.z.abs() < 1e-5, "unexpected roll torque: {:?}", out.torque);
    }
}
