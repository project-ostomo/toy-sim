use crate::travel::{FuelBudget, Order, PlanningPreferences, PlanningProgress, QueuedOrder};
use serde::{Deserialize, Serialize};

pub const MAX_ORDERS: usize = 256;
pub const REQUEST_GAS: u64 = 8192;
pub const POLL_GAS: u64 = 8192;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub orders: Vec<Order>,
    pub preferences: PlanningPreferences,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub planned_tick: u64,
    pub travel_revision: u64,
    pub topology_revision: u64,
    pub orders: Vec<QueuedOrder>,
    pub fuel_budget: FuelBudget,
    pub estimated_loss_ppm: f64,
    pub beacon_assumptions: Vec<crate::EntityId>,
    pub exotic_fuel_kg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Unknown,
    Pending { progress: PlanningProgress },
    Ready { plan: Plan },
    Failed { reason: String },
}
