use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_asset_loader::prelude::*;
use smol_str::SmolStr;

use crate::{
    GameState,
    assets::TomlAssetLoader,
    physics::RigidBody,
    vessel::{
        modules::{thruster, torquer},
        part_cfg::PartCfg,
        vessel_cfg::VesselCfg,
    },
};

mod consumable;
mod modules;
mod part_cfg;
mod spawn;

mod controls;
mod vessel_cfg;

pub use consumable::ConsumableTanks;
pub use controls::VesselControls;
pub use modules::thruster::Thruster;

pub struct VesselsPlugin;

impl Plugin for VesselsPlugin {
    fn build(&self, app: &mut App) {
        app.configure_loading_state(
            LoadingStateConfig::new(GameState::Loading).load_collection::<VesselAssets>(),
        )
        .register_asset_loader(TomlAssetLoader::<VesselCfg>::new("vessel.toml"))
        .register_asset_loader(TomlAssetLoader::<PartCfg>::new("part.toml"))
        .init_asset::<VesselCfg>()
        .init_asset::<PartCfg>()
        .add_systems(OnEnter(GameState::Game), load_vessels)
        .add_plugins((
            spawn::run_spawn,
            thruster::run_thrusters,
            torquer::start_torquers,
            controls::run_controls,
        ));
    }
}

fn load_vessels(
    mut commands: Commands,
    assets: Res<VesselAssets>,
    vessels: Res<Assets<VesselCfg>>,
    parts: Res<Assets<PartCfg>>,
) {
    // TODO: validation!!!
    let mut loaded = LoadedVessels::default();
    for vessel in assets.vessels.iter() {
        let vessel = vessels.get(vessel).unwrap();
        loaded.vessels.insert(vessel.name.clone(), vessel.clone());
    }
    for part in assets.parts.iter() {
        let part = parts.get(part).unwrap();
        loaded.parts.insert(part.name.clone(), part.clone());
    }
    commands.insert_resource(loaded);
}

#[derive(Component)]
#[require(RigidBody, ConsumableTanks, VesselControls)]
pub struct Vessel {
    pub class_name: SmolStr,
    pub vessel_name: SmolStr,
}

#[derive(Resource, Default)]
pub struct LoadedVessels {
    pub vessels: BTreeMap<SmolStr, VesselCfg>,
    pub parts: BTreeMap<SmolStr, PartCfg>,
}

#[derive(AssetCollection, Resource)]
struct VesselAssets {
    #[asset(path = "vessels", collection(typed))]
    vessels: Vec<Handle<VesselCfg>>,

    #[asset(path = "parts", collection(typed))]
    parts: Vec<Handle<PartCfg>>,
}
