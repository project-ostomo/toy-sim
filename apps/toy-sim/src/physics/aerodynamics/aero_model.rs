use std::f64::consts::PI;

use bevy::{math::DVec3, prelude::*};

use crate::{
    physics::{AccumulatedForce, aerodynamics::AeroEnv},
    precision::PreciseTransform,
};

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
    fn projected_area(&self, direction: DVec3) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::AccumulatedTorque;
    use bevy::math::DQuat;

    #[test]
    fn silhouette_area_matches_principal_axes_and_sphere() {
        let model = AeroModel::new(DVec3::new(2.0, 3.0, 5.0));
        for (direction, area) in [
            (DVec3::X, 15.0 * PI),
            (DVec3::Y, 10.0 * PI),
            (DVec3::Z, 6.0 * PI),
        ] {
            assert!((model.projected_area(direction) - area).abs() < 1e-12);
        }
        let sphere = AeroModel::new(DVec3::splat(2.0));
        assert!(
            (sphere.projected_area(DVec3::new(1.0, 2.0, 3.0).normalize()) - 4.0 * PI).abs() < 1e-12
        );
    }

    #[test]
    fn drag_opposes_flow_and_scales_with_density_and_speed_squared() {
        let model = AeroModel::new(DVec3::new(2.0, 3.0, 5.0));
        let velocity = DVec3::new(100.0, -200.0, 300.0);
        let force = model.drag_force(velocity, 1e-8);
        assert!(force.dot(velocity) < 0.0);
        assert!(force.cross(velocity).length() < 1e-10);
        assert!(
            model
                .drag_force(velocity * 2.0, 1e-8)
                .abs_diff_eq(force * 4.0, 1e-12)
        );
        assert!(
            model
                .drag_force(velocity, 2e-8)
                .abs_diff_eq(force * 2.0, 1e-12)
        );
        assert_eq!(model.drag_force(velocity, 0.0), DVec3::ZERO);
        assert_eq!(model.drag_force(DVec3::ZERO, 1.0), DVec3::ZERO);
    }

    #[test]
    fn orientation_changes_drag_magnitude_but_not_direction_or_torque() {
        let mut app = App::new();
        app.add_systems(Update, calc_aerodynamics);
        let spawn = |world: &mut World, rotation| {
            world
                .spawn((
                    AeroModel::new(DVec3::new(1.0, 2.0, 10.0)),
                    AeroEnv {
                        airspeed: DVec3::Z * 1000.0,
                        density: 1e-8,
                        ..default()
                    },
                    PreciseTransform {
                        rotation,
                        ..default()
                    },
                    AccumulatedForce::default(),
                    AccumulatedTorque::default(),
                ))
                .id()
        };
        let nose_on = spawn(app.world_mut(), DQuat::IDENTITY);
        let broadside = spawn(
            app.world_mut(),
            DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
        );
        app.update();
        let nose_force = app.world().get::<AccumulatedForce>(nose_on).unwrap().0;
        let broad_force = app.world().get::<AccumulatedForce>(broadside).unwrap().0;
        assert!(broad_force.abs_diff_eq(nose_force * 10.0, 1e-10));
        for entity in [nose_on, broadside] {
            let force = app.world().get::<AccumulatedForce>(entity).unwrap().0;
            assert!(force.z < 0.0 && force.x.abs() < 1e-10 && force.y.abs() < 1e-10);
            assert_eq!(
                app.world().get::<AccumulatedTorque>(entity).unwrap().0,
                DVec3::ZERO
            );
        }
    }
}
