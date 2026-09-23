use crate::sim::precision::PreciseTransform;

use bevy::prelude::*;

#[derive(Component, Clone, Copy)]
pub struct SpatialBody {
    pub radius_m: f64,
    pub occludes: bool,
}

mod index;
mod lighting;
pub use index::{SpatialIndex, SpatialKey, SpatialObject, sphere_blocks, sphere_fully_blocks};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensorSystems {
    Index,
    Scan,
}

pub struct SpatialPlugin;
impl Plugin for SpatialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpatialIndex>().configure_sets(
            FixedLast,
            (SensorSystems::Index, SensorSystems::Scan).chain(),
        );
        app.add_systems(
            FixedLast,
            collect
                .in_set(SensorSystems::Index)
                .run_if(in_state(super::GameState::Game)),
        );
    }
}

pub(crate) fn rebuild(world: &mut World) {
    world.init_resource::<SpatialIndex>();
    world
        .run_system_cached(collect)
        .expect("collect spatial records");
}

pub(crate) fn collect(
    mut index: ResMut<SpatialIndex>,
    bodies: Query<
        (
            Entity,
            &PreciseTransform,
            &SpatialBody,
            Has<super::orrery::Celestial>,
            Option<&super::orrery::activity::CelestialState>,
            Option<&super::orrery::Star>,
            Option<&super::vessel::ShipDesign>,
            Option<&super::hardware::ShipThermal>,
            Option<&super::hardware::PartDevices>,
            Option<&super::physics::Velocity>,
        ),
        (
            Without<crate::sim::physics::collision::Projectile>,
            Without<super::travel::Dormant>,
        ),
    >,
    devices: Query<&super::hardware::Device>,
    projectiles: Query<
        (
            Entity,
            &super::physics::collision::Projectile,
            &PreciseTransform,
        ),
        Without<super::travel::Dormant>,
    >,
    universe: Option<Res<super::orrery::Universe>>,
    time: Option<Res<Time<Fixed>>>,
    clock: Option<Res<super::simulation::SimulationCounters>>,
) {
    index.clear();
    index.tick = clock.map_or(0, |clock| clock.ticks);
    index
        .sky
        .set_universe(universe.map(|universe| universe.0.clone()));
    index.seed_catalogue();
    index.sky.epoch = time.as_ref().map_or_else(hifitime::Epoch::default, |time| {
        super::physics::sim_time(&**time)
    });
    index.tick_seconds = time.as_ref().map_or(0.1, |time| time.delta_secs_f64());
    let _profile = crate::sim::diagnostics::ProfileScope::new("spatial_collect_loop");
    let mut celestial_count = 0_usize;
    let mut stars = 0_usize;
    for (entity, pose, body, celestial, celestial_state, star, design, thermal, parts, velocity) in
        &bodies
    {
        if let Some(velocity) = velocity {
            index.velocities.insert(entity, velocity.0);
        }
        if let Some(state) = celestial_state {
            index.velocities.insert(entity, state.velocity);
            if index.sky.universe.is_none() {
                index.capture_radii.insert(
                    entity,
                    osg_model::travel::slip::exclusion_radius_m(state.body.mass),
                );
            }
        }
        let emitted = star.map_or_else(
            || lighting::emitted_luminosity(design, thermal, parts, &devices),
            |star| star.lumens / lighting::LUMENS_PER_OPTICAL_WATT,
        );
        if star.is_some() {
            stars += 1;
            index.sky.local.push(lighting::Light {
                position: pose.translation_um,
                radius: body.radius_m,
                power: emitted,
            });
        }
        let object = SpatialObject {
            optical_luminosity_w: if celestial { 0.0 } else { emitted },
            entity,
            position: pose.translation_um,
            radius_m: body.radius_m,
            occludes: body.occludes,
            optical_occludes: body.occludes && design.is_none(),
        };
        if celestial {
            celestial_count += 1;
            index.insert_celestial(
                object,
                celestial_state.map_or(usize::MAX, |state| state.system),
            );
        } else {
            index.insert(object);
        }
    }
    if std::env::var_os("OSG_SPATIAL_PROFILE").is_some() {
        debug!(target: "osg_server::profile", tick = index.tick,
            objects = index.objects.len(), celestial_count, stars,
            non_celestial = index.objects.len() - celestial_count,
            "spatial population");
    }
    for (entity, projectile, pose) in &projectiles {
        index.insert_collision(entity, pose.translation_um, projectile.radius_m);
    }
    index.finish_geometry();
    // Exact illumination is evaluated only when an optical observer requests it.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::precision::GalacticPosition;
    use bevy::math::DVec3;
    use rand::{RngExt, SeedableRng};

    #[test]
    fn uninstantiated_stars_illuminate_cold_ships() {
        let universe =
            super::super::orrery::Universe::init(osg_universe::example_config()).unwrap();
        let anchor = universe.systems[0].position;
        let mut app = App::new();
        app.insert_resource(universe)
            .init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let position = anchor.offset_by(DVec3::X * 149_597_870_700.);
        let ship = app
            .world_mut()
            .spawn((
                PreciseTransform {
                    translation_um: position,
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 100.,
                    occludes: false,
                },
            ))
            .id();
        app.update();
        let index = app.world().resource::<SpatialIndex>();
        let target = index.object_index(ship).unwrap();
        let observer = position.offset_by(-DVec3::X * 1000.);
        assert!(index.visible(observer, 1e-12).contains(&target));
        assert!(index.observed_luminosity(target, observer) > 0.);
        assert!(index.fully_occluded(ship, target, anchor.offset_by(-DVec3::X * 149_597_870_700.)));
        assert_eq!(index.objects.len(), 1);
    }

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
            let radius_m = 10_f64.powf(rng.random_range(1.0..10.0));
            let occludes = rng.random_bool(0.5);
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m,
                occludes,
                optical_occludes: occludes,
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
                optical_occludes: true,
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
        let star = app
            .world_mut()
            .spawn((
                super::super::orrery::Star {
                    lumens: 1e12,
                    color_temp: 5772.0,
                },
                PreciseTransform {
                    translation_um: GalacticPosition::ZERO.offset_by(DVec3::Y * 10.0),
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 1.0,
                    occludes: true,
                },
            ))
            .id();
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
        assert!(
            index
                .visible(GalacticPosition::ZERO, 1e-12)
                .iter()
                .all(|&id| index.objects[id].entity != star)
        );
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
    fn ship_bounds_keep_sensor_occlusion_without_blocking_optical_sightlines() {
        let catalogue = osg_ships::Catalogue::builtin();
        let design = std::sync::Arc::new(
            osg_ships::starter(osg_ships::EXAMPLE_CONTROLLER.to_vec())
                .compile(&catalogue)
                .unwrap(),
        );
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let observer = app.world_mut().spawn_empty().id();
        let target = app.world_mut().spawn(object(DVec3::X * 100.0, 2.0)).id();
        let hollow_ship = app
            .world_mut()
            .spawn((object(DVec3::X * 50.0, 5.0), vessel::ShipDesign(design)))
            .id();

        app.update();
        let index = app.world().resource::<SpatialIndex>();
        let target_index = index.object_index(target).unwrap();
        assert!(index.occluded(observer, target_index, GalacticPosition::ZERO));
        assert!(!index.fully_occluded(observer, target_index, GalacticPosition::ZERO));

        app.world_mut()
            .entity_mut(hollow_ship)
            .remove::<vessel::ShipDesign>();
        app.update();
        let index = app.world().resource::<SpatialIndex>();
        let target_index = index.object_index(target).unwrap();
        assert!(index.occluded(observer, target_index, GalacticPosition::ZERO));
        assert!(index.fully_occluded(observer, target_index, GalacticPosition::ZERO));
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
        let catalogue = osg_ships::Catalogue::builtin();
        let design = std::sync::Arc::new(
            osg_ships::starter(osg_ships::EXAMPLE_CONTROLLER.to_vec())
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
                    .spawn(hardware::Device(osg_ships::DeviceState::default()))
                    .id()
            })
            .collect();
        let engine = design
            .parts
            .iter()
            .position(|part| {
                matches!(
                    part.definition.equipment,
                    osg_ships::Equipment::Engine { .. }
                        | osg_ships::Equipment::ThermalEngine { .. }
                        | osg_ships::Equipment::MicropulseEngine { .. }
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
}
