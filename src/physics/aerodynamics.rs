mod aero_env;
pub use aero_env::*;

use std::f64::consts::PI;

use bevy::math::DVec3;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::GameState;

use crate::orrery::{Celestial, Orrery};
use crate::physics::{
    AccumulatedForce, AccumulatedTorque, AngularVelocity, Velocity, WithinSoi, sim_time,
};
use crate::precision::{PreciseTransform, ToMetersExt, ToMillimetersExt};

pub(super) fn run_aero(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (update_aero_env, trivial_drag).run_if(in_state(GameState::Game)),
    );
}

/// Applies aerodynamic drag and a simple rotational drag torque assuming a 1m-radius sphere.
/// Drag force: F = -½·ρ·C_d·A·|v|²·v̂
/// Rotational drag torque: T = -½·ρ·C_d·A·|ω|²·R·ω̂
fn trivial_drag(
    mut objects: Query<(
        &AngularVelocity,
        &mut AccumulatedForce,
        &mut AccumulatedTorque,
        &AeroEnv,
    )>,
) {
    const DRAG_COEFF: f64 = 0.47;
    const RADIUS: f64 = 1.0;
    const AREA: f64 = PI * RADIUS * RADIUS;

    for (ang_vel, mut force, mut torque, params) in objects.iter_mut() {
        // Linear drag based on relative airspeed and local density.
        let v_rel = params.airspeed;
        let speed = v_rel.length();
        if speed > 0.0 {
            let drag_mag = 0.5 * params.density * DRAG_COEFF * AREA * speed * speed;
            force.0 += -v_rel.normalize() * drag_mag;
        }

        // Rotational drag torque based on angular speed relative to atmosphere.
        let omega = ang_vel.0;
        let ang_speed = omega.length();
        if ang_speed > 0.0 {
            let torque_mag =
                0.5 * params.density * DRAG_COEFF * AREA * ang_speed * ang_speed * RADIUS;
            torque.0 += -omega.normalize() * torque_mag;
        }
    }
}
