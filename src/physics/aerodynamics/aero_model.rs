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
    /// Computes a force and torque, based on a *relative* airspeed (forward is -Z, as usual) and angular velocity.
    pub fn relative_force(
        &self,
        relative_airspeed: DVec3,
        relative_angvel: DVec3,
        env: &AeroEnv,
    ) -> AeroModelOutput {
        let make_flow = |speed: f64| -> Flow {
            let mach = (speed / env.speed_of_sound).abs();
            let q = 0.5 * env.density * speed * speed;
            Flow { mach, q }
        };

        let mut total_force = DVec3::ZERO;
        let mut total_torque = DVec3::ZERO;

        let v_body = relative_airspeed;
        let speed_body = v_body.length();
        if speed_body > 0.0 {
            let flow_body = make_flow(speed_body);
            let drag_mag = self.main.drag(flow_body);
            total_force += -v_body / speed_body * drag_mag;
        }

        for (wing_tf, wing) in &self.wings {
            let r = wing_tf.translation_mm.to_meters_64();
            let v_local_body = v_body - relative_angvel.cross(r);

            let v_local_wing = wing_tf.rotation.inverse() * v_local_body;
            let speed_wing = v_local_wing.length();

            let flow = make_flow(speed_wing);
            let aoa = v_local_wing.y.atan2(-v_local_wing.z);
            let WingForces { lift, drag } = wing.eval_forces(aoa, flow);
            let v_dir = v_local_wing / speed_wing;
            let drag_dir_local = -v_dir;
            let span_axis_local = DVec3::X;
            let lift_dir_local = (v_dir.cross(span_axis_local).cross(v_dir))
                .try_normalize()
                .unwrap_or(DVec3::Y);
            let f_local = drag_dir_local * drag + lift_dir_local * lift;
            let f_body = wing_tf.rotation * f_local;

            total_force += f_body;
            total_torque += r.cross(f_body);
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
            clift_min: -1.2,
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
        let qS = flow.q * self.area;
        WingForces {
            lift: c.cl * qS,
            drag: c.cd * qS,
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
    use crate::physics::aerodynamics::aero_model::{Flow, Wing};

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
}
