//! Hill metadata lives on celestial entities; object locations are sampled once per tick.

use super::{
    orrery::Celestial,
    precision::PreciseTransform,
    simulation::SimulationCounters,
    spatial::{SpatialBody, SpatialIndex},
    travel::{DockedIn, Transit},
};
use bevy::prelude::*;
use osg_model::{
    location::{LocationContext, LocationRegion},
    travel::CelestialRef,
};
use osg_universe::{orrery_cfg::Body, universe::SystemDefinition};

#[derive(Component, Clone, Debug)]
pub struct HillSphere {
    pub reference: CelestialRef,
    pub radius_m: f64,
    pub hierarchy: Vec<CelestialRef>,
}

impl HillSphere {
    pub(crate) fn for_body(system: &SystemDefinition, body: &Body) -> Option<Self> {
        let reference = super::registry::model_reference(system.body_id(&body.name)?);
        let radius_m = if body.name == system.root_name {
            system.influence
        } else {
            let parent = system.solver.get_body(body.parent.as_deref()?)?;
            hill_radius(
                body.orbit.semi_major,
                body.orbit.eccentricity,
                body.mass,
                parent.mass,
            )
        };
        if !radius_m.is_finite() || radius_m <= 0.0 {
            return None;
        }
        let mut hierarchy = vec![reference];
        let mut ancestor = body.parent.as_deref();
        while let Some(name) = ancestor {
            let parent = system.solver.get_body(name)?;
            hierarchy.push(super::registry::model_reference(system.body_id(name)?));
            ancestor = parent.parent.as_deref();
        }
        hierarchy.reverse();
        Some(Self {
            reference,
            radius_m,
            hierarchy,
        })
    }
}

#[derive(Component, Clone, Debug, Default)]
pub struct SpatialLocation(pub LocationContext);

fn hill_radius(semi_major: f64, eccentricity: f64, mass: f64, parent_mass: f64) -> f64 {
    semi_major * (1.0 - eccentricity) * (mass / (3.0 * parent_mass)).cbrt()
}

/// The same pipeline serves scheduled updates, initial provisioning, and restore.
pub(crate) fn refresh(world: &mut World) {
    let _profile = super::diagnostics::ProfileScope::new("location.refresh");
    world.init_resource::<SpatialIndex>();
    world
        .run_system_cached(ensure_locations)
        .expect("provision spatial locations");
    world
        .run_system_cached(super::spatial::sync_hill_spheres)
        .expect("sync Hill sphere bounds");
    world
        .run_system_cached(update_locations)
        .expect("update spatial membership");
    inherit_docked_locations(world);
}

fn ensure_locations(
    mut commands: Commands,
    missing: Query<
        Entity,
        (
            With<PreciseTransform>,
            Without<SpatialLocation>,
            Without<Celestial>,
            Or<(With<SpatialBody>, With<DockedIn>, With<Transit>)>,
        ),
    >,
) {
    for entity in &missing {
        commands.entity(entity).insert(SpatialLocation::default());
    }
}

/// Parallel membership reads the persistent Hill tree and ECS metadata only.
pub(crate) fn update_locations(
    index: Res<SpatialIndex>,
    spheres: Query<&HillSphere>,
    clock: Option<Res<SimulationCounters>>,
    mut objects: Query<
        (&PreciseTransform, Has<Transit>, &mut SpatialLocation),
        (Without<DockedIn>, Without<Celestial>),
    >,
) {
    let tick = clock.as_ref().map_or(0, |clock| clock.ticks);
    objects
        .par_iter_mut()
        .for_each(|(pose, transit, mut location)| {
            let selected = if transit {
                None
            } else {
                index
                    .containing_hill_spheres(pose.translation_um)
                    .into_iter()
                    .filter_map(|entity| {
                        let sphere = spheres.get(entity).ok()?;
                        let position = index.hill_spheres().get(&entity)?.position;
                        let distance = position.relative_to(pose.translation_um).length_squared()
                            / sphere.radius_m.powi(2);
                        Some((sphere, distance))
                    })
                    .min_by(|(a, a_distance), (b, b_distance)| {
                        b.hierarchy
                            .len()
                            .cmp(&a.hierarchy.len())
                            .then_with(|| a_distance.total_cmp(b_distance))
                            .then_with(|| a.reference.cmp(&b.reference))
                    })
                    .map(|(sphere, _)| sphere)
            };
            let primary = selected.map(|sphere| sphere.reference);
            let region = if transit {
                LocationRegion::SlipTransit
            } else if primary.is_some() {
                LocationRegion::System
            } else {
                LocationRegion::Interstellar
            };
            let context = &mut location.0;
            if context.primary != primary || context.region != region {
                context.primary = primary;
                context.system = primary.map(|reference| reference.system);
                context.region = region;
                context.hierarchy.clear();
                if let Some(sphere) = selected {
                    context.hierarchy.extend_from_slice(&sphere.hierarchy);
                }
            }
            context.sample_tick = tick;
        });
}

/// Nested docking follows host links after independent membership is available.
pub(crate) fn inherit_docked_locations(world: &mut World) {
    let docked: Vec<_> = world
        .query::<(Entity, &DockedIn)>()
        .iter(world)
        .map(|(entity, host)| (entity, host.0))
        .collect();
    for _ in 0..docked.len() {
        let mut changed = false;
        for &(entity, host) in &docked {
            let Some(context) = world
                .get::<SpatialLocation>(host)
                .map(|value| value.0.clone())
            else {
                continue;
            };
            if let Some(mut location) = world.get_mut::<SpatialLocation>(entity)
                && location.0 != context
            {
                location.0 = context;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use osg_model::{GalacticPosition, Id};

    fn sphere(world: &mut World, id: u8, centre: f64, radius: f64, ancestors: &[u8]) -> Entity {
        let reference = |body| CelestialRef {
            system: Id([1; 16]),
            body: Id([body; 16]),
        };
        world
            .spawn((
                Celestial(format!("body-{id}").into()),
                SpatialBody {
                    radius_m: 1.0,
                    occludes: true,
                },
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::X * centre),
                    ..Default::default()
                },
                HillSphere {
                    reference: reference(id),
                    radius_m: radius,
                    hierarchy: ancestors
                        .iter()
                        .copied()
                        .chain([id])
                        .map(reference)
                        .collect(),
                },
            ))
            .id()
    }

    fn object(world: &mut World, x: f64) -> Entity {
        world
            .spawn((
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::X * x),
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 1.0,
                    occludes: true,
                },
            ))
            .id()
    }

    #[test]
    fn indexed_membership_matches_exhaustive_selection_and_tracks_movement() {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut world = World::new();
        sphere(&mut world, 1, 0.0, 1000.0, &[]);
        sphere(&mut world, 2, 100.0, 100.0, &[1]);
        let moon = sphere(&mut world, 3, 120.0, 10.0, &[1, 2]);
        sphere(&mut world, 4, 120.0, 10.0, &[1, 2]);
        let objects: Vec<_> = (-1100..=1100)
            .map(|x| object(&mut world, x as f64))
            .collect();
        refresh(&mut world);
        assert!(world.get::<SpatialLocation>(moon).is_none());
        let spheres: Vec<_> = world
            .query::<(&PreciseTransform, &HillSphere)>()
            .iter(&world)
            .map(|(pose, sphere)| (pose.translation_um, sphere.clone()))
            .collect();
        for entity in objects {
            let position = world
                .get::<PreciseTransform>(entity)
                .unwrap()
                .translation_um;
            let expected = spheres
                .iter()
                .filter(|(centre, sphere)| {
                    centre.relative_to(position).length_squared() <= sphere.radius_m.powi(2)
                })
                .min_by(|(a_position, a), (b_position, b)| {
                    b.hierarchy
                        .len()
                        .cmp(&a.hierarchy.len())
                        .then_with(|| {
                            (a_position.relative_to(position).length_squared() / a.radius_m.powi(2))
                                .total_cmp(
                                    &(b_position.relative_to(position).length_squared()
                                        / b.radius_m.powi(2)),
                                )
                        })
                        .then_with(|| a.reference.cmp(&b.reference))
                })
                .map(|(_, sphere)| sphere.reference);
            assert_eq!(
                world.get::<SpatialLocation>(entity).unwrap().0.primary,
                expected
            );
        }
        let ship = object(&mut world, 130.0);
        refresh(&mut world);
        assert_eq!(
            world
                .get::<SpatialLocation>(ship)
                .unwrap()
                .0
                .primary
                .unwrap()
                .body,
            Id([3; 16])
        );
        world
            .get_mut::<PreciseTransform>(moon)
            .unwrap()
            .translation_um = GalacticPosition::ZERO;
        refresh(&mut world);
        assert_eq!(
            world
                .get::<SpatialLocation>(ship)
                .unwrap()
                .0
                .primary
                .unwrap()
                .body,
            Id([4; 16])
        );
    }

    #[test]
    fn nested_docking_inherits_host_without_spatial_body() {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut world = World::new();
        sphere(&mut world, 1, 0.0, 1000.0, &[]);
        let station = object(&mut world, 20.0);
        let carrier = world
            .spawn((PreciseTransform::default(), DockedIn(station)))
            .id();
        let ship = world
            .spawn((PreciseTransform::default(), DockedIn(carrier)))
            .id();
        refresh(&mut world);
        assert_eq!(
            world.get::<SpatialLocation>(ship).unwrap().0,
            world.get::<SpatialLocation>(station).unwrap().0
        );
        assert_eq!(
            world.get::<SpatialLocation>(carrier).unwrap().0,
            world.get::<SpatialLocation>(station).unwrap().0
        );
    }

    #[test]
    fn hill_radius_uses_pericenter_and_parent_mass() {
        assert!((hill_radius(100.0, 0.5, 3.0, 1000.0) - 5.0).abs() < 1e-12);
    }
}
