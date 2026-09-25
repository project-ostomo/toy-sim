use crate::OsgNetClient;
use bevy::{
    asset::{
        AssetLoader, LoadContext,
        io::{AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, Reader, VecReader},
    },
    prelude::*,
};
use osg_model::Id;
use osg_ships::{Catalogue, appearance::PreparedAppearance};
use std::path::Path;

pub fn path(hash: [u8; 32]) -> String {
    use std::fmt::Write;
    let mut path = String::from("server://");
    for byte in hash {
        write!(&mut path, "{byte:02x}").unwrap();
    }
    path
}

struct ServerReader(OsgNetClient);

impl AssetReader for ServerReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let name = path.to_str().unwrap_or_default();
        if name.len() != 64 || !name.is_ascii() {
            return Err(AssetReaderError::NotFound(path.to_owned()));
        }
        let mut hash = [0; 32];
        for (index, byte) in hash.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&name[index * 2..index * 2 + 2], 16)
                .map_err(|_| AssetReaderError::NotFound(path.to_owned()))?;
        }
        let bytes = self
            .0
            .fetch_asset(hash)
            .await
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        Ok(VecReader::new(bytes))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_owned()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_owned()))
    }

    async fn is_directory<'a>(&'a self, _: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(false)
    }
}

pub fn register_source(app: &mut App, client: OsgNetClient) {
    app.register_asset_source(
        "server",
        AssetSourceBuilder::new(move || Box::new(ServerReader(client.clone()))),
    );
}

pub fn install(app: &mut App) {
    app.init_asset::<ShipAppearance>()
        .init_asset::<NavigationDefinition>()
        .init_asset::<NavigationAccess>()
        .init_resource::<NavigationLoad>()
        .init_asset_loader::<ShipLoader>()
        .init_asset_loader::<NavigationLoader>()
        .init_asset_loader::<NavigationAccessLoader>()
        .add_systems(
            Last,
            synchronize_appearances.in_set(crate::state::ClientSystems::Gameplay),
        );
    app.add_systems(
        Last,
        synchronize_navigation.in_set(crate::state::ClientSystems::Gameplay),
    );
}

#[derive(Asset, TypePath)]
pub struct NavigationDefinition(pub std::sync::Arc<osg_model::InhabitedDirectory>);

#[derive(Asset, TypePath)]
pub struct NavigationAccess(pub std::collections::BTreeSet<Id>);

#[derive(Default, TypePath)]
struct NavigationAccessLoader;

impl AssetLoader for NavigationAccessLoader {
    type Asset = NavigationAccess;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> anyhow::Result<NavigationAccess> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(NavigationAccess(
            osg_protocol::navigation::decode_access(&bytes)?
                .into_iter()
                .collect(),
        ))
    }
}

#[derive(Default, TypePath)]
struct NavigationLoader;

impl AssetLoader for NavigationLoader {
    type Asset = NavigationDefinition;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> anyhow::Result<NavigationDefinition> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(NavigationDefinition(std::sync::Arc::new(
            osg_protocol::navigation::decode_directory(&bytes)?,
        )))
    }
}

#[derive(Resource, Default)]
struct NavigationLoad {
    generation: u64,
    hash: Option<[u8; 32]>,
    asset: Option<Handle<NavigationDefinition>>,
}

fn synchronize_navigation(
    identity: Res<crate::state::GameSession>,
    mut session: ResMut<crate::state::NavigationState>,
    mut load: ResMut<NavigationLoad>,
    server: Res<AssetServer>,
    assets: Res<Assets<NavigationDefinition>>,
) {
    use crate::state::NavigationStatus;
    if load.generation != identity.key.generation || load.hash != session.navigation_hash {
        load.generation = identity.key.generation;
        load.hash = session.navigation_hash;
        load.asset = load.hash.map(|hash| server.load(path(hash)));
    }
    let Some(handle) = &load.asset else {
        if session.navigation_status != NavigationStatus::Unavailable {
            session.navigation_status = NavigationStatus::Unavailable;
        }
        return;
    };
    let status = if let Some(definition) = assets.get(handle) {
        if !std::sync::Arc::ptr_eq(&session.inhabited, &definition.0) {
            let universe = &identity.universe;
            if definition
                .0
                .systems
                .iter()
                .any(|id| universe.system_index(id.0).is_none())
            {
                session.navigation_status =
                    NavigationStatus::Failed("directory contains an unknown system".into());
                return;
            }
            let old = session.inhabited.clone();
            let navigation = std::sync::Arc::make_mut(&mut session.navigation);
            for id in old.ownership.keys().chain(definition.0.ownership.keys()) {
                if let Some(index) = universe.system_index(id.0) {
                    navigation.systems[index].sovereignty = definition.0.ownership.get(id).copied();
                }
            }
            navigation.topology_revision = navigation.topology_revision.wrapping_add(1);
            session.inhabited = definition.0.clone();
        }
        NavigationStatus::Ready
    } else {
        match server.load_state(handle.id()) {
            bevy::asset::LoadState::Failed(error) => NavigationStatus::Failed(error.to_string()),
            _ => NavigationStatus::Loading,
        }
    };
    if session.navigation_status != status {
        session.navigation_status = status;
    }
}

#[derive(Asset, TypePath)]
pub struct ShipAppearance(pub PreparedAppearance);

#[derive(Default, TypePath)]
struct ShipLoader;

impl AssetLoader for ShipLoader {
    type Asset = ShipAppearance;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> anyhow::Result<ShipAppearance> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let appearance = osg_ships::appearance::ShipAppearance::from_bytes(&bytes)?;
        Ok(ShipAppearance(appearance.prepare(&Catalogue::builtin())))
    }
}

#[derive(Component)]
pub struct MeshDemand;

#[derive(Component)]
pub struct Appearance {
    pub hash: [u8; 32],
    pub asset: Handle<ShipAppearance>,
}

pub fn synchronize_appearances(
    mut commands: Commands,
    server: Res<AssetServer>,
    sources: Query<
        (
            Entity,
            Option<&crate::state::Optical>,
            Option<&crate::state::OwnedShip>,
            Option<&crate::state::CombatPublication>,
            Option<&Appearance>,
            Option<&MeshDemand>,
        ),
        Or<(
            With<crate::state::Optical>,
            With<crate::state::OwnedShip>,
            With<crate::state::CombatPublication>,
        )>,
    >,
) {
    for (entity, contact, owned, combat, appearance, mesh_demand) in &sources {
        let hash = contact
            .filter(|_| mesh_demand.is_some() || appearance.is_some())
            .and_then(|contact| contact.0.appearance)
            .or_else(|| {
                owned
                    .filter(|_| mesh_demand.is_some() || appearance.is_some())
                    .and_then(|ship| ship.0.appearance)
            })
            .or_else(|| match &combat?.0.kind {
                osg_model::CombatEventKind::Destroyed { appearance, .. } => *appearance,
                _ => None,
            });
        match hash {
            Some(hash) if appearance.is_none_or(|appearance| appearance.hash != hash) => {
                commands.entity(entity).insert(Appearance {
                    hash,
                    asset: server.load(path(hash)),
                });
            }
            None if appearance.is_some() => {
                commands.entity(entity).remove::<Appearance>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
