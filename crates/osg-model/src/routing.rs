use crate::travel::{Directive, FuelBudget, ItineraryEntry, PlanningPreferences, PlanningProgress};
use serde::{Deserialize, Serialize};

pub const MAX_DIRECTIVES: usize = 256;
pub const REQUEST_GAS: u64 = 8192;
pub const POLL_GAS: u64 = 8192;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub directives: Vec<Directive>,
    pub preferences: PlanningPreferences,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub planned_tick: u64,
    pub directive_revision: u64,
    pub topology_revision: u64,
    pub itinerary: Vec<ItineraryEntry>,
    pub fuel_budget: FuelBudget,
    pub estimated_loss_ppm: f64,
    pub exotic_fuel_kg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Unknown,
    Pending { progress: PlanningProgress },
    Ready { plan: Plan },
    Failed { reason: String },
}
