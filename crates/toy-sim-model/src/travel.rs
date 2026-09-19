use crate::{EntityId, GalacticPosition, Id};
use serde::{Deserialize, Serialize};

pub const GATE_ENTRY_SPEED_M_S: f64 = 100.0;
pub const DOCKING_CLEARANCE_M: f64 = 100.0;
pub const DOCKING_SPEED_M_S: f64 = 10.0;
pub const MAX_PREDICTION_SECONDS: f64 = 365.25 * 86_400.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Presence {
    Space,
    Docked { host: EntityId, bay: u32 },
    SlipTransit(Id),
    StoredInWreck(EntityId),
    Destroyed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axes {
    Galactic,
    BodyFixed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reference {
    Celestial(EntityId),
    Beacon(EntityId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Destination {
    Beacon(EntityId),
    Galactic(GalacticPosition),
    Relative {
        reference: Reference,
        offset: GalacticPosition,
        axes: Axes,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Order {
    Jump(EntityId),
    Guidance(Guidance),
    TravelTo(Destination),
    Sublight(Destination),
    Slip { destination: Destination },
    Dock(EntityId),
    Undock,
    WaitUntil(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanningPreferences {
    pub fuel_priority: f64,
}

impl Default for PlanningPreferences {
    fn default() -> Self {
        Self { fuel_priority: 1. }
    }
}

impl PlanningPreferences {
    pub fn valid(self) -> bool {
        self.fuel_priority.is_finite() && (0.1..=1000.).contains(&self.fuel_priority)
    }

    pub fn cost(self, mass_kg: f64) -> crate::transfer::TransferCost {
        crate::transfer::TransferCost {
            seconds_per_kg: 3600. * self.fuel_priority / mass_kg.max(1.),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FuelRequirement {
    pub resource: String,
    pub required_kg: f64,
    pub available_kg: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FuelBudget {
    pub resources: Vec<FuelRequirement>,
    pub complete: bool,
}

impl FuelBudget {
    pub fn exhausted(&self) -> bool {
        self.resources.iter().any(|resource| {
            resource.required_kg > 0. && resource.required_kg >= resource.available_kg
        })
    }

    pub fn valid(&self) -> bool {
        self.resources.len() <= 256
            && self.resources.iter().all(|resource| {
                resource.resource.len() <= 64
                    && resource.required_kg.is_finite()
                    && resource.required_kg >= 0.
                    && resource.available_kg.is_finite()
                    && resource.available_kg >= 0.
            })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedOrder {
    pub action: Order,
    pub estimated_duration_ticks: Option<u64>,
    pub estimated_propellant_kg: Option<f64>,
}

impl From<Order> for QueuedOrder {
    fn from(action: Order) -> Self {
        let estimated_propellant_kg = matches!(
            action,
            Order::Slip { .. } | Order::Undock | Order::WaitUntil(_)
        )
        .then_some(0.);
        Self {
            action,
            estimated_duration_ticks: None,
            estimated_propellant_kg,
        }
    }
}

impl QueuedOrder {
    pub fn with_propellant(mut self, kg: f64) -> Self {
        self.estimated_propellant_kg = (kg.is_finite() && kg >= 0.).then_some(kg);
        self
    }

    pub fn estimated(action: Order, seconds: f64) -> Self {
        Self {
            action,
            estimated_propellant_kg: None,
            estimated_duration_ticks: (seconds.is_finite() && seconds >= 0.)
                .then(|| (seconds * 10.).ceil() as u64),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Idle,
    Planning,
    Active,
    Paused,
    Blocked(String),
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanningStage {
    LoadingCatalogue,
    BuildingGraph,
    SearchingRoutes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanningProgress {
    pub stage: PlanningStage,
    pub completed: u32,
    pub total: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TravelState {
    pub autopilot_enabled: bool,
    pub preferences: PlanningPreferences,
    pub fuel_budget: Option<FuelBudget>,
    pub planning: Option<PlanningProgress>,
    pub search_limited: bool,
    pub revision: u64,
    pub orders: Vec<QueuedOrder>,
    pub order: usize,
    pub status: Status,
    pub estimated_arrival_tick: Option<u64>,
}

impl TravelState {
    pub fn stage_arrivals(&self, now: u64) -> Vec<Option<u64>> {
        let mut arrival = Some(now);
        self.orders
            .iter()
            .enumerate()
            .skip(self.order)
            .map(|(index, stage)| {
                arrival = if !self.autopilot_enabled
                    || matches!(
                        self.status,
                        Status::Paused | Status::Blocked(_) | Status::Idle | Status::Completed
                    ) {
                    None
                } else if let Order::WaitUntil(until) = stage.action {
                    arrival.map(|tick| tick.max(until))
                } else if index == self.order {
                    self.estimated_arrival_tick.map(|tick| tick.max(now))
                } else {
                    arrival
                        .zip(stage.estimated_duration_ticks)
                        .map(|(tick, duration)| tick.saturating_add(duration))
                };
                arrival
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    Direction([f64; 3]),
    Destination(Destination),
    Contact(crate::ContactRef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuidanceMode {
    Align,
    Approach,
    KeepRange,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guidance {
    pub mode: GuidanceMode,
    pub target: Target,
    pub range_m: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuel_priority_exchanges_travel_time_for_fuel_and_checks_each_tank() {
        let fast = PlanningPreferences { fuel_priority: 0.1 }
            .cost(1e5)
            .estimate(1e7, 5., 40.);
        let economy = PlanningPreferences {
            fuel_priority: 100.,
        }
        .cost(1e5)
        .estimate(1e7, 5., 40.);
        assert!(economy.0 > fast.0 && economy.1 < fast.1);
        let budget = FuelBudget {
            resources: vec![
                FuelRequirement {
                    resource: "water".into(),
                    required_kg: 10.,
                    available_kg: 9.,
                },
                FuelRequirement {
                    resource: "hydrogen".into(),
                    required_kg: 10.,
                    available_kg: 1000.,
                },
            ],
            complete: true,
        };
        assert!(budget.valid() && budget.exhausted());
        let mut refuelled = budget.clone();
        refuelled.resources[0].available_kg = 11.;
        assert!(!refuelled.exhausted());
        for value in [f64::NAN, f64::INFINITY, 0., -1., 1001.] {
            assert!(
                !PlanningPreferences {
                    fuel_priority: value
                }
                .valid()
            );
        }
    }

    #[test]
    fn stage_arrivals_accumulate_live_delays_and_stop_at_unknown_durations() {
        let mut state = TravelState {
            autopilot_enabled: true,
            status: Status::Active,
            estimated_arrival_tick: Some(150),
            orders: vec![
                QueuedOrder::estimated(Order::Undock, 1.),
                QueuedOrder::estimated(Order::Undock, 20.),
                Order::WaitUntil(400).into(),
                Order::Undock.into(),
                QueuedOrder::estimated(Order::Undock, 10.),
            ],
            ..Default::default()
        };
        assert_eq!(
            state.stage_arrivals(100),
            vec![Some(150), Some(350), Some(400), None, None]
        );
        state.estimated_arrival_tick = Some(250);
        assert_eq!(
            state.stage_arrivals(100),
            vec![Some(250), Some(450), Some(450), None, None]
        );
        state.order = 1;
        state.estimated_arrival_tick = None;
        assert_eq!(state.stage_arrivals(150), vec![None; 4]);
        state.status = Status::Paused;
        assert_eq!(state.stage_arrivals(150), vec![None; 4]);
    }
}
