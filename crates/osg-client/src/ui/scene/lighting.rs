use super::ViewCamera;
use crate::state::{Celestial, CelestialSystem, DisplayPose, ViewSystems};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use osg_model::{GalacticPosition, Id, presentation::CelestialPresentation};
use std::collections::HashMap;

const STARS_PER_VIEW: usize = 2;

#[derive(Component)]
struct StellarLight(Id);

#[derive(Clone, Copy)]
struct Starlight {
    star: Id,
    illuminance: f32,
    direction: Vec3,
    color: Color,
}

fn relevant_stars<'a>(
    observer: GalacticPosition,
    stars: impl Iterator<Item = (&'a CelestialPresentation, GalacticPosition)>,
) -> Vec<Starlight> {
    let mut stars: Vec<_> = stars
        .filter_map(|(star, position)| {
            let displacement = observer.relative_to(position);
            let illuminance = star.luminosity_lumens
                / (4.
                    * std::f64::consts::PI
                    * displacement
                        .length_squared()
                        .max(star.radius_m.powi(2))
                        .max(1.));
            if !illuminance.is_finite() || illuminance <= 0. {
                return None;
            }
            Some(Starlight {
                star: star.entity,
                illuminance: illuminance.min(f32::MAX as f64) as f32,
                direction: displacement.try_normalize()?.as_vec3(),
                color: Color::linear_rgb(star.color[0], star.color[1], star.color[2]),
            })
        })
        .collect();
    stars.sort_by(|a, b| {
        b.illuminance
            .total_cmp(&a.illuminance)
            .then_with(|| a.star.cmp(&b.star))
    });
    stars.truncate(STARS_PER_VIEW);
    stars
}

struct ViewLighting {
    entity: Entity,
    id: u64,
    layer: usize,
    rotation: Quat,
    stars: Vec<Starlight>,
}

fn allocated_lights(views: &[ViewLighting], budget: usize) -> Vec<(usize, Starlight)> {
    let mut candidates: Vec<_> = views
        .iter()
        .enumerate()
        .flat_map(|(view_index, view)| {
            view.stars
                .iter()
                .enumerate()
                .map(move |(rank, star)| (rank, view.id, view_index, *star))
        })
        .collect();
    // Give every view its primary source before allocating secondary sources.
    candidates.sort_by_key(|&(rank, view_id, _, star)| (rank, view_id, star.star));
    candidates
        .into_iter()
        .take(budget)
        .map(|(_, _, view, star)| (view, star))
        .collect()
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        update.before(bevy::transform::TransformSystems::Propagate),
    );
}

fn update(
    mut commands: Commands,
    cameras: Query<
        (Entity, &ViewCamera, &Camera, &Transform, &ViewSystems),
        Without<DirectionalLight>,
    >,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    mut lights: Query<
        (
            Entity,
            &ChildOf,
            &StellarLight,
            &mut DirectionalLight,
            &mut Transform,
        ),
        Without<ViewCamera>,
    >,
    other_lights: Query<(), (With<DirectionalLight>, Without<StellarLight>)>,
) {
    let views: Vec<_> = cameras
        .iter()
        .filter(|(_, view, camera, _, _)| camera.is_active && !view.private)
        .map(|(entity, view, _, camera, systems)| ViewLighting {
            entity,
            id: view.view,
            layer: view.layer,
            rotation: camera.rotation,
            stars: relevant_stars(
                view.origin.offset_by(camera.translation.as_dvec3()),
                bodies
                    .iter()
                    .filter(|(_, _, system)| systems.0.iter().any(|entry| *entry == system.0))
                    .map(|(body, pose, _)| (&body.0, pose.0.position)),
            ),
        })
        .collect();
    let budget = bevy::pbr::MAX_DIRECTIONAL_LIGHTS.saturating_sub(other_lights.iter().count());
    let mut selected: HashMap<_, _> = allocated_lights(&views, budget)
        .into_iter()
        .map(|(view, star)| ((views[view].entity, star.star), (view, star)))
        .collect();
    for (entity, parent, source, mut light, mut transform) in &mut lights {
        let Some((view, star)) = selected.remove(&(parent.parent(), source.0)) else {
            commands.entity(entity).despawn();
            continue;
        };
        apply_starlight(&mut light, &mut transform, star, views[view].rotation);
        commands
            .entity(entity)
            .insert(RenderLayers::layer(views[view].layer));
    }
    let mut new_lights: Vec<_> = selected.into_values().collect();
    new_lights.sort_by_key(|&(view, star)| (views[view].id, star.star));
    for (view, star) in new_lights {
        let view = &views[view];
        let mut light = DirectionalLight {
            shadow_maps_enabled: true,
            contact_shadows_enabled: true,
            ..default()
        };
        let mut transform = Transform::default();
        apply_starlight(&mut light, &mut transform, star, view.rotation);
        commands.spawn((
            ChildOf(view.entity),
            StellarLight(star.star),
            bevy::light::SunDisk::OFF,
            light,
            transform,
            RenderLayers::layer(view.layer),
        ));
    }
}

fn apply_starlight(
    light: &mut DirectionalLight,
    transform: &mut Transform,
    star: Starlight,
    camera_rotation: Quat,
) {
    light.illuminance = star.illuminance;
    light.color = star.color;
    let direction = camera_rotation.inverse() * star.direction;
    let up = if direction.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    transform.look_to(direction, up);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ViewObservation;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::DVec3;
    use osg_model::{Completion, Pose, ViewState};

    fn star(id: u8, position: DVec3, luminosity: f64) -> CelestialPresentation {
        CelestialPresentation {
            reference: osg_model::travel::CelestialRef {
                system: osg_model::Id::default(),
                body: osg_model::Id::default(),
            },
            entity: Id([id; 16]),
            name: format!("Star {id}"),
            pose: Pose {
                position: GalacticPosition::from_meters(position),
                ..Default::default()
            },
            radius_m: 1.,
            gravitational_parameter: 1.,
            luminosity_lumens: luminosity,
            temperature_k: 5000.,
            color: [1.; 3],
            atmosphere: None,
        }
    }

    #[test]
    fn binary_lights_keep_both_directions_when_relative_brightness_crosses() {
        let stars = [
            star(1, -DVec3::X * 100., 1e6),
            star(2, DVec3::X * 100., 1e6),
        ];
        for offset in [-1., 0., 1.] {
            let selected = relevant_stars(
                GalacticPosition::from_meters(DVec3::X * offset),
                stars.iter().map(|star| (star, star.pose.position)),
            );
            assert_eq!(selected.len(), 2);
            let left = selected
                .iter()
                .find(|light| light.star == stars[0].entity)
                .unwrap();
            let right = selected
                .iter()
                .find(|light| light.star == stars[1].entity)
                .unwrap();
            assert!(left.direction.dot(Vec3::X) > 0.999);
            assert!(right.direction.dot(-Vec3::X) > 0.999);
            let expected = 1e6 / (4. * std::f64::consts::PI * (100. + offset).powi(2));
            assert!((left.illuminance as f64 - expected).abs() < expected * 1e-6);
        }
    }

    #[test]
    fn directional_budget_is_shared_fairly_and_equal_flux_uses_stable_ids() {
        let stars = [
            star(2, DVec3::X * 100., 1e6),
            star(1, -DVec3::X * 100., 1e6),
        ];
        let selected = relevant_stars(
            GalacticPosition::ZERO,
            stars.iter().map(|star| (star, star.pose.position)),
        );
        assert_eq!(selected[0].star, Id([1; 16]));
        let views: Vec<_> = (0..8)
            .rev()
            .map(|id| ViewLighting {
                entity: Entity::PLACEHOLDER,
                id,
                layer: id as usize + 1,
                rotation: Quat::IDENTITY,
                stars: selected.clone(),
            })
            .collect();
        let allocated = allocated_lights(&views, 10);
        assert_eq!(allocated.len(), 10);
        for view in 0..8 {
            assert!(allocated.iter().any(|(index, _)| *index == view));
        }
        assert_eq!(views[allocated[8].0].id, 0);
        assert_eq!(views[allocated[9].0].id, 1);
        assert!(allocated_lights(&views, 0).is_empty());
    }

    #[test]
    fn stellar_entities_survive_brightness_changes_and_leave_with_their_view() {
        let mut world = World::new();
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
        for star in [
            star(1, -DVec3::X * 100., 1e6),
            star(2, DVec3::X * 100., 1e6),
        ] {
            world.spawn((
                DisplayPose(star.pose.clone()),
                Celestial(star),
                CelestialSystem(system),
            ));
        }
        world
            .run_system_once(super::super::camera::setup_views)
            .unwrap();
        *world.get_mut::<Transform>(camera).unwrap() = Transform::from_xyz(-1., 0., 0.);
        world.run_system_once(update).unwrap();
        let original: HashMap<_, _> = world
            .query::<(Entity, &StellarLight)>()
            .iter(&world)
            .map(|(entity, source)| (source.0, entity))
            .collect();
        assert_eq!(original.len(), 2);

        *world.get_mut::<Transform>(camera).unwrap() = Transform::from_xyz(1., 0., 0.);
        world.run_system_once(update).unwrap();
        for (source, entity) in &original {
            assert_eq!(world.get::<StellarLight>(*entity).unwrap().0, *source);
            assert_eq!(world.get::<ChildOf>(*entity).unwrap().parent(), camera);
        }
        world.get_mut::<ViewCamera>(camera).unwrap().private = true;
        world.run_system_once(update).unwrap();
        assert_eq!(world.query::<&StellarLight>().iter(&world).count(), 0);

        world.get_mut::<ViewCamera>(camera).unwrap().private = false;
        world.run_system_once(update).unwrap();
        assert_eq!(world.query::<&StellarLight>().iter(&world).count(), 2);
        world.despawn(camera);
        assert_eq!(world.query::<&StellarLight>().iter(&world).count(), 0);
    }
}
