use super::*;
use bevy::math::DQuat;
use std::collections::{BTreeMap, BTreeSet};
use toy_sim_model::{IffIdentity, travel::*};

#[derive(Default)]
struct Environment {
    gates: Vec<RouteGate>,
    beacons: BTreeMap<Id, Beacon>,
    slip_enabled: bool,
    cancelled: bool,
    contacts: Vec<(toy_sim_model::ContactRef, Pose, f64)>,
}

fn id(value: u128) -> Id {
    Id(value.to_le_bytes())
}

fn pose(position: DVec3) -> Pose {
    Pose {
        position: GalacticPosition::from_meters(position),
        velocity: [0.0; 3],
        rotation: DQuat::IDENTITY.to_array(),
        angular_velocity: [0.0; 3],
    }
}

fn beacon(value: u128, position: DVec3, radius_m: f64) -> Beacon {
    Beacon {
        entity: id(value),
        pose: pose(position),
        radius_m,
        iff: IffIdentity {
            owner: id(999),
            faction: None,
            labels: BTreeSet::new(),
            enabled: true,
            range_m: 1e20,
        },
        bays: BTreeMap::new(),
        gate_exit: None,
        exclusion_m: 0.0,
    }
}

fn performance() -> ShipPerformance {
    ShipPerformance {
        radius_m: 10.0,
        mass_kg: 10_000.0,
        acceleration_m_s2: 2.0,
        propellant_kg_s: 10.0,
        turn_s: 1.0,
        slip_power_w: 0.0,
        fuels: vec![
            FuelRate {
                resource: "water".into(),
                kg_s: 9.0,
                available_kg: 1e8,
            },
            FuelRate {
                resource: "reactor_fuel".into(),
                kg_s: 1.0,
                available_kg: 1e8,
            },
        ],
    }
}

fn request(position: DVec3, orders: Vec<Order>) -> RouteRequest {
    RouteRequest {
        origin: pose(position),
        performance: performance(),
        preferences: PlanningPreferences::default(),
        tick: 100,
        orders,
        docked_at: None,
    }
}

impl Environment {
    fn pair(&mut self, a: u128, b: u128, sa: u128, sb: u128, pa: DVec3, pb: DVec3) {
        for (own, other, system, position) in [(a, b, sa, pa), (b, a, sb, pb)] {
            let mut mouth = beacon(own, position, 220.0);
            mouth.gate_exit = Some(id(other));
            mouth.exclusion_m = 1e7;
            self.gates.push(RouteGate {
                navigation: NavigationGate {
                    entity: mouth.entity,
                    exit: id(other),
                    system: id(system),
                    pose: mouth.pose.clone(),
                    staging: mouth.pose.position.offset_by(DVec3::Z * (1e7 + 1010.0)),
                    slip_ready: self.slip_enabled,
                },
                aperture_radius_m: mouth.radius_m,
                exclusion_m: mouth.exclusion_m,
            });
            self.beacons.insert(mouth.entity, mouth);
        }
    }

    fn at(&self, value: Id, after_s: f64) -> Result<Pose> {
        let mut pose = self.beacon(value)?.pose;
        pose.position = pose
            .position
            .offset_by(DVec3::from_array(pose.velocity) * after_s);
        let angular = DVec3::from_array(pose.angular_velocity);
        if angular.length() > 0.0 {
            pose.rotation =
                (DQuat::from_axis_angle(angular.normalize(), angular.length() * after_s)
                    * DQuat::from_array(pose.rotation))
                .to_array();
        }
        Ok(pose)
    }
}

impl RouteEnvironment for Environment {
    fn gates(&self) -> &[RouteGate] {
        &self.gates
    }

    fn resolve(&self, destination: &Destination, after_s: f64) -> Result<Pose> {
        match destination {
            Destination::Galactic(position) => {
                let mut result = pose(DVec3::ZERO);
                result.position = *position;
                Ok(result)
            }
            Destination::Beacon(value) => self.at(*value, after_s),
            Destination::Relative {
                reference,
                offset,
                axes,
            } => {
                let (Reference::Beacon(value) | Reference::Celestial(value)) = reference;
                let mut result = self.at(*value, after_s)?;
                let mut displacement = offset.relative_to(GalacticPosition::default());
                if *axes == Axes::BodyFixed {
                    displacement = DQuat::from_array(result.rotation) * displacement;
                    result.velocity = (DVec3::from_array(result.velocity)
                        + DVec3::from_array(result.angular_velocity).cross(displacement))
                    .to_array();
                }
                result.position = result.position.offset_by(displacement);
                Ok(result)
            }
        }
    }

    fn beacon(&self, value: Id) -> Result<Beacon> {
        self.beacons
            .get(&value)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown beacon"))
    }

    fn contact(&self, reference: toy_sim_model::ContactRef) -> Result<(Pose, f64)> {
        self.contacts
            .iter()
            .find(|(known, _, _)| *known == reference)
            .map(|(_, pose, radius)| (pose.clone(), *radius))
            .ok_or_else(|| anyhow::anyhow!("contact unavailable"))
    }

    fn slip(
        &self,
        origin: GalacticPosition,
        destination: GalacticPosition,
        departure_after_s: f64,
        arrival_after_s: f64,
    ) -> Result<SlipEstimate> {
        let clear = |point: GalacticPosition, time: f64| {
            self.gates.iter().all(|gate| {
                point
                    .relative_to(self.at(gate.navigation.entity, time).unwrap().position)
                    .length()
                    > gate.exclusion_m + 10.0
            })
        };
        let (preparation_s, duration_s) =
            crate::sim::travel::slip_times(origin, destination, 10_000.0, 1e8, None, 0);
        Ok(SlipEstimate {
            ready: self.slip_enabled
                && clear(origin, departure_after_s)
                && clear(destination, arrival_after_s),
            preparation_s,
            duration_s,
        })
    }

    fn cancelled(&self) -> bool {
        self.cancelled
    }
}

fn duration(plan: &RoutePlan) -> u64 {
    plan.orders
        .iter()
        .map(|order| order.estimated_duration_ticks.unwrap())
        .sum()
}

fn fuel(plan: &RoutePlan) -> f64 {
    plan.fuel_budget
        .resources
        .iter()
        .map(|entry| entry.required_kg)
        .sum()
}

#[test]
fn two_gate_route_emits_only_the_complete_strategic_queue() {
    let mut env = Environment::default();
    env.pair(1, 2, 100, 200, DVec3::X * 1e4, DVec3::X * 1e12);
    env.pair(3, 4, 200, 300, DVec3::X * (1e12 + 1e7), DVec3::X * 2e12);
    let mut station = beacon(5, DVec3::X * (2e12 + 1e5), 1000.0);
    station.bays.insert(0, station.pose.clone());
    env.beacons.insert(station.entity, station);
    let result = plan(&request(DVec3::ZERO, vec![Order::Dock(id(5))]), &env).unwrap();
    let actions: Vec<_> = result
        .orders
        .iter()
        .map(|order| order.action.clone())
        .collect();
    assert_eq!(
        actions,
        vec![Order::Jump(id(1)), Order::Jump(id(3)), Order::Dock(id(5))]
    );
    assert!(result.fuel_budget.complete && duration(&result) > 0 && fuel(&result) > 0.0);
}

#[test]
fn slip_is_selected_when_faster_and_does_not_brake_galactic_velocity() {
    let env = Environment {
        slip_enabled: true,
        ..Default::default()
    };
    let mut input = request(
        DVec3::ZERO,
        vec![Order::TravelTo(Destination::Galactic(
            GalacticPosition::from_meters(DVec3::X * 1.9e16),
        ))],
    );
    input.performance.slip_power_w = 1e8;
    let result = plan(&input, &env).unwrap();
    assert!(matches!(result.orders[0].action, Order::Slip { .. }));
    assert!(duration(&result) < 1000);
    assert_eq!(fuel(&result), 0.0);
    input.preferences.allow_wormholes = false;
    let direct = plan(&input, &env).unwrap();
    assert_eq!(direct.orders, result.orders);
    assert_eq!(fuel(&direct), 0.0);

    input.origin.velocity = [32_000.0, 0.0, 0.0];
    input.orders = vec![Order::Slip {
        destination: Destination::Galactic(GalacticPosition::from_meters(DVec3::X * 1.9e16)),
    }];
    let moving = plan(&input, &env).unwrap();
    assert_eq!(moving.orders.len(), 1);
    assert_eq!(fuel(&moving), 0.0);
}

#[test]
fn exclusion_escape_is_costed_inside_the_strategic_slip_command() {
    let mut env = Environment {
        slip_enabled: true,
        ..Default::default()
    };
    env.pair(1, 2, 100, 200, DVec3::ZERO, DVec3::X * 1e16);
    for gate in &mut env.gates {
        gate.aperture_radius_m = 5.0;
    }
    let destination = Destination::Galactic(GalacticPosition::from_meters(DVec3::Y * 1.9e16));
    let mut input = request(DVec3::Z * 500.0, vec![Order::TravelTo(destination.clone())]);
    input.performance.slip_power_w = 1e8;
    let result = plan(&input, &env).unwrap();
    assert_eq!(result.orders.len(), 1);
    assert_eq!(result.orders[0].action, Order::Slip { destination });
    assert!(result.orders[0].estimated_propellant_kg.unwrap() > 0.0);
}

#[test]
fn fuel_allowance_limits_the_whole_route_per_resource() {
    let env = Environment::default();
    let mut input = request(
        DVec3::ZERO,
        vec![Order::Sublight(Destination::Galactic(
            GalacticPosition::from_meters(DVec3::X * 1e7),
        ))],
    );
    input.preferences.fuel_fraction = 1.;
    let fast = plan(&input, &env).unwrap();
    input.preferences.fuel_fraction = 0.1;
    input.performance.fuels[0].available_kg = fast.fuel_budget.resources[0].required_kg;
    let slow = plan(&input, &env).unwrap();
    assert!(duration(&slow) > duration(&fast));
    assert!(fuel(&slow) < fuel(&fast));
    assert!(
        slow.fuel_budget
            .resources
            .iter()
            .all(|r| r.required_kg <= r.available_kg * 0.1 + 1e-6)
    );
    assert!(
        (slow.fuel_budget.resources[0].required_kg / slow.fuel_budget.resources[1].required_kg
            - 9.0)
            .abs()
            < 1e-10
    );
}

#[test]
fn sublight_estimate_is_invariant_under_common_galactic_velocity() {
    let mut env = Environment::default();
    env.beacons.insert(id(1), beacon(1, DVec3::X * 1e6, 1.0));
    let mut input = request(
        DVec3::ZERO,
        vec![Order::Sublight(Destination::Beacon(id(1)))],
    );
    let stationary = plan(&input, &env).unwrap();
    input.origin.velocity = [32_000.0, -20_000.0, 4000.0];
    env.beacons.get_mut(&id(1)).unwrap().pose.velocity = input.origin.velocity;
    let moving = plan(&input, &env).unwrap();
    assert_eq!(duration(&stationary), duration(&moving));
    assert!((fuel(&stationary) - fuel(&moving)).abs() < 1e-6);
}

#[test]
fn docked_departure_emits_undock_without_local_waypoints() {
    let mut env = Environment::default();
    env.beacons.insert(id(1), beacon(1, DVec3::ZERO, 1000.0));
    let mut input = request(
        DVec3::ZERO,
        vec![Order::Sublight(Destination::Galactic(
            GalacticPosition::from_meters(DVec3::Z * 1e5),
        ))],
    );
    input.docked_at = Some(id(1));
    let result = plan(&input, &env).unwrap();
    assert_eq!(result.orders.len(), 2);
    assert_eq!(result.orders[0].action, Order::Undock);
    assert_eq!(result.orders[1].action, input.orders[0]);
}

#[test]
fn gate_approach_and_exit_remain_inside_one_jump_command() {
    let mut env = Environment::default();
    env.pair(1, 2, 100, 200, DVec3::ZERO, DVec3::X * 1e12);
    env.beacons.get_mut(&id(2)).unwrap().pose.rotation =
        DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array();
    let result = plan(
        &request(DVec3::NEG_X * 1000.0, vec![Order::Jump(id(1))]),
        &env,
    )
    .unwrap();
    assert_eq!(result.orders.len(), 1);
    assert_eq!(result.orders[0].action, Order::Jump(id(1)));
}

#[test]
fn finite_approach_can_precede_another_order_and_empty_preview_is_valid() {
    let mut env = Environment::default();
    env.beacons.insert(id(1), beacon(1, DVec3::X * 1e5, 100.0));
    let mut input = request(
        DVec3::ZERO,
        vec![
            Order::Undock,
            Order::Guidance(Guidance {
                mode: GuidanceMode::Approach,
                target: Target::Destination(Destination::Beacon(id(1))),
                range_m: 500.0,
            }),
            Order::Sublight(Destination::Galactic(GalacticPosition::from_meters(
                DVec3::X * 2e5,
            ))),
        ],
    );
    let result = plan(&input, &env).unwrap();
    assert_eq!(result.orders.len(), 2);
    assert!(result.fuel_budget.complete);
    input.orders.clear();
    assert!(plan(&input, &env).unwrap().orders.is_empty());
}

#[test]
fn cancelled_and_failed_queries_report_bounded_nonzero_work() {
    let input = request(
        DVec3::ZERO,
        vec![Order::Sublight(Destination::Beacon(id(123)))],
    );
    let (result, work) = plan_metered(&input, &Environment::default());
    assert!(result.is_err() && work >= ENVIRONMENT_WORK);
    assert!(work <= work_limit(&input, 0));
    assert!(work_limit(&input, 0) < MAX_WORK / 10);
    let (result, work) = plan_metered(
        &input,
        &Environment {
            cancelled: true,
            ..Default::default()
        },
    );
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(work, 1);
}

#[test]
fn replanning_an_expanded_gate_queue_does_not_accumulate_checkpoints() {
    let mut env = Environment::default();
    env.pair(1, 2, 100, 200, DVec3::ZERO, DVec3::X * 1e12);
    let mut input = request(DVec3::NEG_X * 1000.0, vec![Order::Jump(id(1))]);
    let original = plan(&input, &env).unwrap();
    for _ in 0..5 {
        input.orders = plan(&input, &env)
            .unwrap()
            .orders
            .into_iter()
            .map(|order| order.action)
            .collect();
        assert_eq!(input.orders.len(), original.orders.len());
    }
}

#[test]
fn finite_contact_approach_retains_live_reference_and_rejects_unknown_contacts() {
    let reference = toy_sim_model::ContactRef {
        group: id(100),
        track: id(101),
    };
    let env = Environment {
        contacts: vec![(reference, pose(DVec3::X * 1e5), 200.0)],
        ..Default::default()
    };
    let mut input = request(
        DVec3::ZERO,
        vec![
            Order::Guidance(Guidance {
                mode: GuidanceMode::Approach,
                target: Target::Contact(reference),
                range_m: 1000.0,
            }),
            Order::TravelTo(Destination::Galactic(GalacticPosition::from_meters(
                DVec3::X * 2e5,
            ))),
        ],
    );
    let result = plan(&input, &env).unwrap();
    assert!(
        matches!(&result.orders[0].action, Order::Guidance(Guidance {
        mode: GuidanceMode::Approach, target: Target::Contact(found), .. }) if *found == reference)
    );
    assert!(result.fuel_budget.complete && result.orders.len() == 2);
    if let Order::Guidance(guidance) = &mut input.orders[0] {
        guidance.target = Target::Contact(toy_sim_model::ContactRef {
            group: id(999),
            ..reference
        });
    }
    assert!(
        plan(&input, &env)
            .unwrap_err()
            .to_string()
            .contains("contact unavailable")
    );
}

#[test]
fn local_routes_preserve_metre_precision_at_extreme_galactic_coordinates() {
    let origin = GalacticPosition {
        x: 1_i128 << 100,
        y: 0,
        z: 0,
    };
    let first = origin.offset_by(DVec3::NEG_Z * 100.0);
    let second = origin.offset_by(DVec3::NEG_Z * 200.0);
    let mut input = request(
        DVec3::ZERO,
        vec![
            Order::TravelTo(Destination::Galactic(first)),
            Order::WaitUntil(9999),
            Order::TravelTo(Destination::Galactic(second)),
        ],
    );
    input.origin.position = origin;
    let result = plan(&input, &Environment::default()).unwrap();
    assert_eq!(result.orders.len(), 3);
    assert_eq!(
        result.orders[0].action,
        Order::Sublight(Destination::Galactic(first))
    );
    assert_eq!(
        result.orders[2].action,
        Order::Sublight(Destination::Galactic(second))
    );
    assert!(result.fuel_budget.complete);
}

#[test]
fn slip_can_target_a_station_without_requiring_a_clear_exit() {
    let mut env = Environment {
        slip_enabled: true,
        ..Default::default()
    };
    env.pair(1, 2, 100, 200, DVec3::ZERO, DVec3::X * 1e16);
    for gate in &mut env.gates {
        gate.aperture_radius_m = 5.0;
    }
    let mut station = beacon(5, DVec3::X * (1e16 + 1e4), 1000.0);
    station.bays.insert(0, station.pose.clone());
    env.beacons.insert(id(5), station);
    let mut input = request(DVec3::Z * 2e7, vec![Order::Dock(id(5))]);
    input.performance.slip_power_w = 1e8;
    let result = plan(&input, &env).unwrap();
    assert_eq!(result.orders.len(), 2);
    assert_eq!(
        result.orders[0].action,
        Order::Slip {
            destination: Destination::Beacon(id(5))
        }
    );
    assert_eq!(result.orders[1].action, Order::Dock(id(5)));
}

#[test]
fn future_docking_and_departure_remain_strategic_actions() {
    let mut env = Environment::default();
    let mut station = beacon(1, DVec3::X * 1e5, 1000.0);
    station.bays.insert(0, station.pose.clone());
    env.beacons.insert(id(1), station);
    let destination = Destination::Galactic(GalacticPosition::from_meters(DVec3::X * 2e5));
    let input = request(
        DVec3::ZERO,
        vec![Order::Dock(id(1)), Order::TravelTo(destination.clone())],
    );
    let result = plan(&input, &env).unwrap();
    let actions: Vec<_> = result
        .orders
        .into_iter()
        .map(|order| order.action)
        .collect();
    assert_eq!(
        actions,
        vec![
            Order::Dock(id(1)),
            Order::Undock,
            Order::Sublight(destination)
        ]
    );
}

#[test]
fn short_slip_does_not_avoid_the_cost_of_rendezvous_with_a_stationary_goal() {
    let env = Environment {
        slip_enabled: true,
        ..Default::default()
    };
    for offset in [DVec3::X * 100.0, DVec3::NEG_X * 100.0, DVec3::Y * 100.0] {
        let destination = Destination::Galactic(GalacticPosition::from_meters(offset));
        let mut input = request(DVec3::ZERO, vec![Order::TravelTo(destination.clone())]);
        input.origin.velocity = [30_000.0, 0.0, 0.0];
        input.performance.slip_power_w = 1e8;
        let strategic = plan(&input, &env).unwrap();
        assert_eq!(strategic.orders.len(), 1);
        assert_eq!(
            strategic.orders[0].action,
            Order::Sublight(destination.clone())
        );

        input.orders = vec![
            Order::Slip {
                destination: destination.clone(),
            },
            Order::Sublight(destination.clone()),
        ];
        let forced_slip = plan(&input, &env).unwrap();
        let weights = forced_slip.orders.last().unwrap().transfer_cost;
        let objective = |result: &RoutePlan| {
            duration(result) as f64 * 0.1 + weights.seconds_per_kg * fuel(result)
        };
        assert!(objective(&strategic) < objective(&forced_slip));

        let target = env.resolve(&destination, 0.0).unwrap();
        let rendezvous = slip_rendezvous(&input.performance, weights, &input.origin, &target);
        assert!((fuel(&forced_slip) - rendezvous.1).abs() < 1e-6);
        let slip_seconds = forced_slip.orders[0].estimated_duration_ticks.unwrap() as f64 * 0.1;
        assert!((duration(&forced_slip) as f64 * 0.1 - slip_seconds - rendezvous.0).abs() <= 0.1);
        assert!(rendezvous.0 > 2.0 * 30_000.0 / input.performance.acceleration_m_s2);
    }
}

#[test]
fn retained_velocity_rendezvous_is_continuous_at_zero_separation() {
    let performance = performance();
    let preferences = TransferCost::default();
    let mut origin = pose(DVec3::ZERO);
    origin.position = GalacticPosition {
        x: 1_i128 << 100,
        y: 0,
        z: 0,
    };
    origin.velocity = [30_000.0, 0.0, 0.0];
    let mut target = origin.clone();
    target.velocity = [0.0; 3];
    let at_target = transfer(&performance, preferences, &origin, &target);
    for direction in [DVec3::X, DVec3::NEG_X, DVec3::Y, DVec3::NEG_Y, DVec3::Z] {
        target.position = origin.position.offset_by(direction * 0.001);
        let near_target = transfer(&performance, preferences, &origin, &target);
        assert!((near_target.0 - at_target.0).abs() < 1e-5);
        assert!((near_target.1 - at_target.1).abs() < 1e-4);
    }

    let common = DVec3::new(32_000.0, -19_000.0, 4000.0);
    origin.velocity = (DVec3::from_array(origin.velocity) + common).to_array();
    target.position = origin.position;
    target.velocity = common.to_array();
    let translated = transfer(&performance, preferences, &origin, &target);
    assert_eq!(translated, at_target);
}

#[test]
fn disabled_transit_modes_are_excluded_and_fuel_caps_reject_infeasible_routes() {
    let mut env = Environment {
        slip_enabled: true,
        ..Default::default()
    };
    env.pair(1, 2, 100, 200, DVec3::X * 1e4, DVec3::X * 1e12);
    let mut input = request(DVec3::ZERO, vec![Order::Jump(id(1))]);
    input.preferences.allow_wormholes = false;
    assert!(
        plan(&input, &env)
            .unwrap_err()
            .to_string()
            .contains("Wormholes disabled")
    );
    input.orders = vec![Order::Slip {
        destination: Destination::Galactic(GalacticPosition::from_meters(DVec3::X * 1e8)),
    }];
    input.preferences.allow_slipdrive = false;
    assert!(
        plan(&input, &env)
            .unwrap_err()
            .to_string()
            .contains("Slipdrive disabled")
    );
    input.orders = vec![Order::Sublight(Destination::Galactic(
        GalacticPosition::from_meters(DVec3::X * 1e6),
    ))];
    input.origin.velocity = [1000., 0., 0.];
    for fuel in &mut input.performance.fuels {
        fuel.available_kg = 0.;
    }
    assert!(plan(&input, &env).is_err());
}
