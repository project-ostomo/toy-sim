use bevy::{
    camera::{Exposure, Hdr},
    core_pipeline::tonemapping::Tonemapping,
    input::mouse::MouseWheel,
    light::CascadeShadowConfigBuilder,
    pbr::{AtmosphereMode, AtmosphereSettings},
    post_process::{
        auto_exposure::{AutoExposure, AutoExposureCompensationCurve},
        bloom::Bloom,
    },
    prelude::*,
};

use crate::{
    GameState,
    orrery::{Celestial, Universe},
    physics::aerodynamics::AeroModel,
    precision::{
        FloatingOrigin, FloatingOriginAnchor, PreciseTransform, PrecisionSystems, PresentationPose,
        ToMicrometersExt,
    },
    vessel::Vessel,
};
use bevy::math::{DQuat, DVec2, DVec3};

pub struct MainCameraPlugin;

#[derive(Component)]
#[require(CameraParams, FloatingOriginAnchor)]
pub struct MainCamera;

#[derive(Component)]
pub struct CameraFocus;

#[derive(Message)]
pub struct FollowTarget(pub Entity);

impl Plugin for MainCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<FollowTarget>();
        app.add_systems(
            OnEnter(GameState::Game),
            (|mut commands: Commands,
              orrery: Res<Universe>,
              mut curves: ResMut<Assets<AutoExposureCompensationCurve>>| {
                let scenario = orrery.scenario.as_ref().expect("missing initial scenario");
                let compensation_curve = curves.add(
                    AutoExposureCompensationCurve::from_curve(
                        bevy::math::cubic_splines::LinearSpline::new([
                            // target exposure = compensation - metered log luminance.
                            // Cap dark-scene gain at 16 stops, including an empty histogram.
                            Vec2::new(-24.0, -8.0),
                            Vec2::new(-18.0, -2.0),
                            Vec2::new(24.0, -2.0),
                        ]),
                    )
                    .expect("valid exposure compensation curve"),
                );
                commands.spawn((
                    MainCamera,
                    Hdr,
                    CameraParams {
                        zoom: (scenario.camera_distance / 100.0).ln(),
                        yaw: scenario.camera_yaw,
                        pitch: scenario.camera_pitch,
                        ..default()
                    },
                    Camera3d::default(),
                    Camera { ..default() },
                    Tonemapping::TonyMcMapface,
                    PreciseTransform::default(),
                    // Smaa::default(),
                    Msaa::Off,
                    // ClusterConfig::Single, // NECESSARY FOR DISTANT LIGHTING
                    // MotionBlur {
                    //     shutter_angle: 1.0,
                    //     samples: 10,
                    // },
                    // Exposure::exposure() = 2^-EV / 1.2; neutralize this stage.
                    Exposure {
                        ev100: -1.2_f32.log2(),
                    },
                    Bloom::NATURAL,
                    AtmosphereSettings {
                        rendering_method: AtmosphereMode::Raymarched,
                        sky_max_samples: 64,
                        ..default()
                    },
                    AutoExposure {
                        range: -24.0..=24.0,
                        // Black space counts toward percentile cutoffs. Never trim
                        // the upper end: it may contain the entire visible planet.
                        filter: 0.99..=1.0,
                        compensation_curve,
                        speed_brighten: 6.0,
                        speed_darken: 2.0,
                        ..default()
                    },
                    Projection::Perspective(PerspectiveProjection {
                        near: 0.1,
                        far: 1e15,
                        ..default()
                    }),
                ));

                commands.spawn((
                    CameraLight,
                    bevy::light::SunDisk::OFF,
                    CascadeShadowConfigBuilder {
                        num_cascades: 4,
                        minimum_distance: 0.1,
                        maximum_distance: 100000.0,
                        ..default()
                    }
                    .build(),
                    DirectionalLight {
                        shadow_maps_enabled: true,
                        ..default()
                    },
                ));
            })
            .after(crate::orrery::LoadOrrery),
        );

        app.add_systems(
            Update,
            (follow_target, camera_controls)
                .chain()
                .run_if(in_state(GameState::Game)),
        );
        app.add_systems(
            PostUpdate,
            (
                camera_pose.in_set(PrecisionSystems::Camera),
                camera_lighting.in_set(PrecisionSystems::Project),
            )
                .run_if(in_state(GameState::Game)),
        );
    }
}

#[derive(Component, Default)]
pub struct CameraParams {
    pub zoom: f64,
    pub yaw: f64,
    pub pitch: f64,
    smoothed_angles: Option<DVec2>,
}

impl CameraParams {
    fn smooth_angles(&mut self, dt: f64) -> DVec2 {
        // A short exponential response: about 95% settled after 120 ms.
        const RESPONSE_TIME: f64 = 0.04;
        let target = DVec2::new(self.yaw, self.pitch);
        let current = self.smoothed_angles.get_or_insert(target);
        let blend = -(-dt / RESPONSE_TIME).exp_m1();
        *current = current.lerp(target, blend);
        *current
    }
}

fn target_radius(
    celestial: Option<&Celestial>,
    model: Option<&AeroModel>,
    orrery: &Universe,
) -> f64 {
    celestial
        .and_then(|c| orrery.get_body(&c.0))
        .map(|b| b.radius)
        .or_else(|| model.map(|m| m.semi_axes.max_element()))
        .unwrap_or(10.0)
}

fn follow_target(
    mut requests: MessageReader<FollowTarget>,
    debug: Option<ResMut<crate::orrery::activity::UniverseDebug>>,
    mut commands: Commands,
    targets: Query<
        (Option<&Celestial>, Option<&AeroModel>),
        (With<PreciseTransform>, Or<(With<Celestial>, With<Vessel>)>),
    >,
    focused: Query<Entity, With<CameraFocus>>,
    camera: Single<&mut CameraParams, With<MainCamera>>,
    orrery: Res<Universe>,
) {
    let Some(request) = requests.read().last() else {
        return;
    };
    let Ok((celestial, model)) = targets.get(request.0) else {
        return;
    };
    if let Some(mut debug) = debug {
        debug.inspect = celestial.map(|c| c.0.clone());
    }
    if focused.iter().any(|entity| entity == request.0) {
        return;
    }
    for entity in &focused {
        commands.entity(entity).remove::<CameraFocus>();
    }
    commands.entity(request.0).insert(CameraFocus);
    let radius = target_radius(celestial, model, &orrery);
    camera.into_inner().zoom = ((radius * 3.0).max(30.0) / 100.0).ln();
}

/// Orbit camera relative to focused object
fn camera_controls(
    camera: Single<&mut CameraParams, With<MainCamera>>,
    focus: Single<
        (&PreciseTransform, Option<&Celestial>, Option<&AeroModel>),
        (With<CameraFocus>, Without<MainCamera>),
    >,
    orrery: Res<Universe>,
    mut mouse_evs: MessageReader<bevy::input::mouse::MouseMotion>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut scroll_evs: MessageReader<MouseWheel>,
) {
    const SENS: f64 = 0.0025;
    const ZOOM_SENS: f64 = 100.0;

    let (_, celestial, model) = focus.into_inner();
    let mut cam = camera.into_inner();

    for ev in mouse_evs.read() {
        if mouse_buttons.pressed(MouseButton::Left) {
            cam.yaw -= ev.delta.x as f64 * SENS;
            cam.pitch -= ev.delta.y as f64 * SENS;
        }
    }
    cam.pitch = cam.pitch.clamp(-1.5, 1.5);

    // Zoom wheel
    for ev in scroll_evs.read() {
        cam.zoom -= ev.y as f64 * 0.05;
    }

    let radius = target_radius(celestial, model, &orrery);
    let min_distance = (radius * 1.01).max(1.0);
    cam.zoom = cam
        .zoom
        .clamp((min_distance / ZOOM_SENS).ln(), (1e15 / ZOOM_SENS).ln());
}

/// Finalize the camera after input and simulation, before selecting the render origin.
fn camera_pose(
    camera: Single<(&mut PreciseTransform, &mut CameraParams), With<MainCamera>>,
    focus: Single<
        (&PreciseTransform, Option<&PresentationPose>),
        (With<CameraFocus>, Without<MainCamera>),
    >,
    time: Res<Time>,
) {
    let (mut cam_ptf, mut cam) = camera.into_inner();
    let (authoritative, presentation) = focus.into_inner();
    let focus_ptf = presentation.map_or(authoritative, |pose| &pose.0);
    let angles = cam.smooth_angles(time.delta_secs_f64());
    let up = DVec3::Y;
    let dist = cam.zoom.exp() * 100.0;
    let rotation = DQuat::from_rotation_arc(DVec3::Y, up);
    let dir = rotation
        * DVec3::new(
            angles.x.sin() * angles.y.cos(),
            angles.y.sin(),
            angles.x.cos() * angles.y.cos(),
        );
    cam_ptf.translation_um = focus_ptf.translation_um + (dir * dist).to_micrometers();
    cam_ptf.look_at(focus_ptf.translation_um, up);
}

#[derive(Component)]
#[require(DirectionalLight)]
struct CameraLight;

/// Computes the stars lighting the main camera.
fn camera_lighting(
    origin: Res<FloatingOrigin>,
    camera: Single<&PreciseTransform, With<MainCamera>>,
    mut lights: Query<
        (&mut DirectionalLight, &mut Transform),
        (With<CameraLight>, Without<MainCamera>),
    >,
    universe: Res<Universe>,
) {
    let position = camera.translation_um;
    let brightest = universe
        .tree
        .brightest(position)
        .map(|id| &universe.tree.entries[id]);
    for (mut light, mut tf) in &mut lights {
        let Some(star) = brightest else {
            light.illuminance = 0.0;
            continue;
        };
        let direction = origin.0.rotation.inverse() * position.relative_to(star.position);
        light.illuminance = (star.luminosity
            / (4.0 * std::f64::consts::PI * direction.length_squared().max(star.radius.powi(2))))
            as f32;
        light.color = Color::WHITE;
        if let Some(direction) = direction.try_normalize() {
            tf.look_to(direction.as_vec3(), Vec3::Y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vessel::ControlledVessel;

    #[test]
    fn camera_tracks_presentation_without_modifying_simulation() {
        let mut app = App::new();
        app.init_resource::<Time>().add_systems(Update, camera_pose);
        let authoritative = PreciseTransform {
            translation_um: crate::precision::GalacticPosition::new(1_i128 << 90, 0, 0),
            ..default()
        };
        let presentation = PreciseTransform {
            translation_um: authoritative.translation_um
                - crate::precision::GalacticPosition::new(5_000_000_000, 0, 0),
            ..default()
        };
        let focus = app
            .world_mut()
            .spawn((CameraFocus, authoritative, PresentationPose(presentation)))
            .id();
        let camera = app
            .world_mut()
            .spawn((MainCamera, PreciseTransform::default()))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<PreciseTransform>(camera)
                .unwrap()
                .translation_um,
            presentation.translation_um
                + crate::precision::GalacticPosition::new(0, 0, 100_000_000)
        );
        assert_eq!(
            app.world()
                .get::<PreciseTransform>(focus)
                .unwrap()
                .translation_um,
            authoritative.translation_um
        );
    }

    #[test]
    fn orbit_smoothing_is_frame_rate_independent_and_preserves_initial_angles() {
        let make_camera = || {
            let mut camera = CameraParams {
                yaw: 0.7,
                pitch: 0.2,
                ..default()
            };
            assert_eq!(camera.smooth_angles(0.0), DVec2::new(0.7, 0.2));
            camera.yaw = 1.7;
            camera.pitch = -0.3;
            camera
        };
        let mut slow = make_camera();
        let mut fast = make_camera();
        for _ in 0..3 {
            slow.smooth_angles(1.0 / 30.0);
        }
        for _ in 0..12 {
            fast.smooth_angles(1.0 / 120.0);
        }
        let a = slow.smooth_angles(0.0);
        let b = fast.smooth_angles(0.0);
        assert!(a.abs_diff_eq(b, 1e-12));
        assert!(a.x > 0.7 && a.x < 1.7 && a.y < 0.2 && a.y > -0.3);
        assert!(
            slow.smooth_angles(1.0)
                .abs_diff_eq(DVec2::new(1.7, -0.3), 1e-10)
        );
    }

    #[test]
    fn camera_can_follow_planets_and_return_to_ship_without_changing_control() {
        let orrery = Universe::init(crate::orrery::example_config()).unwrap();
        let planet_cfg = orrery
            .iter()
            .find(|body| body.atmosphere.is_some())
            .unwrap();
        let radius = planet_cfg.radius;
        let name = planet_cfg.name.clone();
        let mut app = App::new();
        app.insert_resource(orrery)
            .init_resource::<Time>()
            .add_message::<FollowTarget>()
            .add_message::<bevy::input::mouse::MouseMotion>()
            .add_message::<MouseWheel>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(
                Update,
                (follow_target, camera_controls, camera_pose).chain(),
            );
        let camera = app
            .world_mut()
            .spawn((MainCamera, PreciseTransform::default()))
            .id();
        let ship = app
            .world_mut()
            .spawn((
                Vessel {
                    class_name: "test".into(),
                    vessel_name: "Test ship".into(),
                },
                ControlledVessel,
                CameraFocus,
                PreciseTransform::default(),
                AeroModel::new(DVec3::splat(5.0)),
            ))
            .id();
        let planet = app
            .world_mut()
            .spawn((Celestial(name), PreciseTransform::default()))
            .id();
        app.world_mut().write_message(FollowTarget(planet));
        app.update();
        assert!(app.world().get::<CameraFocus>(planet).is_some());
        assert!(app.world().get::<CameraFocus>(ship).is_none());
        assert!(app.world().get::<ControlledVessel>(ship).is_some());
        let distance = app
            .world()
            .get::<PreciseTransform>(camera)
            .unwrap()
            .translation_um
            .to_meters_64()
            .length();
        assert!((distance - radius * 3.0).abs() < 0.01);

        // A stale selection cannot clear the current target.
        let stale = app.world_mut().spawn_empty().id();
        app.world_mut().despawn(stale);
        app.world_mut().write_message(FollowTarget(stale));
        app.world_mut()
            .get_mut::<CameraParams>(camera)
            .unwrap()
            .zoom = -100.0;
        app.update();
        assert!(app.world().get::<CameraFocus>(planet).is_some());
        let distance = app
            .world()
            .get::<PreciseTransform>(camera)
            .unwrap()
            .translation_um
            .to_meters_64()
            .length();
        assert!(distance > radius);

        app.world_mut().write_message(FollowTarget(ship));
        app.update();
        assert!(app.world().get::<CameraFocus>(ship).is_some());
        assert!(app.world().get::<CameraFocus>(planet).is_none());
        let distance = app
            .world()
            .get::<PreciseTransform>(camera)
            .unwrap()
            .translation_um
            .to_meters_64()
            .length();
        assert!((distance - 30.0).abs() < 0.01);
    }
}
