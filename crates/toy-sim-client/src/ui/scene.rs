use crate::assets::{Appearance, ShipDesign};
mod atmosphere;
mod camera;
mod glints;
mod lighting;
mod navigation_hud;
mod projection;
mod transit;
pub(super) use camera::{CameraOptions, LOOK_AT_RANGE_M, ViewCamera};
pub(super) use orbit::ViewOptions;
mod combat;
mod orbit;
mod sensor_hud;
mod shield;
mod sky;
mod surfaces;

use crate::state::{
    Celestial, CelestialSystem, DisplayPose, DisplayVisual, Optical, OwnedShip, PresentationSet,
    SystemSubscription, ViewObservation,
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use std::collections::{HashMap, HashSet};
use toy_sim_model::GalacticPosition;
use toy_sim_ship_view::PartVisualAssets;
use toy_sim_ships::CompiledShipDesign;

#[derive(Component)]
#[relationship(relationship_target = ViewMembers)]
pub(super) struct ViewMember(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = ViewMember, linked_spawn)]
pub(super) struct ViewMembers(Vec<Entity>);

#[derive(Component)]
#[relationship(relationship_target = SourceInstances)]
struct RenderSource(Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = RenderSource, linked_spawn)]
struct SourceInstances(Vec<Entity>);

#[derive(Component)]
struct ShipMesh {
    appearance: [u8; 32],
}

#[derive(Component)]
struct BodyMesh;

#[derive(Component)]
struct ViewLayer(usize);

#[derive(Component)]
struct Shield;

pub(super) fn install(app: &mut App) {
    app.add_plugins(toy_sim_ship_view::mechanisms::MechanismPlugin)
        .add_systems(Update, mechanism_time.in_set(PresentationSet::Render))
        .insert_resource(GlobalAmbientLight::NONE)
        .add_systems(Startup, setup_ui_camera)
        .add_systems(
            Update,
            (
                camera::setup_views,
                camera::update_views,
                camera::camera_controls,
            )
                .chain()
                .in_set(PresentationSet::Views),
        )
        .add_systems(
            Update,
            (
                sync_ships,
                sync_celestials,
                own_visuals,
                apply_visuals,
                shield::update_flashes,
            )
                .chain()
                .in_set(PresentationSet::Render),
        )
        .add_systems(PostUpdate, propagate_layers);
    app.add_systems(
        toy_sim_ui::bevy_egui::EguiPrimaryContextPass,
        camera::align_on_double_click,
    );
    lighting::install(app);
    surfaces::install(app);
    glints::install(app);
    transit::install(app);
    navigation_hud::install(app);
    sky::install(app);
    combat::install(app);
    atmosphere::install(app);
    orbit::install(app);
    sensor_hud::install(app);
}

fn setup_ui_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        toy_sim_ui::bevy_egui::PrimaryEguiContext,
        Camera {
            order: 100,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: Some(bevy::render::render_resource::BlendState::ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            ..default()
        },
        RenderLayers::none(),
    ));
}

fn sun_direction(
    origin: GalacticPosition,
    stars: impl Iterator<Item = (f64, GalacticPosition)>,
) -> Option<Vec3> {
    stars
        .filter_map(|(luminosity, position)| {
            let direction = position.relative_to(origin);
            (luminosity > 0.)
                .then_some((luminosity / direction.length_squared().max(1.), direction))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .and_then(|(_, direction)| direction.try_normalize())
        .map(|direction| direction.as_vec3())
}

fn sync_ships(
    mut commands: Commands,
    views: Query<
        (
            Entity,
            &ViewObservation,
            &ViewCamera,
            &Camera,
            &Projection,
            &Transform,
        ),
        Without<ShipMesh>,
    >,
    optical: Query<(Entity, &Optical, &DisplayPose, Option<&Appearance>)>,
    owned: Query<(
        Entity,
        &OwnedShip,
        Option<&DisplayPose>,
        Option<&Appearance>,
    )>,
    mut objects: Query<(
        Entity,
        &ViewMember,
        &RenderSource,
        &ShipMesh,
        &mut Transform,
    )>,
    designs: Res<Assets<ShipDesign>>,
    assets: Res<PartVisualAssets>,
    loader: Res<AssetServer>,
    thermal: Res<toy_sim_ship_view::thermal::ThermalAssets>,
) {
    let existing: HashMap<_, _> = objects
        .iter()
        .map(|(entity, view, source, _, _)| ((view.0, source.0), entity))
        .collect();
    let mut visible = HashSet::new();
    for (view_entity, observation, camera, render_camera, projection, camera_transform) in &views {
        let view = &observation.0;
        let height = render_camera
            .physical_viewport_size()
            .map_or(1080., |size| size.y.max(1) as f64);
        let fov = match projection {
            Projection::Perspective(projection) => projection.fov as f64,
            _ => 1.,
        };
        let private_view = owned
            .iter()
            .find(|(_, ship, _, _)| Some(ship.0.ship) == view.focused_ship)
            .is_some_and(|(_, ship, _, _)| {
                ship.0.presence != toy_sim_model::travel::Presence::Space
            });
        for (source, ship, pose, appearance) in &owned {
            if Some(ship.0.ship) != view.focused_ship
                || matches!(
                    ship.0.presence,
                    toy_sim_model::travel::Presence::Destroyed
                        | toy_sim_model::travel::Presence::StoredInWreck(_)
                )
            {
                continue;
            }
            if appearance.is_none() {
                commands.entity(source).insert(crate::assets::MeshDemand);
            }
            let (Some(pose), Some(appearance)) = (pose, appearance) else {
                continue;
            };
            let Some(design) = designs.get(&appearance.design) else {
                continue;
            };
            let transform =
                Transform::from_translation(pose.0.position.relative_to(camera.origin).as_vec3())
                    .with_rotation(Quat::from_array(pose.0.rotation.map(|v| v as f32)));
            let entity = if let Some((entity, _, _, mesh, mut current)) = existing
                .get(&(view_entity, source))
                .and_then(|e| objects.get_mut(*e).ok())
            {
                if mesh.appearance == appearance.hash {
                    *current = transform;
                    entity
                } else {
                    spawn_ship(
                        &mut commands,
                        &design.0,
                        &assets,
                        &loader,
                        &thermal,
                        transform,
                    )
                }
            } else {
                spawn_ship(
                    &mut commands,
                    &design.0,
                    &assets,
                    &loader,
                    &thermal,
                    transform,
                )
            };
            commands.entity(entity).insert((
                ViewMember(view_entity),
                RenderSource(source),
                ShipMesh {
                    appearance: appearance.hash,
                },
                ViewLayer(camera.layer),
                RenderLayers::layer(camera.layer),
            ));
            if private_view {
                commands.entity(entity).remove::<glints::MeshLod>();
            } else {
                let offset = pose.0.position.relative_to(camera.origin)
                    - camera_transform.translation.as_dvec3();
                let forward = camera_transform.rotation.as_dquat() * bevy::math::DVec3::NEG_Z;
                commands.entity(entity).insert(glints::MeshLod::at(
                    ship.0.radius_m,
                    offset.length(),
                    offset.dot(forward),
                    height,
                    fov,
                ));
            }
            visible.insert(entity);
        }
        if private_view {
            continue;
        }
        for (source, optical, pose, appearance) in &optical {
            let optical = &optical.0;
            if optical.view != view.id
                || optical
                    .known_entity
                    .is_some_and(|id| Some(id) == view.focused_ship)
            {
                continue;
            }
            let displacement = pose.0.position.relative_to(camera.origin);
            let relative_to_camera = displacement - camera_transform.translation.as_dvec3();
            let forward = camera_transform.rotation.as_dquat() * bevy::math::DVec3::NEG_Z;
            let depth = relative_to_camera.dot(forward);
            let pixels = if depth < -optical.radius_m {
                0.
            } else {
                glints::diameter_pixels(optical.radius_m, depth, height, fov)
            };
            let keep_mesh =
                glints::mesh_needed(pixels, existing.contains_key(&(view_entity, source)));
            if !keep_mesh {
                continue;
            }
            if appearance.is_none() {
                commands.entity(source).insert(crate::assets::MeshDemand);
            }
            let Some(appearance) = appearance else {
                continue;
            };
            let hash = appearance.hash;
            let Some(design) = designs.get(&appearance.design) else {
                continue;
            };
            let design = &design.0;
            let transform = Transform::from_translation(displacement.as_vec3())
                .with_rotation(Quat::from_array(pose.0.rotation.map(|value| value as f32)));
            let existing = existing
                .get(&(view_entity, source))
                .and_then(|entity| objects.get_mut(*entity).ok());
            let entity = if let Some((entity, _, _, mesh, mut current)) = existing {
                if mesh.appearance == hash {
                    *current = transform;
                    entity
                } else {
                    spawn_ship(&mut commands, design, &assets, &loader, &thermal, transform)
                }
            } else {
                spawn_ship(&mut commands, design, &assets, &loader, &thermal, transform)
            };
            commands.entity(entity).insert((
                ViewMember(view_entity),
                RenderSource(source),
                ShipMesh { appearance: hash },
                glints::MeshLod::at(
                    optical.radius_m,
                    relative_to_camera.length(),
                    depth,
                    height,
                    fov,
                ),
                ViewLayer(camera.layer),
                RenderLayers::layer(camera.layer),
            ));
            visible.insert(entity);
        }
    }
    for (entity, ..) in &objects {
        if !visible.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
}

fn sync_celestials(
    mut commands: Commands,
    views: Query<(Entity, &ViewCamera, &SystemSubscription)>,
    bodies: Query<(
        Entity,
        &Celestial,
        &DisplayPose,
        &CelestialSystem,
        Option<&super::celestials::PlanetSurface>,
    )>,
    mut objects: Query<(Entity, &ViewMember, &RenderSource, &mut Transform), With<BodyMesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut surfaces: ResMut<surfaces::SurfaceCache>,
) {
    let existing: HashMap<_, _> = objects
        .iter()
        .map(|(entity, view, source, _)| ((view.0, source.0), entity))
        .collect();
    let mut visible = HashSet::new();
    for (view_entity, camera, systems) in &views {
        if camera.private {
            continue;
        }
        for (source, body, pose, system, surface) in &bodies {
            if !systems.0.iter().any(|entry| entry.system == system.0) {
                continue;
            }
            if body.0.luminosity_lumens > 0. {
                continue;
            }
            let transform =
                Transform::from_translation(pose.0.position.relative_to(camera.origin).as_vec3())
                    .with_rotation(Quat::from_array(pose.0.rotation.map(|value| value as f32)));
            let entity = if let Some(&entity) = existing.get(&(view_entity, source)) {
                if let Ok((_, _, _, mut current)) = objects.get_mut(entity) {
                    *current = transform;
                }
                entity
            } else {
                let entity = commands
                    .spawn((
                        transform,
                        Visibility::default(),
                        BodyMesh,
                        ViewMember(view_entity),
                        RenderSource(source),
                    ))
                    .id();
                let mesh = surfaces.mesh(&mut meshes);
                let appearance = surface.map(|surface| {
                    surfaces.materials(
                        &surface.0,
                        Color::srgb_from_array(body.0.color),
                        &mut materials,
                    )
                });
                let ground = appearance
                    .as_ref()
                    .map(|appearance| appearance.ground.clone())
                    .unwrap_or_else(|| {
                        materials.add(StandardMaterial {
                            base_color: Color::srgb_from_array(body.0.color),
                            perceptual_roughness: 1.,
                            ..default()
                        })
                    });
                commands.spawn((
                    ChildOf(entity),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(ground),
                    Transform::from_scale(Vec3::splat(body.0.radius_m as f32)),
                    RenderLayers::layer(camera.layer),
                ));
                if let Some((surface, cloud)) =
                    surface.zip(appearance.and_then(|appearance| appearance.cloud))
                {
                    commands.spawn((
                        ChildOf(entity),
                        Mesh3d(mesh),
                        MeshMaterial3d(cloud),
                        surfaces::CloudLayer(surface.0.cloud_rotation_rad_s()),
                        bevy::light::NotShadowCaster,
                        Transform::from_scale(Vec3::splat(
                            (body.0.radius_m + surface.0.cloud_altitude_m) as f32,
                        )),
                        RenderLayers::layer(camera.layer),
                    ));
                }
                entity
            };
            commands
                .entity(entity)
                .insert((ViewLayer(camera.layer), RenderLayers::layer(camera.layer)));
            visible.insert(entity);
        }
    }
    for (entity, ..) in &objects {
        if !visible.contains(&entity) {
            commands.entity(entity).despawn();
        }
    }
}

fn spawn_ship(
    commands: &mut Commands,
    design: &CompiledShipDesign,
    assets: &PartVisualAssets,
    loader: &AssetServer,
    thermal: &toy_sim_ship_view::thermal::ThermalAssets,
    transform: Transform,
) -> Entity {
    let entity = commands.spawn((transform, Visibility::default())).id();
    toy_sim_ship_view::spawn_parts(commands, entity, design, assets, loader);
    if design.shield_deployed_kg > 0. {
        commands.spawn((
            ChildOf(entity),
            Shield,
            toy_sim_ship_view::thermal::ThermalSphere {
                temperature_k: 0.,
                strength: 0.,
            },
            Mesh3d(thermal.mesh.clone()),
            MeshMaterial3d(thermal.material.clone()),
            bevy::mesh::MeshTag::default(),
            Transform::from_scale(Vec3::splat(
                toy_sim_ships::thermal::shield_radius(design.radius) as f32,
            )),
            Visibility::default(),
        ));
    }
    entity
}

fn propagate_layers(
    mut commands: Commands,
    roots: Query<(Entity, &ViewLayer)>,
    children: Query<&Children>,
    layers: Query<&RenderLayers>,
) {
    for (entity, layer) in &roots {
        for child in children.iter_descendants(entity) {
            let desired = RenderLayers::layer(layer.0);
            if layers.get(child).is_ok_and(|current| *current == desired) {
                continue;
            }
            commands.entity(child).insert(desired);
        }
    }
}

fn apply_visuals(
    ships: Query<&RenderSource>,
    visuals: Query<&DisplayVisual>,
    parts: Query<(&ChildOf, &toy_sim_ship_view::PartVisual)>,
    mut plumes: Query<(
        &ChildOf,
        &mut toy_sim_ship_view::plume::EnginePlume,
        Option<&toy_sim_ship_view::plume::RcsNozzle>,
    )>,
    mut shields: Query<(&ChildOf, &mut toy_sim_ship_view::thermal::ThermalSphere), With<Shield>>,
    mut barrels: Query<(
        &toy_sim_ship_view::weapon::WeaponVisual,
        &ChildOf,
        &mut Transform,
    )>,
) {
    for (parent, mut plume, nozzle) in &mut plumes {
        plume.output = parts
            .get(parent.parent())
            .ok()
            .and_then(|(ship, part)| {
                ships
                    .get(ship.parent())
                    .ok()
                    .and_then(|source| visuals.get(source.0).ok())
                    .and_then(|visual| {
                        visual
                            .0
                            .engines
                            .iter()
                            .find(|engine| engine.part == part.id)
                    })
            })
            .map_or(0., |engine| {
                nozzle.map_or(engine.thrust_fraction as f32, |nozzle| {
                    ((engine.thrust_n[nozzle.axis] * nozzle.sign).max(0.) / plume.max_thrust_n)
                        as f32
                })
            });
    }
    for (parent, mut shield) in &mut shields {
        let state = ships
            .get(parent.parent())
            .ok()
            .and_then(|source| visuals.get(source.0).ok())
            .and_then(|visual| visual.0.shield.as_ref());
        shield.temperature_k = state.map_or(0., |state| state.temperature_k as f32);
        shield.strength = state.map_or(0., |state| state.coverage as f32);
    }
    for (_barrel, parent, mut transform) in &mut barrels {
        let Some(state) = parts.get(parent.parent()).ok().and_then(|(ship, part)| {
            ships
                .get(ship.parent())
                .ok()
                .and_then(|source| visuals.get(source.0).ok())
                .and_then(|visual| {
                    visual
                        .0
                        .turrets
                        .iter()
                        .find(|turret| turret.part == part.id)
                })
        }) else {
            continue;
        };
        transform.rotation = Quat::from_rotation_y(state.yaw_rad as f32)
            * Quat::from_rotation_x(state.pitch_rad as f32);
    }
}

#[cfg(test)]
pub(super) fn install_celestial_render_test(app: &mut App) {
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<surfaces::SurfaceCache>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<bevy::light::atmosphere::ScatteringMedium>>()
        .add_systems(
            Update,
            (camera::setup_views, sync_celestials)
                .chain()
                .after(crate::ui::celestials::CelestialSystems::Evaluate),
        );
    lighting::install(app);
    atmosphere::install(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_and_source_lifetimes_remove_render_instances() {
        let mut world = World::new();
        let view = world.spawn_empty().id();
        let source = world.spawn_empty().id();
        let mesh = world.spawn((ViewMember(view), RenderSource(source))).id();
        world.despawn(source);
        assert!(world.get_entity(mesh).is_err());
        assert!(world.get_entity(view).is_ok());

        let source = world.spawn_empty().id();
        let mesh = world.spawn((ViewMember(view), RenderSource(source))).id();
        world.despawn(view);
        assert!(world.get_entity(mesh).is_err());
        assert!(world.get_entity(source).is_ok());
    }

    #[test]
    fn initial_camera_faces_the_illuminated_side() {
        let origin = GalacticPosition::default();
        let star = origin.offset_by(bevy::math::DVec3::new(10., 20., 30.));
        let direction = sun_direction(origin, [(100., star)].into_iter()).unwrap();
        assert!(direction.dot(Vec3::new(10., 20., 30.).normalize()) > 0.999);
        assert!(sun_direction(origin, std::iter::empty()).is_none());
    }
}

fn mechanism_time(
    time: Res<crate::state::RenderTime>,
    mut clock: ResMut<toy_sim_ship_view::mechanisms::MechanismTime>,
) {
    clock.0 = time.display_ns as f64 * 1e-9;
}

fn own_visuals(
    mut commands: Commands,
    ships: Query<(Entity, &OwnedShip)>,
    optical: Query<(&Optical, &DisplayVisual)>,
    views: Query<&ViewObservation>,
) {
    for (entity, ship) in &ships {
        if let Some((observation, visual)) = optical.iter().find(|(observation, _)| {
            observation.0.known_entity == Some(ship.0.ship)
                && views.iter().any(|view| {
                    view.0.id == observation.0.view && view.0.focused_ship == Some(ship.0.ship)
                })
        }) {
            commands.entity(entity).insert((
                DisplayVisual(visual.0.clone()),
                glints::VisualContact(observation.0.contact),
            ));
        } else {
            commands
                .entity(entity)
                .remove::<(DisplayVisual, glints::VisualContact)>();
        }
    }
}
