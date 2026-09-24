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
pub enum Directive {
    SlipToSystem(Id),
    DockAt(EntityId),
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
pub struct ItineraryEntry {
    pub directive: Directive,
    pub label: String,
    pub max_loss_ppm: f64,
    pub fuel_allowance_kg: f64,
    pub estimated_duration_ticks: Option<u64>,
}

impl Directive {
    pub fn label(&self) -> String {
        match self {
            Self::SlipToSystem(_) => "Slip to system".into(),
            Self::DockAt(_) => "Dock at station".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum FirmwarePhase {
    #[default]
    Idle,
    Planning,
    Waiting { until: Option<u64>, why: String },
    Charging,
    Transit,
    Maneuvering,
    Docking,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanMarker {
    pub position: GalacticPosition,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FirmwareStatus {
    pub phase: FirmwarePhase,
    pub summary: String,
    pub estimated_arrival_tick: Option<u64>,
    pub capture_body: Option<CelestialRef>,
    pub aim_offset_m: Option<[f64; 3]>,
    pub departure_tick: Option<u64>,
    pub planned_delta_v_m_s: f64,
    pub planned_loss_ppm: f64,
    pub spent_loss_ppm: f64,
    pub planned_exotic_fuel_kg: f64,
    pub spent_exotic_fuel_kg: f64,
    pub markers: Vec<PlanMarker>,
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
pub struct AutopilotState {
    pub enabled: bool,
    pub directive_revision: u64,
    pub itinerary: Vec<ItineraryEntry>,
    pub preferences: PlanningPreferences,
    pub risk_budget: RiskBudget,
    pub fuel_budget: Option<FuelBudget>,
    pub status: FirmwareStatus,
    pub failure: Option<String>,
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

}
