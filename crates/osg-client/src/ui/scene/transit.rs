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
use osg_model::travel::Presence;
use std::collections::HashSet;

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
    slip: Handle<TransitMaterial>,
    wall: Handle<StandardMaterial>,
    strip: Handle<StandardMaterial>,
    hangar: Handle<Mesh>,
    ring: Handle<Mesh>,
}
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
    effects: Res<Effects>,
    mut materials: ResMut<Assets<TransitMaterial>>,
    views: Query<(Entity, &ViewCamera, &ViewObservation)>,
    owned: Query<(&OwnedShip, &DisplayPose)>,
    mut rooms: Query<(Entity, &ViewMember, &Chamber, &mut Transform)>,
) {
    let time = (clock.display_ns as f64 * 1e-9 % 10000.) as f32;
    materials.get_mut(&effects.slip).unwrap().parameters.x = time;
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
    }
    for (entity, ..) in &rooms {
        if !retained_rooms.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
}
