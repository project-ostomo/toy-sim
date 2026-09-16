use crate::orrery::activity::CelestialState;
use crate::physics::Velocity;
use bevy::math::DVec3;
use bevy::prelude::*;

use super::{PreciseTransform, PrecisionSystems, ToMicrometersExt};
use crate::simulation::SimulationSystems;

/// Opt a simulated entity into presentation interpolation. Never used by physics.
#[derive(Component, Default)]
#[require(PresentationPose, PresentationVelocity)]
pub struct InterpolatedTransform {
    previous: Option<PreciseTransform>,
    previous_velocity: Option<DVec3>,
    reset_pending: bool,
}

impl InterpolatedTransform {
    /// Apply a discontinuous authoritative move and invalidate presentation history.
    /// Other authoritative state, such as velocity, is the caller's responsibility.
    pub fn teleport(
        &mut self,
        authoritative: &mut PreciseTransform,
        destination: PreciseTransform,
    ) {
        *authoritative = destination;
        self.reset_pending = true;
    }
}

/// Absolute high-precision pose for rendering and camera tracking only.
#[derive(Component, Default, Clone, Copy)]
pub struct PresentationPose(pub PreciseTransform);

/// Velocity at the same presentation instant as the rendered pose. Never used by physics.
#[derive(Component, Default, Clone, Copy)]
pub struct PresentationVelocity(pub DVec3);

pub(super) fn configure(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        remember_previous.in_set(SimulationSystems::History),
    )
    .add_systems(
        PostUpdate,
        interpolate.in_set(PrecisionSystems::Interpolate),
    );
}

fn remember_previous(
    mut objects: Query<(
        &PreciseTransform,
        Option<&Velocity>,
        Option<&CelestialState>,
        &mut InterpolatedTransform,
    )>,
) {
    for (current, velocity, celestial, mut history) in &mut objects {
        history.previous = Some(*current);
        history.previous_velocity = physical_velocity(velocity, celestial);
    }
}

fn physical_velocity(
    velocity: Option<&Velocity>,
    celestial: Option<&CelestialState>,
) -> Option<DVec3> {
    velocity
        .map(|v| v.0)
        .or_else(|| celestial.map(|c| c.velocity))
}

fn interpolate(
    time: Res<Time<Fixed>>,
    mut objects: Query<(
        &PreciseTransform,
        &mut InterpolatedTransform,
        &mut PresentationPose,
        Option<&Velocity>,
        Option<&CelestialState>,
        &mut PresentationVelocity,
    )>,
) {
    let alpha = time.overstep_fraction_f64().clamp(0.0, 1.0);
    for (current, mut history, mut presentation, velocity, celestial, mut presented_velocity) in
        &mut objects
    {
        let velocity = physical_velocity(velocity, celestial);
        if history.reset_pending || history.previous.is_none() {
            history.previous = Some(*current);
            history.previous_velocity = velocity;
            history.reset_pending = false;
        }
        presentation.0 = interpolate_pose(history.previous.as_ref().unwrap(), current, alpha);
        let current_velocity = velocity.unwrap_or(DVec3::ZERO);
        presented_velocity.0 = history
            .previous_velocity
            .unwrap_or(current_velocity)
            .lerp(current_velocity, alpha);
    }
}

fn interpolate_pose(
    previous: &PreciseTransform,
    current: &PreciseTransform,
    alpha: f64,
) -> PreciseTransform {
    // Subtract integer coordinates first: converting absolute astronomical positions
    // to floats would discard small movements. Only round the interpolated offset to mm.
    let displacement = current
        .translation_um
        .saturating_sub(previous.translation_um)
        .to_meters_64();
    PreciseTransform {
        translation_um: previous
            .translation_um
            .saturating_add((displacement * alpha).to_micrometers()),
        rotation: previous.rotation.slerp(current.rotation, alpha).normalize(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GameState,
        precision::{FloatingOrigin, FloatingOriginAnchor, GalacticPosition, PrecisionPlugin},
        simulation::{SimulationCounters, SimulationPlugin},
    };
    use bevy::{
        math::{DQuat, DVec3},
        state::app::StatesPlugin,
        time::TimeUpdateStrategy,
    };
    use std::time::Duration;

    #[derive(Component)]
    struct Moving;

    fn move_object(mut objects: Query<&mut PreciseTransform, With<Moving>>, time: Res<Time>) {
        for mut pose in &mut objects {
            pose.translation_um += (DVec3::X * 100_000.0 * time.delta_secs_f64()).to_micrometers();
        }
    }

    fn setup() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            TransformPlugin,
            SimulationPlugin,
            PrecisionPlugin,
        ))
        .insert_state(GameState::Game)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
        .add_systems(FixedPostUpdate, move_object)
        .add_systems(
            PostUpdate,
            (|target: Single<&PresentationPose, With<Moving>>,
              mut camera: Single<&mut PreciseTransform, With<FloatingOriginAnchor>>| {
                **camera = target.0;
                camera.translation_um.z += 100_000_000;
            })
            .in_set(PrecisionSystems::Camera),
        );
        let object = app
            .world_mut()
            .spawn((
                Moving,
                PreciseTransform {
                    translation_um: GalacticPosition::new(1_i128 << 90, 0, 0),
                    ..default()
                },
                InterpolatedTransform::default(),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((PreciseTransform::default(), FloatingOriginAnchor))
            .id();
        app.update(); // Initialize the clock and spawn presentation without advancing time.
        (app, object, camera)
    }

    fn advance(app: &mut App, millis: u64) {
        *app.world_mut().resource_mut::<TimeUpdateStrategy>() =
            TimeUpdateStrategy::ManualDuration(Duration::from_millis(millis));
        app.update();
    }

    #[test]
    fn interpolation_preserves_micrometres_at_astronomical_coordinates_and_slerps() {
        let previous = PreciseTransform {
            translation_um: GalacticPosition::splat(1_i128 << 90),
            ..default()
        };
        let current = PreciseTransform {
            translation_um: previous.translation_um + GalacticPosition::new(2, 0, 0),
            rotation: DQuat::from_rotation_y(2.0),
        };
        let middle = interpolate_pose(&previous, &current, 0.5);
        assert_eq!(
            middle.translation_um,
            previous.translation_um + GalacticPosition::X
        );
        assert!(
            middle
                .rotation
                .abs_diff_eq(DQuat::from_rotation_y(1.0), 1e-12)
        );
    }

    #[test]
    fn presentation_velocity_tracks_pose_alpha_and_resets_after_teleport() {
        let mut app = App::new();
        let mut time = Time::<Fixed>::from_hz(10.);
        time.accumulate_overstep(Duration::from_millis(25));
        app.insert_resource(time).add_systems(Update, interpolate);
        let previous = PreciseTransform::default();
        let current = PreciseTransform {
            translation_um: GalacticPosition::from_meters(DVec3::X * 100.),
            ..default()
        };
        let entity = app
            .world_mut()
            .spawn((
                current,
                Velocity(DVec3::X * 300.),
                InterpolatedTransform {
                    previous: Some(previous),
                    previous_velocity: Some(DVec3::X * 100.),
                    reset_pending: false,
                },
            ))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<PresentationPose>(entity)
                .unwrap()
                .0
                .translation_um,
            GalacticPosition::from_meters(DVec3::X * 25.)
        );
        assert_eq!(
            app.world().get::<PresentationVelocity>(entity).unwrap().0,
            DVec3::X * 150.
        );
        assert_eq!(
            app.world().get::<Velocity>(entity).unwrap().0,
            DVec3::X * 300.
        );
        let destination = PreciseTransform {
            translation_um: GalacticPosition::from_meters(DVec3::Y * 1000.),
            ..default()
        };
        let mut query = app.world_mut().query::<(
            &mut PreciseTransform,
            &mut InterpolatedTransform,
            &mut Velocity,
        )>();
        let (mut pose, mut history, mut velocity) = query.get_mut(app.world_mut(), entity).unwrap();
        history.teleport(&mut pose, destination);
        velocity.0 = DVec3::Y * 500.;
        app.update();
        assert_eq!(
            app.world()
                .get::<PresentationPose>(entity)
                .unwrap()
                .0
                .translation_um,
            destination.translation_um
        );
        assert_eq!(
            app.world().get::<PresentationVelocity>(entity).unwrap().0,
            DVec3::Y * 500.
        );
    }

    #[test]
    fn celestial_ephemeris_velocity_uses_the_same_presentation_interpolation() {
        let universe = crate::orrery::Universe::init(crate::orrery::example_config()).unwrap();
        let body = universe
            .get_body(crate::scenario::INITIAL_SCENARIO.body)
            .unwrap()
            .clone();
        let mut app = App::new();
        let mut time = Time::<Fixed>::from_hz(10.);
        time.accumulate_overstep(Duration::from_millis(25));
        app.insert_resource(time).add_systems(
            Update,
            (
                remember_previous,
                |mut bodies: Query<&mut CelestialState>| {
                    for mut body in &mut bodies {
                        body.velocity.x += 400.;
                    }
                },
                interpolate,
            )
                .chain(),
        );
        let entity = app
            .world_mut()
            .spawn((
                PreciseTransform::default(),
                InterpolatedTransform::default(),
                CelestialState {
                    body,
                    system: 0,
                    anchor: GalacticPosition::ZERO,
                    influence: 1.,
                    velocity: DVec3::X * 10_000.,
                },
            ))
            .id();
        app.update();
        assert!(app.world().get::<Velocity>(entity).is_none());
        assert_eq!(
            app.world().get::<PresentationVelocity>(entity).unwrap().0,
            DVec3::X * 10_100.
        );
        assert_eq!(
            app.world().get::<CelestialState>(entity).unwrap().velocity,
            DVec3::X * 10_400.
        );
    }

    #[test]
    fn fixed_ticks_catch_up_while_presentation_and_counters_follow_frames() {
        let (mut app, object, _) = setup();
        let base = app
            .world()
            .get::<PreciseTransform>(object)
            .unwrap()
            .translation_um;
        for (millis, ticks, rendered_metres) in
            [(40, 0, 0), (60, 1, 0), (50, 1, 5_000), (250, 4, 30_000)]
        {
            advance(&mut app, millis);
            let world = app.world();
            assert_eq!(world.resource::<SimulationCounters>().ticks, ticks);
            assert_eq!(
                world
                    .get::<PreciseTransform>(object)
                    .unwrap()
                    .translation_um
                    .x
                    - base.x,
                ticks as i128 * 10_000_000_000
            );
            assert_eq!(
                world
                    .get::<PresentationPose>(object)
                    .unwrap()
                    .0
                    .translation_um
                    .x
                    - base.x,
                rendered_metres * 1_000_000
            );
        }
        assert_eq!(app.world().resource::<SimulationCounters>().frames, 5);
        let before = app.world().get::<PresentationPose>(object).unwrap().0;
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        advance(&mut app, 200);
        assert_eq!(app.world().resource::<SimulationCounters>().ticks, 4);
        assert_eq!(
            app.world()
                .get::<PresentationPose>(object)
                .unwrap()
                .0
                .translation_um,
            before.translation_um
        );
        app.world_mut().resource_mut::<Time<Virtual>>().unpause();
        advance(&mut app, 100);
        assert_eq!(app.world().resource::<SimulationCounters>().ticks, 5);
    }

    #[test]
    fn time_scale_changes_tick_frequency_without_changing_the_physics_step() {
        let (mut app, object, _) = setup();
        let base = app
            .world()
            .get::<PreciseTransform>(object)
            .unwrap()
            .translation_um;
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed(10.0);
        advance(&mut app, 25); // 0.25 simulated seconds: two ticks and half an overstep.
        assert_eq!(app.world().resource::<SimulationCounters>().ticks, 2);
        assert_eq!(
            app.world().resource::<Time<Fixed>>().timestep(),
            Duration::from_millis(100)
        );
        assert_eq!(
            app.world()
                .get::<PreciseTransform>(object)
                .unwrap()
                .translation_um
                .x
                - base.x,
            20_000_000_000
        );
        let rendered = app
            .world()
            .get::<PresentationPose>(object)
            .unwrap()
            .0
            .translation_um;
        assert_eq!(rendered.x - base.x, 15_000_000_000);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        advance(&mut app, 100);
        assert_eq!(app.world().resource::<SimulationCounters>().ticks, 2);
        assert_eq!(
            app.world()
                .get::<PresentationPose>(object)
                .unwrap()
                .0
                .translation_um,
            rendered
        );
        {
            let mut time = app.world_mut().resource_mut::<Time<Virtual>>();
            time.set_relative_speed(1.0);
            time.unpause();
        }
        advance(&mut app, 50);
        assert_eq!(app.world().resource::<SimulationCounters>().ticks, 3);
        assert_eq!(
            app.world()
                .get::<PreciseTransform>(object)
                .unwrap()
                .translation_um
                .x
                - base.x,
            30_000_000_000
        );
    }

    #[test]
    fn frame_rates_and_rebases_do_not_change_motion_or_camera_attachment() {
        for frame_ms in [10, 25, 50] {
            let (mut app, object, camera) = setup();
            let base = app
                .world()
                .get::<PreciseTransform>(object)
                .unwrap()
                .translation_um;
            let first_origin = app.world().resource::<FloatingOrigin>().0.translation_um;
            let marker = app
                .world_mut()
                .spawn(Transform::from_xyz(1.0, 2.0, 3.0))
                .id();
            let child = app
                .world_mut()
                .spawn((Transform::from_xyz(0.0, 5.0, 0.0), ChildOf(object)))
                .id();
            for _ in 0..(1500 / frame_ms) {
                advance(&mut app, frame_ms);
                let world = app.world();
                let ship_tf = world.get::<GlobalTransform>(object).unwrap().translation();
                let camera_tf = world.get::<GlobalTransform>(camera).unwrap().translation();
                assert!((camera_tf - ship_tf).abs_diff_eq(Vec3::Z * 100.0, 1e-4));
                assert!(
                    (world.get::<GlobalTransform>(child).unwrap().translation() - ship_tf)
                        .abs_diff_eq(Vec3::Y * 5.0, 1e-4)
                );
                assert_eq!(
                    world.get::<GlobalTransform>(marker).unwrap().translation(),
                    Vec3::new(1.0, 2.0, 3.0)
                );
            }
            let world = app.world();
            assert_eq!(world.resource::<SimulationCounters>().ticks, 15);
            assert_eq!(
                world
                    .get::<PreciseTransform>(object)
                    .unwrap()
                    .translation_um
                    .x
                    - base.x,
                150_000_000_000
            );
            assert_eq!(
                world
                    .get::<PresentationPose>(object)
                    .unwrap()
                    .0
                    .translation_um
                    .x
                    - base.x,
                140_000_000_000
            );
            assert_ne!(
                world.resource::<FloatingOrigin>().0.translation_um,
                first_origin
            );
        }
    }

    #[test]
    fn spawning_and_teleporting_reset_history_even_across_multiple_ticks() {
        let (mut app, object, _) = setup();
        advance(&mut app, 150);
        let destination = PreciseTransform {
            translation_um: GalacticPosition::new(-9_000_000_000_000, 40_000, 0),
            rotation: DQuat::from_rotation_x(0.7),
        };
        let spawned = app
            .world_mut()
            .spawn((destination, InterpolatedTransform::default()))
            .id();
        advance(&mut app, 0);
        assert_eq!(
            app.world()
                .get::<PresentationPose>(spawned)
                .unwrap()
                .0
                .translation_um,
            destination.translation_um
        );
        {
            let world = app.world_mut();
            let mut query = world.query::<(&mut PreciseTransform, &mut InterpolatedTransform)>();
            let (mut authoritative, mut history) = query.get_mut(world, object).unwrap();
            history.teleport(&mut authoritative, destination);
        }
        advance(&mut app, 200); // Reset survives history capture in both catch-up ticks.
        let authoritative = *app.world().get::<PreciseTransform>(object).unwrap();
        let presentation = app.world().get::<PresentationPose>(object).unwrap().0;
        assert_eq!(presentation.translation_um, authoritative.translation_um);
        assert!(
            presentation
                .rotation
                .abs_diff_eq(destination.rotation, 1e-12)
        );
        advance(&mut app, 100); // Normal interpolation resumes on the following tick.
        let next = app.world().get::<PresentationPose>(object).unwrap().0;
        assert_eq!(
            next.translation_um.x - authoritative.translation_um.x,
            5_000_000_000
        );
    }
}
