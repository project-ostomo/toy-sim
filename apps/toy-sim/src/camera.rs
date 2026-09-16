use bevy::{
    anti_alias::fxaa::Fxaa,
    camera::{Exposure, Hdr},
    core_pipeline::tonemapping::Tonemapping,
    input::mouse::MouseWheel,
    light::CascadeShadowConfigBuilder,
    pbr::{AtmosphereMode, AtmosphereSettings},
    post_process::bloom::Bloom,
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
            (|mut commands: Commands| {
                let scenario = &crate::scenario::INITIAL_SCENARIO;
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
                    Msaa::Off,
                    Fxaa::default(),
                    Exposure::default(),
                    Bloom::NATURAL,
                    AtmosphereSettings {
                        rendering_method: AtmosphereMode::Raymarched,
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
    pub navigation_focus: Option<crate::precision::GalacticPosition>,
    saved_navigation_view: Option<(f64, f64, f64, Option<DVec2>)>,
}

impl CameraParams {
    pub fn frame_navigation(
        &mut self,
        focus: crate::precision::GalacticPosition,
        distance: f64,
        direction: Option<DVec3>,
    ) {
        self.saved_navigation_view.get_or_insert((
            self.zoom,
            self.yaw,
            self.pitch,
            self.smoothed_angles,
        ));
        self.navigation_focus = Some(focus);
        self.zoom = (distance.clamp(1., 1e15) / 100.).ln();
        if let Some(dir) = direction {
            self.yaw = dir.x.atan2(dir.z);
            self.pitch = dir.y.asin().clamp(-1.5, 1.5);
        }
        self.smoothed_angles = None;
    }
    pub fn return_from_navigation(&mut self) {
        self.navigation_focus = None;
        if let Some((zoom, yaw, pitch, smooth)) = self.saved_navigation_view.take() {
            self.zoom = zoom;
            self.yaw = yaw;
            self.pitch = pitch;
            self.smoothed_angles = smooth;
        }
    }

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
    let mut camera = camera.into_inner();
    let was_navigation = camera.navigation_focus.take().is_some();
    camera.saved_navigation_view = None;
    if !was_navigation && focused.iter().any(|entity| entity == request.0) {
        return;
    }
    for entity in &focused {
        commands.entity(entity).remove::<CameraFocus>();
    }
    commands.entity(request.0).insert(CameraFocus);
    let radius = target_radius(celestial, model, &orrery);
    camera.zoom = ((radius * 3.0).max(30.0) / 100.0).ln();
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
    mut contexts: Query<&mut bevy_egui::EguiContext>,
    mut gui_drag: Local<bool>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    const SENS: f64 = 0.0025;
    const ZOOM_SENS: f64 = 100.0;

    let (_, celestial, model) = focus.into_inner();
    let mut cam = camera.into_inner();
    let captured = contexts.iter_mut().any(|mut c| {
        let ctx = c.get_mut();
        // Input arrives before this frame's egui pass. Hit-test the current cursor
        // against retained UI geometry as well as egui's previous pointer state.
        let over_ui = windows
            .iter()
            .filter_map(|w| w.cursor_position().map(|p| (p, w.scale_factor())))
            .any(|(p, scale)| {
                ctx.layer_id_at(bevy_egui::egui::pos2(
                    p.x * scale / ctx.pixels_per_point(),
                    p.y * scale / ctx.pixels_per_point(),
                ))
                .is_some_and(|layer| layer.order >= bevy_egui::egui::Order::Middle)
            });
        ctx.egui_wants_pointer_input() || over_ui
    });
    if mouse_buttons.just_pressed(MouseButton::Left) {
        *gui_drag = captured;
    }
    if !mouse_buttons.pressed(MouseButton::Left) {
        *gui_drag = false;
    }

    for ev in mouse_evs.read() {
        if mouse_buttons.pressed(MouseButton::Left) && !captured && !*gui_drag {
            cam.yaw -= ev.delta.x as f64 * SENS;
            cam.pitch -= ev.delta.y as f64 * SENS;
        }
    }
    cam.pitch = cam.pitch.clamp(-1.5, 1.5);

    // Zoom wheel
    for ev in scroll_evs.read() {
        if !captured {
            cam.zoom -= ev.y as f64 * 0.05;
        }
    }

    let radius = target_radius(celestial, model, &orrery);
    let min_distance = if cam.navigation_focus.is_some() {
        1.0
    } else {
        (radius * 1.01).max(1.0)
    };
    cam.zoom = cam.zoom.max((min_distance / ZOOM_SENS).ln());
}

/// Finalize the camera after input and simulation, before selecting the render origin.
fn camera_pose(
    camera: Single<(&mut PreciseTransform, &mut CameraParams), With<MainCamera>>,
    focus: Single<
        (&PreciseTransform, Option<&PresentationPose>),
        (With<CameraFocus>, Without<MainCamera>),
    >,
    time: Res<Time<Real>>,
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
    let focus_position = cam.navigation_focus.unwrap_or(focus_ptf.translation_um);
    cam_ptf.translation_um = focus_position + (dir * dist).to_micrometers();
    cam_ptf.look_at(focus_position, up);
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
    fn drag_started_in_ui_and_wheel_do_not_move_camera_even_after_leaving_panel() {
        use bevy_egui::{EguiContext, egui};
        let mut app = App::new();
        app.insert_resource(Universe::init(crate::orrery::example_config()).unwrap())
            .add_message::<bevy::input::mouse::MouseMotion>()
            .add_message::<MouseWheel>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, camera_controls);
        app.world_mut()
            .spawn((CameraFocus, PreciseTransform::default()));
        let camera = app.world_mut().spawn(MainCamera).id();
        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(50., 50.)));
        let mut context = EguiContext::default();
        let ctx = context.get_mut();
        for _ in 0..2 {
            ctx.begin_pass(egui::RawInput::default());
            egui::Area::new(egui::Id::new("test-ui"))
                .fixed_pos(egui::pos2(0., 0.))
                .show(ctx, |ui| {
                    ui.allocate_exact_size(egui::vec2(200., 200.), egui::Sense::drag());
                });
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
        }
        assert!(ctx.layer_id_at(egui::pos2(50., 50.)).is_some());
        let window = app
            .world_mut()
            .spawn((window, bevy::window::PrimaryWindow, context))
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .write_message(bevy::input::mouse::MouseMotion {
                delta: Vec2::splat(20.),
            });
        app.world_mut().write_message(MouseWheel {
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            x: 0.,
            y: 2.,
            window,
            phase: bevy::input::touch::TouchPhase::Moved,
        });
        app.update();
        let cam = app.world().get::<CameraParams>(camera).unwrap();
        assert_eq!((cam.yaw, cam.pitch, cam.zoom), (0., 0., 0.));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_cursor_position(Some(Vec2::new(500., 500.)));
        app.world_mut()
            .write_message(bevy::input::mouse::MouseMotion {
                delta: Vec2::splat(20.),
            });
        app.update();
        assert_eq!(app.world().get::<CameraParams>(camera).unwrap().yaw, 0.);
    }

    #[test]
    fn navigation_presets_restore_the_original_camera_exactly() {
        let mut camera = CameraParams {
            zoom: 3.,
            yaw: 0.5,
            pitch: -0.2,
            ..default()
        };
        camera.smooth_angles(0.1);
        let angles = camera.smoothed_angles;
        camera.frame_navigation(
            crate::precision::GalacticPosition::default(),
            1e8,
            Some(DVec3::Y),
        );
        camera.frame_navigation(
            crate::precision::GalacticPosition::new(1, 2, 3),
            1000.,
            None,
        );
        camera.return_from_navigation();
        assert_eq!((camera.zoom, camera.yaw, camera.pitch), (3., 0.5, -0.2));
        assert_eq!(camera.smoothed_angles, angles);
        assert!(camera.navigation_focus.is_none());
    }

    #[test]
    fn camera_tracks_presentation_without_modifying_simulation() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .add_systems(Update, camera_pose);
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
    fn camera_smoothing_uses_real_time_at_every_simulation_speed() {
        let mut poses = Vec::new();
        for speed in [0.0, 1.0, 10.0] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                    std::time::Duration::from_millis(20),
                ))
                .add_systems(Update, camera_pose);
            if speed == 0.0 {
                app.world_mut().resource_mut::<Time<Virtual>>().pause();
            } else {
                app.world_mut()
                    .resource_mut::<Time<Virtual>>()
                    .set_relative_speed(speed);
            }
            app.world_mut()
                .spawn((CameraFocus, PreciseTransform::default()));
            let camera = app
                .world_mut()
                .spawn((MainCamera, PreciseTransform::default()))
                .id();
            app.update();
            app.world_mut().get_mut::<CameraParams>(camera).unwrap().yaw = 1.0;
            app.update();
            poses.push(
                app.world()
                    .get::<PreciseTransform>(camera)
                    .unwrap()
                    .translation_um,
            );
        }
        assert_ne!(poses[0].x, 0); // Drag smoothing still advances while paused.
        assert!(poses.iter().all(|pose| *pose == poses[0]));
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
            .init_resource::<Time<Real>>()
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
        app.world_mut()
            .get_mut::<CameraParams>(camera)
            .unwrap()
            .frame_navigation(
                crate::precision::GalacticPosition::new(0, 0, 1_000_000_000),
                1e8,
                None,
            );
        // Selecting the already-focused ship must exit the orbital preset and frame its hull.
        app.world_mut().write_message(FollowTarget(ship));
        app.update();
        let params = app.world().get::<CameraParams>(camera).unwrap();
        assert!(params.navigation_focus.is_none());
        assert!((params.zoom.exp() * 100. - 30.).abs() < 0.01);
    }
}
