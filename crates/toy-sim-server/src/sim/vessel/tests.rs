use super::*;
use crate::sim::hardware::{DeviceSettings, HardwareClock, ShipInventory};
use crate::sim::physics::{AccumulatedForce, AccumulatedTorque};

fn test_controller(interval: Option<f64>) -> Vec<u8> {
    let interval = interval.unwrap_or(0.);
    wat::parse_str(format!(
        r#"(module
      (import "ship_v30" "tick_read" (func $header (param i32 i32) (result i32)))
      (import "ship_v30" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
      (import "ship_v30" "tick_set_interval" (func $interval (param f64) (result i32)))
      (import "ship_v30" "request_info" (func $request (param i32 i32 i32) (result i32)))
      (import "ship_v30" "request_reply" (func $reply (param i64 i64 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "ship_api_version") (result i32) i32.const {})
      (func (export "ship_tick")
        i32.const 1024 i32.const 96 call $header drop
        ;; A request triggers a transient fault before t=8. The retry acknowledges it.
        i32.const 1024 i64.load offset=56 i64.const 0 i64.gt_u
        if
            i32.const 1024 f64.load offset=16 f64.const 8 f64.lt
            if unreachable end
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
    let account = toy_sim_model::Id([11; 16]);
    crate::sim::identity::initialize(app.world_mut(), &[account]);
    let mut entities = vec![];
    for _ in 0..count {
        let entity = spawn_ship(
            app.world_mut(),
            design.clone(),
            PreciseTransform::default(),
            DVec3::ZERO,
            "Regression vessel".into(),
        )
        .unwrap();
        crate::sim::identity::attach_ship(app.world_mut(), entity, account).unwrap();
        entities.push(entity);
    }
    crate::sim::hardware::install(&mut app);
    app.add_systems(
        FixedUpdate,
        (
            prepare_resets.before(HardwareSystems::Initialize),
            (run, clear_computer_resets).chain(),
        ),
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
fn slip_transit_keeps_computer_callbacks_running() {
    let program = wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (global $ticks (mut i32) (i32.const 0))
            (func (export "ship_api_version") (result i32) i32.const {})
            (func (export "ship_tick")
                global.get $ticks i32.const 1 i32.add global.set $ticks))"#,
        abi::VERSION
    ))
    .unwrap();
    let (mut app, ships) = fleet_with_program(1, program);
    let ship = ships[0];
    boot(&mut app, ship);
    crate::sim::travel::set_dormant(
        app.world_mut(),
        ship,
        toy_sim_model::travel::Presence::SlipTransit(toy_sim_model::Id::new()),
    );
    let clock = app.world().get::<HardwareClock>(ship).unwrap().0;
    for _ in 0..10 {
        step(&mut app);
        let software = app.world().get::<ShipSoftware>(ship).unwrap();
        assert!(software.controller.fault.is_none());
        assert!(software.last_gas_used > 0);
    }
    assert_eq!(
        app.world().get::<HardwareClock>(ship).unwrap().0,
        clock + 10
    );
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
    let request_id = app.world().get::<ShipSoftware>(target).unwrap().request_id;
    boot(&mut app, target);
    for tick in 0..10 {
        let software = app.world().get::<ShipSoftware>(target).unwrap();
        assert!(software.controller.fault.is_none());
        if let Some(reply) = software.results.iter().find(|reply| reply.id == request_id) {
            assert_eq!(reply.result, abi::REPLY_ACCEPTED, "{}", reply.message);
            break;
        }
        assert!(
            tick < 9,
            "manual throttle request was not accepted after hardware discovery"
        );
        step(&mut app);
    }
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
    use crate::sim::{identity::Identity, travel};
    use toy_sim_model::{
        Id,
        travel::{Order, Status, TravelState},
    };
    let id = Id::new();
    app.world_mut()
        .init_resource::<crate::sim::simulation::SimulationCounters>();
    app.world_mut().entity_mut(entity).insert((
        Identity(id),
        crate::sim::identity::Control {
            account: id,
            revision: 1,
        },
        travel::Travel(TravelState {
            autopilot_enabled: true,
            revision: 9,
            orders: vec![Order::WaitUntil(9999)]
                .into_iter()
                .map(Into::into)
                .collect(),
            status: Status::Planning,
            planning: Some(toy_sim_model::travel::PlanningProgress {
                stage: toy_sim_model::travel::PlanningStage::BuildingGraph,
                completed: 40,
                total: Some(100),
            }),
            estimated_arrival_tick: Some(9999),
            ..default()
        }),
        travel::SlipDrive {
            preparation: Some(travel::Preparation {
                destination: Default::default(),
                started: 0,
                mass: 100.,
                work_j: 100.,
                required_j: 200.,
            }),
            ..default()
        },
    ));
    let station = app
        .world_mut()
        .spawn(travel::DockingBays(vec![travel::Bay {
            centre_m: [0.; 3],
            rotation: [0., 0., 0., 1.],
            radius_m: 100.,
            mass_capacity_kg: 1e9,
            public: true,
            allowed: default(),
            reservation: Some((id, 9999)),
        }]))
        .id();
    {
        let mut software = app.world_mut().get_mut::<ShipSoftware>(entity).unwrap();
        software.controller.state.weapons = Some(abi::WeaponsState {
            valid_until_s: 1000.,
            mode: abi::WEAPONS_FIRING,
            target_contact: 42,
            ..default()
        });
        software.controller.state.navigation = Some(abi::NavigationState {
            valid_until_s: 1000.,
            ..default()
        });
        software.command(Command::HoldAttitude);
    }
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
    {
        let world = app.world();
        let software = world.get::<ShipSoftware>(entity).unwrap();
        assert!(software.last_gas_used > 0);
        assert!(software.last_gas_used <= toy_sim_ship_wasm::FUEL_PER_TICK);
        assert_eq!(software.last_gas_limit, toy_sim_ship_wasm::FUEL_PER_TICK);
        assert!(software.controller.state.weapons.is_none());
        assert!(software.controller.state.navigation.is_none());
        assert!(!software.controller.has_pending_input());
        assert!(software.inbox.is_empty() && software.world_actions.is_empty());
        assert_eq!(
            world.get::<travel::Travel>(entity).unwrap().0,
            TravelState {
                revision: 10,
                ..default()
            }
        );
        assert!(
            world
                .get::<travel::SlipDrive>(entity)
                .unwrap()
                .preparation
                .is_none()
        );
        assert!(
            world.get::<travel::DockingBays>(station).unwrap().0[0]
                .reservation
                .is_none()
        );
        let presentation = crate::sim::presentation::ship(world, entity, false).unwrap();
        assert!(matches!(
            presentation.computer,
            toy_sim_model::ComputerStatus::Fault {
                reboot_remaining_s: Some(5.),
                ..
            }
        ));
    }
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
    assert_eq!(
        app.world().get::<travel::Travel>(entity).unwrap().0,
        TravelState {
            revision: 10,
            ..default()
        }
    );
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
        assert_eq!(
            app.world()
                .get::<ShipSoftware>(entity)
                .unwrap()
                .last_gas_used,
            0
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
    for _ in 0..49 {
        step(&mut app);
    }
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
    assert!(running <= toy_sim_ship_wasm::MAX_BOOTS_PER_TICK);
    for _ in 0..3 {
        step(&mut app);
    }
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

#[test]
fn fault_reboot_budget_pauses_without_power() {
    let (mut app, ships) = fleet(1);
    let ship = ships[0];
    boot(&mut app, ship);
    app.world_mut()
        .init_resource::<crate::sim::simulation::SimulationCounters>();
    let id = toy_sim_model::Id::new();
    app.world_mut().entity_mut(ship).insert((
        crate::sim::identity::Identity(id),
        crate::sim::identity::Control {
            account: id,
            revision: 1,
        },
    ));
    app.world_mut()
        .get_mut::<ShipSoftware>(ship)
        .unwrap()
        .controller
        .fail("Test fault".into());
    app.world_mut()
        .get_mut::<crate::sim::hardware::Avionics>(ship)
        .unwrap()
        .0
        .operational = false;
    for _ in 0..20 {
        step(&mut app);
    }
    assert_eq!(
        app.world()
            .get::<ShipSoftware>(ship)
            .unwrap()
            .controller
            .boot_remaining_gas(),
        toy_sim_ship_wasm::BOOT_GAS
    );
    let state = crate::sim::presentation::ship(app.world(), ship, false).unwrap();
    assert!(matches!(
        state.computer,
        toy_sim_model::ComputerStatus::Fault {
            reboot_remaining_s: None,
            ..
        }
    ));
    app.world_mut()
        .get_mut::<crate::sim::hardware::Avionics>(ship)
        .unwrap()
        .0
        .operational = true;
    for _ in 0..5 {
        step(&mut app);
    }
    let state = crate::sim::presentation::ship(app.world(), ship, false).unwrap();
    assert!(
        matches!(state.computer, toy_sim_model::ComputerStatus::Fault { reboot_remaining_s: Some(seconds), .. } if seconds > 0. && seconds < 5.)
    );
}

#[test]
fn zero_global_gas_stalls_paid_boot_and_shared_grants_conserve_the_pool() {
    let (mut app, ships) = fleet(3);
    let owner = crate::sim::gas::payer(app.world(), ships[0]).unwrap();
    let ledger = crate::sim::gas::GasLedger::default();
    ledger.ensure_account(owner, 0);
    app.insert_resource(ledger.clone());
    for _ in 0..10 {
        step(&mut app);
    }
    for &ship in &ships {
        let software = app.world().get::<ShipSoftware>(ship).unwrap();
        assert_eq!(
            software.controller.boot_remaining_gas(),
            toy_sim_ship_wasm::BOOT_GAS
        );
        assert_eq!(software.last_gas_used, 0);
        assert_eq!(throttle(app.world(), ship), 0.);
    }
    ledger.deposit(owner, 1_000_000).unwrap();
    step(&mut app);
    let mut total = 0;
    for &ship in &ships {
        let software = app.world().get::<ShipSoftware>(ship).unwrap();
        assert!((333_333..=333_334).contains(&software.last_gas_used));
        total += software.last_gas_used;
    }
    assert_eq!(total, 1_000_000);
    let account = ledger.account(owner).unwrap();
    assert_eq!(
        (account.available, account.reserved, account.spent),
        (0, 0, total)
    );
    assert!(ledger.snapshot().is_ok());
    step(&mut app);
    assert_eq!(ledger.account(owner).unwrap(), account);
}

#[test]
fn long_callbacks_suspend_without_fault_and_preserve_local_progress() {
    let program = wat::parse_str(format!(
        r#"(module
        (import "ship_v30" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (func (export "ship_api_version") (result i32) i32.const {})
        (func (export "ship_tick") (local $remaining i32)
            i32.const 600000 local.set $remaining
            (loop $work
                local.get $remaining i32.const 1 i32.sub local.tee $remaining
                br_if $work)
            i32.const 16 f64.const 0.4 f64.store
            i64.const 6 i64.const 0 i32.const 16 i32.const 8 call $write drop))"#,
        abi::VERSION
    ))
    .unwrap();
    let (mut app, ships) = fleet_with_program(1, program);
    let ship = ships[0];
    let owner = crate::sim::gas::payer(app.world(), ship).unwrap();
    let mut suspended = false;
    let mut completed = false;
    for _ in 0..100 {
        step(&mut app);
        let software = app.world().get::<ShipSoftware>(ship).unwrap();
        assert!(software.controller.fault.is_none());
        assert!(software.last_gas_used <= toy_sim_ship_wasm::FUEL_PER_TICK);
        suspended |= software.controller.is_suspended();
        if throttle(app.world(), ship) == 0.4 {
            completed = true;
            break;
        }
    }
    assert!(suspended && completed);
    let ledger = app.world().resource::<crate::sim::gas::GasLedger>();
    let account = ledger.account(owner).unwrap();
    assert!(account.spent > toy_sim_ship_wasm::BOOT_GAS);
    assert_eq!(
        account.available + account.spent,
        crate::sim::gas::STARTING_GAS
    );
    assert!(ledger.snapshot().is_ok());
}

#[test]
fn suspended_initializers_do_not_keep_later_computers_out_of_the_startup_queue() {
    let program = wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (func (export "ship_api_version") (result i32) i32.const {})
            (func $initialize (loop $forever br $forever))
            (start $initialize)
            (func (export "ship_tick")))"#,
        abi::VERSION
    ))
    .unwrap();
    let (mut app, pending) = fleet_with_program(toy_sim_ship_wasm::MAX_BOOTS_PER_TICK, program);
    let design = Arc::new(
        starter(test_controller(None))
            .compile(&app.world().resource::<ShipCatalogue>().0)
            .unwrap(),
    );
    let later = spawn_ship(
        app.world_mut(),
        design,
        PreciseTransform::default(),
        DVec3::ZERO,
        "Later healthy computer".into(),
    )
    .unwrap();
    crate::sim::identity::attach_ship(app.world_mut(), later, toy_sim_model::Id([11; 16])).unwrap();
    boot(&mut app, later);
    assert_eq!(throttle(app.world(), later), 0.4);
    for ship in pending {
        let software = app.world().get::<ShipSoftware>(ship).unwrap();
        assert!(software.controller.is_booting());
        assert!(!software.controller.needs_instance_start());
        assert!(software.controller.fault.is_none());
    }
    assert!(
        app.world()
            .resource::<crate::sim::gas::GasLedger>()
            .snapshot()
            .is_ok()
    );
}

#[test]
fn shared_missile_callbacks_resume_and_rotate_within_the_parent_account_budget() {
    let program = wat::parse_str(format!(
        r#"(module
            (import "ship_v30" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
            (import "ship_v30" "missile_control" (func $control (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $ship_calls (mut i32) (i32.const 0))
            (func (export "ship_api_version") (result i32) i32.const {})
            (func $work (local $count i32)
                i32.const 40000 local.set $count
                (loop $again
                    local.get $count i32.const 1 i32.sub local.tee $count br_if $again))
            (func (export "ship_tick")
                call $work
                global.get $ship_calls i32.const 1 i32.add global.set $ship_calls
                i32.const 64 global.get $ship_calls f64.convert_i32_u f64.const 0.001 f64.mul f64.store
                i64.const 6 i64.const 0 i32.const 64 i32.const 8 call $write drop)
            (func (export "missile_tick") (param i64)
                call $work
                i32.const 16 f64.const -1 f64.store
                i32.const 24 f64.const 0.2 f64.store
                i32.const 0 i32.const 32 call $control drop))"#,
        abi::VERSION
    )).unwrap();
    let (mut app, ships) = fleet_with_program(1, program);
    let parent = ships[0];
    boot(&mut app, parent);
    let initial_throttle = throttle(app.world(), parent);
    app.world_mut()
        .entity_mut(parent)
        .insert(super::super::missiles::Launchers::default());
    app.insert_resource(super::super::missiles::Callbacks(BTreeMap::from([(
        parent,
        (1..=3)
            .map(|handle| {
                (
                    handle,
                    abi::MissileObservation {
                        handle,
                        rotation: [0., 0., 0., 1.],
                        dt_s: 0.1,
                        ..default()
                    },
                )
            })
            .collect(),
    )])));
    let owner = super::super::gas::payer(app.world(), parent).unwrap();
    let ledger = super::super::gas::GasLedger::default();
    ledger.ensure_account(owner, 0);
    app.insert_resource(ledger.clone());
    let mut seen = std::collections::BTreeSet::new();
    let mut spent = 0;
    let mut suspended = false;
    for _ in 0..100 {
        ledger.deposit(owner, 100_000).unwrap();
        step(&mut app);
        let mut software = app.world_mut().get_mut::<ShipSoftware>(parent).unwrap();
        assert!(software.controller.fault.is_none());
        assert!(software.last_gas_used <= toy_sim_ship_wasm::FUEL_PER_TICK);
        spent += software.last_gas_used;
        suspended |= software.controller.is_suspended();
        for (handle, control) in std::mem::take(&mut software.missile_controls) {
            assert_eq!(control.throttle, 0.2);
            seen.insert(handle);
        }
        let account = ledger.account(owner).unwrap();
        assert_eq!(account.spent, spent);
        assert_eq!(account.reserved, 0);
        assert!(ledger.snapshot().is_ok());
    }
    assert!(suspended, "callbacks must span account-funded slices");
    assert_eq!(seen, [1, 2, 3].into_iter().collect());
    assert!(throttle(app.world(), parent) >= initial_throttle + 0.003);
    assert_eq!(ledger.account(owner).unwrap().available + spent, 10_000_000);
}

#[test]
fn flight_and_display_share_small_grants_without_stranding_atomic_calls() {
    for (flight, display, grant) in [
        (700_000, 700_000, 1_000_000),
        (1, 1, 1),
        (700_000, 1, 1_000_000),
        (1, 700_000, 1_000_000),
        (10, 10, 30),
    ] {
        let mut flight_progress = false;
        let mut display_progress = false;
        for tick in 0..2 {
            let allowance = flight_allowance(grant, flight, Some(display), tick != 0);
            assert!(allowance <= grant);
            assert!(allowance == 0 || allowance >= flight);
            flight_progress |= allowance >= flight;
            display_progress |= grant - allowance >= display;
        }
        assert!(
            flight_progress && display_progress,
            "{flight}/{display} with grant {grant}"
        );
    }
    assert_eq!(flight_allowance(100, 700_000, Some(1), false), 0);
    assert_eq!(flight_allowance(100, 1, Some(700_000), true), 100);
    assert_eq!(flight_allowance(0, 1, Some(1), true), 0);
}
