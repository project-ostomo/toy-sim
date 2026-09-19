use glam::DVec3;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use toy_sim_model::{
    GalacticPosition, Id, NavigationGate, Pose, ProgramAction, ProgramQuery, ProgramReply,
    travel::{Destination, Order, Status, TravelState},
};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{
    ControllerRuntime, FUEL_PER_TICK, Input, MEMORY_LIMIT, Observation, ScanSource, SensorContact,
};
use toy_sim_ships::{Catalogue, EXAMPLE_CONTROLLER, ShipState, expedition_patrol};

fn id(value: u32) -> Id {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&value.to_be_bytes());
    Id(bytes)
}

fn position(system: usize) -> GalacticPosition {
    GalacticPosition::from_meters(
        DVec3::new(
            (system % 20) as f64,
            ((system / 20) % 15) as f64,
            (system / 300) as f64,
        ) * (5. * 9.4607304725808e15),
    )
}

struct PublicNavigation {
    gates: Vec<NavigationGate>,
    origin: GalacticPosition,
    target: GalacticPosition,
    slip: bool,
    active: AtomicBool,
    revision: AtomicU64,
    full_pages: AtomicU64,
    review_shift_m: f64,
    review_blocks_gate: bool,
}

impl PublicNavigation {
    fn tree() -> Self {
        let mut gates = Vec::new();
        for link in 1..=6104 {
            let child = (link - 1) % 2999 + 1;
            for (side, system) in [(0, (child - 1) / 2), (1, child)] {
                let number = link as u32 * 2 + side;
                let point =
                    position(system).offset_by(DVec3::X * (1e6 * f64::from(number % 3 + 1)));
                gates.push(NavigationGate {
                    entity: id(number),
                    system: id(system as u32),
                    pose: Pose {
                        position: point,
                        ..Default::default()
                    },
                    exit: id(number ^ 1),
                    staging: point.offset_by(DVec3::Y * 1e7),
                    slip_ready: false,
                });
            }
        }
        Self {
            gates,
            origin: position(2047),
            target: position(2999),
            slip: false,
            active: AtomicBool::new(true),
            revision: AtomicU64::new(1),
            full_pages: AtomicU64::new(0),
            review_shift_m: 0.,
            review_blocks_gate: false,
        }
    }
}

impl PublicNavigation {
    fn inhabited() -> Self {
        let map = toy_sim_universe::civilization::map();
        let mut gates = Vec::new();
        for (index, link) in map.links.iter().enumerate() {
            for (side, system) in [(0, link.a), (1, link.b)] {
                let number = index as u32 * 2 + side;
                let point = map.systems[system]
                    .position
                    .offset_by(DVec3::X * (1e9 * f64::from(number % 6 + 1)));
                gates.push(NavigationGate {
                    entity: id(number),
                    system: id(system as u32),
                    pose: Pose {
                        position: point,
                        ..Default::default()
                    },
                    exit: id(number ^ 1),
                    staging: point.offset_by(DVec3::Y * 1e7),
                    slip_ready: true,
                });
            }
        }
        let find = |name| {
            map.systems
                .iter()
                .position(|system| system.name == name)
                .unwrap()
        };
        Self {
            gates,
            origin: map.systems[find("Helion system")].position,
            target: map.systems[find("Terminus system")].position,
            slip: true,
            active: AtomicBool::new(true),
            revision: AtomicU64::new(1),
            full_pages: AtomicU64::new(0),
            review_shift_m: 0.,
            review_blocks_gate: false,
        }
    }
}

impl ScanSource for PublicNavigation {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(&self, query: ProgramQuery, _: bool, _: usize) -> anyhow::Result<ProgramReply> {
        Ok(match query {
            ProgramQuery::Travel => ProgramReply::Travel {
                state: TravelState {
                    autopilot_enabled: self.active.load(Ordering::Relaxed),
                    revision: self.revision.load(Ordering::Relaxed),
                    orders: vec![Order::TravelTo(Destination::Galactic(self.target)).into()],
                    status: Status::Planning,
                    ..Default::default()
                },
                pose: Pose {
                    position: self.origin,
                    ..Default::default()
                },
                slip_ready: self.slip,
            },
            ProgramQuery::Resolve {
                destination,
                after_seconds,
            } => {
                let anchored = !matches!(destination, Destination::Galactic(_));
                let (mut pose, offset) = match destination {
                    Destination::Galactic(position) => (
                        Pose {
                            position,
                            ..Default::default()
                        },
                        DVec3::ZERO,
                    ),
                    Destination::Beacon(entity) => (
                        self.gates
                            .iter()
                            .find(|gate| gate.entity == entity)
                            .unwrap()
                            .pose
                            .clone(),
                        DVec3::ZERO,
                    ),
                    Destination::Relative {
                        reference: toy_sim_model::travel::Reference::Beacon(entity),
                        offset,
                        ..
                    } => (
                        self.gates
                            .iter()
                            .find(|gate| gate.entity == entity)
                            .unwrap()
                            .pose
                            .clone(),
                        offset.relative_to(GalacticPosition::ZERO),
                    ),
                    _ => anyhow::bail!("unsupported synthetic destination"),
                };
                pose.position = pose.position.offset_by(
                    offset
                        + DVec3::from_array(pose.velocity) * after_seconds
                        + if anchored {
                            DVec3::Y * self.review_shift_m
                        } else {
                            DVec3::ZERO
                        },
                );
                ProgramReply::Pose(pose)
            }
            ProgramQuery::Navigation { after, limit, .. } => {
                if limit > 1 {
                    self.full_pages.fetch_add(1, Ordering::Relaxed);
                }
                let start = after.map_or(0, |last| {
                    self.gates.partition_point(|gate| gate.entity <= last)
                });
                ProgramReply::Navigation {
                    revision: 7,
                    gates: self
                        .gates
                        .iter()
                        .skip(start)
                        .take(limit as usize)
                        .cloned()
                        .map(|mut gate| {
                            if limit == 1 {
                                let movement = DVec3::Y * self.review_shift_m;
                                gate.pose.position = gate.pose.position.offset_by(movement);
                                gate.staging = gate.staging.offset_by(movement);
                                if self.review_blocks_gate && gate.entity == self.gates[0].entity {
                                    gate.slip_ready = false;
                                }
                            }
                            gate
                        })
                        .collect(),
                }
            }
            ProgramQuery::SlipEligibility { destination, .. } => ProgramReply::SlipEligibility {
                ready: self.slip
                    && !(self.review_blocks_gate
                        && destination
                            == self.gates[0]
                                .staging
                                .offset_by(DVec3::Y * self.review_shift_m)),
                preparation_s: 60.,
                duration_s: if self.slip { 30. } else { f64::INFINITY },
            },
            other => anyhow::bail!("unexpected planner query {other:?}"),
        })
    }
}

fn planned_route(source: PublicNavigation) -> Vec<toy_sim_model::travel::QueuedOrder> {
    run_routes(source, 0, 1).pop().unwrap()
}

fn run_routes(
    source: PublicNavigation,
    warmup: u64,
    repeats: usize,
) -> Vec<Vec<toy_sim_model::travel::QueuedOrder>> {
    let catalogue = Catalogue::builtin();
    let design = expedition_patrol().compile(&catalogue).unwrap();
    let state = ShipState::new(&design, &catalogue);
    let (mass_kg, inertia) = state.mass_properties(&design, &catalogue);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate(EXAMPLE_CONTROLLER).unwrap();
    computer.configure_hardware(&design, &catalogue);
    for _ in 0..60 {
        if !computer.is_booting() {
            break;
        }
        computer
            .run_slice(Input::default(), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(!computer.is_booting());
    let source = Arc::new(source);
    let mut results = Vec::new();
    let mut started_at = warmup;
    let mut previous_pages = None;
    let mut blocks = 0;
    let mut peak_memory = 0;
    let mut peak_gas = 0;
    for tick in 0..4096 {
        source.active.store(tick >= warmup, Ordering::Relaxed);
        let input = Input {
            tick,
            observation: Observation {
                time_s: tick as f64 * 0.1,
                flight: abi::FlightState {
                    rotation: [0., 0., 0., 1.],
                    mass_kg,
                    inertia: inertia.to_cols_array(),
                    radius_m: design.radius,
                    ..Default::default()
                },
                inventory: state.inventory.quantities.clone(),
                resources: abi::ShipResources {
                    energy_j: design.battery_j,
                    hull_hp: design.hull,
                    hull_max_hp: design.hull,
                    ..Default::default()
                },
            },
            devices: state.snapshot(&design),
            ..Default::default()
        };
        let output = computer.run_slice(input, Some(source.clone()), FUEL_PER_TICK, FUEL_PER_TICK);
        assert!(
            output.is_ok(),
            "tick {tick}: {output:?}; {:?}",
            computer.fault
        );
        assert!(
            computer.fault.is_none(),
            "tick {tick}: {:?}",
            computer.fault
        );
        peak_memory = peak_memory.max(computer.memory_bytes());
        peak_gas = peak_gas.max(computer.last_gas_used);
        {
            let output = output.unwrap().output;
            for action in output.world_actions {
                match action {
                    ProgramAction::Route {
                        orders,
                        search_limited,
                        ..
                    } => {
                        if source.review_blocks_gate {
                            assert!(
                                search_limited,
                                "hard alternative search should report its bounded result"
                            );
                            assert!(
                                tick < 650,
                                "changed eligibility took too long to produce a safe alternative"
                            );
                        }
                        assert!(orders.len() <= 256);
                        assert!(peak_memory <= MEMORY_LIMIT);
                        assert!(peak_gas <= FUEL_PER_TICK, "peak gas {peak_gas}");
                        eprintln!(
                            "planned {} orders in {} ticks (warmup {warmup}); peak {peak_gas} gas; {peak_memory} bytes",
                            orders.len(),
                            tick - started_at,
                        );
                        let pages = source.full_pages.load(Ordering::Relaxed);
                        if let Some(previous) = previous_pages {
                            assert_eq!(pages, previous, "warm route reloaded topology");
                        }
                        previous_pages = Some(pages);
                        results.push(orders);
                        if results.len() == repeats {
                            if source.review_blocks_gate {
                                assert_eq!(
                                    blocks, 1,
                                    "changed eligibility did not produce one bounded retry"
                                );
                            }
                            return results;
                        }
                        source.revision.fetch_add(1, Ordering::Relaxed);
                        started_at = tick + 1;
                    }
                    ProgramAction::PlanningProgress { progress, .. } => {
                        if tick % 25 == 0 {
                            eprintln!("tick {tick}: {progress:?}; gas {}", computer.last_gas_used);
                        }
                    }
                    ProgramAction::Block { reason, .. } => {
                        assert!(
                            source.review_blocks_gate,
                            "planner blocked at {tick}: {reason}"
                        );
                        blocks += 1;
                        assert!(
                            blocks <= 1,
                            "planner repeated a stale inadmissible route: {reason}"
                        );
                    }
                    other => panic!("unexpected planning action: {other:?}"),
                }
            }
        }
    }
    panic!("large route failed to finish; peak gas {peak_gas}, memory {peak_memory}");
}

#[test]
fn stock_wasm_routes_three_thousand_systems_without_gas_faults() {
    let orders = planned_route(PublicNavigation::tree());
    let jumps = orders
        .iter()
        .filter(|stage| matches!(stage.action, Order::Jump(_)))
        .count();
    assert!(jumps > 10 && jumps < 30);
    assert!(
        orders
            .iter()
            .all(|stage| !matches!(stage.action, Order::Slip { .. }))
    );
}

fn mixed_navigation() -> PublicNavigation {
    let ly = 9.4607304725808e15;
    let mut source = PublicNavigation::tree();
    for gate in &mut source.gates {
        gate.pose.position = gate.pose.position.offset_by(DVec3::Y * (100. * ly));
        gate.staging = gate.pose.position.offset_by(DVec3::Y * 1e4);
        gate.slip_ready = true;
    }
    source.origin = GalacticPosition::ZERO;
    source.target = GalacticPosition::from_meters(DVec3::X * (250. * ly));
    source.slip = true;
    source.gates[0].pose.position = GalacticPosition::from_meters(DVec3::X * (0.1 * ly));
    source.gates[1].pose.position = GalacticPosition::from_meters(DVec3::X * (249.9 * ly));
    for gate in &mut source.gates[..2] {
        gate.staging = gate.pose.position.offset_by(DVec3::Y * 1e4);
    }
    source
}

#[test]
fn stock_wasm_keeps_a_mixed_gate_and_slip_route_in_a_large_catalogue() {
    let source = mixed_navigation();
    let orders = planned_route(source);
    assert!(
        orders
            .iter()
            .any(|stage| matches!(stage.action, Order::Jump(_)))
    );
    assert!(
        orders
            .iter()
            .any(|stage| matches!(stage.action, Order::Slip { .. }))
    );
}

#[test]
fn inhabited_map_routes_cold_and_from_idle_prefetch_without_reloading() {
    let cold = run_routes(PublicNavigation::inhabited(), 0, 1);
    let warm = run_routes(PublicNavigation::inhabited(), 350, 2);
    assert_eq!(cold[0], warm[0]);
    assert_eq!(warm[0], warm[1]);
}

#[test]
fn selected_gate_staging_is_refreshed_before_route_publication() {
    let mut source = mixed_navigation();
    source.review_shift_m = 20_000.;
    let target = source.target;
    let live_staging: Vec<_> = source
        .gates
        .iter()
        .map(|gate| Destination::Relative {
            reference: toy_sim_model::travel::Reference::Beacon(gate.entity),
            offset: GalacticPosition::from_meters(gate.staging.relative_to(gate.pose.position)),
            axes: toy_sim_model::travel::Axes::Galactic,
        })
        .collect();
    let orders = planned_route(source);
    let arrivals: Vec<_> = orders
        .iter()
        .filter_map(|stage| match &stage.action {
            Order::Slip { destination } if *destination != Destination::Galactic(target) => {
                Some(destination)
            }
            _ => None,
        })
        .collect();
    assert!(!arrivals.is_empty());
    assert!(
        arrivals
            .iter()
            .all(|position| live_staging.contains(position))
    );
}

#[test]
fn changed_slip_eligibility_replans_once_using_refreshed_public_facts() {
    let mut source = mixed_navigation();
    source.review_blocks_gate = true;
    let target = source.target;
    let orders = planned_route(source);
    assert!(
        orders
            .iter()
            .all(|stage| !matches!(stage.action, Order::Jump(_)))
    );
    assert!(
        orders.iter().any(
            |stage| matches!(&stage.action, Order::Slip { destination } if *destination == Destination::Galactic(target))
        )
    );
}

struct MovingSlip {
    tick: AtomicU64,
    destination: Destination,
}

impl ScanSource for MovingSlip {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(&self, query: ProgramQuery, _: bool, _: usize) -> anyhow::Result<ProgramReply> {
        let seconds = self.tick.load(Ordering::Relaxed) as f64 * 0.1;
        Ok(match query {
            ProgramQuery::Travel => ProgramReply::Travel {
                state: TravelState {
                    autopilot_enabled: true,
                    revision: 1,
                    orders: vec![
                        Order::Slip {
                            destination: self.destination.clone(),
                        }
                        .into(),
                    ],
                    status: Status::Active,
                    ..Default::default()
                },
                pose: Pose::default(),
                slip_ready: true,
            },
            ProgramQuery::Resolve {
                destination,
                after_seconds,
            } => {
                assert_eq!(destination, self.destination);
                ProgramReply::Pose(Pose {
                    position: GalacticPosition::from_meters(DVec3::new(
                        1e16,
                        30_000. * (seconds + after_seconds),
                        0.,
                    )),
                    velocity: [0., 30_000., 0.],
                    ..Default::default()
                })
            }
            ProgramQuery::SlipEligibility { .. } => ProgramReply::SlipEligibility {
                ready: true,
                preparation_s: (4. - seconds).max(0.),
                duration_s: 80.,
            },
            other => anyhow::bail!("unexpected moving-target query {other:?}"),
        })
    }
}

#[test]
fn stock_wasm_refreshes_anchored_slip_lead_during_charging_within_gas_budget() {
    let catalogue = Catalogue::builtin();
    let design = expedition_patrol().compile(&catalogue).unwrap();
    let state = ShipState::new(&design, &catalogue);
    let (mass_kg, inertia) = state.mass_properties(&design, &catalogue);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate(EXAMPLE_CONTROLLER).unwrap();
    computer.configure_hardware(&design, &catalogue);
    for _ in 0..60 {
        if !computer.is_booting() {
            break;
        }
        computer
            .run_slice(Input::default(), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(!computer.is_booting());
    let source = Arc::new(MovingSlip {
        tick: AtomicU64::new(0),
        destination: Destination::Relative {
            reference: toy_sim_model::travel::Reference::Beacon(id(42)),
            offset: GalacticPosition::from_meters(DVec3::Z * 1e7),
            axes: toy_sim_model::travel::Axes::Galactic,
        },
    });
    let mut updates = 0;
    let mut peak_gas = 0;
    for tick in 0..100 {
        source.tick.store(tick, Ordering::Relaxed);
        let output = computer
            .run_slice(
                Input {
                    tick,
                    observation: Observation {
                        time_s: tick as f64 * 0.1,
                        flight: abi::FlightState {
                            rotation: [0., 0., 0., 1.],
                            mass_kg,
                            inertia: inertia.to_cols_array(),
                            radius_m: design.radius,
                            ..Default::default()
                        },
                        inventory: state.inventory.quantities.clone(),
                        resources: abi::ShipResources {
                            energy_j: design.battery_j,
                            hull_hp: design.hull,
                            hull_max_hp: design.hull,
                            ..Default::default()
                        },
                    },
                    devices: state.snapshot(&design),
                    ..Default::default()
                },
                Some(source.clone()),
                FUEL_PER_TICK,
                FUEL_PER_TICK,
            )
            .unwrap()
            .output;
        assert!(
            computer.fault.is_none(),
            "tick {tick}: {:?}",
            computer.fault
        );
        peak_gas = peak_gas.max(computer.last_gas_used);
        let destination = output.world_actions.iter().find_map(|action| match action {
            ProgramAction::Slip(destination) => Some(*destination),
            _ => None,
        });
        let Some(destination) = destination else {
            assert_eq!(
                updates, 0,
                "charging refresh stopped at tick {tick}: {:?}",
                output.world_actions
            );
            assert!(tick < 20, "hardware discovery did not complete");
            continue;
        };
        let now = tick as f64 * 0.1;
        let arrival = now + (4. - now).max(0.) + 80.;
        let expected = GalacticPosition::from_meters(DVec3::new(1e16, 30_000. * arrival, 0.));
        assert!(destination.relative_to(expected).length() < 0.001);
        updates += 1;
    }
    assert!(updates >= 80, "only {updates} charging updates");
    assert!(peak_gas <= FUEL_PER_TICK, "peak gas {peak_gas}");
    eprintln!("moving-slip refresh peak gas {peak_gas}");
}
