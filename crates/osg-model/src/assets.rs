//! Bounded asset browsing and goods totals across authorized inventories.
use crate::{Id, industry::CargoItem, ownership::Principal};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AssetKind {
    Ship,
    Installation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetSummary {
    pub id: Id,
    pub name: String,
    pub owner: Principal,
    pub kind: AssetKind,
    pub location: String,
    /// Nearest system to the asset or its docking host; absent during transit.
    pub system: Option<String>,
    pub host: Option<Id>,
    /// Inventory and design measurements, available only with View permission.
    pub telemetry: Option<AssetTelemetry>,
    pub status: String,
    pub can_manage: bool,
    pub can_open: bool,
    pub can_focus: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetTelemetry {
    pub cargo_used_m3: f64,
    pub cargo_capacity_m3: f64,
    pub energy_j: u64,
    pub energy_capacity_j: u64,
    pub consumables: Vec<AssetConsumable>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetConsumable {
    pub resource: String,
    pub name: String,
    pub quantity: u64,
    pub capacity: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StockKey {
    pub entity: Id,
    pub owner: Principal,
    pub storage: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoodsSummary {
    pub item: CargoItem,
    pub name: String,
    pub quantity: u128,
    pub reserved: u128,
    pub locations: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StockLocation {
    pub key: StockKey,
    pub name: String,
    pub quantity: u64,
    pub reserved: u64,
}
