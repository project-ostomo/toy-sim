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
use toy_sim_ships::{Catalogue, CompiledShipDesign, ShipBlueprint};
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
    app.init_asset::<ShipDesign>()
        .init_asset::<SystemDefinition>()
        .init_asset_loader::<ShipLoader>()
        .init_asset_loader::<SystemLoader>()
        .add_systems(
            Update,
            synchronize_appearances
                .after(crate::state::PresentationSet::Interpolate)
                .before(crate::state::PresentationSet::Views),
        );
}

#[derive(Asset, TypePath)]
pub(crate) struct ShipDesign(pub CompiledShipDesign);

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
    type Asset = ShipDesign;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> anyhow::Result<ShipDesign> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let blueprint: ShipBlueprint = toml::from_str(std::str::from_utf8(&bytes)?)?;
        Ok(ShipDesign(blueprint.compile(&Catalogue::builtin())?))
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
    pub design: Handle<ShipDesign>,
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
                    design: server.load(path(hash)),
                });
            }
            None if appearance.is_some() => {
                commands.entity(entity).remove::<Appearance>();
            }
            _ => {}
        }
    }
}
