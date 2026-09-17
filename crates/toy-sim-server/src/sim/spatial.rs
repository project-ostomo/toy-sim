//! Regional BVHs for ship centres and a separate extent BVH for occlusion.
use crate::sim::{GameState, precision::PreciseTransform};

use bevy::prelude::*;

#[derive(Component, Clone, Copy)]
pub struct SpatialBody {
    pub radius_m: f64,
    pub occludes: bool,
}

mod index;
pub use index::{SpatialIndex, SpatialObject, sphere_blocks};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensorSystems {
    Index,
    Scan,
}

pub struct SpatialPlugin;
impl Plugin for SpatialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpatialIndex>()
            .configure_sets(
                FixedLast,
                (SensorSystems::Index, SensorSystems::Scan).chain(),
            )
            .add_systems(
                FixedLast,
                rebuild
                    .in_set(SensorSystems::Index)
                    .run_if(in_state(GameState::Game)),
            );
    }
}

fn rebuild(
    mut index: ResMut<SpatialIndex>,
    bodies: Query<
        (
            Entity,
            &PreciseTransform,
            &SpatialBody,
            Has<super::orrery::Celestial>,
        ),
        Without<crate::sim::physics::collision::Projectile>,
    >,
) {
    // Retained VM snapshots may still own the old data. Do not clone their
    // entire BVHs just to discard them while constructing this tick's snapshot.
    if std::sync::Arc::get_mut(&mut index.0).is_none() {
        *index = SpatialIndex::default();
    }
    index.clear();
    for (entity, pose, body, celestial) in &bodies {
        index.insert(SpatialObject {
            entity,
            position: pose.translation_um,
            radius_m: body.radius_m,
            occludes: body.occludes,
        });
        if celestial {
            index.exclude_sensor_target(entity);
        }
    }
    // Account for shared construction once, outside a guest's metered scan.
    index.targets();
    index.blockers();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::precision::GalacticPosition;
    use bevy::math::DVec3;
    use rand::{RngExt, SeedableRng};

    #[test]
    fn regional_queries_match_brute_force_at_negative_and_galactic_coordinates() {
        let mut world = World::new();
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(67);
        let origin = GalacticPosition::new(1_i128 << 90, -(1_i128 << 90), -1);
        let mut index = SpatialIndex::default();
        for _ in 0..1000 {
            let delta = DVec3::new(
                rng.random_range(-1e10..1e10),
                rng.random_range(-1e10..1e10),
                rng.random_range(-1e10..1e10),
            );
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: 10_f64.powf(rng.random_range(1.0..10.0)),
                occludes: rng.random_bool(0.5),
            });
        }
        // Exact boundary and negative-cell probes.
        for delta in [
            DVec3::ZERO,
            DVec3::X * 1e6,
            -DVec3::X * 1e6,
            DVec3::splat(-0.001),
        ] {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: 1.0,
                occludes: true,
            });
        }
        for radius in [0.0, 0.001, 1e6, 1e7, 1e8, 1e9, 1e10, 1e18] {
            let mut actual = index.within_range(origin, radius);
            let expected: Vec<_> = index
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| o.position.relative_to(origin).length_squared() <= radius * radius)
                .map(|(i, _)| i)
                .collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
            let mut actual = index.occluders_in_range(origin, radius);
            let expected: Vec<_> = index
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| {
                    o.occludes
                        && o.position.relative_to(origin).length_squared()
                            <= (radius + o.radius_m).powi(2)
                })
                .map(|(i, _)| i)
                .collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
        assert_eq!(
            crate::sim::spatial_tree::key(GalacticPosition::new(-1, -100_000_000_000, 0)),
            [-1, -1, 0]
        );
    }
}

#[cfg(test)]
mod projectile_visibility_tests {
    use super::*;
    use crate::sim::precision::GalacticPosition;
    use bevy::math::DVec3;

    #[test]
    fn projectiles_neither_occupy_sensor_slots_nor_occlude_ships() {
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let observer = app.world_mut().spawn_empty().id();
        let target = app
            .world_mut()
            .spawn((
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::X * 100.0),
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 1.0,
                    occludes: false,
                },
            ))
            .id();
        app.world_mut().spawn((
            crate::sim::physics::collision::Projectile::new(10.0, 1.0),
            PreciseTransform {
                translation_um: GalacticPosition::from_meters(DVec3::X * 50.0),
                ..Default::default()
            },
            SpatialBody {
                radius_m: 10.0,
                occludes: true,
            },
        ));
        app.update();
        let contacts = crate::sim::sensors::detect_nearest(
            app.world().resource::<SpatialIndex>(),
            observer,
            GalacticPosition::ZERO,
            &crate::sim::sensors::Sensor::default(),
            1,
        );
        assert_eq!(contacts.visible.len(), 1);
        assert_eq!(contacts.visible[0].entity, target);
    }
}

#[cfg(test)]
mod sensor_target_tests {
    use super::*;
    use crate::sim::{orrery::Celestial, precision::GalacticPosition, sensors};
    use bevy::math::DVec3;

    #[test]
    fn celestial_centers_are_not_sensor_targets_but_still_occlude_ships() {
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let observer = app.world_mut().spawn_empty().id();
        let planet = app
            .world_mut()
            .spawn((
                Celestial("planet".into()),
                PreciseTransform {
                    translation_um: GalacticPosition::ZERO.offset_by(DVec3::X * 5.0),
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 1.0,
                    occludes: true,
                },
            ))
            .id();
        let ship = app
            .world_mut()
            .spawn((
                PreciseTransform {
                    translation_um: GalacticPosition::ZERO.offset_by(DVec3::X * 10.0),
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 0.5,
                    occludes: false,
                },
            ))
            .id();
        app.update();
        let index = app.world().resource::<SpatialIndex>();
        let sensor = sensors::Sensor {
            range_m: 100.0,
            occlusion: false,
        };
        let nearest = sensors::detect_nearest(index, observer, GalacticPosition::ZERO, &sensor, 1);
        assert_eq!(nearest.visible.len(), 1);
        assert_eq!(nearest.visible[0].entity, ship);
        let all = sensors::detect(index, observer, GalacticPosition::ZERO, &sensor);
        assert!(all.visible.iter().all(|contact| contact.entity != planet));
        let occluded = sensors::detect_nearest(
            index,
            observer,
            GalacticPosition::ZERO,
            &sensors::Sensor {
                occlusion: true,
                ..sensor
            },
            1,
        );
        assert_eq!(occluded.candidates, 1);
        assert_eq!(occluded.blocked, 1);
        assert!(occluded.visible.is_empty());
    }
}
