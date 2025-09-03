pub mod aerodynamics;
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
        aerodynamics::{AeroEnv, run_aero},
        docking::{DockChild, run_docking},
    },
    precision::{PreciseTransform, ToMetersExt, ToMillimetersExt},
};

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, gizmos);
        app.add_plugins((run_aero, run_docking));
        app.add_systems(FixedUpdate, gravity.run_if(in_state(GameState::Game)));
        app.add_systems(FixedUpdate, apply_forces.run_if(in_state(GameState::Game)));
    }
}

/// Applies all the forces and torques.
fn apply_forces(
    mut objects: Query<
        (
            &MassProps,
            &mut PreciseTransform,
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
    objects.iter_mut().for_each(
        |(mass, mut ptf, mut vel, mut force, mut ang_vel, mut torque, mut acc_prev)| {
            // // deal with force (velocity-verlet)
            // {
            //     ptf.translation_mm += (vel.0 * dt + acc_prev.0 * half_dt2).to_millimeters();
            //     let acc_new = force.0 / mass.mass;
            //     vel.0 += 0.5 * (acc_prev.0 + acc_new) * dt;
            //     acc_prev.0 = acc_new;
            // }

            // deal with force (symplectic Euler: kick -> drift)
            {
                let acc = force.0 * (1.0 / mass.mass);
                info!(acc = debug(acc.length()), "acc applying");
                vel.0 += acc * dt;
                ptf.translation_mm += (vel.0 * dt).to_millimeters();
            }

            // deal with torques (Euler's equations with gyroscopic term in body frame)
            let rot = DMat3::from_quat(ptf.rotation);
            let omega_b = rot.transpose() * ang_vel.0;
            let tau_b = rot.transpose() * torque.0;
            let j_omega = mass.inertia * omega_b;
            let gyro = omega_b.cross(j_omega);
            let domega_b = mass.inertia_inv * (tau_b - gyro);
            info!(ang_acc = debug(domega_b.length()), "ang_acc applying");
            let omega_b_new = omega_b + domega_b * dt;
            let omega_world_new = rot * omega_b_new;
            let delta_q = DQuat::from_scaled_axis(omega_world_new * dt);
            ptf.rotation = (delta_q * ptf.rotation).normalize();
            ang_vel.0 = omega_world_new;

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
    AeroEnv
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
