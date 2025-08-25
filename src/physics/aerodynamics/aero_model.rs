use std::f64::consts::PI;

use bevy::{math::DVec3, prelude::*};
use serde::{Deserialize, Serialize};

use crate::precision::PreciseTransform;

pub struct AeroModel {
    pub main: MainBodyModel,
    pub wings: Vec<(PreciseTransform, Wing)>,
}

impl AeroModel {
    /// Computes a force and torque, based on a *relative* airspeed (forward is -Z, as usual) and angular velocity.
    pub fn relative_force(
        &self,
        relative_airspeed: DVec3,
        relative_angvel: DVec3,
    ) -> AeroModelOutput {
        todo!()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AeroModelOutput {
    pub torque: f64,
    pub force: f64,
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
