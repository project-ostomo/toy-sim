use osg_ship_api::abi::{self, Record};
use osg_ship_wasm::{CallbackKind, Controller, ControllerRuntime, FUEL_PER_TICK, Input};

fn guest(imports: &str, data: &str, ship: &str, missile: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            {imports}
            (memory (export "memory") 1)
            (func (export "ship_api_version") (result i32) i32.const {})
            {data}
            (func (export "ship_tick") {ship})
            (func (export "missile_tick") (param $handle i64) {missile}))"#,
        abi::VERSION,
    ))
    .unwrap()
}

fn bytes(offset: u32, value: &impl Record) -> String {
    let escaped: String = value
        .bytes()
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect();
    format!(r#"(data (i32.const {offset}) "{escaped}")"#)
}

fn boot(program: &[u8]) -> Controller {
    let mut controller = ControllerRuntime::new()
        .unwrap()
        .instantiate(program)
        .unwrap();
    for _ in 0..60 {
        if !controller.is_booting() {
            return controller;
        }
        controller
            .run_slice(Input::default(), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    panic!("boot did not finish");
}

fn observation(handle: u64) -> abi::MissileObservation {
    abi::MissileObservation {
        handle,
        rotation: [0., 0., 0., 1.],
        maximum_acceleration_m_s2: 50.,
        turn_rate_rad_s: 2.,
        fuel_units: 100,
        dt_s: 0.1,
        ..Default::default()
    }
}

#[test]
fn missile_and_ship_share_memory_while_suspended_callbacks_keep_their_identity() {
    let imports = r#"
        (import "ship_v32" "missile_read" (func $read (param i32 i32) (result i32)))
        (import "ship_v32" "missile_control" (func $control (param i32 i32) (result i32)))
        (import "ship_v32" "persistent_write" (func $save (param i32 i32) (result i32)))
    "#;
    let control = abi::MissileControl {
        direction: [1., 0., 0.],
        throttle: 0.5,
    };
    let data = format!(
        "{} (global $shared (mut i32) (i32.const 0))",
        bytes(1024, &control)
    );
    let ship = r#"
        global.get $shared i32.const 1 i32.add global.set $shared
        i32.const 2048 global.get $shared i32.store
        i32.const 2048 i32.const 4 call $save drop
    "#;
    let missile = r#"
        (local $count i32) (local $saved_handle i64)
        local.get $handle local.set $saved_handle
        global.get $shared i32.const 10 i32.add global.set $shared
        i32.const 5000 local.set $count
        loop $work
            local.get $count i32.const 1 i32.sub local.tee $count br_if $work
        end
        i32.const 0 i32.const 192 call $read i32.const 0 i32.ne if unreachable end
        i32.const 0 i64.load local.get $saved_handle i64.ne if unreachable end
        i32.const 160 i64.load i64.eqz if unreachable end
        i32.const 1024 i32.const 32 call $control i32.const 0 i32.ne if unreachable end
    "#;
    let mut controller = boot(&guest(imports, &data, ship, missile));
    assert!(controller.supports_missiles());
    let first = controller
        .run_slice(Input::default(), None, 1000, FUEL_PER_TICK)
        .unwrap();
    assert_eq!(first.callback, Some(CallbackKind::Ship));
    assert!(first.callback_completed);
    assert_eq!(controller.checkpoint().persistent_data, 1u32.to_le_bytes());

    let kind = CallbackKind::Missile(44);
    let first = controller
        .run_callback_slice(
            kind,
            Input::default(),
            None,
            Some(observation(44)),
            1000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(!first.callback_completed);
    assert_eq!(controller.pending_callback(), Some(kind));
    assert!(
        controller
            .run_slice(Input::default(), None, 1000, FUEL_PER_TICK)
            .is_err()
    );
    assert!(
        controller
            .run_callback_slice(
                CallbackKind::Missile(45),
                Input::default(),
                None,
                Some(observation(45)),
                1000,
                FUEL_PER_TICK
            )
            .is_err()
    );
    assert_eq!(controller.pending_callback(), Some(kind));
    assert!(
        controller
            .run_callback_slice(
                kind,
                Input::default(),
                None,
                Some(observation(45)),
                1000,
                FUEL_PER_TICK
            )
            .is_err()
    );
    assert_eq!(controller.pending_callback(), Some(kind));
    assert!(controller.fault.is_none());

    let mut completed = false;
    for tick in 0..100 {
        let mut live = observation(44);
        live.time_s = tick as f64 * 0.1;
        let result = controller
            .run_callback_slice(
                kind,
                Input::default(),
                None,
                Some(live),
                1000,
                FUEL_PER_TICK,
            )
            .unwrap();
        assert!(controller.last_gas_used <= 1000);
        assert_eq!(result.callback, Some(kind));
        if result.callback_completed {
            assert_eq!(result.output.missiles, vec![(44, control)]);
            completed = true;
            break;
        }
        assert!(result.output.missiles.is_empty());
    }
    assert!(completed);
    assert_eq!(controller.pending_callback(), None);
    let ship = controller
        .run_slice(Input::default(), None, 1000, FUEL_PER_TICK)
        .unwrap();
    assert!(ship.callback_completed);
    assert_eq!(controller.checkpoint().persistent_data, 12u32.to_le_bytes());
}

#[test]
fn missile_control_is_prepaid_and_observations_refresh_before_resuming() {
    let imports = r#"
        (import "ship_v32" "missile_read" (func $read (param i32 i32) (result i32)))
        (import "ship_v32" "missile_control" (func $control (param i32 i32) (result i32)))
    "#;
    let control = abi::MissileControl {
        direction: [0., 1., 0.],
        throttle: 0.75,
    };
    let missile = r#"
        i32.const 0 i32.const 32 call $control i32.const 0 i32.ne if unreachable end
        i32.const 256 i32.const 192 call $read i32.const 0 i32.ne if unreachable end
        i32.const 416 i64.load i64.eqz i32.eqz if unreachable end
    "#;
    let mut controller = boot(&guest(imports, &bytes(0, &control), "", missile));
    let kind = CallbackKind::Missile(7);
    let first = controller
        .run_callback_slice(
            kind,
            Input::default(),
            None,
            Some(observation(7)),
            100,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(!first.callback_completed);
    assert!(first.output.missiles.is_empty());
    assert_eq!(controller.minimum_to_progress(), 104);
    let mut dead = observation(7);
    dead.fuel_units = 0;
    let stopped = controller
        .run_callback_slice(kind, Input::default(), None, Some(dead), 0, FUEL_PER_TICK)
        .unwrap();
    assert!(!stopped.callback_completed);
    assert_eq!(controller.last_gas_used, 0);
    let finished = controller
        .run_callback_slice(
            kind,
            Input::default(),
            None,
            Some(dead),
            1000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(finished.callback_completed);
    assert_eq!(finished.output.missiles, vec![(7, control)]);
    assert!(controller.last_gas_used <= 1000);
}

#[test]
fn invalid_missile_memory_access_reboots_the_shared_computer_without_publishing_controls() {
    let imports = r#"
        (import "ship_v32" "missile_control" (func $control (param i32 i32) (result i32)))
        (import "ship_v32" "missile_read" (func $read (param i32 i32) (result i32)))
    "#;
    let control = abi::MissileControl {
        direction: [1., 0., 0.],
        throttle: 1.,
    };
    let body =
        "i32.const 0 i32.const 32 call $control drop i32.const 65520 i32.const 192 call $read drop";
    let mut controller = boot(&guest(imports, &bytes(0, &control), "", body));
    let result = controller.run_callback_slice(
        CallbackKind::Missile(9),
        Input::default(),
        None,
        Some(observation(9)),
        1000,
        FUEL_PER_TICK,
    );
    assert!(result.is_err());
    assert!(controller.is_booting());
    assert!(controller.fault.is_some());
    assert_eq!(controller.pending_callback(), None);
    assert!(controller.last_gas_used > 0 && controller.last_gas_used <= 1000);
}

#[test]
fn missile_imports_are_unavailable_outside_their_callback_and_reject_invalid_controls() {
    let imports = r#"
        (import "ship_v32" "missile_read" (func $read (param i32 i32) (result i32)))
        (import "ship_v32" "missile_control" (func $control (param i32 i32) (result i32)))
    "#;
    let ship = "i32.const 256 i32.const 192 call $read i32.const -4 i32.ne if unreachable end i32.const 0 i32.const 32 call $control i32.const -4 i32.ne if unreachable end";
    for control in [
        abi::MissileControl {
            direction: [1., 0., 0.],
            throttle: 2.,
        },
        abi::MissileControl {
            direction: [f64::NAN, 0., 0.],
            throttle: 0.,
        },
        abi::MissileControl {
            direction: [2., 0., 0.],
            throttle: 0.5,
        },
    ] {
        let mut controller = boot(&guest(
            imports,
            &bytes(0, &control),
            ship,
            "i32.const 0 i32.const 32 call $control i32.const -3 i32.ne if unreachable end",
        ));
        assert!(
            controller
                .run_slice(Input::default(), None, 1000, FUEL_PER_TICK)
                .unwrap()
                .callback_completed
        );
        let output = controller
            .run_callback_slice(
                CallbackKind::Missile(1),
                Input::default(),
                None,
                Some(observation(1)),
                1000,
                FUEL_PER_TICK,
            )
            .unwrap();
        assert!(output.callback_completed);
        assert!(output.output.missiles.is_empty());
    }
}

#[test]
fn optional_missile_export_requires_the_declared_signature() {
    for signature in ["", "(param i32)", "(param i64) (result i32) i32.const 0"] {
        let program = wat::parse_str(format!(
            r#"(module
                (memory (export "memory") 1)
                (func (export "ship_api_version") (result i32) i32.const {})
                (func (export "ship_tick"))
                (func (export "missile_tick") {signature}))"#,
            abi::VERSION,
        ))
        .unwrap();
        let error = ControllerRuntime::new()
            .unwrap()
            .instantiate(&program)
            .err()
            .unwrap();
        assert!(error.to_string().contains("missile_tick must accept"));
    }
}

#[test]
fn retained_computer_rejects_writes_to_absent_parent_hardware() {
    use osg_ships::{DeviceDescriptor, DeviceHandle, DeviceKind};
    let imports =
        r#"(import "ship_v32" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))"#;
    let body = format!(
        "i64.const 1 i64.const {} i32.const 0 i32.const 8 call $write i32.const -4 i32.ne if unreachable end",
        abi::SET_THROTTLE,
    );
    let data = bytes(0, &abi::ThrottleSetting { fraction: 0.5 });
    let mut controller = boot(&guest(imports, &data, "", &body));
    controller.catalogue = vec![DeviceDescriptor {
        handle: DeviceHandle(0),
        part_id: 1,
        control_enabled: true,
        alias: "destroyed engine".into(),
        groups: Vec::new(),
        kind: DeviceKind::Engine {
            propellant_resource: "water".into(),
            thrust_n: 1.,
            propellant_kg_s: 1.,
            power_w: 0.,
        },
        position_m: [0.; 3],
        rotation: [0., 0., 0., 1.],
    }]
    .into();
    let result = controller
        .run_callback_slice(
            CallbackKind::Missile(5),
            Input::default(),
            None,
            Some(observation(5)),
            1000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(result.callback_completed);
    assert!(result.output.devices.is_empty());
    assert!(controller.fault.is_none());
}
