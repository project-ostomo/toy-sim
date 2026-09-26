use crate::sim::{identity, ownership, physics, precision, registry, spatial, travel};
use bevy::{math::DVec3, prelude::*};
use osg_model::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Resource)]
pub struct NavigationPublication {
    pub directory: Arc<InhabitedDirectory>,
    pub revision: u64,
    pub beacons: BTreeMap<Id, NavigationBeacon>,
    by_system: BTreeMap<Id, Vec<Id>>,
    hash: [u8; 32],
    retained_assets: VecDeque<[u8; 32]>,
}

type SelectionKey = (Vec<Id>, Vec<Id>);

#[derive(Resource, Default)]
struct NavigationQueries {
    membership: Mutex<HashMap<GalacticPosition, Arc<Vec<Id>>>>,
    selections: Mutex<HashMap<SelectionKey, Arc<NavigationSnapshot>>>,
}

impl NavigationQueries {
    fn systems_at(&self, world: &World, position: GalacticPosition) -> Arc<Vec<Id>> {
        if let Some(systems) = self.membership.lock().unwrap().get(&position) {
            #[cfg(test)]
            crate::sim::diagnostics::samples::count("navigation.membership_hits", 1);
            return systems.clone();
        }
        let _profile = crate::sim::diagnostics::ProfileScope::new("navigation.resolve_membership");
        #[cfg(test)]
        crate::sim::diagnostics::samples::count("navigation.membership_resolutions", 1);
        let universe = &world.resource::<registry::UniverseRegistry>().universe;
        let containing = world
            .get_resource::<crate::sim::orrery::activity::ActiveSystems>()
            .map_or_else(
                || universe.containing_segment(position, DVec3::ZERO),
                |active| active.systems_at(universe, position),
            );
        let mut systems: Vec<_> = containing
            .into_iter()
            .map(|index| Id(universe.systems()[index].id))
            .collect();
        systems.sort_unstable();
        systems.dedup();
        let systems = Arc::new(systems);
        self.membership
            .lock()
            .unwrap()
            .insert(position, systems.clone());
        systems
    }
}

/// Start a new navigation publication batch, including when simulation is paused.
fn begin_publication(world: &mut World) {
    world.init_resource::<NavigationQueries>();
    let mut queries = world.resource_mut::<NavigationQueries>();
    queries.membership.get_mut().unwrap().clear();
    queries.selections.get_mut().unwrap().clear();
}

/// Begin a new publication batch and publish current beacon state. Call before
/// display updates and session preparation, even when simulation is paused.
pub fn publish_navigation(world: &mut World) {
    begin_publication(world);
    let mut inhabited = BTreeSet::new();
    let mut votes: BTreeMap<Id, (usize, BTreeMap<Id, usize>)> = BTreeMap::new();
    let beacons: BTreeMap<_, _> = world
        .query_filtered::<(
            &identity::Identity,
            &identity::Transponder,
            &precision::PreciseTransform,
            Option<&physics::Velocity>,
            Option<&physics::AngularVelocity>,
            &spatial::SpatialBody,
            Has<travel::DockingBays>,
            Has<identity::NavigationBeaconEmitter>,
            Option<&ownership::AssetOwner>,
        ), (With<identity::DirectoryEmitter>, Without<travel::Dormant>)>()
        .iter(world)
        .filter(|(_, iff, ..)| iff.0.enabled)
        .map(
            |(id, iff, pose, velocity, angular, body, docking, navigation, owner)| {
                let systems = world
                    .resource::<NavigationQueries>()
                    .systems_at(world, pose.translation_um)
                    .as_ref()
                    .clone();
                inhabited.extend(systems.iter().copied());
                let directory = &(&world
                    .resource::<crate::sim::society::SocietyState>()
                    .directory)
                    .0;
                let organization = owner.and_then(|owner| {
                    directory
                        .lineage(owner.0)
                        .into_iter()
                        .find_map(|principal| {
                            if let osg_model::ownership::Principal::Organization(id) = principal {
                                Some(id)
                            } else {
                                None
                            }
                        })
                });
                for system in &systems {
                    let (total, organizations) = votes.entry(*system).or_default();
                    *total += 1;
                    if let Some(organization) = organization {
                        *organizations.entry(organization).or_default() += 1;
                    }
                }
                let beacon = NavigationBeacon {
                    id: id.0,
                    systems,
                    name: iff
                        .0
                        .labels
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_else(|| "Public installation".into()),
                    pose: crate::sim::identity::pose(pose, velocity, angular),
                    radius_m: body.radius_m,
                    docking,
                    navigation,
                };
                (id.0, beacon)
            },
        )
        .collect();
    let society = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    let ownership: BTreeMap<_, _> = votes
        .into_iter()
        .filter_map(|(system, (total, votes))| {
            let (organization, _) = votes.into_iter().find(|(_, count)| *count > total / 2)?;
            Some((
                system,
                society.organizations.get(&organization)?.sovereignty,
            ))
        })
        .collect();
    let sovereignties = ownership
        .values()
        .filter_map(|id| {
            let sovereignty = society.sovereignties.get(id)?;
            Some((
                *id,
                PublicSovereignty {
                    id: *id,
                    name: sovereignty.name.clone(),
                    bloc: sovereignty.bloc,
                },
            ))
        })
        .collect();
    let directory = InhabitedDirectory {
        systems: inhabited.into_iter().collect(),
        ownership,
        sovereignties,
    };
    let previous = world.remove_resource::<NavigationPublication>();
    let membership_changed = previous
        .as_ref()
        .is_none_or(|old| *old.directory != directory);
    let topology_changed = previous.as_ref().is_none_or(|old| {
        old.beacons.len() != beacons.len()
            || beacons.iter().any(|(id, beacon)| {
                old.beacons.get(id).is_none_or(|old| {
                    old.systems != beacon.systems
                        || old.navigation != beacon.navigation
                        || old.docking != beacon.docking
                        || old.name != beacon.name
                })
            })
    });
    let mut retained_assets = previous
        .as_ref()
        .map(|old| old.retained_assets.clone())
        .unwrap_or_default();
    let assets = world.resource::<identity::AppearanceAssets>();
    let hash = if membership_changed {
        let bytes = osg_protocol::navigation::encode_directory(&directory)
            .expect("valid inhabited directory");
        let hash = *blake3::hash(&bytes).as_bytes();
        assets.insert(hash, bytes);
        retained_assets.retain(|old| *old != hash);
        retained_assets.push_back(hash);
        while retained_assets.len() > 2 {
            assets.remove(&retained_assets.pop_front().unwrap());
        }
        hash
    } else {
        previous.as_ref().unwrap().hash
    };
    let revision = previous.as_ref().map_or(1, |old| {
        old.revision + u64::from(topology_changed || membership_changed)
    });
    let by_system = index_beacons(&beacons);
    world.insert_resource(NavigationPublication {
        directory: if membership_changed {
            Arc::new(directory)
        } else {
            previous.as_ref().unwrap().directory.clone()
        },
        revision,
        beacons,
        by_system,
        hash,
        retained_assets,
    });
}

fn index_beacons(beacons: &BTreeMap<Id, NavigationBeacon>) -> BTreeMap<Id, Vec<Id>> {
    let _profile = crate::sim::diagnostics::ProfileScope::new("navigation.index_beacons");
    let mut by_system: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for (&id, beacon) in beacons {
        for &system in &beacon.systems {
            let ids = by_system.entry(system).or_default();
            if ids.last() != Some(&id) {
                ids.push(id);
            }
        }
    }
    by_system
}

fn beacons_in_systems(
    publication: &NavigationPublication,
    systems: impl IntoIterator<Item = Id>,
) -> BTreeSet<Id> {
    systems
        .into_iter()
        .filter_map(|system| publication.by_system.get(&system))
        .flatten()
        .copied()
        .collect()
}

pub fn navigation_snapshot(
    world: &mut World,
    views: &[ViewState],
    ships: &[Entity],
) -> Arc<NavigationSnapshot> {
    use osg_model::travel::Directive;

    if !world.contains_resource::<NavigationPublication>() {
        publish_navigation(world);
    }
    let publication = world.resource::<NavigationPublication>();
    let queries = world.resource::<NavigationQueries>();
    let mut nearby_systems = BTreeSet::new();
    let positions: HashSet<_> = views
        .iter()
        .map(|view| view.origin)
        .chain(ships.iter().filter_map(|ship| {
            crate::sim::session::ship_pose(world, *ship).map(|pose| pose.position)
        }))
        .collect();
    for position in positions {
        nearby_systems.extend(queries.systems_at(world, position).iter().copied());
    }
    let mut targets = BTreeSet::new();
    for &ship in ships {
        let Some(travel) = world.get::<travel::Travel>(ship) else {
            continue;
        };
        for entry in &travel.0.itinerary {
            match &entry.directive {
                Directive::DockAt(id) => {
                    targets.insert(*id);
                }
                Directive::SlipToSystem(system) => {
                    targets.extend(beacons_in_systems(publication, [*system]));
                }
            }
        }
    }
    let _profile = crate::sim::diagnostics::ProfileScope::new("navigation.select_beacons");
    let key = (
        nearby_systems.iter().copied().collect(),
        targets.iter().copied().collect(),
    );
    if let Some(snapshot) = queries.selections.lock().unwrap().get(&key) {
        #[cfg(test)]
        crate::sim::diagnostics::samples::count("navigation.selection_hits", 1);
        return snapshot.clone();
    }
    #[cfg(test)]
    crate::sim::diagnostics::samples::count("navigation.selection_builds", 1);
    let route_targets = targets.iter().filter_map(|id| publication.beacons.get(id));
    let local_ids = beacons_in_systems(publication, nearby_systems);
    let local = local_ids
        .difference(&targets)
        .filter_map(|id| publication.beacons.get(id));
    let beacons = route_targets
        .chain(local)
        .take(osg_protocol::navigation::MAX_LIVE_BEACONS)
        .cloned()
        .collect();
    let snapshot = Arc::new(NavigationSnapshot {
        directory: Some(publication.hash),
        beacons,
    });
    queries
        .selections
        .lock()
        .unwrap()
        .insert(key, snapshot.clone());
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(registry::UniverseRegistry {
            universe: Arc::new(
                osg_universe::universe::Universe::init(osg_universe::example_config()).unwrap(),
            ),
        });
        world.init_resource::<identity::AppearanceAssets>();
        world.init_resource::<crate::sim::society::SocietyState>();
        publish_navigation(&mut world);
        world
    }

    #[test]
    fn indexed_selection_matches_scan_order_and_limit_and_shares_snapshots() {
        let mut world = world();
        let universe = &world.resource::<registry::UniverseRegistry>().universe;
        let origin = universe.systems()[0].position;
        let local_system = Id(universe.systems()[0].id);
        let remote_system = Id::new();
        let mut publication = world.resource_mut::<NavigationPublication>();
        for i in 1..=1200_u128 {
            let id = Id(i.to_be_bytes());
            publication.beacons.insert(
                id,
                NavigationBeacon {
                    id,
                    systems: if i % 3 == 0 {
                        vec![local_system, remote_system]
                    } else {
                        vec![local_system]
                    },
                    name: "Beacon".into(),
                    pose: Pose::default(),
                    radius_m: 1.0,
                    docking: true,
                    navigation: true,
                },
            );
        }
        publication.by_system = index_beacons(&publication.beacons);
        let targets: BTreeSet<_> = publication
            .beacons
            .values()
            .filter(|beacon| beacon.systems.contains(&remote_system))
            .map(|beacon| beacon.id)
            .chain([Id(1199_u128.to_be_bytes())])
            .collect();
        let expected: Vec<_> = targets
            .iter()
            .copied()
            .chain(
                publication
                    .beacons
                    .values()
                    .filter(|beacon| {
                        !targets.contains(&beacon.id) && beacon.systems.contains(&local_system)
                    })
                    .map(|beacon| beacon.id),
            )
            .take(osg_protocol::navigation::MAX_LIVE_BEACONS)
            .collect();
        let ship = world
            .spawn((
                precision::PreciseTransform {
                    translation_um: origin,
                    ..Default::default()
                },
                travel::Travel(osg_model::travel::AutopilotState {
                    itinerary: [
                        osg_model::travel::Directive::SlipToSystem(remote_system),
                        osg_model::travel::Directive::DockAt(Id(1199_u128.to_be_bytes())),
                    ]
                    .into_iter()
                    .map(|directive| osg_model::travel::ItineraryEntry {
                        directive,
                        label: String::new(),
                    })
                    .collect(),
                    ..Default::default()
                }),
            ))
            .id();
        let views = [
            ViewState {
                focused_ship: None,
                origin,
                id: 1,
                revision: 0,
            },
            ViewState {
                focused_ship: None,
                origin,
                id: 2,
                revision: 0,
            },
        ];
        let snapshot = navigation_snapshot(&mut world, &views, &[ship]);
        assert_eq!(
            snapshot
                .beacons
                .iter()
                .map(|beacon| beacon.id)
                .collect::<Vec<_>>(),
            expected
        );
        let second = navigation_snapshot(&mut world, &views, &[ship]);
        assert!(Arc::ptr_eq(&snapshot, &second));
        assert_eq!(
            world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .len(),
            1
        );
        begin_publication(&mut world);
        assert!(!Arc::ptr_eq(
            &snapshot,
            &navigation_snapshot(&mut world, &views, &[ship])
        ));
    }

    #[test]
    fn membership_matches_exact_query_and_caches_empty_positions() {
        let mut world = world();
        let universe = world
            .resource::<registry::UniverseRegistry>()
            .universe
            .clone();
        let definition = universe.resolve_index(0).unwrap();
        for scale in [0.0, 0.999999, 1.0, 1.000001, 1000.0] {
            let position = definition
                .solver
                .anchor
                .offset_by(DVec3::X * definition.influence * scale);
            let mut expected: Vec<_> = universe
                .containing_segment(position, DVec3::ZERO)
                .into_iter()
                .map(|index| Id(universe.systems()[index].id))
                .collect();
            expected.sort_unstable();
            expected.dedup();
            let queries = world.resource::<NavigationQueries>();
            let actual = queries.systems_at(&world, position);
            assert_eq!(*actual, expected);
            assert!(Arc::ptr_eq(&actual, &queries.systems_at(&world, position)));
        }
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cached_regions_preserve_overlap_and_exact_boundaries_after_movement() {
        use crate::sim::orrery::activity::{ActiveSystems, activate};
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let first = osg_universe::example_config();
        let mut second = first.clone();
        second.key = "overlapping-system".into();
        second.name = "Overlapping system".into();
        let universe = Arc::new(
            osg_universe::universe::Universe::from_configs(vec![first, second], 1e-8).unwrap(),
        );
        let origin = universe.systems()[0].position;
        let influence = universe.resolve_index(0).unwrap().influence;
        let mut app = App::new();
        app.insert_resource(crate::sim::orrery::Universe(universe.clone()))
            .insert_resource(registry::UniverseRegistry {
                universe: universe.clone(),
            })
            .init_resource::<Time<Fixed>>()
            .init_resource::<ActiveSystems>()
            .init_resource::<identity::AppearanceAssets>()
            .init_resource::<crate::sim::society::SocietyState>()
            .add_systems(Update, activate);
        let entity = app
            .world_mut()
            .spawn((
                precision::PreciseTransform {
                    translation_um: origin,
                    ..Default::default()
                },
                spatial::SpatialBody {
                    radius_m: 1.0,
                    occludes: false,
                },
            ))
            .id();
        for scale in [0.0, 0.999999, 1.000001, 1000.0, 0.0] {
            let position = origin.offset_by(DVec3::X * influence * scale);
            app.world_mut()
                .get_mut::<precision::PreciseTransform>(entity)
                .unwrap()
                .translation_um = position;
            app.update();
            let world = app.world_mut();
            publish_navigation(world);
            let actual = world
                .resource::<NavigationQueries>()
                .systems_at(world, position);
            let expected: BTreeSet<_> = universe
                .containing_segment(position, DVec3::ZERO)
                .into_iter()
                .map(|index| Id(universe.systems()[index].id))
                .collect();
            assert_eq!(actual.iter().copied().collect::<BTreeSet<_>>(), expected);
            if scale == 0.0 {
                assert_eq!(actual.len(), 2);
            }
        }
    }

    #[test]
    fn membership_uses_resolved_docked_and_transit_positions_between_batches() {
        let mut world = world();
        world.init_resource::<identity::IdentityIndex>();
        let origin = world
            .resource::<registry::UniverseRegistry>()
            .universe
            .systems()[0]
            .position;
        let stale = origin.offset_by(DVec3::X * 1e20);
        let host_id = Id::new();
        let host = world
            .spawn((
                identity::Identity(host_id),
                precision::PreciseTransform {
                    translation_um: origin,
                    ..Default::default()
                },
                travel::DockingBays(vec![travel::Bay {
                    centre_m: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    radius_m: 100.0,
                    mass_capacity_kg: 1e12,
                    public: true,
                    allowed: BTreeSet::new(),
                    reservation: None,
                }]),
            ))
            .id();
        let ship = world
            .spawn((
                precision::PreciseTransform {
                    translation_um: stale,
                    ..Default::default()
                },
                travel::PresenceState(osg_model::travel::Presence::Docked {
                    host: host_id,
                    bay: 0,
                }),
            ))
            .id();
        navigation_snapshot(&mut world, &[], &[ship]);
        assert!(
            world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .contains_key(&origin)
        );
        assert!(
            !world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .contains_key(&stale)
        );

        world.entity_mut(ship).remove::<travel::PresenceState>();
        world.entity_mut(ship).insert(travel::Transit {
            ignored_capture_body: None,
            origin,
            position: origin,
            destination: stale,
            departed: 0,
            advanced_tick: 0,
            direction: [1.0, 0.0, 0.0],
            speed_ly_s: 1.0,
            retained_velocity: [0.0; 3],
            requested_delta_v: [0.0; 3],
            departure_mass_kg: 1.0,
            distance_ly: 0.0,
            consumed_fuel_g: 0,
            navigation_beacon: None,
            beacon_lost: false,
            nominal_direction: [1.0, 0.0, 0.0],
            variance_m2: 0.0,
        });
        publish_navigation(&mut world);
        navigation_snapshot(&mut world, &[], &[ship]);
        assert!(
            world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .contains_key(&origin)
        );
        world.get_mut::<travel::Transit>(ship).unwrap().position = stale;
        publish_navigation(&mut world);
        navigation_snapshot(&mut world, &[], &[ship]);
        assert!(
            !world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .contains_key(&origin)
        );
        assert!(
            world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .contains_key(&stale)
        );

        world.entity_mut(ship).remove::<travel::Transit>();
        world
            .entity_mut(ship)
            .insert(travel::PresenceState(osg_model::travel::Presence::Docked {
                host: host_id,
                bay: 0,
            }));
        world.despawn(host);
        publish_navigation(&mut world);
        navigation_snapshot(&mut world, &[], &[ship]);
        assert!(
            world
                .resource::<NavigationQueries>()
                .membership
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn membership_follows_equipment_broadcasts_and_actual_influence() {
        let mut world = World::new();
        let universe =
            osg_universe::universe::Universe::init(osg_universe::example_config()).unwrap();
        let origin = universe.systems()[0].position;
        let system = Id(universe.systems()[0].id);
        world.insert_resource(registry::UniverseRegistry {
            universe: Arc::new(universe),
        });
        world.init_resource::<identity::AppearanceAssets>();
        world.init_resource::<crate::sim::society::SocietyState>();
        let spawn = |world: &mut World| {
            world
                .spawn((
                    identity::Identity(Id::new()),
                    identity::Transponder(IffIdentity {
                        owner: Id::new(),
                        faction: None,
                        labels: ["Installation".into()].into(),
                        enabled: true,
                    }),
                    precision::PreciseTransform {
                        translation_um: origin,
                        ..Default::default()
                    },
                    spatial::SpatialBody {
                        radius_m: 1.0,
                        occludes: false,
                    },
                ))
                .id()
        };
        let ordinary_ship = spawn(&mut world);
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .systems
                .is_empty()
        );

        let first = spawn(&mut world);
        world.entity_mut(first).insert(identity::DirectoryEmitter);
        let second = spawn(&mut world);
        world.entity_mut(second).insert(identity::DirectoryEmitter);
        publish_navigation(&mut world);
        assert_eq!(
            world.resource::<NavigationPublication>().directory.systems,
            vec![system]
        );

        let organization_a = Id::new();
        let organization_b = Id::new();
        let sovereignty_a = Id::new();
        let sovereignty_b = Id::new();
        for (organization, sovereignty) in [
            (organization_a, sovereignty_a),
            (organization_b, sovereignty_b),
        ] {
            let directory = &mut world
                .resource_mut::<crate::sim::society::SocietyState>()
                .map_unchanged(|state| &mut state.directory)
                .0;
            directory.sovereignties.insert(
                sovereignty,
                osg_model::ownership::Sovereignty {
                    id: sovereignty,
                    name: sovereignty.to_string(),
                    bloc: Default::default(),
                    officers: Default::default(),
                },
            );
            directory.organizations.insert(
                organization,
                osg_model::ownership::Organization {
                    id: organization,
                    name: organization.to_string(),
                    sovereignty,
                    open_membership: false,
                    officers: Default::default(),
                },
            );
        }
        world.entity_mut(first).insert(ownership::AssetOwner(
            osg_model::ownership::Principal::Organization(organization_a),
        ));
        world.entity_mut(second).insert(ownership::AssetOwner(
            osg_model::ownership::Principal::Organization(organization_b),
        ));
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .ownership
                .is_empty()
        );
        let third = spawn(&mut world);
        world.entity_mut(third).insert((
            identity::DirectoryEmitter,
            ownership::AssetOwner(osg_model::ownership::Principal::Organization(
                organization_a,
            )),
        ));
        publish_navigation(&mut world);
        assert_eq!(
            world
                .resource::<NavigationPublication>()
                .directory
                .ownership[&system],
            sovereignty_a
        );
        let old_hash = world.resource::<NavigationPublication>().hash;
        // Advertised identity cannot override the legal owner.
        world
            .get_mut::<identity::Transponder>(third)
            .unwrap()
            .0
            .faction = Some(organization_b);
        publish_navigation(&mut world);
        assert_eq!(world.resource::<NavigationPublication>().hash, old_hash);
        world.entity_mut(third).insert(ownership::AssetOwner(
            osg_model::ownership::Principal::Organization(organization_b),
        ));
        publish_navigation(&mut world);
        assert_eq!(
            world
                .resource::<NavigationPublication>()
                .directory
                .ownership[&system],
            sovereignty_b
        );
        assert_ne!(world.resource::<NavigationPublication>().hash, old_hash);
        world
            .get_mut::<identity::Transponder>(third)
            .unwrap()
            .0
            .enabled = false;
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .ownership
                .is_empty()
        );
        world.despawn(third);

        world
            .get_mut::<identity::Transponder>(first)
            .unwrap()
            .0
            .enabled = false;
        publish_navigation(&mut world);
        assert_eq!(
            world.resource::<NavigationPublication>().directory.systems,
            vec![system]
        );
        world
            .entity_mut(second)
            .remove::<identity::DirectoryEmitter>();
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .systems
                .is_empty()
        );
        assert!(world.get_entity(first).is_ok());
        assert!(world.get_entity(ordinary_ship).is_ok());

        world
            .get_mut::<identity::Transponder>(first)
            .unwrap()
            .0
            .enabled = true;
        world
            .get_mut::<precision::PreciseTransform>(first)
            .unwrap()
            .translation_um = origin.offset_by(DVec3::X * 1e20);
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .systems
                .is_empty()
        );
        world
            .get_mut::<precision::PreciseTransform>(first)
            .unwrap()
            .translation_um = origin;
        publish_navigation(&mut world);
        assert_eq!(
            world.resource::<NavigationPublication>().directory.systems,
            vec![system]
        );
        world.entity_mut(first).insert(travel::Dormant);
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .systems
                .is_empty()
        );
        world.entity_mut(first).remove::<travel::Dormant>();
        world.despawn(first);
        publish_navigation(&mut world);
        assert!(
            world
                .resource::<NavigationPublication>()
                .directory
                .systems
                .is_empty()
        );
    }
}
