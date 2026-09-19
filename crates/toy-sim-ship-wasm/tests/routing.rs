use glam::DVec3;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use toy_sim_model::{
    GalacticPosition, Id, Pose, ProgramAction, ProgramQuery, ProgramReply,
    travel::{CurrentOrder, Destination, Order, Status},
};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{
    ControllerRuntime, FUEL_PER_TICK, Input, Observation, ScanSource, SensorContact,
};
use toy_sim_ships::{Catalogue, EXAMPLE_CONTROLLER, ShipState, expedition_patrol};

fn id(value: u32) -> Id {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&value.to_be_bytes());
    Id(bytes)
}

struct CurrentCommand {
    state: Mutex<CurrentOrder>,
    reads: AtomicU64,
}

impl ScanSource for CurrentCommand {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(&self, query: ProgramQuery, _: bool, _: usize) -> anyhow::Result<ProgramReply> {
        match query {
            ProgramQuery::Travel => {
                self.reads.fetch_add(1, Ordering::Relaxed);
                Ok(ProgramReply::Travel {
                    state: self.state.lock().unwrap().clone(),
                    pose: Pose::default(),
                    slip_ready: false,
                })
            }
            other => {
                anyhow::bail!("elementary wait command queried unrelated world data: {other:?}")
            }
        }
    }
}

#[test]
fn stock_wasm_waits_for_server_plan_and_executes_only_the_current_command() {
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
    let source = Arc::new(CurrentCommand {
        state: Mutex::new(CurrentOrder {
            autopilot_enabled: true,
            revision: 7,
            index: 3,
            order: Some(Order::WaitUntil(0).into()),
            status: Status::Planning,
            ..Default::default()
        }),
        reads: AtomicU64::new(0),
    });
    let mut completed = false;
    for tick in 0..80 {
        if tick == 40 {
            source.state.lock().unwrap().status = Status::Active;
        }
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
        assert!(computer.last_gas_used <= FUEL_PER_TICK);
        if tick < 40 {
            assert!(
                output.world_actions.is_empty(),
                "planning emitted actions: {:?}",
                output.world_actions
            );
        } else {
            for action in output.world_actions {
                match action {
                    ProgramAction::CompleteOrder { revision, order } => {
                        assert_eq!((revision, order), (7, 3));
                        completed = true;
                    }
                    ProgramAction::Estimate {
                        revision, order, ..
                    } => {
                        assert_eq!((revision, order), (7, 3));
                    }
                    other => panic!("unexpected wait execution action: {other:?}"),
                }
            }
        }
    }
    assert!(source.reads.load(Ordering::Relaxed) >= 60);
    assert!(completed, "current elementary command never completed");
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
            ProgramQuery::LocalSpace { .. } => ProgramReply::LocalSpace(Default::default()),
            ProgramQuery::Travel => ProgramReply::Travel {
                state: CurrentOrder {
                    autopilot_enabled: true,
                    revision: 1,
                    index: 4,
                    order: Some(
                        Order::Slip {
                            destination: self.destination.clone(),
                        }
                        .into(),
                    ),
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
            ProgramAction::Slip {
                revision,
                order,
                destination,
            } => {
                assert_eq!((*revision, *order), (1, 4));
                Some(*destination)
            }
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
