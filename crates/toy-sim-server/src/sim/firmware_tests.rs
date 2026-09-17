use super::{hardware, vessel};
use bevy::{
    ecs::system::RunSystemOnce,
    math as glam,
    prelude::{App, Entity, Fixed, FixedUpdate, Time},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::*;
use toy_sim_ships::{DeviceDescriptor, DeviceHandle, DeviceKind, DeviceReading, DeviceStatus};

struct HardwareFixture {
    app: App,
    ship: Entity,
}

impl HardwareFixture {
    fn new(
        design: &toy_sim_ships::CompiledShipDesign,
        catalogue: &toy_sim_ships::Catalogue,
    ) -> Self {
        let mut app = App::new();
        app.insert_resource(vessel::ShipCatalogue(catalogue.clone()));
        app.init_resource::<vessel::WasmRuntime>();
        app.insert_resource(Time::<Fixed>::from_hz(10.));
        hardware::install(&mut app);
        let ship = vessel::spawn_ship(
            app.world_mut(),
            Arc::new(design.clone()),
            super::precision::PreciseTransform::default(),
            glam::DVec3::ZERO,
            "Firmware test".into(),
        )
        .unwrap();
        app.world_mut()
            .run_system_once(hardware::initialize)
            .unwrap();
        Self { app, ship }
    }

    fn advance(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_millis(100));
        self.app.world_mut().run_schedule(FixedUpdate);
    }

    fn snapshot(&self) -> toy_sim_ships::ShipState {
        hardware::snapshot(self.app.world(), self.ship).unwrap()
    }

    fn command(&mut self, commands: &[toy_sim_ships::DeviceCommand]) {
        let design = self
            .app
            .world()
            .get::<vessel::ShipDesign>(self.ship)
            .unwrap()
            .0
            .clone();
        self.app
            .world_mut()
            .query::<hardware::HardwareWrite>()
            .get_mut(self.app.world_mut(), self.ship)
            .unwrap()
            .apply_commands(&design, commands)
            .unwrap();
    }
}

fn ready(runtime: &mut ControllerRuntime, bytes: &[u8]) -> Controller {
    let mut controller = runtime.instantiate(bytes).unwrap();
    controller.catalogue = vec![DeviceDescriptor {
        handle: DeviceHandle(0),
        part_id: 1,
        control_enabled: true,
        alias: "main_engine".into(),
        groups: vec![],
        kind: DeviceKind::Engine {
            thrust_n: 20_000.,
            propellant_kg_s: 1.,
            power_w: 200_000.,
        },
        position_m: [0.; 3],
        rotation: [0., 0., 0., 1.],
    }]
    .into();
    controller.advance(5.);
    assert!(runtime.boot(&mut controller).unwrap());
    controller.advance(0.4);
    controller
}

fn input(time: f64) -> Input {
    Input {
        dt: 0.1,
        observation: Observation {
            time_s: time,
            flight: abi::FlightState {
                rotation: [0., 0., 0., 1.],
                ..Default::default()
            },
            ..Default::default()
        },
        devices: vec![DeviceStatus {
            operational: true,
            powered: true,
            reading: DeviceReading::Engine { thrust_n: 0. },
        }],
        requested_screens: vec![0],
        ..Default::default()
    }
}

struct CountedSource(Arc<AtomicUsize>);

impl ScanSource for CountedSource {
    fn scan(&self, _: f64, maximum: usize) -> Vec<SensorContact> {
        self.0.fetch_add(1, Ordering::Relaxed);
        assert_eq!(maximum, 32);
        vec![SensorContact {
            measured: abi::Contact {
                id: 7,
                position_m: [100., 0., 0.],
                ..Default::default()
            },
            name: "Observed".into(),
        }]
    }
}

#[test]
fn real_custom_firmware_registers_draws_and_acknowledges_input() {
    use toy_sim_ships::{Catalogue, starter};

    let bytes = include_bytes!("../../../toy-sim-ship-wasm/tests/fixtures/custom-screen.wasm");
    let catalogue = Catalogue::builtin();
    let design = starter(bytes.to_vec()).compile(&catalogue).unwrap();
    let mut fixture = HardwareFixture::new(&design, &catalogue);
    fixture.advance();
    let hardware = fixture.snapshot();
    let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate_display(bytes).unwrap();
    computer.configure_hardware(&design, &catalogue);

    let mut observation = input(0.);
    observation.devices = hardware.snapshot(&design);
    observation.observation.flight.mass_kg = mass;
    observation.observation.flight.inertia = inertia.to_cols_array();
    observation.observation.inventory = hardware.inventory.quantities.clone();
    let out = computer.run(observation.clone()).unwrap().unwrap();
    assert_eq!(computer.screens.len(), 1);
    assert_eq!(out.screens.len(), 1);
    assert!(computer.state.attitude.is_none());
    assert!(computer.state.navigation.is_none());

    observation.tick = 1;
    observation.observation.time_s = 0.1;
    observation.screen_events.push(abi::ScreenEvent {
        id: 42,
        screen: 0,
        kind: abi::EVENT_POINTER_PRESS,
        x: 40.,
        y: 120.,
        ..Default::default()
    });
    computer.advance(0.2);
    let out = computer.run(observation.clone()).unwrap().unwrap();
    assert!(
        out.screens[0].draws.iter().any(|draw| {
            matches!(draw, screens::Draw::Text { text, .. } if text == "PRESSES 1")
        })
    );
    observation.screen_events.clear();
    computer.advance(0.2);
    assert!(
        computer
            .run(observation)
            .unwrap()
            .unwrap()
            .screens
            .is_empty()
    );
}

#[test]
fn bundled_firmware_finishes_full_forecasts_with_retained_sources_under_fuel_limit() {
    use toy_sim_ships::{Catalogue, starter};

    struct Target;
    impl ScanSource for Target {
        fn scan(&self, _: f64, maximum: usize) -> Vec<SensorContact> {
            assert_eq!(maximum, 32);
            vec![SensorContact {
                measured: abi::Contact {
                    id: 7,
                    kind: abi::CONTACT_SHIP,
                    radius_m: 10.,
                    position_m: [0., 0., -50_000.],
                    ..Default::default()
                },
                name: "Target".into(),
            }]
        }
    }

    let bytes = toy_sim_ships::EXAMPLE_CONTROLLER;
    let catalogue = Catalogue::builtin();
    let design = starter(bytes.to_vec()).compile(&catalogue).unwrap();
    let mut fixture = HardwareFixture::new(&design, &catalogue);
    fixture.advance();
    let mut hardware = fixture.snapshot();
    let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = ready(&mut runtime, bytes);
    computer.configure_hardware(&design, &catalogue);
    computer.instrument_interest = abi::INTEREST_MARKERS | abi::INTEREST_PATHS;
    let galactic_origin = [1_i128 << 100; 3];
    let mut found = false;

    for tick in 0..30 {
        let now = tick as f64 * 0.1;
        computer.observer_origin = spatial::offset(galactic_origin, [1e6 * now, 0., 0.]).unwrap();
        let mut observation = input(now);
        observation.tick = tick;
        observation.observation.flight.mass_kg = mass;
        observation.observation.flight.inertia = inertia.to_cols_array();
        observation.observation.flight.velocity = [1e6, 0., 0.];
        observation.observation.flight.radius_m = design.radius;
        observation.observation.inventory = hardware.inventory.quantities.clone();
        observation.devices = hardware.snapshot(&design);

        for state in &mut observation.devices {
            if let DeviceReading::Accelerometer { sample } = &mut state.reading {
                *sample = Some(toy_sim_ships::AccelerometerSample {
                    time_s: now,
                    acceleration_m_s2: [0.; 3],
                });
            }
        }

        if tick == 0 {
            observation.commands = vec![
                Request {
                    id: 1,
                    command: Command::SelectTarget(7),
                },
                Request {
                    id: 2,
                    command: Command::EngageNavigation {
                        throttle_limit: 1.,
                        stand_off_m: 100.,
                    },
                },
            ];
        }

        computer.advance(0.1);
        let out = computer
            .run_with_scan(observation, Some(Arc::new(Target)))
            .unwrap()
            .unwrap();
        assert!(
            out.replies
                .iter()
                .all(|reply| reply.result == abi::REPLY_ACCEPTED),
            "{:?}",
            out.replies
        );
        assert!(!computer.is_booting());
        fixture.command(&out.devices);
        fixture.advance();
        hardware = fixture.snapshot();
        assert!(computer.memory_bytes() <= MEMORY_LIMIT);

        if let Some(path) = computer.state.spatial.paths.get(&1) {
            let source = path.snapshot.unwrap();
            assert_eq!(
                source.origin,
                spatial::offset(galactic_origin, [1e6 * source.epoch, 0., 0.]).unwrap()
            );
            assert_eq!(path.vertices[0].time_s, source.epoch);
            assert_eq!(path.vertices[0].position_m, [0.; 3]);
            assert!(path.vertices.last().unwrap().time_s - source.epoch > 30.);
            assert!(path.vertices.len() <= 128);
            assert_eq!(computer.state.spatial.paths[&2].header.subject_contact, 7);
            found = true;
            break;
        }
    }

    assert!(
        found,
        "no complete forecast within three seconds: nav={:?}, markers={:?}",
        computer.state.navigation, computer.state.spatial.markers
    );
}

#[test]
fn armed_firmware_engagement_does_not_replace_manual_flight_and_stays_within_budget() {
    use toy_sim_ships::{Catalogue, DeviceSetting, armed_starter};

    struct Target;
    impl ScanSource for Target {
        fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
            vec![SensorContact {
                measured: abi::Contact {
                    id: 7,
                    kind: abi::CONTACT_SHIP,
                    radius_m: 10.0,
                    position_m: [0.0, 0.0, -1000.0],
                    ..Default::default()
                },
                name: "Target".into(),
            }]
        }
    }

    let catalogue = Catalogue::builtin();
    let design = armed_starter().compile(&catalogue).unwrap();
    let mut fixture = HardwareFixture::new(&design, &catalogue);
    fixture.advance();
    let mut hardware = fixture.snapshot();
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = ready(&mut runtime, toy_sim_ships::EXAMPLE_CONTROLLER);
    computer.configure_hardware(&design, &catalogue);
    computer.instrument_interest = abi::INTEREST_MARKERS;
    hardware.shield_activation(&design, true);
    fixture
        .app
        .world_mut()
        .get_mut::<hardware::ShipThermal>(fixture.ship)
        .unwrap()
        .0 = hardware.thermal;
    let mut fired = false;

    for tick in 0..60 {
        let now = tick as f64 * 0.1;
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        let mut observation = input(now);
        observation.tick = tick;
        observation.observation.flight.mass_kg = mass;
        observation.observation.flight.inertia = inertia.to_cols_array();
        observation.observation.flight.radius_m = design.radius;
        observation.observation.inventory = hardware.inventory.quantities.clone();
        observation.devices = hardware.snapshot(&design);
        if tick == 0 {
            observation.commands = vec![
                Request {
                    id: 1,
                    command: Command::Manual {
                        throttle: 0.1,
                        steering: [0.0; 3],
                    },
                },
                Request {
                    id: 2,
                    command: Command::EngageWeapons {
                        contact: 7,
                        maximum_flight_time_s: 2.0,
                    },
                },
            ];
        } else if tick == 40 {
            observation.commands.push(Request {
                id: 3,
                command: Command::HoldFire,
            });
        }
        computer.advance(0.1);
        let output = computer
            .run_with_scan(observation, Some(Arc::new(Target)))
            .unwrap()
            .expect("armed callback ran out of reserve");
        assert!(!computer.is_booting());
        assert!(
            output
                .replies
                .iter()
                .all(|r| r.result == abi::REPLY_ACCEPTED),
            "{:?}",
            output.replies
        );
        assert!(computer.memory_bytes() <= MEMORY_LIMIT);
        let Some(weapons) = computer.state.weapons else {
            assert!(tick < 2, "device discovery did not complete");
            continue;
        };
        assert_eq!(
            weapons.mode,
            if tick < 40 {
                abi::WEAPONS_ENGAGE
            } else {
                abi::WEAPONS_HOLD
            }
        );
        assert!(
            output
                .devices
                .iter()
                .any(|command| matches!(command.setting,
            DeviceSetting::Throttle(value) if (value - 0.1).abs() < 0.01))
        );

        let trigger = output.devices.iter().any(|command| {
            matches!(command.setting,
            DeviceSetting::Weapon(value) if value.trigger == 1)
        });
        assert!(
            !output
                .devices
                .iter()
                .any(|command| matches!(command.setting, DeviceSetting::ShieldEnabled(_)))
        );
        if trigger {
            assert!(tick < 40);
            fired = true;
        }
        fixture.command(&output.devices);
        fixture.advance();
        hardware = fixture.snapshot();

        // Substitute only the native integration layer: real host calls and
        // firmware see the resulting measured servos, battery and shield state.
        let parts = fixture
            .app
            .world()
            .get::<hardware::PartDevices>(fixture.ship)
            .unwrap()
            .0
            .clone();
        for (&index, spec) in design.weapon_parts.iter().zip(&design.weapon_specs) {
            let mut weapon = fixture
                .app
                .world_mut()
                .get_mut::<hardware::Weapon>(parts[index])
                .unwrap();
            let state = &mut weapon.0;
            state.advanced_s = now;
            if let Some(command) = &mut state.command {
                command.epoch_s = now;
            }
            state.advance(spec, now + 0.1, |_| glam::DQuat::IDENTITY);
        }
        hardware = fixture.snapshot();
        assert_eq!(hardware.thermal.shield_state, abi::SHIELD_ACTIVE);
    }
    assert!(fired);
}

#[test]
fn armed_starter_discovers_rcs_and_accepts_distant_pursuit_after_boot() {
    use toy_sim_ships::{Catalogue, DeviceSetting, armed_starter};

    struct Player;
    impl ScanSource for Player {
        fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
            vec![SensorContact {
                measured: abi::Contact {
                    id: 7,
                    kind: abi::CONTACT_SHIP,
                    radius_m: 5.0,
                    position_m: [0.0, 0.0, -100_000.0],
                    ..Default::default()
                },
                name: "Player".into(),
            }]
        }
    }

    let catalogue = Catalogue::builtin();
    let design = armed_starter().compile(&catalogue).unwrap();
    let mut fixture = HardwareFixture::new(&design, &catalogue);
    fixture.advance();
    let mut hardware = fixture.snapshot();
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = ready(&mut runtime, toy_sim_ships::EXAMPLE_CONTROLLER);
    computer.configure_hardware(&design, &catalogue);
    let mut accepted = 0;
    let mut thrust = false;

    for tick in 0..30 {
        let now = tick as f64 * 0.1;
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        let mut observation = input(now);
        observation.tick = tick;
        observation.observation.flight.mass_kg = mass;
        observation.observation.flight.inertia = inertia.to_cols_array();
        observation.observation.flight.radius_m = design.radius;
        observation.observation.inventory = hardware.inventory.quantities.clone();
        observation.devices = hardware.snapshot(&design);
        for device in &mut observation.devices {
            if let DeviceReading::Accelerometer { sample } = &mut device.reading {
                *sample = Some(toy_sim_ships::AccelerometerSample {
                    time_s: now,
                    acceleration_m_s2: [0.0; 3],
                });
            }
        }
        if tick == 0 {
            observation.commands = vec![
                Request {
                    id: 1,
                    command: Command::SelectTarget(7),
                },
                Request {
                    id: 2,
                    command: Command::EngageNavigation {
                        throttle_limit: 1.0,
                        stand_off_m: 1000.0,
                    },
                },
            ];
        }

        computer.advance(0.1);
        let output = computer
            .run_with_scan(observation, Some(Arc::new(Player)))
            .unwrap()
            .expect("pursuit callback must fit its gas budget");
        for reply in &output.replies {
            assert_eq!(reply.result, abi::REPLY_ACCEPTED, "{reply:?}");
            accepted += 1;
        }
        if accepted == 2 {
            let navigation = computer.state.navigation.unwrap();
            assert_eq!(navigation.status, abi::NAV_ACTIVE, "{navigation:?}");
            assert_eq!(navigation.target_contact, 7);
            assert_eq!(
                output
                    .devices
                    .iter()
                    .filter(|c| matches!(c.setting, DeviceSetting::RcsThrust(_)))
                    .count(),
                4
            );
        }
        thrust |= output.devices.iter().any(|c| {
            matches!(c.setting,
            DeviceSetting::Throttle(value) if value > 0.1)
        });
        fixture.command(&output.devices);
        fixture.advance();
        hardware = fixture.snapshot();
        assert!(!computer.is_booting());
        assert!(computer.memory_bytes() <= MEMORY_LIMIT);
    }
    assert_eq!(accepted, 2);
    assert!(thrust);
}

#[test]
fn armed_idle_computer_publishes_sensor_instrument_with_rotated_ship() {
    let catalogue = toy_sim_ships::Catalogue::builtin();
    let design = toy_sim_ships::armed_starter().compile(&catalogue).unwrap();
    let mut fixture = HardwareFixture::new(&design, &catalogue);
    let mut hardware = fixture.snapshot();
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime
        .instantiate(toy_sim_ships::EXAMPLE_CONTROLLER)
        .unwrap();
    computer.configure_hardware(&design, &catalogue);
    for tick in 0..150 {
        computer.advance(0.1);
        if computer.is_booting() {
            runtime.boot(&mut computer).unwrap();
        }
        let mut observation = input(tick as f64 * 0.1);
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        observation.observation.flight.mass_kg = mass;
        observation.observation.flight.inertia = inertia.to_cols_array();
        observation.observation.flight.rotation =
            (glam::DQuat::from_rotation_x(0.7) * glam::DQuat::from_rotation_y(1.2)).to_array();
        observation.observation.inventory = hardware.inventory.quantities.clone();
        observation.devices = hardware.snapshot(&design);
        if let Some(out) = computer
            .run_with_scan(
                observation,
                Some(Arc::new(CountedSource(Arc::new(AtomicUsize::new(0))))),
            )
            .unwrap()
        {
            fixture.command(&out.devices);
        }
        fixture.advance();
        hardware = fixture.snapshot();
        if tick > 70 {
            assert!(
                computer.state.contacts.is_some(),
                "tick={tick}, fault={:?}, gas={}, attitude={}, weapons={}, scan={:?}",
                computer.fault,
                computer.gas_remaining(),
                computer.state.attitude.is_some(),
                computer.state.weapons.is_some(),
                computer.scan_time
            );
        }
    }
}

#[test]
fn dense_sensor_results_keep_stock_callbacks_within_continuous_gas_refill() {
    struct Dense;
    impl ScanSource for Dense {
        fn scan(&self, _: f64, maximum: usize) -> Vec<SensorContact> {
            assert!((1..=256).contains(&maximum));
            (0..maximum)
                .map(|index| SensorContact {
                    measured: abi::Contact {
                        id: index as u64 + 7,
                        kind: abi::CONTACT_SHIP,
                        position_m: [index as f64 * 100., 0., -10_000.],
                        radius_m: 5.,
                        ..Default::default()
                    },
                    name: format!("Contact {index}"),
                })
                .collect()
        }
    }

    let catalogue = toy_sim_ships::Catalogue::builtin();
    let design = toy_sim_ships::armed_starter().compile(&catalogue).unwrap();
    let mut fixture = HardwareFixture::new(&design, &catalogue);
    fixture.advance();
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = ready(&mut runtime, toy_sim_ships::EXAMPLE_CONTROLLER);
    computer.configure_hardware(&design, &catalogue);

    for tick in 0..100 {
        let hardware = fixture.snapshot();
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        let mut observation = input(tick as f64 * 0.1);
        observation.tick = tick;
        observation.observation.flight.mass_kg = mass;
        observation.observation.flight.inertia = inertia.to_cols_array();
        observation.observation.inventory = hardware.inventory.quantities.clone();
        observation.devices = hardware.snapshot(&design);
        computer.advance(0.1);
        let output = computer
            .run_with_scan(observation, Some(Arc::new(Dense)))
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "dense callback starved at tick {tick}, gas={}",
                    computer.gas_remaining()
                )
            });
        fixture.command(&output.devices);
        fixture.advance();
        assert!(!computer.is_booting());
        assert!(computer.memory_bytes() <= MEMORY_LIMIT);
    }
}
