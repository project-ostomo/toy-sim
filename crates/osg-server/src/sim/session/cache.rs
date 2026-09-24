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
    ownership::{self, AssetAccess, AssetOwner, Directory},
    vessel::Vessel,
};

pub(super) struct AccountView {
    pub ships: Vec<(Id, Entity, bool)>,
}

#[derive(Resource, Default)]
struct Cache(HashMap<AccountId, Arc<AccountView>>);

fn invalidate(
    mut cache: ResMut<Cache>,
    directory: Option<Res<Directory>>,
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
    if removed != 0
        || directory.is_some_and(|value| value.is_changed())
        || identities.is_some_and(|value| value.is_changed())
        || !changed.is_empty()
    {
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

// Run at the application update boundary, where Session components are attached.
// Consume removals before Bevy expires them and retain only connected accounts.
pub(crate) fn maintain(world: &mut World) {
    if world.contains_resource::<Cache>() {
        world
            .run_system_cached(invalidate)
            .expect("invalidate publication cache");
        world
            .run_system_cached(prune_disconnected)
            .expect("prune disconnected publication accounts");
    }
}

pub(super) fn get(world: &mut World, account: AccountId) -> Arc<AccountView> {
    let _profile = crate::sim::diagnostics::ProfileScope::new("session.account_cache");
    world.init_resource::<Cache>();
    // Session::frame temporarily takes its Session component out of the world.
    // Invalidate here for freshness, but prune only at the update boundary.
    world
        .run_system_cached(invalidate)
        .expect("invalidate publication cache");
    if let Some(view) = world.resource::<Cache>().0.get(&account) {
        return view.clone();
    }

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
        let views: Vec<_> = accounts
            .iter()
            .map(|&account| get(&mut world, account))
            .collect();

        for _ in 0..2 {
            maintain(&mut world);
            for (&account, expected) in accounts.iter().zip(&views) {
                assert!(Arc::ptr_eq(expected, &get(&mut world, account)));
            }
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
        let view = get(&mut world, account);
        let other_view = get(&mut world, other);

        super::super::disconnect(&mut world, first);
        maintain(&mut world);
        assert!(Arc::ptr_eq(&view, &get(&mut world, account)));

        // Direct despawning must be handled as well as explicit disconnection.
        world.despawn(second);
        maintain(&mut world);
        assert!(!world.resource::<Cache>().0.contains_key(&account));
        assert!(Arc::ptr_eq(&other_view, &get(&mut world, other)));

        super::super::connect(&mut world, account, Default::default()).unwrap();
        assert!(!Arc::ptr_eq(&view, &get(&mut world, account)));
    }

    #[test]
    fn cached_accounts_follow_permissions_names_and_removal() {
        let mut world = World::new();
        let owner = Id::new();
        let viewer = Id::new();
        crate::sim::identity::initialize(&mut world, &[owner, viewer]);
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

        let first = get(&mut world, viewer);
        assert!(first.ships.is_empty());
        assert!(Arc::ptr_eq(&first, &get(&mut world, viewer)));

        world
            .get_mut::<AssetAccess>(ship)
            .unwrap()
            .0
            .public
            .insert(Permission::View);
        let visible = get(&mut world, viewer);
        assert_eq!(visible.ships.len(), 1);
        assert!(!visible.ships[0].2);
        assert!(Arc::ptr_eq(&visible, &get(&mut world, viewer)));

        world.get_mut::<Vessel>(ship).unwrap().vessel_name = "Renamed".into();
        assert!(!Arc::ptr_eq(&visible, &get(&mut world, viewer)));
        world.get_mut::<AssetAccess>(ship).unwrap().0.public.clear();
        assert!(get(&mut world, viewer).ships.is_empty());
        assert_eq!(get(&mut world, owner).ships.len(), 1);

        let replacement = Id::new();
        world.entity_mut(ship).insert(Identity(replacement));
        assert_eq!(get(&mut world, owner).ships, [(replacement, ship, true)]);

        world.entity_mut(ship).remove::<Identity>();
        assert!(get(&mut world, owner).ships.is_empty());

        world.despawn(ship);
        assert!(get(&mut world, owner).ships.is_empty());
    }
}
