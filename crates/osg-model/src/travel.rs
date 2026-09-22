use crate::{EntityId, GalacticPosition, Id};
use serde::{Deserialize, Serialize};

pub mod slip;
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
    Celestial(CelestialRef),
    Beacon(EntityId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CelestialRef {
    pub system: Id,
    pub body: Id,
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
    Guidance(Guidance),
    TravelTo(Destination),
    TravelToSystem(Id),
    Sublight(Destination),
    Slip {
        destination: Destination,
        navigation_beacon: Option<EntityId>,
    },
    Dock(EntityId),
    Undock,
    WaitUntil(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanningPreferences {
    pub fuel_fraction: f64,
    pub max_loss_ppm: f64,
    pub allow_slipdrive: bool,
}

impl Default for PlanningPreferences {
    fn default() -> Self {
        Self {
            fuel_fraction: 0.5,
            max_loss_ppm: 100.0,
            allow_slipdrive: true,
        }
    }
}

impl PlanningPreferences {
    pub fn valid(self) -> bool {
        self.fuel_fraction.is_finite()
            && (0.01..=1.).contains(&self.fuel_fraction)
            && self.max_loss_ppm.is_finite()
            && (0.0..=1_000_000.0).contains(&self.max_loss_ppm)
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
    pub label: String,
    pub transfer_cost: crate::transfer::TransferCost,
    pub action: Order,
    pub estimated_duration_ticks: Option<u64>,
    pub estimated_propellant_kg: Option<f64>,
    pub estimated_loss_ppm: Option<f64>,
}

impl From<Order> for QueuedOrder {
    fn from(action: Order) -> Self {
        let estimated_propellant_kg = matches!(
            action,
            Order::Slip { .. } | Order::Undock | Order::WaitUntil(_)
        )
        .then_some(0.);
        Self {
            label: action.label(),
            transfer_cost: Default::default(),
            action,
            estimated_duration_ticks: None,
            estimated_propellant_kg,
            estimated_loss_ppm: None,
        }
    }
}

impl QueuedOrder {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_propellant(mut self, kg: f64) -> Self {
        self.estimated_propellant_kg = (kg.is_finite() && kg >= 0.).then_some(kg);
        self
    }

    pub fn estimated(action: Order, seconds: f64) -> Self {
        Self {
            label: action.label(),
            transfer_cost: Default::default(),
            action,
            estimated_propellant_kg: None,
            estimated_loss_ppm: None,
            estimated_duration_ticks: (seconds.is_finite() && seconds >= 0.)
                .then(|| (seconds * crate::TICK_RATE_HZ).ceil() as u64),
        }
    }
}

impl Order {
    /// Default queue label for producers without catalogue names.
    pub fn label(&self) -> String {
        match self {
            Self::Guidance(guidance) => format!("{:?}", guidance.mode),
            Self::TravelTo(_) => "Travel to destination".into(),
            Self::TravelToSystem(_) => "Travel to system".into(),
            Self::Sublight(_) => "Sublight transfer".into(),
            Self::Slip { .. } => "Slip arrival".into(),
            Self::Dock(_) => "Dock".into(),
            Self::Undock => "Undock".into(),
            Self::WaitUntil(tick) => format!("Wait until tick {tick}"),
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
    pub risk_budget: RiskBudget,
    pub goals: Vec<Order>,
    pub fuel_budget: Option<FuelBudget>,
    pub planning: Option<PlanningProgress>,
    pub revision: u64,
    pub orders: Vec<QueuedOrder>,
    pub order: usize,
    pub status: Status,
    pub estimated_arrival_tick: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskBudget {
    pub max_log_loss: f64,
    pub spent_log_loss: f64,
}

impl Default for RiskBudget {
    fn default() -> Self {
        Self::new(PlanningPreferences::default().max_loss_ppm)
    }
}

impl RiskBudget {
    pub fn new(max_loss_ppm: f64) -> Self {
        Self {
            max_log_loss: slip::log_loss_from_ppm(max_loss_ppm),
            spent_log_loss: 0.0,
        }
    }

    pub fn remaining_log_loss(self) -> f64 {
        if self.max_log_loss == f64::INFINITY {
            f64::INFINITY
        } else {
            (self.max_log_loss - self.spent_log_loss).max(0.0)
        }
    }

    pub fn remaining_ppm(self) -> f64 {
        slip::ppm_from_log_loss(self.remaining_log_loss())
    }

    pub fn spend(&mut self, loss_ppm: f64) {
        self.spent_log_loss += slip::log_loss_from_ppm(loss_ppm);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CurrentOrder {
    pub autopilot_enabled: bool,
    pub preferences: PlanningPreferences,
    pub revision: u64,
    pub index: usize,
    pub order: Option<QueuedOrder>,
    pub status: Status,
    pub estimated_arrival_tick: Option<u64>,
}

impl From<&TravelState> for CurrentOrder {
    fn from(state: &TravelState) -> Self {
        Self {
            autopilot_enabled: state.autopilot_enabled,
            preferences: state.preferences,
            revision: state.revision,
            index: state.order,
            order: state.orders.get(state.order).cloned(),
            status: state.status.clone(),
            estimated_arrival_tick: state.estimated_arrival_tick,
        }
    }
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
    fn itinerary_allowance_survives_spending_and_replanning() {
        let mut budget = RiskBudget::new(100.0);
        budget.spend(40.0);
        let remaining = budget.remaining_ppm();
        assert!(remaining > 60.0 && remaining < 60.01);
        budget.spend(remaining);
        assert!(budget.remaining_ppm() < 1e-10);
        let mut unlimited = RiskBudget::new(1_000_000.0);
        unlimited.spend(1_000_000.0);
        assert_eq!(unlimited.remaining_ppm(), 1_000_000.0);

        for risk in [0.0, 0.001, 100.0, 1_000_000.0] {
            assert!(
                PlanningPreferences {
                    max_loss_ppm: risk,
                    ..Default::default()
                }
                .valid()
            );
        }
        for risk in [-1.0, 1_000_001.0, f64::NAN, f64::INFINITY] {
            assert!(
                !PlanningPreferences {
                    max_loss_ppm: risk,
                    ..Default::default()
                }
                .valid()
            );
        }
    }

    #[test]
    fn fuel_allowance_validates_fraction_and_checks_each_tank() {
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
                    fuel_fraction: value,
                    ..Default::default()
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
