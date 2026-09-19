use crate::{AccountId, EntityId, Id, ownership::Principal};
use serde::{Deserialize, Serialize};

pub const MAX_DIRECTORY_ENTRIES: usize = 128;
pub const MAX_SUBSCRIBED_INVENTORIES: usize = 8;
pub const MAX_CARGO_STACKS: usize = 1024;
pub const MAX_FACILITY_JOBS: usize = 128;
pub const MAX_CATALOGUE_RECIPES: usize = 1024;
pub const MAX_CATALOGUE_BLUEPRINTS: usize = 32;
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
pub const MAX_RECIPE_BATCHES: u32 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CargoItem {
    Resource(String),
    Part(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: CargoItem,
    pub quantity: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CargoStack {
    pub item: CargoItem,
    pub quantity: u64,
    pub reserved: u64,
    pub name: String,
    pub unit_mass_kg: f64,
    pub unit_volume_m3: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndustryCapability {
    Refinery,
    FuelPlant,
    Fabricator,
    Shipyard,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub capability: IndustryCapability,
    pub inputs: Vec<ItemStack>,
    pub outputs: Vec<ItemStack>,
    pub duration_ticks: u64,
    pub energy_j: u64,
    pub stored_energy_j: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Running,
    AwaitingPower,
    AwaitingCargoSpace,
    AwaitingBerth,
    ModuleUnavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    pub id: Id,
    pub name: String,
    pub capability: IndustryCapability,
    pub progress_ticks: u64,
    pub duration_ticks: u64,
    pub status: JobStatus,
    pub owner: Principal,
    pub created_by: AccountId,
    pub module_part: Option<u64>,
    pub requested_power_w: u64,
    pub supplied_power_w: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacilityCapability {
    pub part: u64,
    pub capability: IndustryCapability,
    pub lanes: u32,
    pub power_per_lane_w: u64,
    pub max_radius_m: Option<f64>,
    pub operational: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacilityView {
    pub entity: EntityId,
    pub owner: Principal,
    pub name: String,
    pub can_manage: bool,
    pub can_transfer: bool,
    pub cargo_capacity_m3: f64,
    pub cargo_used_m3: f64,
    pub items: Vec<CargoStack>,
    pub products: Vec<CargoStack>,
    pub jobs: Vec<JobView>,
    pub capabilities: Vec<FacilityCapability>,
    pub location: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlueprintView {
    pub name: String,
    pub blueprint: Vec<u8>,
    pub inputs: Vec<ItemStack>,
    pub duration_ticks: u64,
    pub energy_j: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IndustryCommand {
    UnloadProduct {
        source: EntityId,
        target: EntityId,
        resource: String,
        quantity: u64,
    },
    Refill {
        source: EntityId,
        ship: EntityId,
        resource: String,
        quantity: u64,
    },
    StartRecipe {
        facility: EntityId,
        recipe: String,
        batches: u32,
    },
    BuildShip {
        facility: EntityId,
        owner: Principal,
        blueprint: Vec<u8>,
    },
    CancelJob {
        facility: EntityId,
        job: Id,
    },
    Transfer {
        source: EntityId,
        target: EntityId,
        item: CargoItem,
        quantity: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndustrySubscription {
    pub revision: u64,
    pub directory: bool,
    pub directory_after: Option<Id>,
    pub inventories: Vec<EntityId>,
    pub catalogue: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacilitySummary {
    pub entity: EntityId,
    pub owner: Principal,
    pub name: String,
    pub location: Option<EntityId>,
    pub capabilities: Vec<IndustryCapability>,
    pub can_manage: bool,
    pub can_transfer: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndustryCatalogue {
    pub revision: [u8; 32],
    pub recipes: Vec<Recipe>,
    pub blueprints: Vec<BlueprintView>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IndustrySnapshot {
    pub subscription_revision: u64,
    pub error: Option<String>,
    pub omitted_inventories: Vec<EntityId>,
    pub directory: Vec<FacilitySummary>,
    pub directory_next: Option<Id>,
    pub facilities: Vec<FacilityView>,
    pub catalogue: Option<IndustryCatalogue>,
}
