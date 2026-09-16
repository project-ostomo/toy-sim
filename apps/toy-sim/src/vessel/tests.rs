use super::*;

fn test_controller(interval: Option<f64>) -> Vec<u8> {
    let interval = interval.unwrap_or(0.);
    wat::parse_str(format!(
        r#"(module
      (import "ship_v11" "tick_read" (func $header (param i32 i32) (result i32)))
      (import "ship_v11" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
      (import "ship_v11" "tick_set_interval" (func $interval (param f64) (result i32)))
      (import "ship_v11" "request_info" (func $request (param i32 i32 i32) (result i32)))
      (import "ship_v11" "request_reply" (func $reply (param i64 i64 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "ship_api_version") (result i32) i32.const {})
      (func (export "ship_tick")
        i32.const 1024 i32.const 96 call $header drop
        ;; A request triggers a transient fault before t=8. The retry acknowledges it.
        i32.const 1024 i64.load offset=56 i64.const 0 i64.gt_u
        if
            i32.const 1024 f64.load offset=16 f64.const 8 f64.lt
            if (loop $forever br $forever) end
            i32.const 0 i32.const 2048 i32.const 24 call $request drop
            i32.const 2048 i64.load i64.const 0 i32.const 0 i32.const 0 call $reply drop
        end
        i32.const 0 i64.const 5 i64.store
        i32.const 16 f64.const 0.4 f64.store
        i64.const 6 i64.const 0 i32.const 16 i32.const 8 call $write drop
        f64.const {interval} call $interval drop))"#,
        abi::VERSION
    ))
    .unwrap()
}
fn step(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time<Fixed>>()
        .advance_by(std::time::Duration::from_millis(100));
    app.update();
}

fn fleet(count: usize) -> (App, Vec<Entity>) {
    fleet_with_interval(count, None)
}
fn fleet_with_interval(count: usize, interval: Option<f64>) -> (App, Vec<Entity>) {
    fleet_with_program(count, test_controller(interval))
}
fn fleet_with_program(count: usize, wasm_bytes: Vec<u8>) -> (App, Vec<Entity>) {
    let mut app = App::new();
    let mut pools = bevy::app::TaskPoolOptions::default();
    if let Ok(n) = std::env::var("SHIP_PROFILE_THREADS") {
        let n = n.parse().unwrap();
        pools.compute.min_threads = n;
        pools.compute.max_threads = n;
    }
    app.add_plugins(bevy::app::TaskPoolPlugin {
        task_pool_options: pools,
    });
    let cat = Catalogue::builtin();
    let design = Arc::new(starter(wasm_bytes.clone()).compile(&cat).unwrap());
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut entities = vec![];
    for _ in 0..count {
        let mut hardware = ShipState::new(&design, &cat);
        hardware.test_loadout(&design, &cat);
        let mut controller = runtime.instantiate(&wasm_bytes).unwrap();
        controller.configure_hardware(&design, &cat);
        let entity = app
            .world_mut()
            .spawn((
                Vessel {
                    class_name: "test".into(),
                    vessel_name: "test".into(),
                },
                ShipDesign(design.clone()),
                ShipHardware(hardware),
                PreciseTransform::default(),
                ShipSoftware {
                    controller,
                    inbox: vec![],
                    mfds: default(),
                    screen_requests: vec![],
                    results: vec![],
                    reset: false,
                    hull_energy_j: 0.,
                    shield_energy_j: 0.,
                    last_seconds: 0.,
                    timings: default(),
                    callback_dt: 0.,
                    manual_input_sent: false,
                    schedule: default(),
                    request_id: 0,
                },
            ))
            .id();
        entities.push(entity);
    }
    app.insert_resource(ShipCatalogue(cat))
        .insert_resource(WasmRuntime(runtime))
        .insert_resource(Time::<Fixed>::from_hz(10.))
        .add_systems(Update, run);
    (app, entities)
}

fn boot(app: &mut App, entity: Entity) {
    for _ in 0..60 {
        step(app);
        if app
            .world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .manual_input_sent
        {
            return;
        }
    }
    panic!("computer did not boot");
}
fn throttle(h: &ShipState) -> f64 {
    match h.settings[5] {
        Some(DeviceSetting::Throttle(v)) => v,
        _ => 0.,
    }
}

#[test]
fn manual_tumbling_ship_keeps_requested_thrust_without_automatic_attitude_hold() {
    let (mut app, entities) = fleet_with_program(2, EXAMPLE_CONTROLLER.to_vec());
    let target = entities[1];
    let spin = crate::scenario::TRAFFIC_TUMBLE_BODY;
    app.world_mut()
        .get_mut::<AngularVelocity>(target)
        .unwrap()
        .0 = spin;
    app.world_mut()
        .get_mut::<ShipSoftware>(target)
        .unwrap()
        .command(Command::Manual {
            throttle: crate::scenario::TRAFFIC_CHALLENGE_THROTTLE,
            steering: [0.; 3],
        });
    boot(&mut app, target);
    let mut directions = vec![];
    for i in 0..100 {
        // Exercise the real WASM and hardware throughout a tumble, including angular
        // rates that would inhibit thrust in the pursuit controller's attitude gate.
        let q = bevy::math::DQuat::from_scaled_axis(spin * (i as f64 * 0.1));
        app.world_mut()
            .get_mut::<PreciseTransform>(target)
            .unwrap()
            .rotation = q;
        app.world_mut()
            .get_mut::<AccumulatedForce>(target)
            .unwrap()
            .0 = DVec3::ZERO;
        app.world_mut()
            .get_mut::<AccumulatedTorque>(target)
            .unwrap()
            .0 = DVec3::ZERO;
        step(&mut app);
        let world = app.world();
        assert!((throttle(&world.get::<ShipHardware>(target).unwrap().0) - 0.1).abs() < 1e-8);
        assert_eq!(
            throttle(&world.get::<ShipHardware>(entities[0]).unwrap().0),
            0.
        );
        let software = world.get::<ShipSoftware>(target).unwrap();
        assert!(software.controller.fault.is_none());
        assert!(software.inbox.is_empty());
        assert_eq!(
            software.controller.state.attitude.as_ref().unwrap().present,
            0
        );
        let force = world.get::<AccumulatedForce>(target).unwrap().0;
        assert!((force.length() - 20_000.).abs() < 1.);
        directions.push(force.normalize());
    }
    assert!(directions.first().unwrap().dot(*directions.last().unwrap()) < 0.5);
}
#[test]
fn startup_waits_then_fault_clears_actuators_and_automatically_recovers() {
    let (mut app, entities) = fleet(1);
    let entity = entities[0];
    for _ in 0..49 {
        step(&mut app);
        assert!(
            app.world()
                .get::<ShipSoftware>(entity)
                .unwrap()
                .controller
                .is_booting()
        );
    }
    boot(&mut app, entity);
    assert_eq!(
        throttle(&app.world().get::<ShipHardware>(entity).unwrap().0),
        0.4
    );
    let fuel_before = app
        .world()
        .get::<ShipHardware>(entity)
        .unwrap()
        .0
        .inventory
        .quantities
        .clone();
    app.world_mut()
        .get_mut::<ShipSoftware>(entity)
        .unwrap()
        .command(Command::HoldAttitude);
    step(&mut app);
    assert!(
        app.world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .controller
            .is_booting()
    );
    assert!(
        app.world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .controller
            .fault
            .is_some()
    );
    assert_eq!(
        throttle(&app.world().get::<ShipHardware>(entity).unwrap().0),
        0.
    );
    for _ in 0..49 {
        step(&mut app);
        assert!(
            app.world()
                .get::<ShipSoftware>(entity)
                .unwrap()
                .controller
                .is_booting()
        );
    }
    for _ in 0..3 {
        step(&mut app);
    }
    assert!(
        !app.world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .controller
            .is_booting()
    );
    assert!(
        app.world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .controller
            .fault
            .is_none()
    );
    assert_eq!(
        throttle(&app.world().get::<ShipHardware>(entity).unwrap().0),
        0.4
    );
    let hardware = &app.world().get::<ShipHardware>(entity).unwrap().0;
    assert!(hardware.tick >= 100);
    for (before, after) in fuel_before.iter().zip(&hardware.inventory.quantities) {
        assert!(after <= before, "reboot restored consumables");
    }
}
#[test]
fn one_fault_does_not_reset_other_computers() {
    let (mut app, entities) = fleet(8);
    boot(&mut app, entities[0]);
    app.world_mut()
        .get_mut::<ShipSoftware>(entities[0])
        .unwrap()
        .command(Command::HoldAttitude);
    step(&mut app);
    for (i, &entity) in entities.iter().enumerate() {
        let sw = app.world().get::<ShipSoftware>(entity).unwrap();
        assert_eq!(sw.controller.is_booting(), i == 0);
        assert_eq!(
            throttle(&app.world().get::<ShipHardware>(entity).unwrap().0),
            if i == 0 { 0. } else { 0.4 }
        );
    }
    assert_eq!(app.world().resource::<WasmRuntime>().0.cached_modules(), 1);
}
#[test]
fn sleeping_computers_keep_hardware_running_and_commands_wake_them() {
    let (mut app, entities) = fleet_with_interval(1, Some(1.));
    let entity = entities[0];
    boot(&mut app, entity);
    for _ in 0..8 {
        step(&mut app);
        assert!(
            !app.world()
                .get::<ShipSoftware>(entity)
                .unwrap()
                .controller
                .is_booting()
        );
        assert_eq!(
            throttle(&app.world().get::<ShipHardware>(entity).unwrap().0),
            0.4
        );
    }
    app.world_mut()
        .get_mut::<ShipSoftware>(entity)
        .unwrap()
        .command(Command::HoldAttitude);
    step(&mut app);
    assert!(
        app.world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .controller
            .is_booting()
    );
}
#[test]
fn simultaneous_startups_are_limited_per_tick() {
    let (mut app, entities) = fleet(toy_sim_ship_wasm::MAX_BOOTS_PER_TICK + 5);
    for &entity in &entities {
        app.world_mut()
            .get_mut::<ShipSoftware>(entity)
            .unwrap()
            .controller
            .advance(5.);
    }
    // Only one bounded startup batch is admitted in this tick.
    step(&mut app);
    let running = entities
        .iter()
        .filter(|&&e| {
            !app.world()
                .get::<ShipSoftware>(e)
                .unwrap()
                .controller
                .is_booting()
        })
        .count();
    assert_eq!(running, toy_sim_ship_wasm::MAX_BOOTS_PER_TICK);
    step(&mut app);
    assert!(entities.iter().all(|&e| {
        !app.world()
            .get::<ShipSoftware>(e)
            .unwrap()
            .controller
            .is_booting()
    }));
}

/// Run with SHIP_PROFILE_MODE=idle|no_instruments|empty_scan|custom_screen and --ignored --nocapture.
/// Frozen historical 501-ship orbital positions isolate ship work from the orbital integrator.
#[test]
#[ignore = "manual wall-clock benchmark"]
fn profile_default_fleet() {
    use crate::spatial::{SpatialIndex, SpatialObject};
    let mode = std::env::var("SHIP_PROFILE_MODE").unwrap_or_else(|_| "idle".into());
    assert!(["idle", "no_instruments", "empty_scan", "custom_screen"].contains(&mode.as_str()));
    let universe = crate::orrery::Universe::init(crate::orrery::example_config()).unwrap();
    let scenario = &crate::scenario::InitialScenario {
        traffic_count: 500,
        traffic_orbit_start: 1,
        ..crate::scenario::INITIAL_SCENARIO.clone()
    };
    let body = universe.get_body(scenario.body).unwrap();
    let states = scenario.fleet_states(body.radius, body.mass).unwrap();
    let (mut app, entities) = fleet_with_program(states.len(), EXAMPLE_CONTROLLER.to_vec());
    if mode == "custom_screen" {
        let computer = app
            .world_mut()
            .resource_mut::<WasmRuntime>()
            .0
            .instantiate(include_bytes!(
                "../../../../crates/toy-sim-ship-wasm/tests/fixtures/custom-screen.wasm"
            ))
            .unwrap();
        app.world_mut()
            .get_mut::<ShipSoftware>(entities[0])
            .unwrap()
            .controller = computer;
    }
    let epoch = crate::physics::sim_time(app.world().resource::<Time<Fixed>>());
    let centre = universe.solve_position(scenario.body, epoch).unwrap();
    let centre_v = universe.solve_velocity(scenario.body, epoch).unwrap();
    let mut index = SpatialIndex::default();
    for (&entity, (p, v)) in entities.iter().zip(states) {
        let mut pose = PreciseTransform {
            translation_um: centre.offset_by(p),
            ..default()
        };
        pose.look_to(v.normalize(), p.normalize());
        let radius = app.world().get::<ShipDesign>(entity).unwrap().0.radius;
        index.insert(SpatialObject {
            entity,
            position: pose.translation_um,
            radius_m: radius,
            occludes: false,
        });
        app.world_mut().entity_mut(entity).insert((
            pose,
            Velocity(centre_v + v),
            crate::sensors::Sensor::default(),
        ));
    }
    for system in &universe.systems {
        for body in system.solver.iter() {
            let position = universe.solve_position(&body.name, epoch).unwrap();
            let radius = body.radius;
            let entity = app
                .world_mut()
                .spawn((
                    Name::new(body.name.to_string()),
                    PreciseTransform {
                        translation_um: position,
                        ..default()
                    },
                    crate::spatial::SpatialBody {
                        radius_m: radius,
                        occludes: true,
                    },
                ))
                .id();
            index.insert(SpatialObject {
                entity,
                position,
                radius_m: radius,
                occludes: true,
            });
        }
    }
    if mode != "empty_scan" {
        app.insert_resource(index);
    }
    if mode != "no_instruments" {
        app.world_mut()
            .entity_mut(entities[0])
            .insert(ControlledVessel);
    }
    println!(
        "Compute threads: {}",
        bevy::tasks::ComputeTaskPool::get().thread_num()
    );
    let mut focused = vec![];
    let mut traffic = vec![];
    let mut stages = [0.; 5];
    let mut traffic_stages = [0.; 5];
    let mut fleet_seconds = 0.;
    let mut callbacks = [0usize; 2];
    for tick in 0..300 {
        let mut first = app
            .world_mut()
            .get_mut::<ShipSoftware>(entities[0])
            .unwrap();
        first.controller.instrument_interest = u64::from(mode != "no_instruments");
        if mode == "custom_screen" {
            first.screen_requests = vec![0];
        }
        drop(first);
        // Supply a fresh, valid IMU sample; positions remain fixed for repeatability.
        let now = app.world().resource::<Time<Fixed>>().elapsed_secs_f64();
        for &entity in &entities {
            app.world_mut()
                .get_mut::<crate::physics::AccelerometerState>(entity)
                .unwrap()
                .time_s = Some(now);
        }
        let start = std::time::Instant::now();
        step(&mut app);
        if tick < 100 {
            continue;
        }
        fleet_seconds += start.elapsed().as_secs_f64();
        for (i, &entity) in entities.iter().enumerate() {
            let sw = app.world().get::<ShipSoftware>(entity).unwrap();
            assert!(sw.controller.fault.is_none(), "{:?}", sw.controller.fault);
            let t = sw.timings;
            callbacks[usize::from(i != 0)] += usize::from(t.callback > 0.);
            let values = [
                t.prepare,
                t.callback - t.scan,
                t.scan,
                t.publish,
                t.hardware,
            ];
            let (samples, sums) = if i == 0 {
                (&mut focused, &mut stages)
            } else {
                (&mut traffic, &mut traffic_stages)
            };
            samples.push(sw.last_seconds * 1e6);
            for (sum, value) in sums.iter_mut().zip(values) {
                *sum += value * 1e6;
            }
        }
    }
    println!(
        "mode={mode}; {} ships, 200 measured ticks; fleet wall {:.3} ms/tick",
        entities.len(),
        fleet_seconds * 1e3 / 200.
    );
    println!(
        "Completed callbacks: first ship {}/200, traffic {}/100000",
        callbacks[0], callbacks[1]
    );
    for (label, mut samples, sums) in [
        ("first ship", focused, stages),
        ("traffic", traffic, traffic_stages),
    ] {
        samples.sort_by(f64::total_cmp);
        let n = samples.len();
        println!(
            "{label}: mean {:.2}, p50 {:.2}, p95 {:.2}, max {:.2} µs; mean stages [prepare, WASM+ABI, native scan, publish, hardware] = {:?}",
            samples.iter().sum::<f64>() / n as f64,
            samples[n / 2],
            samples[n * 95 / 100],
            samples[n - 1],
            sums.map(|v| v / n as f64)
        );
    }
}
