use std::cmp::Reverse;

use bevy::{
    core_pipeline::{
        auto_exposure::AutoExposure, bloom::Bloom, fxaa::Fxaa, smaa::Smaa, tonemapping::Tonemapping,
    },
    input::mouse::MouseWheel,
    pbr::{Atmosphere, AtmosphereSettings, CascadeShadowConfigBuilder},
    prelude::*,
    render::camera::Exposure,
};

use ordered_float::OrderedFloat;

use crate::{
    GameState,
    orrery::{BodyClass, Celestial, Orrery, Star},
    physics::WithinSoi,
    precision::{FloatingOrigin, PreciseTransform, ToMetersExt, ToMillimetersExt},
};
use bevy::math::{DQuat, DVec3};

pub struct MainCameraPlugin;

#[derive(Component)]
#[require(CameraParams)]
pub struct MainCamera;

#[derive(Component)]
pub struct CameraFocus;

impl Plugin for MainCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Game), |mut commands: Commands| {
            let k = (10_000.0f32).ln() / 144_000.0; // ≈ 6.14e-5
            commands.spawn((
                MainCamera,
                CameraParams::default(),
                Camera3d::default(),
                Camera {
                    hdr: true,
                    ..default()
                },
                Tonemapping::TonyMcMapface,
                PreciseTransform::default(),
                Smaa::default(),
                Msaa::Off,
                // ClusterConfig::Single, // NECESSARY FOR DISTANT LIGHTING
                // MotionBlur {
                //     shutter_angle: 1.0,
                //     samples: 10,
                // },
                Exposure::OVERCAST,
                Bloom::ANAMORPHIC,
                // AtmosphereCamera::default(),
                Atmosphere {
                    // hardcoded values for Taale,
                    bottom_radius: 1.65800e7,
                    top_radius: 1.65800e7 + 250e3,
                    rayleigh_density_exp_scale: k,
                    // mie_density_exp_scale: k * 0.95,
                    ..Atmosphere::EARTH.with_density_multiplier(10000.0)
                },
                AtmosphereSettings {
                    // transmittance_lut_size: UVec2::new(512, 128),
                    // sky_view_lut_size: UVec2::new(768, 192),
                    // aerial_view_lut_size: UVec3::new(160, 96, 96),

                    // // integration samples
                    // transmittance_lut_samples: 512,
                    // multiscattering_lut_dirs: 64,
                    // multiscattering_lut_samples: 128,
                    // sky_view_lut_samples: 256,
                    // aerial_view_lut_samples: 128,

                    // how far we integrate fog from the camera
                    aerial_view_lut_max_distance: 4.0e6, // 4 000 km
                    scene_units_to_m: 100.0,
                    ..default()
                },
                // AutoExposure::default(),
                Projection::Perspective(PerspectiveProjection {
                    near: 0.1,
                    far: 1e15,
                    ..default()
                }),
            ));

            commands.spawn((
                CameraLight,
                CascadeShadowConfigBuilder {
                    num_cascades: 4,
                    minimum_distance: 0.1,
                    maximum_distance: 100000.0,
                    ..default()
                }
                .build(),
                DirectionalLight {
                    shadows_enabled: true,
                    ..default()
                },
            ));
        });

        app.add_systems(
            FixedPostUpdate,
            (camera_controls, atmo_and_float_origin, camera_lighting)
                .chain()
                .run_if(in_state(GameState::Game)),
        );
    }
}

#[derive(Component, Default)]
pub struct CameraParams {
    pub zoom: f64,
    pub yaw: f64,
    pub pitch: f64,
    pub mode: CameraMode,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Orbit,
    #[default]
    WarThunderLike,
}

/// Orbit camera relative to focused object
fn camera_controls(
    camera: Single<(&mut PreciseTransform, &mut CameraParams), With<MainCamera>>,
    focus: Single<
        (&PreciseTransform, Option<&WithinSoi>),
        (With<CameraFocus>, Without<MainCamera>),
    >,
    celestials: Query<&PreciseTransform, (With<Celestial>, Without<MainCamera>)>,
    mut mouse_evs: EventReader<bevy::input::mouse::MouseMotion>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut scroll_evs: EventReader<MouseWheel>,
) {
    const SENS: f64 = 0.01;
    const ZOOM_SENS: f64 = 100.0;

    let (focus_ptf, soi_opt) = focus.into_inner();
    // Determine the "up" vector for the current local horizon.
    let up: DVec3 = if let Some(WithinSoi(body_ent)) = soi_opt {
        let cel_tf = celestials.get(*body_ent).unwrap();
        let delta_m = (focus_ptf.translation_mm - cel_tf.translation_mm).to_meters_64();
        delta_m.normalize()
    } else {
        DVec3::Y
    };

    let (mut cam_ptf, mut cam) = camera.into_inner();

    match cam.mode {
        CameraMode::Orbit => {
            if mouse_buttons.pressed(MouseButton::Left) {
                for ev in mouse_evs.read() {
                    let yaw = -(ev.delta.x as f64) * SENS;
                    let pitch = (ev.delta.y as f64) * SENS;
                    cam.yaw += yaw;
                    cam.pitch += pitch;
                }
            }
        }
        CameraMode::WarThunderLike => {
            for ev in mouse_evs.read() {
                let yaw = -(ev.delta.x as f64) * SENS;
                let pitch = (ev.delta.y as f64) * SENS;
                cam.yaw += yaw;
                cam.pitch += pitch;
            }
        }
    }
    cam.pitch = cam.pitch.clamp(-1.5, 1.5);

    // Zoom wheel
    for ev in scroll_evs.read() {
        cam.zoom -= ev.y as f64 * 0.05;
    }

    // Offset along forward based on zoom
    let dist = cam.zoom.exp() * ZOOM_SENS;
    let rotation = DQuat::from_rotation_arc(DVec3::Y, up);
    let dir = rotation
        * DVec3::new(
            cam.yaw.sin() * cam.pitch.cos(),
            cam.pitch.sin(),
            cam.yaw.cos() * cam.pitch.cos(),
        );
    cam_ptf.translation_mm = focus_ptf.translation_mm + (dir * dist).to_millimeters();
    cam_ptf.look_at(focus_ptf.translation_mm, up);
}

#[derive(Component)]
#[require(DirectionalLight)]
struct CameraLight;

/// Computes the stars lighting the main camera.
fn camera_lighting(
    origin: Res<FloatingOrigin>,
    camera: Single<(&PreciseTransform, &Transform), With<MainCamera>>,
    mut lights: Query<
        (&mut DirectionalLight, &mut Transform),
        (With<CameraLight>, Without<MainCamera>),
    >,
    stars: Query<(&Star, &PreciseTransform)>,
) {
    let (camera_ptf, camera_tf) = camera.into_inner();
    // we assign lights to stars from brightest to least brightest
    // TODO relative brightness instead of absolute
    for ((star, star_ptf), (mut light, mut light_tf)) in stars
        .iter()
        .sort_unstable_by_key::<(&Star, &PreciseTransform), _>(|s| {
            Reverse(OrderedFloat(s.0.lumens))
        })
        .zip(lights.iter_mut())
    {
        // recompute lighting every once in a while
        // if camera_tf.translation.distance(light_tf.translation) > 100.0 {
        let camera_loc = origin.project_loc(camera_ptf.translation_mm);
        let star_loc = origin.project_loc(star_ptf.translation_mm);
        let star_to_camera = camera_loc - star_loc;
        light.illuminance =
            star.lumens as f32 / (4.0 * std::f32::consts::PI * star_to_camera.length_squared());
        light.color = Color::WHITE;
        light.shadows_enabled = true;
        // light_tf.translation = camera_tf.translation; // this centers the shadow-enabled area properly
        light_tf.look_at(star_to_camera, Vec3::Y);
        // dbg!(light_tf);
        // }
    }
}

/// Compute the floating origin and spawn. Currently, it's always the closest planet's closest surface.
fn atmo_and_float_origin(
    star_sys: Res<Orrery>,
    mut origin: ResMut<FloatingOrigin>,
    camera: Single<&PreciseTransform, With<MainCamera>>,
    cel: Query<(&Celestial, &PreciseTransform)>,
) {
    let camera_ptf = camera.into_inner();
    // Find the planet whose surface is nearest the camera
    let mut min_dist_surface = f64::MAX;
    let mut best_origin = *camera_ptf;

    for (cel_body, body_pt) in cel.iter() {
        let body = star_sys.get_body(&cel_body.0).unwrap();
        if let BodyClass::Planet = body.class_params {
            // Vector planet-centre → camera in metres.
            let delta_m = (camera_ptf.translation_mm - body_pt.translation_mm).to_meters_64();
            let dist_center = delta_m.length();
            let radius = body.radius; // planet radius (m)
            let dist_surface = (dist_center - radius).abs(); // camera altitude over surface

            if dist_surface < min_dist_surface {
                min_dist_surface = dist_surface;

                // “Up” direction (unit vector, away from planet).
                let up_dir = delta_m / dist_center;

                // Choose altitude for the floating origin: 100x smaller than the real origin
                let origin_alt_m = dist_surface * 0.99;

                // Position = planet centre + up_dir * (radius + origin_alt_m)
                let origin_mm = body_pt
                    .translation_mm
                    .saturating_add((up_dir * (radius + origin_alt_m)).to_millimeters());

                // Align local Y to the up direction.
                let rotation = DQuat::from_rotation_arc(DVec3::Y, up_dir);

                best_origin = PreciseTransform {
                    translation_mm: origin_mm,
                    rotation,
                };
            }
        }
    }

    origin.0 = best_origin;
}
