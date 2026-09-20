//! Emissive galactic-coordinate spheres for stars too resolvable to bake.
//!
//! `bake::Snapshot` diverts stars above the handover angular size into its
//! geometry set; this module renders that set as real meshes so the stars get
//! true per-frame parallax and participate in bloom as HDR point sources.
use super::{Settings, ViewSky, bake};
use crate::state::{Celestial, DisplayPose};
use crate::ui::scene::{ViewCamera, ViewMember, surfaces};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use osg_model::{GalacticPosition, Id};
use std::collections::{HashMap, HashSet};

#[derive(Component)]
pub(super) struct StarMesh {
    view: Entity,
    key: bake::GeometryKey,
}

struct Desired {
    view: Entity,
    origin: GalacticPosition,
    layer: usize,
    star: bake::GeometryStar,
    position: GalacticPosition,
}

fn emissive(star: &bake::GeometryStar, brightness: f32) -> [f32; 3] {
    let radiance = bake::star_radiance(star.luminosity, star.radius_m) * brightness as f64;
    star.colour.map(|c| (c as f64 * radiance) as f32)
}

fn star_transform(entry: &Desired) -> Transform {
    Transform::from_translation(entry.position.relative_to(entry.origin).as_vec3())
        .with_scale(Vec3::splat(entry.star.radius_m as f32))
}

pub(super) fn sync(
    mut commands: Commands,
    cameras: Query<(Entity, &ViewCamera, &ViewSky)>,
    bodies: Query<(&Celestial, &DisplayPose)>,
    mut stars: Query<
        (
            Entity,
            &StarMesh,
            &mut Transform,
            &MeshMaterial3d<StandardMaterial>,
        ),
        Without<ViewCamera>,
    >,
    settings: Res<Settings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut surfaces: ResMut<surfaces::SurfaceCache>,
) {
    // Celestial poses are read live so diverted stars get true parallax;
    // catalogue stars keep the position captured in the snapshot.
    let poses: HashMap<Id, GalacticPosition> = bodies
        .iter()
        .map(|(Celestial(body), pose)| (body.entity, pose.0.position))
        .collect();
    let mut desired: Vec<Desired> = Vec::new();
    for (view, camera, sky) in &cameras {
        let Some(snapshot) = &sky.snapshot else {
            continue;
        };
        for star in &snapshot.geometry {
            let position = match star.key {
                bake::GeometryKey::Celestial(id) => {
                    let Some(position) = poses.get(&id).copied() else {
                        // Unsubscribed since this snapshot: hidden until the
                        // next bake replaces it.
                        continue;
                    };
                    position
                }
                bake::GeometryKey::Catalogue(_) => star.position,
            };
            desired.push(Desired {
                view,
                origin: camera.origin,
                layer: camera.layer,
                star: star.clone(),
                position,
            });
        }
    }
    let index: HashMap<_, usize> = desired
        .iter()
        .enumerate()
        .map(|(i, entry)| ((entry.view, entry.star.key), i))
        .collect();
    let mut existing: HashSet<(Entity, bake::GeometryKey)> = HashSet::new();
    for (entity, mesh, mut transform, material) in &mut stars {
        let Some(entry) = index.get(&(mesh.view, mesh.key)).map(|&i| &desired[i]) else {
            commands.entity(entity).despawn();
            materials.remove(material.0.id());
            continue;
        };
        *transform = star_transform(entry);
        existing.insert((mesh.view, mesh.key));
        let emissive = emissive(&entry.star, settings.brightness);
        let stale = materials.get(&material.0).is_none_or(|material| {
            material.emissive.red != emissive[0]
                || material.emissive.green != emissive[1]
                || material.emissive.blue != emissive[2]
        });
        if stale && let Some(mut material) = materials.get_mut(&material.0) {
            material.emissive = LinearRgba::rgb(emissive[0], emissive[1], emissive[2]);
        }
    }
    let sphere = surfaces.mesh(&mut meshes);
    for entry in &desired {
        if existing.contains(&(entry.view, entry.star.key)) {
            continue;
        }
        let emissive = emissive(&entry.star, settings.brightness);
        let material = materials.add(StandardMaterial {
            base_color: Color::BLACK,
            emissive: LinearRgba::rgb(emissive[0], emissive[1], emissive[2]),
            perceptual_roughness: 1.,
            ..default()
        });
        commands.spawn((
            star_transform(entry),
            Visibility::default(),
            ViewMember(entry.view),
            StarMesh {
                view: entry.view,
                key: entry.star.key,
            },
            Mesh3d(sphere.clone()),
            MeshMaterial3d(material),
            RenderLayers::layer(entry.layer),
            bevy::light::NotShadowCaster,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{CelestialSystem, ViewObservation, ViewSystems};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::DVec3;
    use osg_model::{Completion, Pose, ViewState, presentation::CelestialPresentation};
    use osg_stars::{SOLAR_LUMENS, Star, StarId};
    use std::sync::Arc;

    fn celestial(
        entity: Id,
        position: DVec3,
        radius_m: f64,
        luminosity: f64,
    ) -> CelestialPresentation {
        CelestialPresentation {
            reference: osg_model::travel::CelestialRef {
                system: osg_model::Id::default(),
                body: osg_model::Id::default(),
            },
            entity,
            name: "Star".into(),
            pose: Pose {
                position: GalacticPosition::from_meters(position),
                ..Default::default()
            },
            radius_m,
            gravitational_parameter: 1.,
            luminosity_lumens: luminosity,
            temperature_k: 5000.,
            color: [1., 0.9, 0.8],
            atmosphere: None,
        }
    }

    fn celestial_source(star: &CelestialPresentation) -> bake::Source {
        let mut identity = [0; 8];
        identity.copy_from_slice(&star.entity.0[..8]);
        bake::Source {
            star: Star {
                id: StarId {
                    namespace: 2,
                    value: u64::from_le_bytes(identity),
                },
                position: star.pose.position,
                luminosity: star.luminosity_lumens,
                temperature_k: star.temperature_k,
                colour: star.color,
            },
            radius_m: star.radius_m,
            key: bake::GeometryKey::Celestial(star.entity),
        }
    }

    fn catalogue_source(position: DVec3) -> bake::Source {
        let id = StarId::gaia(7);
        bake::Source {
            star: Star {
                id,
                position: GalacticPosition::from_meters(position),
                luminosity: SOLAR_LUMENS,
                temperature_k: 5772.,
                colour: [1., 0.9, 0.8],
            },
            radius_m: 6.96e8,
            key: bake::GeometryKey::Catalogue(id),
        }
    }

    #[test]
    fn spheres_follow_live_poses_and_leave_with_their_snapshot_or_view() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<surfaces::SurfaceCache>();
        world.insert_resource(Settings {
            brightness: 1.0,
            ..Default::default()
        });
        let system = Id([3; 16]);
        let camera = world
            .spawn((
                ViewObservation(ViewState {
                    id: 1,
                    revision: 1,
                    group: Id([4; 16]),
                    focused_ship: None,
                    origin: GalacticPosition::ZERO,
                    tracks: Vec::new(),
                    completion: Completion::Complete,
                }),
                ViewSystems(vec![system]),
            ))
            .id();
        let star = celestial(Id([1; 16]), DVec3::X * 2.0e11, 6.96e8, SOLAR_LUMENS);
        let body = world
            .spawn((
                DisplayPose(star.pose.clone()),
                Celestial(star.clone()),
                CelestialSystem(system),
            ))
            .id();
        world
            .run_system_once(crate::ui::scene::camera::setup_views)
            .unwrap();
        world.get_mut::<ViewSky>(camera).unwrap().snapshot = Some(Arc::new(bake::Snapshot::new(
            vec![
                celestial_source(&star),
                catalogue_source(-DVec3::X * 1.0e12),
            ],
            GalacticPosition::ZERO,
            6.0,
            0,
            0,
            f64::INFINITY,
        )));

        world.run_system_once(sync).unwrap();
        let instances: HashMap<bake::GeometryKey, (Entity, &Transform, Handle<StandardMaterial>)> =
            world
                .query::<(&StarMesh, &Transform, &MeshMaterial3d<StandardMaterial>)>()
                .iter(&world)
                .map(|(mesh, transform, material)| {
                    (mesh.key, (mesh.view, transform, material.0.clone()))
                })
                .collect();
        assert_eq!(instances.len(), 2);
        let (view, transform, _) = instances
            .get(&bake::GeometryKey::Celestial(star.entity))
            .unwrap();
        assert_eq!(*view, camera);
        assert!((transform.translation.x - 2.0e11).abs() < 1.0e4);
        assert!((transform.scale.x - 6.96e8).abs() < 1.0);
        let (_, transform, material) = instances
            .get(&bake::GeometryKey::Catalogue(StarId::gaia(7)))
            .unwrap();
        assert!((transform.translation.x + 1.0e12).abs() < 1.0e5);
        let expected = emissive(
            &bake::GeometryStar {
                key: bake::GeometryKey::Catalogue(StarId::gaia(7)),
                position: GalacticPosition::ZERO,
                radius_m: 6.96e8,
                luminosity: SOLAR_LUMENS,
                colour: [1., 0.9, 0.8],
            },
            1.,
        );
        let material = world
            .resource::<Assets<StandardMaterial>>()
            .get(material)
            .unwrap();
        assert_eq!(material.emissive.red, expected[0]);
        assert_eq!(material.emissive.green, expected[1]);
        assert_eq!(material.emissive.blue, expected[2]);

        // Live pose moves are reflected every frame: the sphere has real
        // parallax instead of waiting for a skybox rebake.
        let mut pose = world.get_mut::<DisplayPose>(body).unwrap();
        pose.0.position = pose.0.position.offset_by(DVec3::X * 5.0e10);
        drop(pose);
        world.run_system_once(sync).unwrap();
        for (mesh, transform) in world.query::<(&StarMesh, &Transform)>().iter(&world) {
            if matches!(mesh.key, bake::GeometryKey::Celestial(_)) {
                assert!((transform.translation.x - 2.5e11).abs() < 1.0e4);
            }
        }

        // A snapshot without the stars retires their spheres and materials.
        world.get_mut::<ViewSky>(camera).unwrap().snapshot = Some(Arc::new(bake::Snapshot::new(
            vec![],
            GalacticPosition::ZERO,
            6.0,
            0,
            0,
            f64::INFINITY,
        )));
        world.run_system_once(sync).unwrap();
        assert_eq!(world.query::<&StarMesh>().iter(&world).count(), 0);
        assert_eq!(
            world.resource::<Assets<StandardMaterial>>().iter().count(),
            0
        );

        // Restored stars respawn, and despawning the view removes its spheres.
        world.get_mut::<ViewSky>(camera).unwrap().snapshot = Some(Arc::new(bake::Snapshot::new(
            vec![
                celestial_source(&star),
                catalogue_source(-DVec3::X * 1.0e12),
            ],
            GalacticPosition::ZERO,
            6.0,
            0,
            0,
            f64::INFINITY,
        )));
        world.run_system_once(sync).unwrap();
        assert_eq!(world.query::<&StarMesh>().iter(&world).count(), 2);
        world.despawn(camera);
        assert_eq!(world.query::<&StarMesh>().iter(&world).count(), 0);
    }
}
