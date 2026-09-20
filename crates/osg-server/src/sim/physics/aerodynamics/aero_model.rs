use bevy::{math::DVec3, prelude::*};

use crate::sim::{
    physics::{AccumulatedForce, aerodynamics::AeroEnv},
    precision::PreciseTransform,
};

use std::f64::consts::PI;

/// A ship-aligned ellipsoid enclosing the assembled part bounds.
/// Drag acts at the centre of mass; this model generates no lift or torque.
#[derive(Component, Debug)]
pub struct AeroModel {
    /// Semi-axis lengths in metres, in ship coordinates.
    pub semi_axes: DVec3,
    /// Constant toy-model coefficient, independent of Mach and Reynolds number.
    pub drag_coefficient: f64,
}

impl AeroModel {
    pub fn new(semi_axes: DVec3) -> Self {
        Self {
            semi_axes,
            drag_coefficient: 2.2,
        }
    }

    /// Orthogonal silhouette area, not the central cross-section area.
    pub fn projected_area(&self, direction: DVec3) -> f64 {
        let a = self.semi_axes;
        PI * (DVec3::new(a.y * a.z, a.x * a.z, a.x * a.y) * direction).length()
    }

    /// Force in ship coordinates, strictly opposite air-relative velocity.
    pub fn drag_force(&self, airspeed: DVec3, density: f64) -> DVec3 {
        let speed = airspeed.length();
        if speed == 0.0 || density <= 0.0 {
            return DVec3::ZERO;
        }
        let direction = airspeed / speed;
        let area = self.projected_area(direction);
        -direction * (0.5 * density * speed * speed * self.drag_coefficient * area)
    }
}

pub(crate) fn calc_aerodynamics(
    mut ships: Query<(
        &AeroEnv,
        &AeroModel,
        &PreciseTransform,
        &mut AccumulatedForce,
    )>,
) {
    for (env, model, ptf, mut force) in &mut ships {
        let local_airspeed = ptf.rotation.inverse() * env.airspeed;
        force.0 += ptf.rotation * model.drag_force(local_airspeed, env.density);
    }
}
