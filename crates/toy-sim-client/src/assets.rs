use crate::AssetClient;
use bevy::{
    asset::{
        AssetLoader, LoadContext,
        io::{AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, Reader, VecReader},
    },
    prelude::*,
};
use std::path::Path;
use toy_sim_model::Id;
use toy_sim_ships::{Catalogue, appearance::PreparedAppearance};
use toy_sim_universe::{orrery_cfg::Body, replication::SystemAsset, solver::Orrery};

pub(crate) fn path(hash: [u8; 32]) -> String {
    use std::fmt::Write;
    let mut path = String::from("server://");
    for byte in hash {
        write!(&mut path, "{byte:02x}").unwrap();
    }
    path
}

struct ServerReader(AssetClient);

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
            .fetch(hash)
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

pub(crate) fn register_source(app: &mut App, client: AssetClient) {
    app.register_asset_source(
        "server",
        AssetSourceBuilder::new(move || Box::new(ServerReader(client.clone()))),
    );
}

pub(crate) fn install(app: &mut App) {
    app.init_asset::<ShipAppearance>()
        .init_asset::<SystemDefinition>()
        .init_asset::<NavigationDefinition>()
        .init_resource::<NavigationLoad>()
        .init_asset_loader::<ShipLoader>()
        .init_asset_loader::<SystemLoader>()
        .init_asset_loader::<NavigationLoader>()
        .add_systems(
            Update,
            synchronize_appearances
                .after(crate::state::PresentationSet::Interpolate)
                .before(crate::state::PresentationSet::Views),
        );
    app.add_systems(
        Update,
        synchronize_navigation
            .after(crate::state::PresentationSet::Interpolate)
            .before(crate::state::PresentationSet::Views),
    );
}

#[derive(Asset, TypePath)]
pub(crate) struct NavigationDefinition(pub std::sync::Arc<toy_sim_model::NavigationCatalogue>);

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
            toy_sim_protocol::navigation::decode_catalogue(&bytes)?,
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
    session: Option<ResMut<crate::state::SessionInfo>>,
    mut load: ResMut<NavigationLoad>,
    server: Res<AssetServer>,
    assets: Res<Assets<NavigationDefinition>>,
) {
    use crate::state::NavigationStatus;
    let Some(mut session) = session else {
        return;
    };
    if load.generation != session.generation || load.hash != session.navigation_hash {
        load.generation = session.generation;
        load.hash = session.navigation_hash;
        load.asset = load.hash.map(|hash| server.load(path(hash)));
        session.navigation = Default::default();
    }
    let Some(handle) = &load.asset else {
        if session.navigation_status != NavigationStatus::Unavailable {
            session.navigation_status = NavigationStatus::Unavailable;
        }
        return;
    };
    let status = if let Some(definition) = assets.get(handle) {
        if !std::sync::Arc::ptr_eq(&session.navigation, &definition.0) {
            session.navigation = definition.0.clone();
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
pub(crate) struct ShipAppearance(pub PreparedAppearance);

#[derive(Asset, TypePath)]
pub(crate) struct SystemDefinition {
    pub system: Id,
    pub solver: Orrery,
    pub bodies: Vec<(Id, Body)>,
}

impl SystemDefinition {
    pub(crate) fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let definition = SystemAsset::decode(bytes)?;
        let solver = definition.solver()?;
        let mut bodies = Vec::with_capacity(definition.body_ids.len());
        for identity in definition.body_ids {
            let body = solver
                .get_body(&identity.name)
                .ok_or_else(|| anyhow::anyhow!("missing body in ephemeris"))?
                .clone();
            bodies.push((Id(identity.id), body));
        }
        Ok(Self {
            system: Id(definition.system_id),
            solver,
            bodies,
        })
    }
}

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
        let appearance = toy_sim_ships::appearance::ShipAppearance::from_bytes(&bytes)?;
        Ok(ShipAppearance(appearance.prepare(&Catalogue::builtin())?))
    }
}

#[derive(Default, TypePath)]
struct SystemLoader;

impl AssetLoader for SystemLoader {
    type Asset = SystemDefinition;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> anyhow::Result<SystemDefinition> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        SystemDefinition::decode(&bytes)
    }
}

#[derive(Component)]
pub(crate) struct MeshDemand;

#[derive(Component)]
pub(crate) struct Appearance {
    pub hash: [u8; 32],
    pub asset: Handle<ShipAppearance>,
}

pub(crate) fn synchronize_appearances(
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
                toy_sim_model::CombatEventKind::Destroyed { appearance, .. } => *appearance,
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
