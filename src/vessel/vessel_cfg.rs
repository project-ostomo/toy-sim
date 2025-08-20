use bevy::prelude::*;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Asset, Clone, Debug, Serialize, Deserialize, TypePath)]
pub struct VesselCfg {
    pub name: SmolStr,
    #[serde(default)]
    pub title: SmolStr,
    #[serde(default)]
    pub description: SmolStr,
    pub parts: Vec<VesselPartCfg>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VesselPartCfg {
    pub id: SmolStr,
    pub proto: SmolStr,
    #[serde(default)]
    pub position_dm: IVec3,
    #[serde(default)]
    pub top_face: Face,
    #[serde(default)]
    pub turn: QuarterTurn,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Hash, Default)]
pub enum QuarterTurn {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Face {
    #[default]
    Top,
    Bottom,
    Front,
    Back,
    Right,
    Left,
}
