use crate::sim::{
    GameState,
    precision::PreciseTransform,
    spatial::{SensorSystems, SpatialIndex},
};
use bevy::prelude::*;

#[derive(Component, Clone, Copy)]
#[require(SensorContacts)]
pub struct Sensor {
    pub range_m: f64,
    pub occlusion: bool,
}
impl Default for Sensor {
    fn default() -> Self {
        Self {
            range_m: 100_000_000.0,
            occlusion: true,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Contact {
    pub entity: Entity,
    pub distance_m: f64,
}

#[derive(Component, Default)]
pub struct SensorContacts {
    pub visible: Vec<Contact>,
    pub candidates: usize,
    pub blocked: usize,
    pub occluders: usize,
    pub scan_time_s: f64,
}

pub struct SensorsPlugin;
impl Plugin for SensorsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedLast,
            scan.in_set(SensorSystems::Scan)
                .run_if(in_state(GameState::Game)),
        );
    }
}

/// True when the open sightline enters a sphere. Tangency is not blocking.
/// Sensor-relative f64 vectors avoid converting absolute galactic positions.
pub use super::spatial::sphere_blocks;

fn scan(
    index: Res<SpatialIndex>,
    time: Res<Time<Fixed>>,
    mut sensors: Query<
        (
            Entity,
            &PreciseTransform,
            &Sensor,
            Option<&crate::sim::hardware::SensorRange>,
            &mut SensorContacts,
        ),
        Without<crate::sim::hardware::SensorRange>,
    >,
) {
    for (entity, pose, sensor, hardware, mut contacts) in &mut sensors {
        let sensor = Sensor {
            range_m: hardware.map_or(sensor.range_m, |h| sensor.range_m.min(h.0)),
            ..*sensor
        };
        *contacts = detect(&index, entity, pose.translation_um, &sensor);
        contacts.scan_time_s = time.elapsed_secs_f64();
    }
}

pub fn detect(
    index: &SpatialIndex,
    observer: Entity,
    origin: crate::sim::precision::GalacticPosition,
    sensor: &Sensor,
) -> SensorContacts {
    if sensor.range_m <= 0. || !sensor.range_m.is_finite() {
        return SensorContacts::default();
    }
    let candidates = index.within_range(origin, sensor.range_m);
    // Gather and transform blockers once per sensor, not once per target.
    let mut blockers: Vec<_> = if sensor.occlusion {
        index
            .occluders_in_range(origin, sensor.range_m)
            .into_iter()
            .map(|id| index.objects[id])
            .filter(|o| o.entity != observer)
            .map(|o| (o.entity, o.position.relative_to(origin), o.radius_m))
            .collect()
    } else {
        vec![]
    };
    blockers.sort_unstable_by(|a, b| a.1.length_squared().total_cmp(&b.1.length_squared()));
    let mut result = SensorContacts {
        occluders: blockers.len(),
        ..default()
    };
    for id in candidates {
        let target = index.objects[id];
        if target.entity == observer {
            continue;
        }
        result.candidates += 1;
        let displacement = target.position.relative_to(origin);
        if blockers.iter().any(|&(entity, centre, radius)| {
            entity != target.entity && sphere_blocks(displacement, centre, radius)
        }) {
            result.blocked += 1;
        } else {
            result.visible.push(Contact {
                entity: target.entity,
                distance_m: displacement.length(),
            });
        }
    }
    result.visible.sort_unstable_by(|a, b| {
        a.distance_m
            .total_cmp(&b.distance_m)
            .then_with(|| a.entity.cmp(&b.entity))
    });
    result
}

/// Nearest N geometrical candidates first, then occlusion. Hidden candidates
/// are never replaced with more distant contacts. Selection avoids a full sort.
pub fn detect_nearest(
    index: &SpatialIndex,
    observer: Entity,
    origin: crate::sim::precision::GalacticPosition,
    sensor: &Sensor,
    n: usize,
) -> SensorContacts {
    if n == 0 || sensor.range_m <= 0. || !sensor.range_m.is_finite() {
        return SensorContacts::default();
    }
    let candidates = index.nearest(observer, origin, sensor.range_m, n);
    let mut result = SensorContacts {
        candidates: candidates.len(),
        ..default()
    };
    for id in candidates {
        let object = index.objects[id];
        if sensor.occlusion && index.occluded(observer, id, origin) {
            result.blocked += 1;
        } else {
            result.visible.push(Contact {
                entity: object.entity,
                distance_m: object.position.relative_to(origin).length(),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{
        precision::GalacticPosition,
        simulation::SimulationPlugin,
        spatial::{SpatialBody, SpatialObject, SpatialPlugin},
    };
    use bevy::math::DVec3;
    use bevy::{state::app::StatesPlugin, time::TimeUpdateStrategy};
    use std::time::Duration;

    #[test]
    fn nearest_limit_is_applied_before_occlusion_without_backfill() {
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let hidden = world.spawn_empty().id();
        let blocker = world.spawn_empty().id();
        let farther = world.spawn_empty().id();
        let mut index = SpatialIndex::default();
        for (entity, position, radius_m, occludes) in [
            (hidden, DVec3::X * 10., 1., false),
            (blocker, DVec3::X * 12., 3., true),
            (farther, DVec3::Y * 15., 1., false),
        ] {
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity,
                position: GalacticPosition::ZERO.offset_by(position),
                radius_m,
                occludes,
                optical_occludes: occludes,
            });
        }
        let sensor = Sensor {
            range_m: 100.,
            occlusion: true,
        };
        let result = detect_nearest(&index, observer, GalacticPosition::ZERO, &sensor, 1);
        assert_eq!(result.candidates, 1);
        assert_eq!(result.blocked, 1);
        assert!(result.visible.is_empty());
        let result = detect_nearest(
            &index,
            observer,
            GalacticPosition::ZERO,
            &Sensor {
                occlusion: false,
                ..sensor
            },
            1,
        );
        assert_eq!(result.visible.len(), 1);
        assert_eq!(result.visible[0].entity, hidden);
        assert!(
            detect_nearest(&index, observer, GalacticPosition::ZERO, &sensor, 0)
                .visible
                .is_empty()
        );
    }

    #[test]
    fn sphere_sightlines_handle_tangency_surface_and_blocker_depth() {
        let target = DVec3::X * 10.0;
        assert!(sphere_blocks(target, DVec3::X * 5.0, 1.0));
        assert!(!sphere_blocks(target, DVec3::X * 12.0, 1.0));
        assert!(!sphere_blocks(target, DVec3::new(5.0, 1.0, 0.0), 1.0));
        assert!(!sphere_blocks(target, -DVec3::X, 1.0)); // On surface, looking away.
        assert!(sphere_blocks(target, DVec3::X, 1.0)); // On surface, looking inward.
        assert!(!sphere_blocks(DVec3::ZERO, DVec3::ZERO, 1.0));
    }

    #[test]
    fn huge_off_range_occluder_is_found_and_target_does_not_occlude_itself() {
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let hidden = world.spawn_empty().id();
        let star = world.spawn_empty().id();
        let clear = world.spawn_empty().id();
        let origin = GalacticPosition::splat(1_i128 << 90);
        let mut index = SpatialIndex::default();
        for (entity, delta, radius_m, occludes) in [
            (observer, DVec3::ZERO, 10.0, false),
            (hidden, DVec3::X * 15e6, 10.0, false),
            (star, DVec3::X * 1.01e9, 1e9, true),
            (clear, -DVec3::X * 15e6, 1e6, true),
        ] {
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity,
                position: origin.offset_by(delta),
                radius_m,
                occludes,
                optical_occludes: occludes,
            });
        }
        let mut sensor = Sensor {
            range_m: 20e6,
            occlusion: true,
        };
        let result = detect(&index, observer, origin, &sensor);
        assert_eq!(result.candidates, 2);
        assert_eq!(result.blocked, 1);
        assert_eq!(result.visible.len(), 1);
        assert_eq!(result.visible[0].entity, clear);
        sensor.occlusion = false;
        assert_eq!(detect(&index, observer, origin, &sensor).visible.len(), 2);
    }

    #[test]
    fn indexed_visibility_matches_all_objects_brute_force() {
        use rand::{RngExt, SeedableRng};
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(14);
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let origin = GalacticPosition::splat(-(1_i128 << 90));
        let mut index = SpatialIndex::default();
        for i in 0..500 {
            let delta = DVec3::new(
                rng.random_range(-1e8..1e8),
                rng.random_range(-1e8..1e8),
                rng.random_range(-1e8..1e8),
            );
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: if i % 7 == 0 {
                    rng.random_range(1e6..4e7)
                } else {
                    10.0
                },
                occludes: i % 7 == 0,
                optical_occludes: i % 7 == 0,
            });
        }
        for range_m in [1e7, 5e7, 1e8, 2e8] {
            let actual = detect(
                &index,
                observer,
                origin,
                &Sensor {
                    range_m,
                    occlusion: true,
                },
            );
            let mut actual: Vec<_> = actual.visible.iter().map(|c| c.entity).collect();
            let mut expected: Vec<_> = index
                .objects
                .iter()
                .filter(|target| {
                    let d = target.position.relative_to(origin);
                    d.length() <= range_m
                        && !index.objects.iter().any(|b| {
                            b.occludes
                                && b.entity != target.entity
                                && sphere_blocks(d, b.position.relative_to(origin), b.radius_m)
                        })
                })
                .map(|o| o.entity)
                .collect();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn scans_use_post_integration_positions_and_drop_despawned_objects() {
        #[derive(Component)]
        struct Target;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            SimulationPlugin,
            SpatialPlugin,
            SensorsPlugin,
        ))
        .insert_state(GameState::Game)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )))
        .add_systems(
            FixedPostUpdate,
            |mut target: Query<&mut PreciseTransform, With<Target>>| {
                for mut pose in &mut target {
                    pose.translation_um = GalacticPosition::new(5_000_000, 0, 0);
                }
            },
        );
        let sensor = app
            .world_mut()
            .spawn((
                PreciseTransform::default(),
                Sensor {
                    range_m: 10.0,
                    occlusion: true,
                },
            ))
            .id();
        let target = app
            .world_mut()
            .spawn((
                Target,
                PreciseTransform {
                    translation_um: GalacticPosition::new(50_000_000, 0, 0),
                    ..default()
                },
                SpatialBody {
                    radius_m: 1.0,
                    occludes: false,
                },
            ))
            .id();
        app.update();
        app.update();
        assert_eq!(
            app.world().get::<SensorContacts>(sensor).unwrap().visible[0].entity,
            target
        );
        assert_eq!(
            app.world().get::<SensorContacts>(sensor).unwrap().visible[0].distance_m,
            5.0
        );
        app.world_mut().despawn(target);
        app.update();
        assert!(
            app.world()
                .get::<SensorContacts>(sensor)
                .unwrap()
                .visible
                .is_empty()
        );
    }
}
