use super::{ViewCamera, ViewLayer, ViewMember};
use crate::state::{DisplayPose, OwnedShip, PresentationSet, RenderTime, ViewObservation};
use bevy::{
    asset::embedded_asset,
    camera::visibility::RenderLayers,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};
use osg_model::{Id, travel::Presence};
use std::collections::{HashMap, HashSet};

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct TransitMaterial {
    #[uniform(0)]
    parameters: Vec4,
}
impl Material for TransitMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://osg_client/ui/scene/transit.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        Self::vertex_shader()
    }
    fn alpha_mode(&self) -> AlphaMode {
        if self.parameters.y > 0.5 {
            AlphaMode::Opaque
        } else {
            AlphaMode::Blend
        }
    }
    fn enable_shadows() -> bool {
        false
    }
    fn enable_prepass() -> bool {
        false
    }
    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}
#[derive(Resource)]
struct Effects {
    sphere: Handle<Mesh>,
    gate: Handle<TransitMaterial>,
    slip: Handle<TransitMaterial>,
    wall: Handle<StandardMaterial>,
    strip: Handle<StandardMaterial>,
    hangar: Handle<Mesh>,
    ring: Handle<Mesh>,
}
#[derive(Component)]
struct GateMesh(Id);
#[derive(Component)]
struct Chamber(bool);

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "transit.wgsl");
    app.add_plugins(MaterialPlugin::<TransitMaterial>::default())
        .add_systems(Startup, setup)
        .add_systems(Update, update.in_set(PresentationSet::Render));
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TransitMaterial>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(Effects {
        sphere: meshes.add(Sphere::new(1.).mesh().uv(96, 64)),
        gate: materials.add(TransitMaterial {
            parameters: Vec4::new(0., 0., 1.8, 0.),
        }),
        slip: materials.add(TransitMaterial {
            parameters: Vec4::new(0., 1., 1., 0.),
        }),
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
    clock: Res<RenderTime>,
    beacons: Query<(&crate::state::NavigationObject, &DisplayPose)>,
    effects: Res<Effects>,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<TransitMaterial>>,
    views: Query<(Entity, &ViewCamera, &ViewObservation)>,
    owned: Query<(&OwnedShip, &DisplayPose)>,
    mut gates: Query<(Entity, &ViewMember, &GateMesh, &mut Transform)>,
    mut rooms: Query<(Entity, &ViewMember, &Chamber, &mut Transform), Without<GateMesh>>,
) {
    let time = (clock.display_ns as f64 * 1e-9 % 10000.) as f32;
    materials.get_mut(&effects.gate).unwrap().parameters.x = time;
    materials.get_mut(&effects.slip).unwrap().parameters.x = time;
    let existing: HashMap<_, _> = gates
        .iter()
        .map(|(entity, member, gate, _)| ((member.0, gate.0), entity))
        .collect();
    let mut retained = HashSet::new();
    let mut retained_rooms = HashSet::new();
    for (view, camera, observation) in &views {
        let focused = owned
            .iter()
            .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship);
        let private = focused.is_some_and(|(ship, _)| {
            matches!(
                ship.0.presence,
                Presence::Docked { .. } | Presence::SlipTransit(_)
            )
        });
        if private {
            let (ship, pose) = focused.unwrap();
            let slip = matches!(ship.0.presence, Presence::SlipTransit(_));
            let rotation = Quat::from_array(pose.0.rotation.map(|n| n as f32));
            let transform =
                Transform::from_translation(pose.0.position.relative_to(camera.origin).as_vec3())
                    .with_rotation(rotation);
            if let Some((entity, _, _, mut current)) = rooms
                .iter_mut()
                .find(|(_, member, room, _)| member.0 == view && room.0 == slip)
            {
                *current = transform;
                retained_rooms.insert(entity);
            } else {
                let radius = (ship.0.radius_m as f32 * 15.).max(150.);
                let entity = commands
                    .spawn((
                        Chamber(slip),
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
                            illuminance: if slip { 1200000. } else { 180000. },
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
                            illuminance: if slip { 450000. } else { 90000. },
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(-1., -1., -2.).looking_at(Vec3::ZERO, Vec3::Y),
                    ));
                    if slip {
                        parent.spawn((
                            Mesh3d(effects.sphere.clone()),
                            MeshMaterial3d(effects.slip.clone()),
                            Transform::from_scale(Vec3::splat(10000.)),
                        ));
                    } else {
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
        for (gate, pose) in &beacons {
            let gate = &gate.0;
            if gate.gate_exit.is_none() {
                continue;
            }
            let relative = pose.0.position.relative_to(camera.origin);
            if relative.length() > 1e7 {
                continue;
            }
            let transform = Transform::from_translation(relative.as_vec3());
            let entity = if let Some(&entity) = existing.get(&(view, gate.id)) {
                if let Ok((_, _, _, mut current)) = gates.get_mut(entity) {
                    *current = transform;
                }
                entity
            } else {
                let entity = commands
                    .spawn((
                        GateMesh(gate.id),
                        ViewMember(view),
                        ViewLayer(camera.layer),
                        RenderLayers::layer(camera.layer),
                        transform,
                        Visibility::default(),
                    ))
                    .id();
                commands.entity(entity).with_children(|parent| {
                    parent.spawn(WorldAssetRoot(
                        server.load("models/stations/wormhole-frame-512m.glb#Scene0"),
                    ));
                    parent.spawn((
                        Mesh3d(effects.sphere.clone()),
                        MeshMaterial3d(effects.gate.clone()),
                        Transform::from_scale(Vec3::splat(gate.radius_m as f32)),
                    ));
                    parent.spawn((
                        PointLight {
                            color: Color::srgb(0.25, 0.55, 1.),
                            intensity: 2e10,
                            range: gate.radius_m as f32 * 2.,
                            radius: 80.,
                            shadow_maps_enabled: true,
                            ..default()
                        },
                        Transform::default(),
                    ));
                });
                entity
            };
            retained.insert(entity);
        }
    }
    for (entity, ..) in &gates {
        if !retained.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, ..) in &rooms {
        if !retained_rooms.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
}
