use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use osg_model::{AccountId, Id, IffIdentity};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

#[derive(Component, Clone, Copy)]
#[component(immutable, on_insert = index_identity, on_discard = unindex_identity)]
pub struct Identity(pub Id);

fn index_identity(mut world: DeferredWorld<'_>, HookContext { entity, .. }: HookContext) {
    let id = world.get::<Identity>(entity).unwrap().0;
    let Some(mut index) = world.get_resource_mut::<IdentityIndex>() else {
        return;
    };
    assert!(
        index.0.get(&id).is_none_or(|previous| *previous == entity),
        "Identity already belongs to another entity"
    );
    index.0.insert(id, entity);
}

fn unindex_identity(mut world: DeferredWorld<'_>, HookContext { entity, .. }: HookContext) {
    let id = world.get::<Identity>(entity).unwrap().0;
    let Some(mut index) = world.get_resource_mut::<IdentityIndex>() else {
        return;
    };
    if index.0.get(&id) == Some(&entity) {
        index.0.remove(&id);
    }
}

#[derive(Component, Clone, Copy)]
pub struct SpatialInstance(pub Id);

pub fn renew_spatial_instance(world: &mut World, entity: Entity) {
    super::sensors::invalidate(world, entity);
    world.entity_mut(entity).insert(SpatialInstance(Id::new()));
}

pub fn observation_spatial_instance(observation: Id, instance: Id) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame observation spatial instance v1");
    hash.update(&observation.0);
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
pub struct Account {
    pub debug: bool,
}

#[derive(Resource, Default)]
pub struct IdentityIndex(HashMap<Id, Entity>);

impl IdentityIndex {
    pub fn entries(&self) -> &HashMap<Id, Entity> {
        &self.0
    }
}

#[derive(Resource)]
pub struct WorldEpoch(pub Id);

#[derive(Component)]
pub struct DirectoryEmitter;

#[derive(Component)]
pub struct NavigationBeaconEmitter;

pub fn public_directory_emitter(world: &World, entity: Entity) -> bool {
    world.get::<DirectoryEmitter>(entity).is_some()
        && world
            .get::<Transponder>(entity)
            .is_some_and(|transponder| transponder.0.enabled)
        && world.get::<super::travel::Dormant>(entity).is_none()
}

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

    pub(crate) fn remove(&self, hash: &[u8; 32]) {
        self.0.write().expect("asset store poisoned").remove(hash);
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

pub fn register(world: &mut World, entity: Entity, id: Id) -> anyhow::Result<()> {
    anyhow::ensure!(
        world
            .resource::<IdentityIndex>()
            .0
            .get(&id)
            .is_none_or(|owner| *owner == entity),
        "Identity already belongs to another entity"
    );
    anyhow::ensure!(world.get_entity(entity).is_ok(), "Entity unavailable");
    world.entity_mut(entity).insert(Identity(id));
    if world.get::<super::vessel::ShipDesign>(entity).is_some()
        && world.get::<SpatialInstance>(entity).is_none()
    {
        renew_spatial_instance(world, entity);
    }
    Ok(())
}

pub fn lookup(world: &World, id: Id) -> anyhow::Result<Entity> {
    world
        .resource::<IdentityIndex>()
        .0
        .get(&id)
        .copied()
        .filter(|entity| {
            world
                .get::<Identity>(*entity)
                .is_some_and(|identity| identity.0 == id)
        })
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
    let entity = world.spawn((Account { debug }, OwnedShips::default())).id();
    register(world, entity, id).expect("new account identity is available");
    entity
}

pub fn attach_ship(world: &mut World, ship: Entity, owner: Id, id: Id) -> anyhow::Result<()> {
    anyhow::ensure!(id != owner, "Ship identity must differ from its account");
    anyhow::ensure!(
        world
            .resource::<IdentityIndex>()
            .0
            .get(&id)
            .is_none_or(|entity| *entity == ship),
        "Identity already belongs to another entity"
    );
    let account = add_account(world, owner, false);
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
        Transponder(IffIdentity {
            owner,
            faction,
            labels: name.into_iter().collect(),
            enabled: true,
        }),
        Appearance(appearance),
    ));
    register(world, ship, id)?;
    Ok(())
}

pub fn identify_celestials(
    mut commands: Commands,
    bodies: Query<(Entity, &super::orrery::activity::CelestialState), Without<Identity>>,
) {
    for (entity, celestial) in &bodies {
        let id = super::registry::celestial_identity(celestial.reference);
        commands.entity(entity).insert(Identity(id));
    }
}

pub fn pose(
    transform: &super::precision::PreciseTransform,
    velocity: Option<&super::physics::Velocity>,
    angular: Option<&super::physics::AngularVelocity>,
) -> osg_model::Pose {
    osg_model::Pose {
        position: transform.translation_um,
        rotation: transform.rotation.to_array(),
        velocity: velocity.map_or([0.; 3], |value| value.0.to_array()),
        angular_velocity: angular.map_or([0.; 3], |value| value.0.to_array()),
    }
}

#[cfg(test)]
mod tests;
