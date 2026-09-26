//! Temporary account cache for publication queries.
//! Invalidate all accounts on relevant ECS changes; replace with indexed queries
//! when ownership storage supports them.

use bevy::prelude::*;
use osg_model::{AccountId, Id};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::sim::{
    identity::{Control, Identity, IdentityIndex},
    ownership::{self, AssetAccess, AssetOwner},
    vessel::Vessel,
};

pub(super) struct AccountView {
    pub ships: Vec<(Id, Entity, bool)>,
}

#[derive(Resource, Default)]
struct Cache(HashMap<AccountId, Arc<AccountView>>);

#[cfg(test)]
#[derive(Resource, Default)]
struct ScanCount(usize);

fn invalidate(
    mut cache: ResMut<Cache>,
    directory: Option<Res<crate::sim::society::SocietyState>>,
    mut social_revision: Local<Option<u64>>,
    identities: Option<Res<IdentityIndex>>,
    changed: Query<
        (),
        Or<(
            Changed<Identity>,
            Changed<Control>,
            Changed<Vessel>,
            Changed<AssetOwner>,
            Changed<AssetAccess>,
        )>,
    >,
) {
    let revision = directory.as_ref().map(|value| value.social_revision);
    if revision != *social_revision
        || identities.is_some_and(|value| value.is_changed())
        || !changed.is_empty()
    {
        cache.0.clear();
    }
    *social_revision = revision;
}

fn invalidate_removed(
    mut cache: ResMut<Cache>,
    mut identities_removed: RemovedComponents<Identity>,
    mut controls_removed: RemovedComponents<Control>,
    mut vessels_removed: RemovedComponents<Vessel>,
    mut owners_removed: RemovedComponents<AssetOwner>,
    mut access_removed: RemovedComponents<AssetAccess>,
) {
    // Consume all removal streams even when another invalidation already fired.
    let removed = identities_removed.read().count()
        + controls_removed.read().count()
        + vessels_removed.read().count()
        + owners_removed.read().count()
        + access_removed.read().count();
    if removed != 0 {
        cache.0.clear();
    }
}

fn prune_disconnected(mut cache: ResMut<Cache>, sessions: Query<&super::Session>) {
    if cache.0.is_empty() {
        return;
    }

    let connected: HashSet<_> = sessions.iter().map(|session| session.account).collect();
    cache.0.retain(|account, _| connected.contains(account));
}

// Consume removals before Bevy expires them, even across updates without publication.
pub(crate) fn maintain(world: &mut World) {
    if world.contains_resource::<Cache>() {
        world
            .run_system_cached(invalidate_removed)
            .expect("invalidate publication cache");
    }
}

/// Prepare once before publishing a batch, while all Session components are attached.
/// Call after `infrastructure::publish_navigation` and display updates; readings
/// are shared only across the frames that follow this preparation.
/// Relevant ownership and identity mutations must precede this boundary.
pub fn prepare_publication(world: &mut World) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("session.prepare_publication");
    world.init_resource::<Cache>();
    world.insert_resource(crate::sim::presentation::DeviceReadings::default());
    maintain(world);
    #[cfg(test)]
    {
        world.init_resource::<ScanCount>();
        world.resource_mut::<ScanCount>().0 += 1;
        crate::sim::diagnostics::samples::count("account_cache.invalidation_scans", 1);
    }
    world
        .run_system_cached(invalidate)
        .expect("invalidate publication cache");
    world
        .run_system_cached(prune_disconnected)
        .expect("prune disconnected publication accounts");
}

pub(super) fn get(world: &mut World, account: AccountId) -> Arc<AccountView> {
    let _profile = crate::sim::diagnostics::ProfileScope::new("session.account_cache");
    if let Some(view) = world.resource::<Cache>().0.get(&account) {
        #[cfg(test)]
        crate::sim::diagnostics::samples::count("account_cache.hits", 1);
        return view.clone();
    }
    #[cfg(test)]
    crate::sim::diagnostics::samples::count("account_cache.misses", 1);

    let ships = world
        .query_filtered::<(Entity, &Identity), With<Vessel>>()
        .iter(world)
        .filter_map(|(entity, id)| {
            super::observe(world, account, id.0).ok()?;
            let can_control = ownership::can_access(
                world,
                account,
                entity,
                osg_model::ownership::Permission::Control,
            );
            Some((id.0, entity, can_control))
        })
        .collect();
    let view = Arc::new(AccountView { ships });
    let mut cache = world.resource_mut::<Cache>();
    cache.0.insert(account, view.clone());
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::ownership::{AccessPolicy, Permission, Principal};

    #[test]
    fn connected_accounts_above_old_limit_keep_their_views() {
        let mut world = World::new();
        let accounts: Vec<_> = (0..300).map(|_| Id::new()).collect();
        crate::sim::identity::initialize(&mut world, &accounts);
        world.init_resource::<super::super::Events>();

        let sessions: Vec<_> = accounts
            .iter()
            .map(|&account| super::super::connect(&mut world, account, Default::default()).unwrap())
            .collect();
        prepare_publication(&mut world);
        let views: Vec<_> = accounts
            .iter()
            .map(|&account| get(&mut world, account))
            .collect();

        for batch in 2..=3 {
            prepare_publication(&mut world);
            for (&account, expected) in accounts.iter().zip(&views) {
                assert!(Arc::ptr_eq(expected, &get(&mut world, account)));
            }
            assert_eq!(world.resource::<ScanCount>().0, batch);
        }

        // Match the temporary component removal performed by Session::frame.
        let detached = world
            .entity_mut(sessions[0])
            .take::<super::super::Session>()
            .unwrap();
        assert!(Arc::ptr_eq(&views[0], &get(&mut world, accounts[0])));
        world.entity_mut(sessions[0]).insert(detached);
    }

    #[test]
    fn account_is_pruned_only_after_its_last_session_leaves() {
        let mut world = World::new();
        let account = Id::new();
        let other = Id::new();
        crate::sim::identity::initialize(&mut world, &[account, other]);
        world.init_resource::<super::super::Events>();
        let first = super::super::connect(&mut world, account, Default::default()).unwrap();
        let second = super::super::connect(&mut world, account, Default::default()).unwrap();
        super::super::connect(&mut world, other, Default::default()).unwrap();
        prepare_publication(&mut world);
        let view = get(&mut world, account);
        let other_view = get(&mut world, other);

        super::super::disconnect(&mut world, first);
        prepare_publication(&mut world);
        assert!(Arc::ptr_eq(&view, &get(&mut world, account)));

        // Direct despawning must be handled as well as explicit disconnection.
        world.despawn(second);
        prepare_publication(&mut world);
        assert!(!world.resource::<Cache>().0.contains_key(&account));
        assert!(Arc::ptr_eq(&other_view, &get(&mut world, other)));

        super::super::connect(&mut world, account, Default::default()).unwrap();
        prepare_publication(&mut world);
        assert!(!Arc::ptr_eq(&view, &get(&mut world, account)));
    }

    #[test]
    fn cached_accounts_follow_permissions_names_and_removal() {
        let mut world = World::new();
        let owner = Id::new();
        let viewer = Id::new();
        crate::sim::identity::initialize(&mut world, &[owner, viewer]);
        world.init_resource::<super::super::Events>();
        super::super::connect(&mut world, owner, Default::default()).unwrap();
        super::super::connect(&mut world, viewer, Default::default()).unwrap();
        let ship = world
            .spawn((
                Vessel {
                    vessel_name: "Original".into(),
                },
                Control {
                    account: owner,
                    revision: 1,
                },
                AssetOwner(Principal::Player(owner)),
                AssetAccess(AccessPolicy::default()),
            ))
            .id();
        crate::sim::identity::register(&mut world, ship, Id::new()).unwrap();

        prepare_publication(&mut world);
        let first = get(&mut world, viewer);
        assert!(first.ships.is_empty());
        assert!(Arc::ptr_eq(&first, &get(&mut world, viewer)));

        world
            .get_mut::<AssetAccess>(ship)
            .unwrap()
            .0
            .public
            .insert(Permission::View);
        prepare_publication(&mut world);
        let visible = get(&mut world, viewer);
        assert_eq!(visible.ships.len(), 1);
        assert!(!visible.ships[0].2);
        assert!(Arc::ptr_eq(&visible, &get(&mut world, viewer)));

        world.get_mut::<Vessel>(ship).unwrap().vessel_name = "Renamed".into();
        prepare_publication(&mut world);
        assert!(!Arc::ptr_eq(&visible, &get(&mut world, viewer)));
        world.get_mut::<AssetAccess>(ship).unwrap().0.public.clear();
        prepare_publication(&mut world);
        assert!(get(&mut world, viewer).ships.is_empty());
        assert_eq!(get(&mut world, owner).ships.len(), 1);

        world.get_mut::<AssetOwner>(ship).unwrap().0 = Principal::Player(viewer);
        world.get_mut::<Control>(ship).unwrap().account = viewer;
        prepare_publication(&mut world);
        assert!(get(&mut world, owner).ships.is_empty());
        let transferred = get(&mut world, viewer);
        assert_eq!(transferred.ships.len(), 1);
        assert!(transferred.ships[0].2);

        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .social_revision += 1;
        prepare_publication(&mut world);
        let refreshed = get(&mut world, viewer);
        assert!(!Arc::ptr_eq(&transferred, &refreshed));
        world.get_mut::<Control>(ship).unwrap().revision += 1;
        prepare_publication(&mut world);
        assert!(!Arc::ptr_eq(&refreshed, &get(&mut world, viewer)));

        let replacement = Id::new();
        world.entity_mut(ship).insert(Identity(replacement));
        prepare_publication(&mut world);
        assert_eq!(get(&mut world, viewer).ships, [(replacement, ship, true)]);

        world.entity_mut(ship).remove::<Identity>();
        prepare_publication(&mut world);
        assert!(get(&mut world, viewer).ships.is_empty());

        world.despawn(ship);
        prepare_publication(&mut world);
        assert!(get(&mut world, owner).ships.is_empty());
        assert!(get(&mut world, viewer).ships.is_empty());
    }

    #[test]
    fn removed_vessel_stays_invalidated_across_updates_without_publication() {
        let mut app = App::new();
        app.add_systems(Last, maintain);
        let account = Id::new();
        let world = app.world_mut();
        crate::sim::identity::initialize(world, &[account]);
        world.init_resource::<super::super::Events>();
        super::super::connect(world, account, Default::default()).unwrap();
        let ship = world
            .spawn((
                Vessel {
                    vessel_name: "Ship".into(),
                },
                Control {
                    account,
                    revision: 1,
                },
                AssetOwner(Principal::Player(account)),
            ))
            .id();
        crate::sim::identity::register(world, ship, Id::new()).unwrap();
        prepare_publication(world);
        assert_eq!(get(world, account).ships.len(), 1);

        // Removing Vessel leaves IdentityIndex intact: only the removal stream
        // can invalidate the previously cached ship after the component is gone.
        world.entity_mut(ship).remove::<Vessel>();
        for _ in 0..4 {
            app.update();
        }
        let world = app.world_mut();
        assert_eq!(world.resource::<ScanCount>().0, 1);
        prepare_publication(world);
        assert!(get(world, account).ships.is_empty());
    }
}
