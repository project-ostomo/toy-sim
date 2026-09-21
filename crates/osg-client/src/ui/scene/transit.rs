use super::{ViewCamera, ViewLayer, ViewMember};
use crate::state::{DisplayPose, OwnedShip, PresentationSet, ViewObservation};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use osg_model::travel::Presence;
use std::collections::HashSet;

#[derive(Resource)]
struct Effects {
    wall: Handle<StandardMaterial>,
    strip: Handle<StandardMaterial>,
    hangar: Handle<Mesh>,
    ring: Handle<Mesh>,
}
#[derive(Component)]
struct Chamber;

pub(super) fn install(app: &mut App) {
    app.add_systems(Startup, setup)
        .add_systems(Update, update.in_set(PresentationSet::Render));
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(Effects {
        wall: standard.add(StandardMaterial {
            base_color: Color::srgb(0.09, 0.14, 0.19),
            metallic: 0.6,
            perceptual_roughness: 0.7,
            cull_mode: None,
            double_sided: true,
            ..default()
        }),
        strip: standard.add(StandardMaterial {
            base_color: Color::srgb(0.4, 0.7, 0.9),
            emissive: LinearRgba::rgb(7000., 18000., 30000.),
            ..default()
        }),
        hangar: meshes.add(Cylinder::new(1., 2.).mesh().resolution(48)),
        ring: meshes.add(Torus::new(0.992, 1.)),
    });
}
fn update(
    mut commands: Commands,
    effects: Res<Effects>,
    views: Query<(Entity, &ViewCamera, &ViewObservation)>,
    owned: Query<(&OwnedShip, &DisplayPose)>,
    mut rooms: Query<(Entity, &ViewMember, &Chamber, &mut Transform)>,
) {
    let mut retained_rooms = HashSet::new();
    for (view, camera, observation) in &views {
        let focused = owned
            .iter()
            .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship);
        let private =
            focused.is_some_and(|(ship, _)| matches!(ship.0.presence, Presence::Docked { .. }));
        if private {
            let (ship, pose) = focused.unwrap();
            let rotation = Quat::from_array(pose.0.rotation.map(|n| n as f32));
            let transform =
                Transform::from_translation(pose.0.position.relative_to(camera.origin).as_vec3())
                    .with_rotation(rotation);
            if let Some((entity, _, _, mut current)) =
                rooms.iter_mut().find(|(_, member, _, _)| member.0 == view)
            {
                *current = transform;
                retained_rooms.insert(entity);
            } else {
                let radius = (ship.0.radius_m as f32 * 15.).max(150.);
                let entity = commands
                    .spawn((
                        Chamber,
                        ViewMember(view),
                        ViewLayer(camera.layer),
                        RenderLayers::layer(camera.layer),
                        transform,
                        Visibility::default(),
                    ))
                    .id();
                commands.entity(entity).with_children(|parent| {
                    parent.spawn((
                        RenderLayers::layer(camera.layer),
                        bevy::light::SunDisk::OFF,
                        DirectionalLight {
                            color: Color::srgb(0.6, 0.8, 1.),
                            illuminance: 180000.,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(1., 2., 3.).looking_at(Vec3::ZERO, Vec3::Y),
                    ));
                    parent.spawn((
                        RenderLayers::layer(camera.layer),
                        bevy::light::SunDisk::OFF,
                        DirectionalLight {
                            color: Color::srgb(0.3, 0.5, 1.),
                            illuminance: 90000.,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(-1., -1., -2.).looking_at(Vec3::ZERO, Vec3::Y),
                    ));
                    {
                        parent.spawn((
                            Mesh3d(effects.hangar.clone()),
                            MeshMaterial3d(effects.wall.clone()),
                            Transform::from_rotation(Quat::from_rotation_x(
                                std::f32::consts::FRAC_PI_2,
                            ))
                            .with_scale(Vec3::new(radius, radius, radius)),
                        ));
                        for z in [-0.8, -0.4, 0., 0.4, 0.8] {
                            parent.spawn((
                                Mesh3d(effects.ring.clone()),
                                MeshMaterial3d(effects.strip.clone()),
                                Transform::from_xyz(0., 0., z * radius)
                                    .with_rotation(Quat::from_rotation_x(
                                        std::f32::consts::FRAC_PI_2,
                                    ))
                                    .with_scale(Vec3::splat(radius * 0.995)),
                            ));
                        }
                        for (position, color) in [
                            (Vec3::new(0.4, 0.5, 0.3), Color::srgb(0.6, 0.8, 1.)),
                            (Vec3::new(-0.3, -0.2, -0.4), Color::srgb(1., 0.7, 0.4)),
                        ] {
                            parent.spawn((
                                RenderLayers::layer(camera.layer),
                                PointLight {
                                    color,
                                    intensity: 2e9,
                                    range: radius * 2.,
                                    shadow_maps_enabled: true,
                                    ..default()
                                },
                                Transform::from_translation(position * radius),
                            ));
                        }
                    }
                });
                retained_rooms.insert(entity);
            }
            continue;
        }
    }
    for (entity, ..) in &rooms {
        if !retained_rooms.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
}
