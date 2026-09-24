mod graph;

#[cfg(test)]
mod tests;

use anyhow::{Result, ensure};
use osg_model::{
    GalacticPosition, Id, Pose,
    travel::{Directive, FuelBudget, FuelRequirement, ItineraryEntry, PlanningPreferences, slip},
};

pub const MAX_WORK: u64 = 2_400_000;
pub const ENVIRONMENT_WORK: u64 = 4096;
pub const MAX_DIRECTIVES: usize = osg_model::routing::MAX_DIRECTIVES;

#[derive(Clone, Debug)]
pub struct FuelRate {
    pub resource: String,
    pub kg_s: f64,
    pub available_kg: f64,
}

#[derive(Clone, Debug)]
pub struct ShipPerformance {
    pub radius_m: f64,
    pub mass_kg: f64,
    pub acceleration_m_s2: f64,
    pub propellant_kg_s: f64,
    pub turn_s: f64,
    pub slip_power_w: f64,
    pub exotic_available_kg: f64,
    pub fuels: Vec<FuelRate>,
}

#[derive(Clone, Debug)]
pub struct RouteRequest {
    pub origin: Pose,
    pub performance: ShipPerformance,
    pub preferences: PlanningPreferences,
    pub tick: u64,
    pub directives: Vec<Directive>,
}

#[derive(Clone, Copy, Debug)]
pub struct SystemTarget {
    pub id: Id,
    pub position: GalacticPosition,
    pub influence_m: f64,
}

/// Strategic catalogue access. Local geometry belongs to ship firmware.
pub trait RouteEnvironment: Send + Sync {
    fn system(&self, id: Id) -> Result<SystemTarget>;
    fn containing(&self, position: GalacticPosition) -> Result<Option<Id>>;
    fn station_system(&self, id: Id) -> Result<Id>;
    fn candidates(
        &self,
        origin: GalacticPosition,
        goal: GalacticPosition,
        limit: usize,
    ) -> Result<Vec<SystemTarget>>;
    fn label(&self, directive: &Directive) -> String {
        directive.label()
    }
    fn cancelled(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct RoutePlan {
    pub itinerary: Vec<ItineraryEntry>,
    pub fuel_budget: FuelBudget,
    pub work: u64,
    pub estimated_loss_ppm: f64,
    pub exotic_fuel_kg: f64,
}

struct Work {
    spent: u64,
    deadline: std::time::Instant,
}

impl Work {
    fn charge(&mut self, count: u64, environment: &impl RouteEnvironment) -> Result<()> {
        self.spent = self.spent.saturating_add(count);
        ensure!(
            self.spent <= MAX_WORK,
            "route computation work limit exceeded"
        );
        ensure!(!environment.cancelled(), "route computation cancelled");
        ensure!(
            std::time::Instant::now() < self.deadline,
            "route search time budget exhausted"
        );
        Ok(())
    }
}

fn hop_seconds(performance: &ShipPerformance, distance_m: f64) -> f64 {
    let charge = slip::charging_energy_j(performance.mass_kg, distance_m / slip::LY_M)
        / performance.slip_power_w;
    charge.max(slip::MIN_CHARGE_SECONDS) + slip::flight_seconds(distance_m, false)
}

pub fn plan(request: &RouteRequest, environment: &impl RouteEnvironment) -> Result<RoutePlan> {
    plan_metered(request, environment).0
}

pub fn plan_metered(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
) -> (Result<RoutePlan>, u64) {
    let mut work = Work {
        spent: 0,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
    };
    let result = plan_inner(request, environment, &mut work);
    (result, work.spent)
}

fn plan_inner(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
    work: &mut Work,
) -> Result<RoutePlan> {
    ensure!(request.preferences.valid(), "invalid route preferences");
    ensure!(
        request.directives.len() <= MAX_DIRECTIVES,
        "too many route directives"
    );
    let performance = &request.performance;
    ensure!(
        performance.mass_kg.is_finite()
            && performance.mass_kg > 0.0
            && performance.exotic_available_kg.is_finite()
            && performance.exotic_available_kg >= 0.0
            && performance.slip_power_w.is_finite()
            && performance.slip_power_w >= 0.0,
        "invalid ship performance"
    );
    let fuel_limit = performance.exotic_available_kg * request.preferences.fuel_fraction;
    let mut origin = request.origin.position;
    let mut current_system = environment.containing(origin)?;
    let mut itinerary = Vec::new();
    let mut baseline = Vec::new();
    let mut exotic_fuel_kg = 0.0;

    for directive in &request.directives {
        work.charge(ENVIRONMENT_WORK, environment)?;
        let target_id = match directive {
            Directive::SlipToSystem(id) => *id,
            Directive::DockAt(id) => environment.station_system(*id)?,
        };
        let goal = environment.system(target_id)?;
        if current_system != Some(target_id) {
            ensure!(
                request.preferences.allow_slipdrive,
                "Slipdrive disabled in route preferences"
            );
            ensure!(
                request.preferences.max_loss_ppm > 0.0,
                "zero destruction risk excludes slip travel"
            );
            ensure!(
                performance.slip_power_w > 0.0,
                "ship has no available slipdrive"
            );
            for target in graph::search(
                request,
                environment,
                work,
                origin,
                goal,
                (fuel_limit - exotic_fuel_kg).max(0.0),
            )? {
                let distance = target.position.relative_to(origin).length();
                let fuel = slip::exotic_fuel_kg(performance.mass_kg, distance / slip::LY_M);
                exotic_fuel_kg += fuel;
                let action = Directive::SlipToSystem(target.id);
                itinerary.push(ItineraryEntry {
                    label: environment.label(&action),
                    directive: action,
                    max_loss_ppm: 0.0,
                    fuel_allowance_kg: fuel,
                    estimated_duration_ticks: Some(
                        (hop_seconds(performance, distance) * osg_model::TICK_RATE_HZ).ceil()
                            as u64,
                    ),
                });
                baseline.push(fuel);
                origin = target.position;
            }
            current_system = Some(target_id);
        }
        if let Directive::DockAt(_) = directive {
            itinerary.push(ItineraryEntry {
                label: environment.label(directive),
                directive: directive.clone(),
                max_loss_ppm: 0.0,
                fuel_allowance_kg: 0.0,
                estimated_duration_ticks: None,
            });
            baseline.push(0.0);
        }
        ensure!(
            itinerary.len() <= MAX_DIRECTIVES,
            "expanded itinerary exceeds 256 directives"
        );
    }

    // Preserve distance fuel first. Every stage, including local docking, shares
    // the discretionary reserve for arrival velocity changes and local slips.
    let stages = itinerary.len().max(1) as f64;
    let extra = (fuel_limit - exotic_fuel_kg).max(0.0) / stages;
    let loss =
        slip::ppm_from_log_loss(slip::log_loss_from_ppm(request.preferences.max_loss_ppm) / stages);
    for (entry, fuel) in itinerary.iter_mut().zip(baseline) {
        entry.fuel_allowance_kg = fuel + extra;
        entry.max_loss_ppm = loss;
    }
    let mut resources: Vec<_> = performance
        .fuels
        .iter()
        .map(|fuel| FuelRequirement {
            resource: fuel.resource.clone(),
            required_kg: 0.0,
            available_kg: fuel.available_kg,
        })
        .collect();
    resources.push(FuelRequirement {
        resource: slip::EXOTIC_RESOURCE.to_owned(),
        required_kg: exotic_fuel_kg,
        available_kg: performance.exotic_available_kg,
    });
    let estimated_loss_ppm = if itinerary.is_empty() {
        0.0
    } else {
        request.preferences.max_loss_ppm
    };
    let fuel_budget = FuelBudget {
        resources,
        complete: itinerary.is_empty(),
    };
    ensure!(fuel_budget.valid(), "invalid fuel observations");
    Ok(RoutePlan {
        itinerary,
        fuel_budget,
        work: work.spent,
        // This is an allocated upper allowance, not a capture-geometry forecast.
        estimated_loss_ppm,
        exotic_fuel_kg,
    })
}
