use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Consumable {
    Water,
    LiquidHydrogen,
    LiquidOxygen,

    ElectricJoules,
}

impl Consumable {
    /// Density, in kg/m³
    pub fn density(&self) -> f64 {
        match self {
            Consumable::Water => 1_000.0,
            Consumable::LiquidHydrogen => 70.85,
            Consumable::LiquidOxygen => 1_141.0,

            Consumable::ElectricJoules => 0.0,
        }
    }
}

/// Tracks *all* the consumables within a vessel
#[derive(Component, Default, Debug)]
pub struct ConsumableTanks(pub BTreeMap<Consumable, f64>); // f64 is accurate enough even for nuclear reactors connected to phone chargers

impl ConsumableTanks {
    pub fn consume(&mut self, cons: Consumable, amt: f64) -> f64 {
        let new = (self.0.get(&cons).copied().unwrap_or_default() - amt).max(0.0);
        self.0.insert(cons, new);
        new
    }
}
