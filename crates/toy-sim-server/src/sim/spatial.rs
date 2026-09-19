use crate::sim::{GameState, precision::PreciseTransform};

use bevy::prelude::*;

#[derive(Component, Clone, Copy)]
pub struct SpatialBody {
    pub radius_m: f64,
    pub occludes: bool,
}

mod index;
mod lighting;
pub(crate) mod swept;
pub use index::{SpatialIndex, SpatialObject, sphere_blocks, sphere_fully_blocks};

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

pub(crate) fn rebuild(
    mut index: ResMut<SpatialIndex>,
    active: Option<Res<super::orrery::activity::ActiveSystems>>,
    bodies: Query<
        (
            Entity,
            &PreciseTransform,
            &SpatialBody,
            Has<super::orrery::Celestial>,
            Option<&super::orrery::Star>,
            Option<&super::vessel::ShipDesign>,
            Option<&super::hardware::ShipThermal>,
            Option<&super::hardware::PartDevices>,
            Option<&super::travel::Gate>,
            Option<&super::infrastructure::GateOrbit>,
        ),
        (
            Without<crate::sim::physics::collision::Projectile>,
            Without<super::travel::Dormant>,
        ),
    >,
    devices: Query<&super::hardware::Device>,
) {
    if std::sync::Arc::get_mut(&mut index.0).is_none() {
        *index = SpatialIndex::default();
    }
    index.clear();
    let mut sources = toy_sim_spatial::SpatialHash::default();
    let mut intrinsic = Vec::new();
    for (entity, pose, body, celestial, star, design, thermal, parts, gate, orbit) in &bodies {
        if orbit.is_some_and(|orbit| {
            active
                .as_ref()
                .is_some_and(|active| !active.entities.contains_key(&orbit.system))
        }) {
            continue;
        }
        let id = index.objects.len();
        let mut emitted = star.map_or_else(
            || lighting::emitted_luminosity(design, thermal, parts, &devices),
            |star| star.lumens / lighting::LUMENS_PER_OPTICAL_WATT,
        );
        if gate.is_some_and(|gate| gate.enabled) {
            emitted += 2e10 / lighting::LUMENS_PER_OPTICAL_WATT;
        }
        if star.is_some() || gate.is_some_and(|gate| gate.enabled) {
            sources.insert(
                id as u32,
                toy_sim_spatial::Entry {
                    position: pose.translation_um,
                    radius_m: body.radius_m,
                    luminosity: emitted,
                },
            );
        }
        intrinsic.push(emitted);
        index.insert(SpatialObject {
            optical_luminosity_w: emitted,
            entity,
            position: pose.translation_um,
            radius_m: body.radius_m,
            occludes: body.occludes,
        });
        if celestial {
            index.exclude_sensor_target(entity);
        }
        if design.is_some() {
            index.exclude_optical_blocker(entity);
        }
    }
    let mut source_cache = lighting::SourceCache::default();
    for (id, emitted) in intrinsic.into_iter().enumerate() {
        let candidates =
            lighting::nearby_sources(index.objects[id].position, &sources, &mut source_cache);
        let reflected = lighting::reflection_sources(&index, id, &sources, candidates);
        index.set_illumination(id, emitted, reflected);
    }
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
                optical_luminosity_w: 0.0,
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
                optical_luminosity_w: 0.0,
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

#[cfg(test)]
mod optical_tests {
    use super::*;
    use crate::sim::{hardware, orrery, precision::GalacticPosition, travel, vessel};
    use bevy::math::DVec3;

    fn object(position: DVec3, radius_m: f64) -> (PreciseTransform, SpatialBody) {
        (
            PreciseTransform {
                translation_um: GalacticPosition::from_meters(position),
                ..Default::default()
            },
            SpatialBody {
                radius_m,
                occludes: true,
            },
        )
    }

    #[test]
    fn moving_into_planetary_shadow_updates_brightness_each_tick() {
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        app.world_mut().spawn((
            object(DVec3::X * 1e6, 100.0),
            orrery::Star {
                lumens: 1e18,
                color_temp: 5778.0,
            },
        ));
        let ship = app.world_mut().spawn(object(DVec3::ZERO, 10.0)).id();
        let planet = app
            .world_mut()
            .spawn((
                object(DVec3::new(5e5, 1e5, 0.0), 1000.0),
                orrery::Celestial::default(),
            ))
            .id();
        app.update();
        let lit_snapshot = app.world().resource::<SpatialIndex>().clone();
        let ship_index = lit_snapshot.object_index(ship).unwrap();
        let observer = GalacticPosition::from_meters(DVec3::X * 1000.0);
        let full = lit_snapshot.observed_luminosity(ship_index, observer);
        assert!(full > 0.0);

        app.world_mut()
            .get_mut::<PreciseTransform>(planet)
            .unwrap()
            .translation_um = GalacticPosition::from_meters(DVec3::X * 5e5);
        app.update();
        let dark = app.world().resource::<SpatialIndex>();
        assert_eq!(
            dark.observed_luminosity(dark.object_index(ship).unwrap(), observer),
            0.0
        );
        assert_eq!(lit_snapshot.observed_luminosity(ship_index, observer), full);

        app.world_mut().entity_mut(planet).despawn();
        app.update();
        let relit = app.world().resource::<SpatialIndex>();
        assert_eq!(
            relit.observed_luminosity(relit.object_index(ship).unwrap(), observer),
            full
        );
    }

    #[test]
    fn engine_activation_and_dormancy_change_optical_visibility() {
        let catalogue = toy_sim_ships::Catalogue::builtin();
        let design = std::sync::Arc::new(
            toy_sim_ships::starter(toy_sim_ships::EXAMPLE_CONTROLLER.to_vec())
                .compile(&catalogue)
                .unwrap(),
        );
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let parts: Vec<_> = design
            .parts
            .iter()
            .map(|_| {
                app.world_mut()
                    .spawn(hardware::Device(toy_sim_ships::DeviceState::default()))
                    .id()
            })
            .collect();
        let engine = design
            .parts
            .iter()
            .position(|part| {
                matches!(
                    part.definition.equipment,
                    toy_sim_ships::Equipment::Engine { .. }
                        | toy_sim_ships::Equipment::ThermalEngine { .. }
                        | toy_sim_ships::Equipment::MicropulseEngine { .. }
                )
            })
            .unwrap();
        let engine = parts[engine];
        let ship = app
            .world_mut()
            .spawn((
                object(DVec3::ZERO, 10.0),
                vessel::ShipDesign(design),
                hardware::PartDevices(parts),
            ))
            .id();
        app.update();
        let observer = GalacticPosition::from_meters(DVec3::X * 1e6);
        assert!(
            app.world()
                .resource::<SpatialIndex>()
                .visible(observer, 1e-12)
                .is_empty()
        );

        app.world_mut()
            .get_mut::<hardware::Device>(engine)
            .unwrap()
            .0
            .actual = 1e6;
        app.update();
        let scene = app.world().resource::<SpatialIndex>();
        assert_eq!(
            scene.visible(observer, 1e-12),
            vec![scene.object_index(ship).unwrap()]
        );

        app.world_mut().entity_mut(ship).insert(travel::Dormant);
        app.update();
        assert!(
            app.world()
                .resource::<SpatialIndex>()
                .object_index(ship)
                .is_none()
        );
    }

    #[test]
    #[ignore = "profiling workload"]
    fn dense_optical_observer_profile() {
        use rand::{RngExt, SeedableRng};
        use std::time::Instant;

        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(4721);
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let mut observers = Vec::new();
        for system in 0..20 {
            let centre = DVec3::X * (system as f64 * 3e16);
            app.world_mut().spawn((
                object(centre, 7e8),
                orrery::Star {
                    lumens: 1e28,
                    color_temp: 5778.0,
                },
            ));
            app.world_mut().spawn((
                object(centre + DVec3::X * 0.99e11, 6e6),
                orrery::Celestial::default(),
            ));
            for _ in 0..1000 {
                let offset = DVec3::new(
                    rng.random_range(-1e7..1e7),
                    rng.random_range(-1e7..1e7),
                    rng.random_range(-1e7..1e7),
                );
                let position = centre + DVec3::X * 1e11 + offset;
                let (pose, mut body) = object(position, 10.0);
                body.occludes = false;
                let entity = app.world_mut().spawn((pose, body)).id();
                if observers.len() < (system + 1) * 25 {
                    observers.push((entity, GalacticPosition::from_meters(position)));
                }
            }
        }
        app.update();
        let begin = Instant::now();
        app.update();
        let rebuild_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let index = app.world().resource::<SpatialIndex>();
        let begin = Instant::now();
        let mut candidates = 0;
        let mut visible = 0;
        for (observer, origin) in observers.iter().copied() {
            for id in index.visible(origin, 1e-9) {
                candidates += 1;
                let object = index.objects[id];
                let distance2 = object.position.relative_to(origin).length_squared();
                if object.entity != observer
                    && index.observed_luminosity(id, origin) >= distance2 * 1e-9
                    && !index.fully_occluded(observer, id, origin)
                {
                    visible += 1;
                }
            }
        }
        let query_ms = begin.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "optical profile: objects={} observers={} cells={} rebuild_ms={rebuild_ms:.3} query_ms={query_ms:.3} candidates={candidates} visible={visible}",
            index.objects.len(),
            observers.len(),
            index.occupied_cells()
        );
        assert!(visible > 0);
    }
}
