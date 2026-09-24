pub mod aerodynamics;
pub mod collision;
pub(crate) mod rotation;

use bevy::{
    math::{DMat3, DVec3},
    prelude::*,
};
use hifitime::Epoch;

use crate::sim::{
    GameState,
    orrery::Universe,
    physics::aerodynamics::{AeroEnv, run_aero},
    precision::PreciseTransform,
    simulation::SimulationSystems,
};

pub const GRAVITATIONAL_CONSTANT: f64 = 6.67430e-11;
/// Acceleration threshold in m/s² used to bound simulated system influence.
pub const GRAVITY_CUTOFF: f64 = 1e-8;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(run_aero);
        collision::install(app);
        app.add_systems(
            FixedUpdate,
            gravity
                .in_set(SimulationSystems::Forces)
                .run_if(in_state(GameState::Game)),
        );
        app.add_systems(
            FixedPostUpdate,
            apply_forces
                .in_set(SimulationSystems::Integrate)
                .run_if(in_state(GameState::Game)),
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
            Option<&mut GravityAcceleration>,
            Option<&mut AccelerometerState>,
        ),
        Without<collision::CollisionBody>,
    >,
    time: Res<Time<Fixed>>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.apply_forces");
    let dt = time.delta_secs_f64();
    // Symplectic Euler: kick with forces at the starting positions, then drift
    // using the updated velocity. No acceleration history survives the tick.
    objects.iter_mut().for_each(
        |(mass, mut ptf, mut vel, mut force, mut ang_vel, mut torque, gravity, reading)| {
            let gravity = gravity.map_or(DVec3::ZERO, |mut g| std::mem::take(&mut g.0));
            let specific_force = force.0 / mass.mass - gravity;
            let old_w = ptf.rotation.inverse() * ang_vel.0;
            let alpha_world = ptf.rotation
                * (mass.inertia_inv
                    * (ptf.rotation.inverse() * torque.0 - old_w.cross(mass.inertia * old_w)));
            let (velocity, momentum) =
                force_kick(vel.0, ptf.rotation, ang_vel.0, *mass, force.0, torque.0, dt);
            vel.0 = velocity;
            ptf.translation_um = ptf.translation_um.offset_by(velocity * dt);
            (ptf.rotation, ang_vel.0) =
                rotation::drift(ptf.rotation, momentum, mass.inertia_inv, dt);
            if let Some(mut reading) = reading {
                *reading = AccelerometerState {
                    specific_force_body: ptf.rotation.inverse() * specific_force,
                    angular_acceleration_body: ptf.rotation.inverse() * alpha_world,
                    angular_velocity_body: ptf.rotation.inverse() * ang_vel.0,
                    time_s: Some(time.elapsed_secs_f64()),
                };
            }

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
    AeroEnv,
    GravityAcceleration,
    AccelerometerState
)]
pub struct RigidBody;

#[derive(Component, Default)]
pub struct Velocity(pub DVec3);

#[derive(Component, Default)]
pub struct AngularVelocity(pub DVec3);

#[derive(Component, Default)]
pub struct AccumulatedForce(pub DVec3);

#[derive(Component, Default)]
pub struct AccumulatedTorque(pub DVec3);

#[derive(Component, Default)]
pub struct GravityAcceleration(pub DVec3);

/// Latest ideal accelerometer sample. No accumulated delta-v or callback history.
#[derive(Component, Default)]
pub struct AccelerometerState {
    pub specific_force_body: DVec3,
    pub angular_acceleration_body: DVec3,
    pub angular_velocity_body: DVec3,
    pub time_s: Option<f64>,
}
impl AccelerometerState {
    pub fn at_mount(
        &self,
        position: DVec3,
        rotation: bevy::math::DQuat,
    ) -> Option<api_accel::AccelerometerSample> {
        Some(api_accel::AccelerometerSample {
            time_s: self.time_s?,
            acceleration_m_s2: (rotation.inverse()
                * (self.specific_force_body
                    + self.angular_acceleration_body.cross(position)
                    + self
                        .angular_velocity_body
                        .cross(self.angular_velocity_body.cross(position))))
            .to_array(),
        })
    }
}
use osg_ships as api_accel;

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

/// Read-only gravity field shared by ordinary bodies and boundary launches.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GravityField<'w, 's> {
    universe: Option<Res<'w, Universe>>,
    active: Option<Res<'w, super::orrery::activity::ActiveSystems>>,
    celestials: Query<
        'w,
        's,
        (
            Entity,
            &'static PreciseTransform,
            &'static super::orrery::activity::CelestialState,
        ),
    >,
}

impl GravityField<'_, '_> {
    pub(crate) fn sample(
        &self,
        entity: Entity,
        position: osg_space::GalacticPosition,
    ) -> (DVec3, Option<Entity>) {
        let (Some(universe), Some(active)) = (&self.universe, &self.active) else {
            return (DVec3::ZERO, None);
        };
        let mut strongest = 0.0;
        let mut closest = None;
        let mut acceleration = DVec3::ZERO;
        for system in active.systems_for_object(universe, entity, position).iter() {
            for (celestial, pose, state) in self
                .celestials
                .iter_many(active.entities.get(system).into_iter().flatten())
            {
                if position.relative_to(state.anchor).length_squared() > state.influence.powi(2) {
                    continue;
                }
                let separation = pose.translation_um.relative_to(position);
                let squared = separation.length_squared();
                if squared == 0.0 {
                    continue;
                }
                let strength = GRAVITATIONAL_CONSTANT * state.body.mass / squared;
                acceleration += separation.normalize() * strength;
                if strength > strongest {
                    strongest = strength;
                    closest = Some(celestial);
                }
            }
        }
        (acceleration, closest)
    }
}

/// Apply one external force/torque kick before either free flight or Rapier.
pub(crate) fn force_kick(
    velocity: DVec3,
    rotation: bevy::math::DQuat,
    angular: DVec3,
    mass: MassProps,
    force: DVec3,
    torque: DVec3,
    dt: f64,
) -> (DVec3, DVec3) {
    (
        velocity + force / mass.mass * dt,
        rotation * (mass.inertia * (rotation.inverse() * angular)) + torque * dt,
    )
}

fn gravity(
    commands: ParallelCommands,
    field: GravityField,
    mut objects: Query<(
        Entity,
        &MassProps,
        &PreciseTransform,
        &mut AccumulatedForce,
        Option<&WithinSoi>,
        Option<&mut GravityAcceleration>,
    )>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("physics.gravity");
    for (entity, mass, pose, mut force, soi, measured) in &mut objects {
        let (acceleration, closest) = field.sample(entity, pose.translation_um);
        force.0 += acceleration * mass.mass;
        if let Some(mut measured) = measured {
            measured.0 = acceleration;
        }
        if closest != soi.map(|soi| soi.0) {
            commands.command_scope(|mut commands| {
                if let Some(celestial) = closest {
                    commands.entity(entity).insert(WithinSoi(celestial));
                } else {
                    commands.entity(entity).remove::<WithinSoi>();
                }
            });
        }
    }
}

pub fn sim_time<T: Default>(t: &Time<T>) -> Epoch {
    Epoch::from_mjd_utc(osg_universe::SIMULATION_EPOCH_MJD_UTC)
        + hifitime::Duration::from_seconds(t.elapsed_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::orrery::Celestial;
    use crate::sim::precision::GalacticPosition;
    use std::time::Duration;

    fn index_test_bodies(world: &mut World) {
        let universe = world.resource::<Universe>().clone();
        let mut active = super::super::orrery::activity::ActiveSystems::default();
        for (entity, state) in world
            .query::<(Entity, &super::super::orrery::activity::CelestialState)>()
            .iter(world)
        {
            active
                .entities
                .entry(state.system)
                .or_default()
                .push(entity);
        }
        for (entity, pose) in world
            .query_filtered::<(Entity, &PreciseTransform), With<MassProps>>()
            .iter(world)
        {
            active.object_systems.insert(
                entity,
                universe
                    .index()
                    .containing_segment(pose.translation_um, DVec3::ZERO),
            );
        }
        world.insert_resource(active);
    }

    fn celestial_state(
        universe: &Universe,
        name: &str,
    ) -> crate::sim::orrery::activity::CelestialState {
        let reference = universe.authored_body(name).unwrap();
        let system = universe.system_index(reference.system).unwrap();
        let definition = universe.resolve_index(system).unwrap();
        crate::sim::orrery::activity::CelestialState {
            reference: super::super::registry::model_reference(reference),
            body: universe.body(reference).unwrap(),
            system,
            anchor: definition.solver.anchor,
            influence: definition.influence,
            velocity: DVec3::ZERO,
        }
    }

    #[test]
    fn activating_remote_gravity_sources_does_not_change_local_forces() {
        let configs = vec![
            osg_universe::example_config(),
            toml::from_str(include_str!("../../../../tests/fixtures/remote.star.toml")).unwrap(),
        ];
        let universe = Universe::from_configs(configs, 1e-8).unwrap();
        let remote = universe.systems()[1].position;
        let local_state = celestial_state(&universe, "Helion");
        let remote_state = celestial_state(&universe, "Remote");
        let mut app = App::new();
        app.insert_resource(universe)
            .init_resource::<Time<Fixed>>()
            .add_systems(Update, (index_test_bodies, gravity).chain());
        app.world_mut().spawn((
            Celestial("Helion".into()),
            local_state,
            PreciseTransform::default(),
        ));
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
            Celestial("Remote".into()),
            remote_state,
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

        // A body born after activation still receives local gravity this tick.
        let newborn = app
            .world_mut()
            .spawn((
                RigidBody,
                PreciseTransform {
                    translation_um: GalacticPosition::ZERO.offset_by(DVec3::X * 1e11),
                    ..default()
                },
            ))
            .id();
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut().run_system_once(gravity).unwrap();
        assert!(
            app.world()
                .get::<AccumulatedForce>(newborn)
                .unwrap()
                .0
                .length()
                > 0.0
        );
    }
    #[test]
    fn ordinary_motion_integrates_one_complete_tick() {
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>()
            .add_systems(Update, apply_forces);
        let ship = app
            .world_mut()
            .spawn((
                RigidBody,
                PreciseTransform::default(),
                Velocity(DVec3::X * 10.0),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(osg_model::TICK_DURATION);
        app.update();
        let x = app
            .world()
            .get::<PreciseTransform>(ship)
            .unwrap()
            .translation_um
            .x;
        assert!((x - 1_000_000).abs() <= 1);
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(osg_model::TICK_DURATION);
        app.update();
        let x = app
            .world()
            .get::<PreciseTransform>(ship)
            .unwrap()
            .translation_um
            .x;
        assert!((x - 2_000_000).abs() <= 1);
    }

    #[test]
    fn force_changes_apply_immediately_without_acceleration_history() {
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>()
            .add_systems(Update, apply_forces);
        let ship = app
            .world_mut()
            .spawn((
                RigidBody,
                PreciseTransform::default(),
                AccumulatedForce(DVec3::X * 20.0),
            ))
            .id();
        for (force, expected_velocity, expected_x_um) in [
            (20.0, 2.0, 200_000),
            (0.0, 2.0, 400_000),
            (-20.0, 0.0, 400_000),
        ] {
            app.world_mut().get_mut::<AccumulatedForce>(ship).unwrap().0 = DVec3::X * force;
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(osg_model::TICK_DURATION);
            app.update();
            assert_eq!(
                app.world().get::<Velocity>(ship).unwrap().0,
                DVec3::X * expected_velocity
            );
            assert_eq!(
                app.world()
                    .get::<PreciseTransform>(ship)
                    .unwrap()
                    .translation_um
                    .x,
                expected_x_um
            );
            assert_eq!(
                app.world().get::<AccumulatedForce>(ship).unwrap().0,
                DVec3::ZERO
            );
        }
    }

    #[test]
    fn low_orbit_energy_and_radius_remain_bounded_over_two_revolutions() {
        let universe = Universe::init(osg_universe::example_config()).unwrap();
        let scenario = &crate::sim::scenario::INITIAL_SCENARIO;
        let planet = universe
            .body(universe.authored_body(scenario.body).unwrap())
            .unwrap();
        let mu = GRAVITATIONAL_CONSTANT * planet.mass;
        // Keep this low-orbit regression independent of the startup scenario.
        let radius = planet.radius + 170_000.0;
        let period = std::f64::consts::TAU * (radius.powi(3) / mu).sqrt();
        let name = planet.name.clone();
        let state = celestial_state(&universe, &name);
        let mut app = App::new();
        app.insert_resource(universe)
            .init_resource::<Time<Fixed>>()
            .add_systems(Update, (index_test_bodies, gravity, apply_forces).chain());
        app.world_mut()
            .spawn((Celestial(name), state, PreciseTransform::default()));
        let ship = app
            .world_mut()
            .spawn((
                RigidBody,
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::X * radius),
                    ..default()
                },
                Velocity(DVec3::Y * (mu / radius).sqrt()),
            ))
            .id();
        let energy_initial = -mu / (2.0 * radius);
        let mut max_energy_error: f64 = 0.0;
        let mut max_radial_error: f64 = 0.0;
        for _ in 0..(2.0 * period / osg_model::TICK_SECONDS).ceil() as usize {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(osg_model::TICK_DURATION);
            app.update();
            let position = app
                .world()
                .get::<PreciseTransform>(ship)
                .unwrap()
                .translation_um
                .to_meters_64();
            let velocity = app.world().get::<Velocity>(ship).unwrap().0;
            let energy = velocity.length_squared() * 0.5 - mu / position.length();
            max_energy_error =
                max_energy_error.max(((energy - energy_initial) / energy_initial).abs());
            max_radial_error = max_radial_error.max((position.length() - radius).abs());
        }
        assert!(
            max_energy_error < 1e-7,
            "relative energy error: {max_energy_error}"
        );
        assert!(
            max_radial_error < 450.0,
            "radius error: {max_radial_error} m"
        );
    }

    #[test]
    fn ten_hz_orbit_stays_close_to_finer_integration() {
        fn integrate(hz: u32, origin: GalacticPosition) -> (DVec3, f64) {
            let mut cfg = osg_universe::example_config();
            cfg.position_um = origin;
            let orrery = Universe::init(cfg).unwrap();
            let definition = orrery.resolve_index(0).unwrap();
            let body = definition
                .solver
                .iter()
                .find(|b| b.atmosphere.is_some())
                .unwrap();
            let name = body.name.clone();
            let state = celestial_state(&orrery, &name);
            let mu = GRAVITATIONAL_CONSTANT * body.mass;
            let radius = body.radius + 30_000_000.0;
            let speed = (mu / radius).sqrt();
            let energy_initial = -mu / (2.0 * radius);
            let mut app = App::new();
            app.insert_resource(orrery)
                .init_resource::<Time<Fixed>>()
                .add_systems(Update, (index_test_bodies, gravity, apply_forces).chain());
            app.world_mut().spawn((
                Celestial(name),
                state,
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
                    .resource_mut::<Time<Fixed>>()
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

#[cfg(test)]
mod rendezvous_tests;
