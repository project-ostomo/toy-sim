use super::*;
use crate::sim::hardware::{DeviceSettings, HardwareClock, ShipInventory};
use crate::sim::physics::{AccumulatedForce, AccumulatedTorque};

fn test_controller(interval: Option<f64>) -> Vec<u8> {
    let interval = interval.unwrap_or(0.);
    wat::parse_str(format!(
        r#"(module
      (import "ship_v15" "tick_read" (func $header (param i32 i32) (result i32)))
      (import "ship_v15" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
      (import "ship_v15" "tick_set_interval" (func $interval (param f64) (result i32)))
      (import "ship_v15" "request_info" (func $request (param i32 i32 i32) (result i32)))
      (import "ship_v15" "request_reply" (func $reply (param i64 i64 i32 i32) (result i32)))
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
    app.world_mut().run_schedule(FixedUpdate);
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
    app.insert_resource(ShipCatalogue(cat))
        .insert_resource(WasmRuntime(ControllerRuntime::new().unwrap()))
        .insert_resource(Time::<Fixed>::from_hz(10.));
    let mut entities = vec![];
    for _ in 0..count {
        entities.push(
            spawn_ship(
                app.world_mut(),
                design.clone(),
                PreciseTransform::default(),
                DVec3::ZERO,
                "Regression vessel".into(),
            )
            .unwrap(),
        );
    }
    crate::sim::hardware::install(&mut app);
    app.add_systems(
        FixedUpdate,
        (prepare_resets.before(HardwareSystems::Initialize), run),
    );
    (app, entities)
}

fn boot(app: &mut App, entity: Entity) {
    for _ in 0..60 {
        step(app);
        if app
            .world()
            .get::<ShipSoftware>(entity)
            .unwrap()
            .last_input
            .is_some()
        {
            return;
        }
    }
    panic!("computer did not boot");
}
fn throttle(world: &World, entity: Entity) -> f64 {
    match world.get::<DeviceSettings>(entity).unwrap().0[5] {
        Some(DeviceSetting::Throttle(value)) => value,
        _ => 0.0,
    }
}

#[test]
fn manual_tumbling_ship_keeps_requested_thrust_without_automatic_attitude_hold() {
    let (mut app, entities) = fleet_with_program(2, EXAMPLE_CONTROLLER.to_vec());
    let target = entities[1];
    let spin = crate::sim::scenario::TRAFFIC_TUMBLE_BODY;
    app.world_mut()
        .get_mut::<AngularVelocity>(target)
        .unwrap()
        .0 = spin;
    app.world_mut()
        .get_mut::<ShipSoftware>(target)
        .unwrap()
        .command(Command::Manual {
            throttle: crate::sim::scenario::TRAFFIC_CHALLENGE_THROTTLE,
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
        assert!((throttle(world, target) - 0.1).abs() < 1e-8);
        assert_eq!(throttle(world, entities[0]), 0.);
        let software = world.get::<ShipSoftware>(target).unwrap();
        assert!(software.controller.fault.is_none());
        assert!(software.inbox.is_empty());
        assert_eq!(
            software.controller.state.attitude.as_ref().unwrap().present,
            0
        );
        let force = world.get::<AccumulatedForce>(target).unwrap().0;
        let design = &world.get::<ShipDesign>(target).unwrap().0;
        let max_thrust: f64 = design
            .device_catalogue
            .iter()
            .filter_map(|device| match device.kind {
                DeviceKind::Engine { thrust_n, .. } => Some(thrust_n),
                _ => None,
            })
            .sum();
        assert!(
            (force.length() - max_thrust * crate::sim::scenario::TRAFFIC_CHALLENGE_THROTTLE).abs()
                < 1.
        );
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
    assert_eq!(throttle(app.world(), entity), 0.4);
    let fuel_before = app
        .world()
        .get::<ShipInventory>(entity)
        .unwrap()
        .0
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
    assert_eq!(throttle(app.world(), entity), 0.);
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
    assert_eq!(throttle(app.world(), entity), 0.4);
    assert!(app.world().get::<HardwareClock>(entity).unwrap().0 >= 100);
    for (before, after) in fuel_before.iter().zip(
        &app.world()
            .get::<ShipInventory>(entity)
            .unwrap()
            .0
            .quantities,
    ) {
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
        assert_eq!(throttle(app.world(), entity), if i == 0 { 0. } else { 0.4 });
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
        assert_eq!(throttle(app.world(), entity), 0.4);
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
    step(&mut app);
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

#[test]
#[ignore = "manual wall-clock benchmark"]
fn profile_default_fleet() {
    let (mut app, entities) = fleet_with_program(501, EXAMPLE_CONTROLLER.to_vec());
    for _ in 0..100 {
        step(&mut app);
    }
    let mut samples = Vec::new();
    for _ in 0..200 {
        let start = std::time::Instant::now();
        step(&mut app);
        samples.push(start.elapsed().as_secs_f64());
        for &entity in &entities {
            let software = app.world().get::<ShipSoftware>(entity).unwrap();
            assert!(
                software.controller.fault.is_none(),
                "{:?}",
                software.controller.fault
            );
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{} ships; 200 ticks; mean {:.3} ms, p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
        entities.len(),
        samples.iter().sum::<f64>() * 1000.0 / samples.len() as f64,
        samples[samples.len() / 2] * 1000.0,
        samples[samples.len() * 95 / 100] * 1000.0,
        samples.last().unwrap() * 1000.0,
    );
}
