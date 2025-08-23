use bevy::math::{DVec3, UVec3};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::vessel::consumable::Consumable;
use crate::vessel::modules::reactor::NuclearReactorCfg;

#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct PartCfg {
    pub name: SmolStr,
    #[serde(default)]
    pub title: SmolStr,
    #[serde(default)]
    pub description: SmolStr,
    pub model: SmolStr,
    pub dimensions_dm: UVec3,

    pub empty_mass: f64,

    #[serde(default)]
    pub modules: Vec<PartModuleCfg>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartModuleCfg {
    #[serde(default)]
    pub offset: DVec3,
    #[serde(default)]
    pub direction: DVec3,
    #[serde(flatten)]
    pub kind: PartModuleCfgInner,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum PartModuleCfgInner {
    MagicTorquer {
        torque: f64,
    },
    MagicThruster {
        thrust: f64,
        flame: Option<ThrusterFlameCfg>,
    },
    ElectricFan {
        power: f64,
        efficiency: f64,
        diameter: f64,
    },
    Tank {
        consumable: Consumable,
        capacity: f64,
        fraction: f64,
    },
    NuclearReactor(NuclearReactorCfg),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThrusterFlameCfg {
    Simple { radius: f32, max_length: f32 },
}
