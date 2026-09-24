use super::*;
use bevy::math::DVec3;

struct Catalogue {
    systems: Vec<SystemTarget>,
    station: Id,
    cancelled: bool,
}

impl RouteEnvironment for Catalogue {
    fn cancelled(&self) -> bool {
        self.cancelled
    }

    fn system(&self, id: Id) -> Result<SystemTarget> {
        self.systems
            .iter()
            .find(|system| system.id == id)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("unknown system"))
    }

    fn containing(&self, position: GalacticPosition) -> Result<Option<Id>> {
        Ok(self
            .systems
            .iter()
            .find(|system| system.position.relative_to(position).length() <= system.influence_m)
            .map(|system| system.id))
    }

    fn station_system(&self, id: Id) -> Result<Id> {
        ensure!(id == self.station, "unknown station");
        Ok(self.systems.last().unwrap().id)
    }

    fn candidates(
        &self,
        _: GalacticPosition,
        _: GalacticPosition,
        _: usize,
    ) -> Result<Vec<SystemTarget>> {
        Ok(self.systems.clone())
    }
}

fn fixture() -> (RouteRequest, Catalogue) {
    let systems: Vec<_> = [0.0, 1.0, 2.0]
        .into_iter()
        .map(|distance| SystemTarget {
            id: Id::new(),
            position: GalacticPosition::from_meters(DVec3::X * distance * slip::LY_M),
            influence_m: 1e12,
        })
        .collect();
    let request = RouteRequest {
        origin: Pose {
            position: systems[0].position,
            ..Default::default()
        },
        performance: ShipPerformance {
            radius_m: 10.0,
            mass_kg: 100_000.0,
            acceleration_m_s2: 1.0,
            propellant_kg_s: 1.0,
            turn_s: 10.0,
            slip_power_w: 1e12,
            exotic_available_kg: 2.1,
            fuels: vec![],
        },
        preferences: PlanningPreferences {
            fuel_fraction: 1.0,
            ..Default::default()
        },
        tick: 0,
        directives: vec![Directive::SlipToSystem(systems[2].id)],
    };
    (
        request,
        Catalogue {
            systems,
            station: Id::new(),
            cancelled: false,
        },
    )
}

#[test]
fn fuel_constrained_search_keeps_an_intermediate_system() {
    let (request, environment) = fixture();
    let plan = plan(&request, &environment).unwrap();
    assert_eq!(plan.itinerary.len(), 2);
    assert_eq!(
        plan.itinerary[0].directive,
        Directive::SlipToSystem(environment.systems[1].id)
    );
    assert_eq!(plan.itinerary[1].directive, request.directives[0]);
    assert!((plan.exotic_fuel_kg - 2.0).abs() < 1e-9);
    let allocated: f64 = plan
        .itinerary
        .iter()
        .map(|entry| entry.fuel_allowance_kg)
        .sum();
    assert!((allocated - 2.1).abs() < 1e-9);
    let loss: f64 = plan
        .itinerary
        .iter()
        .map(|entry| slip::log_loss_from_ppm(entry.max_loss_ppm))
        .sum();
    assert!((loss - slip::log_loss_from_ppm(100.0)).abs() < 1e-12);
}

#[test]
fn remote_docking_expands_only_system_hops_and_reserves_local_allowances() {
    let (mut request, environment) = fixture();
    request.directives = vec![Directive::DockAt(environment.station)];
    let plan = plan(&request, &environment).unwrap();
    assert_eq!(plan.itinerary.len(), 3);
    let local = plan.itinerary.last().unwrap();
    assert_eq!(local.directive, Directive::DockAt(environment.station));
    assert!(local.estimated_duration_ticks.is_none());
    assert!(local.fuel_allowance_kg > 0.0 && local.max_loss_ppm > 0.0);
    assert!(!plan.fuel_budget.complete);
}

#[test]
fn local_docking_needs_no_drive_and_has_no_fake_transfer_estimate() {
    let (mut request, environment) = fixture();
    request.origin.position = environment.systems[2].position;
    request.performance.slip_power_w = 0.0;
    request.preferences.allow_slipdrive = false;
    request.directives = vec![Directive::DockAt(environment.station)];
    let plan = plan(&request, &environment).unwrap();
    assert_eq!(plan.itinerary.len(), 1);
    assert_eq!(plan.exotic_fuel_kg, 0.0);
    assert!(plan.itinerary[0].estimated_duration_ticks.is_none());
}

#[test]
fn unreachable_route_and_cancellation_fail_without_partial_itineraries() {
    let (mut request, mut environment) = fixture();
    request.performance.exotic_available_kg = 1.9;
    assert!(plan(&request, &environment).is_err());
    request.performance.exotic_available_kg = 10.0;
    request.preferences.max_loss_ppm = 0.0;
    assert!(plan(&request, &environment).is_err());
    request.preferences.max_loss_ppm = 100.0;
    environment.cancelled = true;
    assert!(
        plan(&request, &environment)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}
