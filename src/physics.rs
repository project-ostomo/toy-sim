pub mod aerodynamics;
pub mod docking;

use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use hifitime::Epoch;

use crate::{
    GameState,
    orrery::{Celestial, Universe},
    physics::{
        aerodynamics::{AeroEnv, run_aero},
        docking::{DockChild, run_docking},
    },
    precision::{InterpolatedTransform, PreciseTransform},
    simulation::SimulationSystems,
};

pub const GRAVITATIONAL_CONSTANT: f64 = 6.67430e-11;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((run_aero, run_docking));
        app.add_systems(
            FixedUpdate,
            gravity
                .in_set(SimulationSystems::Forces)
                .run_if(in_state(GameState::Game)),
        );
        app.add_systems(
            FixedPostUpdate,
            apply_forces.run_if(in_state(GameState::Game)),
        );
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
            // deal with force (velocity-verlet)
            {
                ptf.translation_um = ptf
                    .translation_um
                    .offset_by(vel.0 * dt + acc_prev.0 * half_dt2);
                let acc_new = force.0 / mass.mass;
                vel.0 += 0.5 * (acc_prev.0 + acc_new) * dt;
                acc_prev.0 = acc_new;
            }

            // // deal with force (symplectic Euler: kick -> drift)
            // {
            //     let acc = force.0 * (1.0 / mass.mass);
            //     info!(acc = debug(acc.length()), "acc applying");
            //     vel.0 += acc * dt;
            //     ptf.translation_um += (vel.0 * dt).to_micrometers();
            // }

            // deal with torques (Euler's equations with gyroscopic term in body frame)
            // Work in body frame for dynamics, then update orientation and world ω consistently.
            let rot = DMat3::from_quat(ptf.rotation);
            let omega_b = rot.transpose() * ang_vel.0;
            let tau_b = rot.transpose() * torque.0;
            let j_omega = mass.inertia * omega_b;
            let gyro = omega_b.cross(j_omega);
            let domega_b = mass.inertia_inv * (tau_b - gyro);
            let omega_b_new = omega_b + domega_b * dt;

            // Update orientation using body-frame incremental rotation (right-multiply for local axes)
            let delta_q_body = DQuat::from_scaled_axis(omega_b_new * dt);
            ptf.rotation = (ptf.rotation * delta_q_body).normalize();

            // After updating orientation, update world-frame angular velocity consistently
            let rot_new = DMat3::from_quat(ptf.rotation);
            ang_vel.0 = rot_new * omega_b_new;

            // clear
            force.0 = DVec3::ZERO;
            torque.0 = DVec3::ZERO;
        },
    );
}

#[derive(Component, Default)]
#[require(
    InterpolatedTransform,
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
pub(crate) struct PreviousAcceleration(pub DVec3);

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
    star: Res<Universe>,
    celestials: Query<(
        Entity,
        &Celestial,
        &PreciseTransform,
        Option<&crate::orrery::activity::CelestialState>,
    )>,
    mut objects: Query<(
        Entity,
        &MassProps,
        &PreciseTransform,
        &mut AccumulatedForce,
        Option<&WithinSoi>,
    )>,
) {
    objects
        .iter_mut()
        .for_each(|(object_ent, props, obj_ptf, mut force, soi)| {
            const GEE: f64 = GRAVITATIONAL_CONSTANT;
            let mut closest_celestial = None;
            let mut biggest_gravity = 0.0;
            for (cel_entity, celestial, cel_ptf, state) in celestials.iter() {
                let applies = state.map_or_else(
                    || star.gravity_applies(&celestial.0, obj_ptf.translation_um),
                    |s| {
                        obj_ptf
                            .translation_um
                            .relative_to(s.anchor)
                            .length_squared()
                            <= s.influence.powi(2)
                    },
                );
                if !applies {
                    continue;
                }
                let cel_mass = state.map_or_else(
                    || star.get_body(&celestial.0).unwrap().mass,
                    |s| s.body.mass,
                );
                let obj_to_cel = cel_ptf.translation_um.relative_to(obj_ptf.translation_um);
                let r_squared = obj_to_cel.length_squared();
                if r_squared <= 0.0 {
                    continue;
                }
                let f = GEE * cel_mass * props.mass / r_squared;
                if f > biggest_gravity {
                    biggest_gravity = f;
                    closest_celestial = Some(cel_entity);
                }
                force.0 += obj_to_cel.normalize() * f;
            }
            if closest_celestial.is_none() && soi.is_some() {
                commands.command_scope(|mut commands| {
                    commands.entity(object_ent).remove::<WithinSoi>();
                });
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

pub fn sim_time<T: Default>(t: &Time<T>) -> Epoch {
    Epoch::from_mjd_utc(0.0) + hifitime::Duration::from_seconds(t.elapsed_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::precision::GalacticPosition;
    use std::time::Duration;

    #[test]
    fn activating_remote_gravity_sources_does_not_change_local_forces() {
        use crate::orrery::universe::{SyntheticCfg, generate};
        let mut configs = vec![crate::orrery::example_config()];
        configs.extend(generate(
            &SyntheticCfg {
                count: 1,
                seed: 42,
                min_distance_pc: 2.0,
                max_distance_pc: 2.0,
            },
            GalacticPosition::ZERO,
        ));
        let universe = Universe::from_configs(configs, "Helion system", 1e-8).unwrap();
        let remote = universe.systems[1].solver.anchor;
        let mut app = App::new();
        app.insert_resource(universe).add_systems(Update, gravity);
        app.world_mut()
            .spawn((Celestial("Helion".into()), PreciseTransform::default()));
        let ship = app
            .world_mut()
            .spawn((
                RigidBody,
                PreciseTransform {
                    translation_um: GalacticPosition::ZERO.offset_by(DVec3::X * 1e11),
                    ..default()
                },
            ))
            .id();
        app.update();
        let before = app.world().get::<AccumulatedForce>(ship).unwrap().0;
        app.world_mut().get_mut::<AccumulatedForce>(ship).unwrap().0 = DVec3::ZERO;
        app.world_mut().spawn((
            Celestial("Synth 001".into()),
            PreciseTransform {
                translation_um: remote,
                ..default()
            },
        ));
        app.update();
        assert_eq!(before, app.world().get::<AccumulatedForce>(ship).unwrap().0);
        app.world_mut().get_mut::<AccumulatedForce>(ship).unwrap().0 = DVec3::ZERO;
        app.world_mut()
            .get_mut::<PreciseTransform>(ship)
            .unwrap()
            .translation_um = GalacticPosition::splat(1 << 90);
        app.update();
        assert_eq!(
            app.world().get::<AccumulatedForce>(ship).unwrap().0,
            DVec3::ZERO
        );
        assert!(app.world().get::<WithinSoi>(ship).is_none());
    }
    #[test]
    fn ten_hz_orbit_stays_close_to_finer_integration() {
        fn integrate(hz: u32, origin: GalacticPosition) -> (DVec3, f64) {
            let mut cfg = crate::orrery::example_config();
            cfg.position_um = origin;
            let orrery = Universe::init(cfg).unwrap();
            let body = orrery.iter().find(|b| b.atmosphere.is_some()).unwrap();
            let name = body.name.clone();
            let mu = GRAVITATIONAL_CONSTANT * body.mass;
            let radius = body.radius + 30_000_000.0;
            let speed = (mu / radius).sqrt();
            let energy_initial = -mu / (2.0 * radius);
            let mut app = App::new();
            app.insert_resource(orrery)
                .init_resource::<Time>()
                .add_systems(Update, (gravity, apply_forces).chain());
            app.world_mut().spawn((
                Celestial(name),
                PreciseTransform {
                    translation_um: origin,
                    ..default()
                },
            ));
            let object = app
                .world_mut()
                .spawn((
                    RigidBody,
                    PreciseTransform {
                        translation_um: origin.offset_by(DVec3::X * radius),
                        ..default()
                    },
                    Velocity(DVec3::Y * speed),
                ))
                .id();
            for _ in 0..120 * hz {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(Duration::from_secs_f64(1.0 / hz as f64));
                app.update();
            }
            let position = app
                .world()
                .get::<PreciseTransform>(object)
                .unwrap()
                .translation_um
                .relative_to(origin);
            let velocity = app.world().get::<Velocity>(object).unwrap().0;
            let energy = velocity.length_squared() / 2.0 - mu / position.length();
            (position, ((energy - energy_initial) / energy_initial).abs())
        }
        let (coarse, energy_error) = integrate(10, GalacticPosition::ZERO);
        let (distant, distant_energy_error) = integrate(10, GalacticPosition::splat(1_i128 << 90));
        assert_eq!(coarse, distant);
        assert_eq!(energy_error, distant_energy_error);
        let (fine, _) = integrate(100, GalacticPosition::ZERO);
        assert!(
            coarse.distance(fine) < 5.0,
            "10/100 Hz separation: {} m",
            coarse.distance(fine)
        );
        assert!(
            energy_error < 1e-5,
            "relative orbital energy error: {energy_error}"
        );
    }
}
