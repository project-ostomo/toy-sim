use super::Landmark;
use crate::sim::{identity, physics, precision, registry, spatial, travel};
use anyhow::Result;
use bevy::{math::DVec3, prelude::*};
use std::sync::Arc;
use toy_sim_model::*;

#[derive(Resource)]
pub struct NavigationPublication {
    pub catalogue: Arc<NavigationCatalogue>,
    hash: [u8; 32],
    systems: std::collections::BTreeMap<Id, Vec<Id>>,
}

fn current_topology_revision(world: &mut World) -> u64 {
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
    let mut records: Vec<_> = world
        .query_filtered::<(
            &identity::Identity,
            Option<&Landmark>,
            &precision::PreciseTransform,
            Option<&travel::Gate>,
            Has<travel::DockingBays>,
            &identity::Transponder,
            &spatial::SpatialBody,
        ), (With<identity::BeaconEmitter>, Without<travel::Dormant>)>()
        .iter(world)
        .map(|(identity, landmark, pose, gate, docking, iff, body)| {
            let system = landmark.map_or_else(
                || {
                    let index = universe
                        .index
                        .nearest(pose.translation_um)
                        .expect("nonempty universe");
                    registry::system_identity(&universe.systems[index].solver.name)
                },
                |landmark| landmark.system,
            );
            (
                identity.0,
                system,
                gate.filter(|gate| gate.enabled).map(|gate| gate.paired),
                docking,
                beacon_metadata(
                    landmark
                        .map(|landmark| landmark.name.as_str())
                        .or_else(|| iff.0.labels.iter().next().map(String::as_str))
                        .unwrap_or("Station beacon"),
                    gate.map_or(body.radius_m, |gate| gate.radius_m),
                ),
            )
        })
        .collect();
    records.sort_by_key(|record| record.0);
    topology_revision(records)
}

pub fn publish_navigation(world: &mut World) {
    let revision = current_topology_revision(world);
    if world
        .get_resource::<NavigationPublication>()
        .is_some_and(|published| published.catalogue.topology_revision == revision)
    {
        return;
    }
    let catalogue = Arc::new(build_catalogue(world));
    install_navigation(world, catalogue);
}

fn install_navigation(world: &mut World, catalogue: Arc<NavigationCatalogue>) {
    let bytes = toy_sim_protocol::navigation::encode_catalogue(&catalogue)
        .expect("valid authoritative navigation catalogue");
    let hash = *blake3::hash(&bytes).as_bytes();
    world
        .resource::<identity::AppearanceAssets>()
        .insert(hash, bytes);
    let mut systems = std::collections::BTreeMap::<Id, Vec<Id>>::new();
    for beacon in &catalogue.beacons {
        systems.entry(beacon.system).or_default().push(beacon.id);
    }
    world.insert_resource(NavigationPublication {
        catalogue,
        hash,
        systems,
    });
}

pub(crate) fn capture_navigation(world: &World) -> Vec<u8> {
    toy_sim_protocol::navigation::encode_catalogue(
        &world.resource::<NavigationPublication>().catalogue,
    )
    .expect("valid navigation publication")
}

pub(crate) fn restore_navigation(world: &mut World, bytes: &[u8]) -> Result<()> {
    let catalogue = toy_sim_protocol::navigation::decode_catalogue(bytes)?;
    install_navigation(world, Arc::new(catalogue));
    publish_navigation(world);
    Ok(())
}

#[cfg(test)]
pub fn catalogue(world: &mut World) -> Arc<NavigationCatalogue> {
    if !world.contains_resource::<NavigationPublication>() {
        publish_navigation(world);
    }
    world.resource::<NavigationPublication>().catalogue.clone()
}

pub fn navigation_snapshot(
    world: &mut World,
    views: &[ViewState],
    ships: &[Entity],
) -> Arc<NavigationSnapshot> {
    use std::collections::BTreeSet;
    use toy_sim_model::travel::{Destination, Order, Reference};
    if !world.contains_resource::<NavigationPublication>() {
        publish_navigation(world);
    }
    let published = world.resource::<NavigationPublication>();
    let universe = &world.resource::<registry::UniverseRegistry>().universe;
    let mut systems = BTreeSet::new();
    let mut beacons = BTreeSet::new();
    for origin in views
        .iter()
        .map(|view| view.origin)
        .chain(ships.iter().filter_map(|&ship| {
            world
                .get::<precision::PreciseTransform>(ship)
                .map(|pose| pose.translation_um)
        }))
    {
        for index in universe.index.containing_segment(origin, DVec3::ZERO) {
            systems.insert(registry::system_identity(
                &universe.systems[index].solver.name,
            ));
        }
    }
    for system in systems {
        if let Some(members) = published.systems.get(&system) {
            beacons.extend(members.iter().copied());
        }
    }
    for &ship in ships {
        if let Some(travel) = world.get::<travel::Travel>(ship) {
            for order in travel.0.orders.iter().skip(travel.0.order) {
                let destination = match &order.action {
                    Order::Jump(id) | Order::Dock(id) => {
                        beacons.insert(*id);
                        None
                    }
                    action => order_destination(action),
                };
                match destination {
                    Some(Destination::Beacon(id)) => {
                        beacons.insert(*id);
                    }
                    Some(Destination::Relative {
                        reference: Reference::Beacon(id),
                        ..
                    }) => {
                        beacons.insert(*id);
                    }
                    _ => {}
                }
            }
        }
    }
    let live = beacons
        .into_iter()
        .filter_map(|id| {
            let entity = identity::lookup(world, id).ok()?;
            if world.get::<travel::Dormant>(entity).is_some() {
                return None;
            }
            let template = published
                .catalogue
                .beacons
                .binary_search_by_key(&id, |beacon| beacon.id)
                .ok()
                .map(|index| &published.catalogue.beacons[index])?;
            let pose = world.get::<precision::PreciseTransform>(entity)?;
            let mut beacon = template.clone();
            beacon.pose = crate::sim::intelligence::pose(
                pose,
                world.get::<physics::Velocity>(entity),
                world.get::<physics::AngularVelocity>(entity),
            );
            Some(beacon)
        })
        .collect();
    Arc::new(NavigationSnapshot {
        catalogue: Some(published.hash),
        beacons: live,
        ephemerides: route_ephemerides(world, views, ships),
    })
}

fn order_destination(
    action: &toy_sim_model::travel::Order,
) -> Option<&toy_sim_model::travel::Destination> {
    use toy_sim_model::travel::{Order, Target};
    match action {
        Order::TravelTo(destination)
        | Order::Sublight(destination)
        | Order::Slip { destination } => Some(destination),
        Order::Guidance(guidance) => match &guidance.target {
            Target::Destination(destination) => Some(destination),
            _ => None,
        },
        _ => None,
    }
}

fn route_ephemerides(
    world: &World,
    views: &[ViewState],
    ships: &[Entity],
) -> Vec<CelestialSystemRef> {
    use std::collections::BTreeMap;
    use toy_sim_model::travel::{Destination, Reference};

    let registry = world.resource::<registry::UniverseRegistry>();
    let mut references = BTreeMap::new();
    for view in views {
        let Some(ship) = view
            .focused_ship
            .and_then(|id| identity::lookup(world, id).ok())
        else {
            continue;
        };
        if !ships.contains(&ship) {
            continue;
        }
        let Some(route) = world.get::<travel::Travel>(ship) else {
            continue;
        };
        for order in route.0.orders.iter().skip(route.0.order) {
            let Some(Destination::Relative {
                reference: Reference::Celestial(body),
                ..
            }) = order_destination(&order.action)
            else {
                continue;
            };
            if let Some(reference) = registry.celestial_ref(view.id, *body) {
                references.insert((view.id, reference.system), reference);
            }
        }
    }
    references.into_values().collect()
}

fn build_catalogue(world: &mut World) -> NavigationCatalogue {
    let map = toy_sim_universe::civilization::map();
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
    let systems: Vec<_> = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .systems
        .iter()
        .enumerate()
        .map(|(index, system)| NavigationSystem {
            id: registry::system_identity(&system.solver.name),
            name: system.solver.name.to_string(),
            position: system.solver.anchor,
            sovereignty: map
                .systems
                .get(index)
                .filter(|settlement| settlement.name == system.solver.name)
                .map(|settlement| crate::sim::ownership::sovereignty_id(&settlement.sovereignty)),
            population: map
                .systems
                .get(index)
                .filter(|settlement| settlement.name == system.solver.name)
                .map_or(0, |settlement| settlement.population),
        })
        .collect();
    let mut beacons: Vec<_> = world
        .query_filtered::<(
            &identity::Identity,
            Option<&Landmark>,
            &identity::Transponder,
            &precision::PreciseTransform,
            Option<&physics::Velocity>,
            Option<&physics::AngularVelocity>,
            &spatial::SpatialBody,
            Option<&travel::Gate>,
            Option<&travel::DockingBays>,
        ), (With<identity::BeaconEmitter>, Without<travel::Dormant>)>()
        .iter(world)
        .map(
            |(id, landmark, iff, pose, velocity, angular, spatial, gate, bays)| NavigationBeacon {
                id: id.0,
                system: landmark.map_or_else(
                    || {
                        let index = universe
                            .index
                            .nearest(pose.translation_um)
                            .expect("nonempty universe");
                        registry::system_identity(&universe.systems[index].solver.name)
                    },
                    |landmark| landmark.system,
                ),
                name: landmark.map_or_else(
                    || {
                        iff.0
                            .labels
                            .iter()
                            .next()
                            .cloned()
                            .unwrap_or_else(|| "Station beacon".into())
                    },
                    |landmark| landmark.name.clone(),
                ),
                pose: crate::sim::intelligence::pose(pose, velocity, angular),
                radius_m: gate.map_or(spatial.radius_m, |g| g.radius_m),
                gate_exit: gate.filter(|g| g.enabled).map(|g| g.paired),
                docking: bays.is_some(),
            },
        )
        .collect();
    beacons.sort_by_key(|beacon| beacon.id);
    let topology_revision = topology_revision(beacons.iter().map(|beacon| {
        (
            beacon.id,
            beacon.system,
            beacon.gate_exit,
            beacon.docking,
            beacon_metadata(&beacon.name, beacon.radius_m),
        )
    }));
    NavigationCatalogue {
        topology_revision,
        systems,
        beacons,
    }
}

fn beacon_metadata(name: &str, radius_m: f64) -> u64 {
    let mut hash = blake3::Hasher::new_derive_key("toy-sim navigation beacon metadata v1");
    hash.update(name.as_bytes());
    hash.update(&radius_m.to_bits().to_le_bytes());
    u64::from_le_bytes(hash.finalize().as_bytes()[..8].try_into().unwrap())
}

fn topology_revision(beacons: impl IntoIterator<Item = (Id, Id, Option<Id>, bool, u64)>) -> u64 {
    let mut hash = blake3::Hasher::new_derive_key("toy-sim navigation topology v1");
    static MAP_FINGERPRINT: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    let map_fingerprint = MAP_FINGERPRINT.get_or_init(|| {
        let map = toy_sim_universe::civilization::map();
        let mut hash = blake3::Hasher::new_derive_key("toy-sim inhabited map identity v1");
        hash.update(&map.generation_version.to_le_bytes());
        for system in &map.systems {
            hash.update(system.catalogue_id.as_bytes());
            hash.update(system.name.as_bytes());
            hash.update(system.sovereignty.as_bytes());
            hash.update(&system.population.to_le_bytes());
            hash.update(&system.position.x.to_le_bytes());
            hash.update(&system.position.y.to_le_bytes());
            hash.update(&system.position.z.to_le_bytes());
        }
        *hash.finalize().as_bytes()
    });
    hash.update(map_fingerprint);
    for (id, system, exit, docking, metadata) in beacons {
        hash.update(&id.0);
        hash.update(&system.0);
        hash.update(&exit.unwrap_or_default().0);
        hash.update(&[exit.is_some() as u8, docking as u8]);
        hash.update(&metadata.to_le_bytes());
    }
    u64::from_le_bytes(hash.finalize().as_bytes()[..8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_waypoint_ephemerides_do_not_expand_visible_systems() {
        use toy_sim_model::travel::{
            Axes, Destination, Guidance, GuidanceMode, Order, Reference, Target, TravelState,
        };

        let mut app = crate::sim::provision(&[Id::new()], None, None).unwrap();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<crate::sim::vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let station = world
            .query_filtered::<Entity, With<travel::DockingBays>>()
            .single(world)
            .unwrap();
        let ship_id = world.get::<identity::Identity>(ship).unwrap().0;
        let origin = world
            .get::<precision::PreciseTransform>(ship)
            .unwrap()
            .translation_um;
        let registry = world.resource::<registry::UniverseRegistry>().clone();
        let body = |system: usize| registry::identity(&registry.universe.systems[system].star_name);
        let relative = |body| Destination::Relative {
            reference: Reference::Celestial(body),
            offset: GalacticPosition::from_meters(DVec3::X * 1e10),
            axes: Axes::Galactic,
        };
        world.entity_mut(ship).insert(travel::Travel(TravelState {
            orders: vec![
                Order::TravelTo(relative(body(11))).into(),
                Order::TravelTo(relative(registry::identity("Earth"))).into(),
                Order::Sublight(relative(registry::identity("Mars"))).into(),
                Order::Slip {
                    destination: relative(body(10)),
                }
                .into(),
                Order::Guidance(Guidance {
                    mode: GuidanceMode::Approach,
                    target: Target::Destination(relative(body(12))),
                    range_m: 0.0,
                })
                .into(),
            ],
            order: 1,
            ..Default::default()
        }));
        world
            .entity_mut(station)
            .insert(travel::Travel(TravelState {
                orders: vec![Order::TravelTo(relative(body(13))).into()],
                ..Default::default()
            }));
        let views: Vec<_> = [17, 39]
            .into_iter()
            .map(|id| ViewState {
                id,
                focused_ship: Some(ship_id),
                origin,
                revision: 0,
                group: Id::default(),
                tracks: Vec::new(),
                completion: Completion::Complete,
            })
            .collect();
        let visible = registry.system_refs(&mut views.clone(), None);
        let snapshot = navigation_snapshot(world, &views, &[ship, station]);
        assert_eq!(snapshot.ephemerides.len(), 6);
        for view in &views {
            for target in [registry::identity("Earth"), body(10), body(12)] {
                let expected = registry.celestial_ref(view.id, target).unwrap();
                assert!(snapshot.ephemerides.contains(&expected));
                assert!(
                    !visible
                        .iter()
                        .any(|reference| reference.system == expected.system)
                );
                let bytes = world
                    .resource::<identity::AppearanceAssets>()
                    .get(&expected.definition)
                    .unwrap();
                let definition =
                    toy_sim_universe::replication::SystemAsset::decode(&bytes).unwrap();
                assert!(definition.body_ids.iter().any(|body| body.id == target.0));
            }
        }
        assert_eq!(registry.system_refs(&mut views.clone(), None), visible);
        assert!(
            navigation_snapshot(world, &views, &[station])
                .ephemerides
                .is_empty()
        );
        assert!(
            navigation_snapshot(world, &[], &[ship, station])
                .ephemerides
                .is_empty()
        );

        world
            .get_mut::<travel::Travel>(ship)
            .unwrap()
            .0
            .orders
            .clear();
        assert!(
            navigation_snapshot(world, &views, &[ship, station])
                .ephemerides
                .is_empty()
        );
        assert_eq!(registry.system_refs(&mut views.clone(), None), visible);
    }

    #[test]
    fn navigation_asset_changes_for_metadata_and_keeps_live_motion_separate() {
        let mut app = crate::sim::provision(&[Id::new()], None, None).unwrap();
        let world = app.world_mut();
        let before = world.resource::<NavigationPublication>().hash;
        let station = world
            .query_filtered::<Entity, With<travel::DockingBays>>()
            .iter(world)
            .next()
            .unwrap();
        let position = world
            .get::<precision::PreciseTransform>(station)
            .unwrap()
            .translation_um;
        world
            .get_mut::<precision::PreciseTransform>(station)
            .unwrap()
            .translation_um = position.offset_by(DVec3::X * 100.0);
        publish_navigation(world);
        assert_eq!(world.resource::<NavigationPublication>().hash, before);
        let snapshot = navigation_snapshot(world, &[], &[station]);
        let station_id = world.get::<identity::Identity>(station).unwrap().0;
        let live = snapshot
            .beacons
            .iter()
            .find(|beacon| beacon.id == station_id)
            .unwrap();
        assert_eq!(
            live.pose.position,
            world
                .get::<precision::PreciseTransform>(station)
                .unwrap()
                .translation_um
        );
        assert!(snapshot.beacons.len() < 16);

        world.get_mut::<Landmark>(station).unwrap().name = "Renamed anchorage".into();
        publish_navigation(world);
        let after = world.resource::<NavigationPublication>().hash;
        assert_ne!(before, after);
        let bytes = world
            .resource::<identity::AppearanceAssets>()
            .get(&after)
            .unwrap();
        let catalogue = toy_sim_protocol::navigation::decode_catalogue(&bytes).unwrap();
        assert_eq!(
            catalogue
                .beacons
                .iter()
                .find(|beacon| beacon.id == station_id)
                .unwrap()
                .name,
            "Renamed anchorage"
        );
    }
}
