use bevy::{
    camera::{Exposure, Hdr},
    mesh::MeshTag,
    post_process::bloom::Bloom,
    prelude::*,
};
use toy_sim_ship_view::thermal::{ThermalAssets, ThermalPlugin, ThermalSphere};

#[derive(Resource)]
struct Settings {
    temperature_k: f32,
    radius: f32,
    camera_distance: f32,
    strength: f32,
}

fn setting(name: &str, fallback: f32) -> f32 {
    let value = std::env::var(name)
        .map(|value| value.parse::<f32>().expect(name))
        .unwrap_or(fallback);
    assert!(value.is_finite(), "{name} must be finite");
    value
}

fn main() {
    let settings = Settings {
        temperature_k: setting("THERMAL_TEMPERATURE_K", 3500.0),
        radius: setting("THERMAL_RADIUS", 5.0),
        camera_distance: setting("THERMAL_CAMERA_DISTANCE", 14.0),
        strength: setting("THERMAL_STRENGTH", 1.0),
    };
    assert!(settings.radius > 0.0);
    assert!(settings.camera_distance > 0.0);
    assert!(settings.temperature_k >= 0.0);
    assert!((0.0..=1.0).contains(&settings.strength));

    App::new()
        .insert_resource(settings)
        .insert_resource(ClearColor(Color::srgb(0.008, 0.012, 0.018)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Thermal shield test".into(),
                resolution: (960, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(ThermalPlugin)
        .add_systems(PostStartup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    settings: Res<Settings>,
    thermal: Res<ThermalAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let direction = Vec3::new(1.0, 0.35, 1.0).normalize();
    commands.spawn((
        Camera3d::default(),
        Exposure::SUNLIGHT,
        Hdr,
        Bloom::NATURAL,
        Transform::from_translation(direction * settings.camera_distance)
            .looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(8.0, 12.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(
            settings.radius * 0.35,
            settings.radius * 0.24,
            settings.radius * 1.35,
        ))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.28, 0.42, 0.56),
            metallic: 0.2,
            perceptual_roughness: 0.65,
            ..default()
        })),
        Transform::default(),
    ));
    commands.spawn((
        ThermalSphere {
            temperature_k: settings.temperature_k,
            strength: settings.strength,
        },
        Mesh3d(thermal.mesh.clone()),
        MeshMaterial3d(thermal.material.clone()),
        MeshTag::default(),
        Transform::from_scale(Vec3::splat(settings.radius)),
        Visibility::default(),
    ));
}
