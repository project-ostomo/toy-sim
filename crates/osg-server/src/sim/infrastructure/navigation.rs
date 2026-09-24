use crate::sim::{identity, ownership, physics, precision, registry, spatial, travel};
use bevy::{math::DVec3, prelude::*};
use osg_model::*;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[derive(Resource)]
pub struct NavigationPublication {
    pub directory: Arc<InhabitedDirectory>,
    pub revision: u64,
    pub beacons: BTreeMap<Id, NavigationBeacon>,
    hash: [u8; 32],
    retained_assets: VecDeque<[u8; 32]>,
}

pub fn publish_navigation(world: &mut World) {
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
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
                let containing = world
                    .get_resource::<crate::sim::orrery::activity::ActiveSystems>()
                    .map_or_else(
                        || universe.containing_segment(pose.translation_um, DVec3::ZERO),
                        |active| active.systems_at(&universe, pose.translation_um),
                    );
                let mut systems: Vec<_> = containing
                    .into_iter()
                    .map(|index| Id(universe.systems()[index].id))
                    .collect();
                systems.sort_unstable();
                systems.dedup();
                inhabited.extend(systems.iter().copied());
                let directory = &world.resource::<ownership::Directory>().0;
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
    let society = &world.resource::<ownership::Directory>().0;
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
    world.insert_resource(NavigationPublication {
        directory: if membership_changed {
            Arc::new(directory)
        } else {
            previous.as_ref().unwrap().directory.clone()
        },
        revision,
        beacons,
        hash,
        retained_assets,
    });
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
    let universe = &world.resource::<registry::UniverseRegistry>().universe;
    let mut nearby_systems = BTreeSet::new();
    for position in views
        .iter()
        .map(|view| view.origin)
        .chain(ships.iter().filter_map(|ship| {
            world
                .get::<precision::PreciseTransform>(*ship)
                .map(|pose| pose.translation_um)
        }))
    {
        nearby_systems.extend(
            universe
                .containing_segment(position, DVec3::ZERO)
                .into_iter()
                .map(|index| Id(universe.systems()[index].id)),
        );
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
                    targets.extend(
                        publication
                            .beacons
                            .values()
                            .filter(|beacon| beacon.systems.contains(system))
                            .map(|beacon| beacon.id),
                    );
                }
            }
        }
    }
    let route_targets = targets.iter().filter_map(|id| publication.beacons.get(id));
    let local = publication.beacons.values().filter(|beacon| {
        !targets.contains(&beacon.id)
            && beacon
                .systems
                .iter()
                .any(|system| nearby_systems.contains(system))
    });
    let beacons = route_targets
        .chain(local)
        .take(osg_protocol::navigation::MAX_LIVE_BEACONS)
        .cloned()
        .collect();
    Arc::new(NavigationSnapshot {
        directory: Some(publication.hash),
        beacons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        world.init_resource::<ownership::Directory>();
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
            let directory = &mut world.resource_mut::<ownership::Directory>().0;
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
