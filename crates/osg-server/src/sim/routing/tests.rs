use super::*;
use osg_model::travel::{Axes, CelestialRef, Destination, Reference, slip};

struct Environment {
    targets: Vec<CaptureTarget>,
    cancelled: bool,
}

impl RouteEnvironment for Environment {
    fn system(&self, id: Id) -> Result<(Pose, f64)> {
        let target = self
            .targets
            .iter()
            .find(|target| target.reference.system == id)
            .ok_or_else(|| anyhow::anyhow!("unknown system"))?;
        Ok((target.pose.clone(), target.radius_m * 10.0))
    }
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
    fn departure(&self, _: &Pose, _: GalacticPosition, _: f64) -> Result<Option<Destination>> {
        Ok(None)
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
fn transfer_to_a_moving_waypoint_preserves_shared_orbital_velocity() {
    let performance = request(&target(1, 2.0)).performance;
    let weights = TransferCost::default();
    let offset = DVec3::X * 130_000_000.0;
    let stationary = Pose {
        position: GalacticPosition::from_meters(offset),
        ..Default::default()
    };
    let expected = transfer(&performance, weights, &Pose::default(), &stationary);
    let velocity = DVec3::new(22_000.0, -2_000.0, 22_000.0);
    let origin = Pose {
        velocity: velocity.to_array(),
        ..Default::default()
    };
    let (arrival, actual) = intercept_transfer(&performance, weights, &origin, |seconds| {
        Ok(Pose {
            position: GalacticPosition::from_meters(offset + velocity * seconds),
            velocity: velocity.to_array(),
            ..Default::default()
        })
    })
    .unwrap();
    assert!((actual.0 - expected.0).abs() < 0.01);
    assert!((actual.1 - expected.1).abs() < 0.01);
    assert!(arrival.position.relative_to(stationary.position).length() > 1_000_000.0);
}

#[test]
fn system_arrival_stops_at_capture_without_spending_fuel_on_the_star_centre() {
    let destination = target(1, 2.0);
    let mut request = request(&destination);
    request.orders = vec![Order::TravelToSystem(destination.reference.system)];
    request.performance.propellant_kg_s = 1.0;
    request.performance.fuels = vec![FuelRate {
        resource: "propellant".into(),
        kg_s: 1.0,
        available_kg: 1.0,
    }];
    let environment = Environment {
        targets: vec![destination.clone()],
        cancelled: false,
    };
    let result = plan(&request, &environment).unwrap();
    assert!(matches!(&result.orders[..], [order] if matches!(order.action, Order::Slip { .. })));
    assert_eq!(result.fuel_budget.resources[0].required_kg, 0.0);

    // Already inside the system: there is no trip to the centre to perform.
    request.origin.position = destination
        .pose
        .position
        .offset_by(DVec3::X * destination.radius_m * 2.0);
    assert!(plan(&request, &environment).unwrap().orders.is_empty());

    // A specific location still retains its final approach.
    request.origin = Pose::default();
    request.performance.fuels[0].available_kg = 1e9;
    request.orders = vec![Order::TravelTo(Destination::Relative {
        reference: Reference::Celestial(destination.reference),
        offset: GalacticPosition::from_meters(DVec3::Y * destination.radius_m * 0.5),
        axes: Axes::Galactic,
    })];
    assert!(matches!(
        plan(&request, &environment)
            .unwrap()
            .orders
            .last()
            .unwrap()
            .action,
        Order::Sublight(_)
    ));
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
    let combined = slip::ppm_from_log_loss(
        plan.orders
            .iter()
            .filter(|order| matches!(order.action, Order::Slip { .. }))
            .map(|order| slip::log_loss_from_ppm(order.estimated_loss_ppm.unwrap()))
            .sum(),
    );
    assert!((combined - plan.estimated_loss_ppm).abs() < 1e-6);
}

#[test]
fn unlimited_risk_allows_later_waypoints_after_rounded_total_loss() {
    let mut first = target(1, 100.0);
    first.radius_m = 1000.0;
    first.surface_radius_m = 100.0;
    let mut second = first.clone();
    second.reference = target(2, 200.0).reference;
    second.pose = target(2, 200.0).pose;
    let mut request = request(&second);
    request.preferences.max_loss_ppm = 1_000_000.0;
    request.orders = vec![
        Order::TravelToSystem(first.reference.system),
        Order::TravelToSystem(second.reference.system),
    ];
    let environment = Environment {
        targets: vec![first, second.clone()],
        cancelled: false,
    };

    let plan = plan(&request, &environment).unwrap();
    assert_eq!(plan.orders.len(), 2);
    assert_eq!(plan.orders[0].estimated_loss_ppm, Some(1_000_000.0));
    assert!(matches!(
        &plan.orders[1].action,
        Order::Slip {
            destination: Destination::Relative { reference: Reference::Celestial(reference), .. },
            ..
        } if *reference == second.reference
    ));
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
