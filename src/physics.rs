mod aerodynamics;
pub mod docking;

use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use hifitime::Epoch;

use crate::{
    GameState,
    orrery::{Celestial, Orrery},
    physics::{
        aerodynamics::run_aero,
        docking::{DockChild, run_docking},
    },
    precision::{PreciseTransform, ToMetersExt, ToMillimetersExt},
};

pub use aerodynamics::AeroParams;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, gizmos);
        app.add_plugins((run_aero, run_docking));
        app.add_systems(
            FixedUpdate,
            (gravity, apply_forces).run_if(in_state(GameState::Game)),
        );
    }
}

/// Applies all the forces and torques.
fn apply_forces(
    mut objects: Query<
        (
            &MassProps,
            &mut PreciseTransform,
            &Transform,
            &mut Velocity,
            &mut AccumulatedForce,
            &mut AngularVelocity,
            &mut AccumulatedTorque,
            &mut PreviousAcceleration,
        ),
        Without<DockChild>,
    >,
    time: Res<Time>,
) {
    let dt = time.delta_secs_f64();
    let half_dt2 = dt.powi(2) * 0.5;

    // currently, we use velocity-verlet for motion + symplectic Euler for rotation, this might change in the future
    objects.par_iter_mut().for_each(
        |(mass, mut ptf, tf, mut vel, mut force, mut ang_vel, mut torque, mut acc_prev)| {
            // deal with force
            ptf.translation_mm += (vel.0 * dt + acc_prev.0 * half_dt2).to_millimeters();
            let acc_new = force.0 / mass.mass;
            vel.0 += 0.5 * (acc_prev.0 + acc_new) * dt;
            acc_prev.0 = acc_new;

            // deal with torques
            let rot = DMat3::from_quat(ptf.rotation);
            // let i_world = rot * mass.inertia * rot.transpose();
            let inv_world = rot * mass.inertia_inv * rot.transpose();
            let ang_accel = inv_world * torque.0;
            ang_vel.0 += ang_accel * dt;
            let delta_q = DQuat::from_scaled_axis(ang_vel.0 * dt);
            ptf.rotation = (delta_q * ptf.rotation).normalize();

            // clear
            force.0 = DVec3::ZERO;
            torque.0 = DVec3::ZERO;
        },
    );
}

#[derive(Component, Default)]
#[require(
    Transform,
    MassProps,
    Velocity,
    AngularVelocity,
    AccumulatedForce,
    AccumulatedTorque,
    PreviousAcceleration,
    AeroParams
)]
pub struct RigidBody;

#[derive(Component, Default)]
pub struct Velocity(pub DVec3);

#[derive(Component, Default)]
struct PreviousAcceleration(pub DVec3);

#[derive(Component, Default)]
pub struct AngularVelocity(pub DVec3);

#[derive(Component, Default)]
pub struct AccumulatedForce(pub DVec3);

#[derive(Component, Default)]
pub struct AccumulatedTorque(pub DVec3);

#[derive(Component, Clone, Copy)]
pub struct MassProps {
    pub mass: f64,
    pub inertia: DMat3,
    pub inertia_inv: DMat3,
}

impl Default for MassProps {
    fn default() -> Self {
        Self {
            mass: 1.0,
            inertia: DMat3::IDENTITY,
            inertia_inv: DMat3::IDENTITY,
        }
    }
}

#[derive(Component)]
#[relationship(relationship_target = HasWithinSoi)]
pub struct WithinSoi(pub Entity);

#[derive(Component)]
#[relationship_target(relationship = WithinSoi)]
pub struct HasWithinSoi(Vec<Entity>);

/// Applies gravitational forces.
fn gravity(
    commands: ParallelCommands,
    star: Res<Orrery>,
    celestials: Query<(Entity, &Celestial, &PreciseTransform)>,
    mut objects: Query<(
        Entity,
        &MassProps,
        &PreciseTransform,
        &mut AccumulatedForce,
        Option<&WithinSoi>,
    )>,
) {
    objects
        .par_iter_mut()
        .for_each(|(object_ent, props, obj_ptf, mut force, soi)| {
            const GEE: f64 = 6.6473e-11;
            let mut closest_celestial = None;
            let mut biggest_gravity = 0.0;
            for (cel_entity, celestial, cel_ptf) in celestials.iter() {
                let cel_mass = star.get_body(&celestial.0).unwrap().mass;
                let obj_to_cel = (cel_ptf.translation_mm - obj_ptf.translation_mm).to_meters_64();
                let r_squared = obj_to_cel.length_squared();
                let f = GEE * cel_mass * props.mass / r_squared;
                if f > biggest_gravity {
                    biggest_gravity = f;
                    closest_celestial = Some(cel_entity);
                }
                force.0 += obj_to_cel.normalize() * f;
            }
            if let Some(cel_entity) = closest_celestial {
                if soi.map(|s| s.0) != Some(cel_entity) {
                    commands.command_scope(|mut commands| {
                        commands.entity(object_ent).insert(WithinSoi(cel_entity));
                    });
                }
            }
        });
}

pub fn sim_time(t: &Time) -> Epoch {
    Epoch::from_tai_seconds(t.elapsed_secs_f64())
}

fn gizmos(mut gizmos: Gizmos, objects: Query<&Transform, With<MassProps>>) {
    for &transform in objects {
        gizmos.axes(transform, 10.);
    }
}
