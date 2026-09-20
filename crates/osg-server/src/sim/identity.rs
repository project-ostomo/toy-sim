use bevy::prelude::*;
use osg_model::{AccountId, Id, IffIdentity, InfoGroupKey};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

#[derive(Component, Clone, Copy)]
pub struct Identity(pub Id);

#[derive(Component, Clone, Copy)]
pub struct SpatialInstance(pub Id);

pub fn renew_spatial_instance(world: &mut World, entity: Entity) {
    world.entity_mut(entity).insert(SpatialInstance(Id::new()));
}

pub fn track_spatial_instance(track: Id, instance: Id) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame track spatial instance v1");
    hash.update(&track.0);
    hash.update(&instance.0);
    Id(hash.finalize().as_bytes()[..16].try_into().unwrap())
}

#[derive(Component)]
pub struct Control {
    pub account: AccountId,
    pub revision: u64,
}

#[derive(Component)]
pub struct Transponder(pub IffIdentity);

#[derive(Component)]
#[relationship(relationship_target = GroupShips)]
pub struct Membership(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = Membership)]
pub struct GroupShips(Vec<Entity>);

#[derive(Component)]
pub struct Account {
    pub group: Entity,
    pub debug: bool,
}

#[derive(Resource, Default)]
pub struct IdentityIndex(pub HashMap<Id, Entity>);

#[derive(Resource, Default)]
pub struct GroupIndex(pub HashMap<InfoGroupKey, Entity>);

#[derive(Resource)]
pub struct WorldEpoch(pub Id);

#[derive(Resource)]
pub struct SensorSeed(pub [u8; 32]);

#[derive(Component)]
pub struct BeaconEmitter;

#[derive(Component)]
pub struct FixedBeacon;

#[derive(Component)]
pub struct Appearance(pub [u8; 32]);

type AssetMap = HashMap<[u8; 32], Arc<[u8]>>;

#[derive(Resource, Default, Clone)]
pub struct AppearanceAssets(Arc<RwLock<AssetMap>>);

impl AppearanceAssets {
    pub fn insert(&self, hash: [u8; 32], bytes: impl Into<Arc<[u8]>>) {
        self.0
            .write()
            .expect("asset store poisoned")
            .insert(hash, bytes.into());
    }

    pub fn get(&self, hash: &[u8; 32]) -> Option<Arc<[u8]>> {
        self.0
            .read()
            .expect("asset store poisoned")
            .get(hash)
            .cloned()
    }

    pub fn extend<T: Into<Arc<[u8]>>>(&self, assets: impl IntoIterator<Item = ([u8; 32], T)>) {
        self.0
            .write()
            .expect("asset store poisoned")
            .extend(assets.into_iter().map(|(hash, bytes)| (hash, bytes.into())));
    }

    pub fn snapshot(&self) -> AssetMap {
        self.0.read().expect("asset store poisoned").clone()
    }
}

pub fn register(world: &mut World, entity: Entity, id: Id) {
    world.entity_mut(entity).insert(Identity(id));
    if world.get::<super::vessel::ShipDesign>(entity).is_some()
        && world.get::<SpatialInstance>(entity).is_none()
    {
        renew_spatial_instance(world, entity);
    }
    world.resource_mut::<IdentityIndex>().0.insert(id, entity);
}

pub fn lookup(world: &World, id: Id) -> anyhow::Result<Entity> {
    world
        .resource::<IdentityIndex>()
        .0
        .get(&id)
        .copied()
        .filter(|entity| world.get_entity(*entity).is_ok())
        .ok_or_else(|| anyhow::anyhow!("Entity unavailable"))
}

#[derive(Component)]
#[relationship(relationship_target = OwnedShips)]
pub struct ControlledBy(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = ControlledBy)]
pub struct OwnedShips(Vec<Entity>);

pub fn initialize(world: &mut World, accounts: &[AccountId]) {
    world.init_resource::<IdentityIndex>();
    world.init_resource::<AppearanceAssets>();
    world.insert_resource(WorldEpoch(Id::new()));
    world.insert_resource(SensorSeed(rand::random()));
    super::intelligence::initialize(world);
    super::ownership::initialize(world);
    for &id in accounts {
        add_account(world, id, false);
    }
}

pub fn add_account(world: &mut World, id: Id, debug: bool) -> Entity {
    super::ownership::add_account(world, id);
    if let Some(entity) = world.resource::<IdentityIndex>().0.get(&id).copied() {
        if debug && let Some(mut account) = world.get_mut::<Account>(entity) {
            account.debug = true;
        }
        return entity;
    }
    let group = super::intelligence::join(world, InfoGroupKey(rand::random()));
    let entity = world
        .spawn((Account { group, debug }, OwnedShips::default()))
        .id();
    register(world, entity, id);
    entity
}

pub fn attach_ship(world: &mut World, ship: Entity, owner: Id) -> anyhow::Result<()> {
    let account = add_account(world, owner, false);
    let group = world.get::<Account>(account).unwrap().group;
    let design = &world.get::<super::vessel::ShipDesign>(ship).unwrap().0;
    let bytes = osg_ships::appearance::ShipAppearance::from(design.as_ref()).to_bytes()?;
    let appearance = *blake3::hash(&bytes).as_bytes();
    world
        .resource::<AppearanceAssets>()
        .insert(appearance, bytes);
    let name = world
        .get::<super::vessel::Vessel>(ship)
        .map(|v| v.vessel_name.to_string())
        .filter(|n| !n.is_empty());
    let faction = world.resource::<super::ownership::Directory>().0.players[&owner].organization;
    world.entity_mut(ship).insert((
        Control {
            account: owner,
            revision: 1,
        },
        ControlledBy(account),
        super::ownership::AssetOwner(osg_model::ownership::Principal::Player(owner)),
        super::ownership::AssetAccess::default(),
        Membership(group),
        Transponder(IffIdentity {
            owner,
            faction,
            labels: name.into_iter().collect(),
            enabled: true,
            range_m: 1e8,
        }),
        Appearance(appearance),
    ));
    register(world, ship, Id::new());
    Ok(())
}

pub fn identify_celestials(
    mut commands: Commands,
    mut index: ResMut<IdentityIndex>,
    bodies: Query<(Entity, &super::orrery::Celestial), Without<Identity>>,
) {
    for (entity, celestial) in &bodies {
        let id = super::registry::identity(&celestial.0);
        commands.entity(entity).insert(Identity(id));
        index.0.insert(id, entity);
    }
}

pub fn clean_indexes(
    mut identities: ResMut<IdentityIndex>,
    mut groups: ResMut<GroupIndex>,
    alive: Query<Entity>,
) {
    identities.0.retain(|_, entity| alive.contains(*entity));
    groups.0.retain(|_, entity| alive.contains(*entity));
}
