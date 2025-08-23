use std::collections::BTreeMap;

use anyhow::Context;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Consumable {
    Water,
    LiquidHydrogen,
    LiquidOxygen,

    Uranium235,
    Plutonium239,

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

            Consumable::Uranium235 => todo!(),
            Consumable::Plutonium239 => todo!(),
        }
    }
}

/// Tracks *all* the consumables within a vessel
#[derive(Component, Default, Debug)]
pub struct ConsumableTanks {
    mapping: BTreeMap<Consumable, (f64, f64)>,
}

impl ConsumableTanks {
    pub fn add_tank(&mut self, cons: Consumable, amt: f64, total: f64) {
        self.mapping
            .entry(cons)
            .and_modify(|slot| {
                slot.0 += amt;
                slot.1 += total;
            })
            .or_insert((amt, total));
    }

    pub fn produce(&mut self, cons: Consumable, amt: f64) -> anyhow::Result<()> {
        let tank = self.mapping.get_mut(&cons).context("no such tank")?;
        if tank.0 == tank.1 {
            anyhow::bail!("tank is full")
        }
        tank.0 = (tank.0 + amt).min(tank.1);
        Ok(())
    }

    pub fn consume(&mut self, cons: Consumable, amt: f64) -> f64 {
        if let Some(slot) = self.mapping.get_mut(&cons) {
            slot.0 = (slot.0 - amt).max(0.0);
            slot.0
        } else {
            0.0
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (Consumable, (f64, f64))> {
        self.mapping.iter().map(|s| (*s.0, *s.1))
    }
}
