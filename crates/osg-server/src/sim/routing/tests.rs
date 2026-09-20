use super::*;
use osg_model::travel::{Axes, CelestialRef, Destination, Reference, slip};

struct Environment {
    targets: Vec<CaptureTarget>,
    cancelled: bool,
}

impl RouteEnvironment for Environment {
    fn candidates(
        &self,
        _: GalacticPosition,
        _: GalacticPosition,
        _: usize,
    ) -> Result<Vec<CaptureTarget>> {
        Ok(self.targets.clone())
    }
    fn capture_target(&self, destination: &Destination, _: f64) -> Result<Option<CaptureTarget>> {
        Ok(match destination {
            Destination::Relative {
                reference: Reference::Celestial(reference),
                ..
            } => self
                .targets
                .iter()
                .find(|target| target.reference == *reference)
                .cloned(),
            _ => None,
        })
    }
    fn departure(&self, origin: &Pose, _: GalacticPosition, _: f64) -> Result<Pose> {
        Ok(origin.clone())
    }
    fn resolve(&self, destination: &Destination, _: f64) -> Result<Pose> {
        match destination {
            Destination::Galactic(position) => Ok(Pose {
                position: *position,
                ..Default::default()
            }),
            _ => Ok(self.capture_target(destination, 0.0)?.unwrap().pose),
        }
    }
    fn beacon(&self, _: Id) -> Result<Beacon> {
        anyhow::bail!("no beacon")
    }
    fn contact(&self, _: osg_model::ContactRef) -> Result<(Pose, f64)> {
        anyhow::bail!("no contact")
    }
    fn slip(
        &self,
        origin: GalacticPosition,
        destination: GalacticPosition,
        _: f64,
        _: f64,
        speed: f64,
    ) -> Result<SlipEstimate> {
        Ok(SlipEstimate {
            ready: true,
            preparation_s: 10.0,
            duration_s: destination.relative_to(origin).length() / slip::LY_M / speed,
        })
    }
    fn cancelled(&self) -> bool {
        self.cancelled
    }
}

fn target(index: u8, distance_ly: f64) -> CaptureTarget {
    CaptureTarget {
        reference: CelestialRef {
            system: Id([index; 16]),
            body: Id([0; 16]),
        },
        pose: Pose {
            position: GalacticPosition::from_meters(DVec3::X * distance_ly * slip::LY_M),
            ..Default::default()
        },
        radius_m: slip::exclusion_radius_m(slip::SOLAR_MASS_KG),
        surface_radius_m: 7e8,
        navigation_beacon: None,
    }
}

fn request(target: &CaptureTarget) -> RouteRequest {
    RouteRequest {
        origin: Pose::default(),
        performance: ShipPerformance {
            radius_m: 10.0,
            mass_kg: 10_000.0,
            acceleration_m_s2: 2.0,
            propellant_kg_s: 0.0,
            turn_s: 1.0,
            slip_power_w: 1e9,
            exotic_available_kg: 10_000.0,
            fuels: Vec::new(),
        },
        preferences: PlanningPreferences::default(),
        tick: 0,
        orders: vec![Order::TravelTo(Destination::Relative {
            reference: Reference::Celestial(target.reference),
            offset: GalacticPosition::default(),
            axes: Axes::Galactic,
        })],
        docked_at: None,
    }
}

#[test]
fn direct_route_near_floor_uses_entire_itinerary_allowance() {
    let target = target(1, 2.7);
    let request = request(&target);
    let environment = Environment {
        targets: vec![target],
        cancelled: false,
    };
    let plan = plan(&request, &environment).unwrap();
    assert_eq!(
        plan.orders
            .iter()
            .filter(|order| matches!(order.action, Order::Slip { .. }))
            .count(),
        1
    );
    assert!(plan.estimated_loss_ppm <= 100.0 + 1e-6);
}

#[test]
fn intermediate_captures_share_the_whole_risk_budget() {
    let destination = target(3, 6.0);
    let mut request = request(&destination);
    request.orders.insert(
        0,
        Order::TravelTo(Destination::Relative {
            reference: Reference::Celestial(target(2, 4.0).reference),
            offset: GalacticPosition::default(),
            axes: Axes::Galactic,
        }),
    );
    let environment = Environment {
        targets: vec![target(1, 2.0), target(2, 4.0), destination],
        cancelled: false,
    };
    let plan = plan(&request, &environment).unwrap();
    assert!(
        plan.orders
            .iter()
            .filter(|order| matches!(order.action, Order::Slip { .. }))
            .count()
            >= 2
    );
    assert!(plan.estimated_loss_ppm <= request.preferences.max_loss_ppm + 1e-6);
}

#[test]
fn cancelled_search_stops_before_resolving_routes() {
    let target = target(1, 2.0);
    let request = request(&target);
    let environment = Environment {
        targets: vec![target],
        cancelled: true,
    };
    assert!(
        plan(&request, &environment)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}

#[test]
fn exhausted_search_keeps_a_qualifying_route_and_finishes_validation() {
    let target = target(1, 2.7);
    let request = request(&target);
    let environment = Environment {
        targets: vec![target],
        cancelled: false,
    };
    let mut work = Work {
        spent: 0,
        limit: ENVIRONMENT_WORK * MAX_ORDERS as u64 * 32 + ENVIRONMENT_WORK * 6,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
    };
    let plan = plan_inner(
        &request,
        &environment,
        &mut work,
        TransferCost {
            seconds_per_kg: 0.0,
        },
    )
    .unwrap();
    assert!(
        plan.orders
            .iter()
            .any(|order| matches!(order.action, Order::Slip { .. }))
    );
    assert!(plan.estimated_loss_ppm <= 100.0 + 1e-6);
    assert!(work.spent <= work.limit);
}
