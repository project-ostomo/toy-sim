use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use toy_sim_ship_api::abi::{self, Record};
use toy_sim_ship_wasm::*;
use toy_sim_ships::{DeviceDescriptor, DeviceHandle, DeviceKind, DeviceReading, DeviceStatus};

fn guest(imports: &str, data: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
        {imports}
        (memory (export "memory") 1)
        (func (export "ship_api_version") (result i32) i32.const {version})
        {data}
        (func (export "ship_tick") {body}))"#,
        version = abi::VERSION
    ))
    .unwrap()
}

fn record_data<T: Record>(offset: u32, record: &T) -> String {
    let bytes: String = record
        .bytes()
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect();
    format!("(data (i32.const {offset}) \"{bytes}\")")
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
            propellant_resource: "propellant".into(),
            thrust_n: 20_000.,
            propellant_kg_s: 1.,
            power_w: 200_000.,
        },
        position_m: [0.; 3],
        rotation: [0., 0., 0., 1.],
    }]
    .into();
    boot(&mut controller);
    controller
}

fn boot(controller: &mut Controller) {
    for _ in 0..60 {
        if !controller.is_booting() {
            return;
        }
        let slice = controller
            .run_slice(input(0.), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
        assert!(!slice.callback_completed);
        assert!(controller.last_gas_used <= FUEL_PER_TICK);
    }
    panic!("boot did not complete");
}

fn run(controller: &mut Controller, observation: Input) -> anyhow::Result<Output> {
    let slice = controller.run_slice(observation, None, FUEL_PER_TICK, FUEL_PER_TICK)?;
    assert!(slice.callback_completed);
    Ok(slice.output)
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

#[test]
fn old_versions_and_foreign_imports_are_rejected() {
    let mut runtime = ControllerRuntime::new().unwrap();
    runtime.validate_program(&guest("", "", "")).unwrap();

    for version in [12, 28] {
        let old = wat::parse_str(format!(
            r#"(module
            (memory (export "memory") 1)
            (func (export "ship_api_version") (result i32) i32.const {version})
            (func (export "ship_tick")))"#,
        ))
        .unwrap();
        let error = runtime.validate_program(&old).unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "unsupported ship controller API {version}; expected {}",
                abi::VERSION
            )),
            "{error:#}"
        );

        let imports = format!(
            r#"(import "ship_v{version}" "tick_read" (func (param i32 i32) (result i32)))"#
        );
        let error = runtime.compile(&guest(&imports, "", "")).unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "unsupported controller import ship_v{version}.tick_read"
            )),
            "{error:#}"
        );
    }
}

#[test]
fn c_and_no_std_rust_share_the_abi_with_persistent_isolated_memory() {
    for (bytes, destination) in [
        (
            include_bytes!("fixtures/controller.wasm").as_slice(),
            [0., 42., 0.],
        ),
        (
            include_bytes!("fixtures/embedded.wasm").as_slice(),
            [0., 0., 100.],
        ),
    ] {
        let mut runtime = ControllerRuntime::new().unwrap();
        let mut a = ready(&mut runtime, bytes);
        let mut b = ready(&mut runtime, bytes);
        assert_eq!(runtime.cached_modules(), 1);

        let out = run(&mut a, input(0.)).unwrap();
        assert_eq!(
            out.devices[0].setting,
            toy_sim_ships::DeviceSetting::Throttle(0.25)
        );
        assert_eq!(
            a.state.spatial.paths[&1].vertices[1].position_m,
            destination
        );
        assert!(b.state.spatial.paths.is_empty());
        assert!(a.memory_bytes() <= MEMORY_LIMIT);

        run(&mut b, input(0.)).unwrap();
        a.reboot();
        assert!(a.state.spatial.paths.is_empty());
        assert!(!b.state.spatial.paths.is_empty());
        assert_eq!(a.memory_bytes(), 0);
    }
}

#[test]
fn memory_grows_on_demand_up_to_eight_mebibytes_and_reboot_resets_it() {
    let bytes = guest(
        "",
        "",
        r#"
        memory.size i32.const 1 i32.eq if
            i32.const 127 memory.grow i32.const 1 i32.ne if unreachable end
            i32.const 8388607 i32.const 42 i32.store8
        end
        memory.size i32.const 128 i32.ne if unreachable end
        i32.const 1 memory.grow i32.const -1 i32.ne if unreachable end
        i32.const 8388607 i32.load8_u i32.const 42 i32.ne if unreachable end
    "#,
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &bytes);
    assert_eq!(controller.memory_bytes(), 65536);

    for time in [0., 0.1] {
        run(&mut controller, input(time)).unwrap();
        assert_eq!(controller.memory_bytes(), MEMORY_LIMIT);
    }

    controller.reboot();
    for _ in 0..49 {
        controller
            .run_slice(input(0.), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(controller.is_booting());
    boot(&mut controller);
    assert_eq!(controller.memory_bytes(), 65536);
}

#[test]
fn runaway_callback_suspends_without_losing_its_stack_or_rebooting() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest("", "", "(loop br 0)"));
    for tick in 0..10 {
        let slice = controller
            .run_slice(input(tick as f64), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
        assert!(!slice.callback_completed);
        assert!(controller.is_suspended());
        assert!(!controller.is_booting());
        assert!(controller.fault.is_none());
        assert!(controller.last_gas_used <= FUEL_PER_TICK);
    }
}

#[test]
fn invalid_record_lengths_return_status_but_actual_memory_access_traps() {
    let imports = r#"(import "ship_v29" "flight_read" (func $read (param i32 i32) (result i32)))"#;
    let body = r#"
        i32.const 0 i32.const 123 i32.store
        i32.const 0 i32.const 169 call $read i32.const -2 i32.ne if unreachable end
        i32.const 0 i32.load i32.const 123 i32.ne if unreachable end
        i32.const 0 i32.const 168 call $read i32.const 0 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(imports, "", body));
    run(&mut controller, input(0.)).unwrap();

    for pointer in [-16, 65520] {
        let body = format!("i32.const {pointer} i32.const 168 call $read drop");
        let mut controller = ready(&mut runtime, &guest(imports, "", &body));
        assert!(run(&mut controller, input(0.)).is_err());
        assert!(controller.is_booting());
    }
}

const PATH_IMPORTS: &str = r#"
    (import "ship_v29" "spatial_path_put" (func $path (param i32 i32 i32 i32) (result i32)))
    (import "ship_v29" "spatial_marker_put" (func $marker (param i32 i32) (result i32)))
    (import "ship_v29" "snapshot_keep" (func $keep (param i64) (result i32)))
    (import "ship_v29" "snapshot_drop" (func $drop (param i64) (result i32)))
"#;

fn path_data() -> String {
    let path = abi::SpatialPath {
        meta: abi::SpatialMeta {
            id: 1,
            valid_until_s: 10.,
            ..Default::default()
        },
        frame: abi::SpatialFrame {
            reference: 2,
            ..Default::default()
        },
        kind: abi::PATH_TIMED,
        ..Default::default()
    };
    let end = abi::SpatialVertex {
        time_s: 60.,
        position_m: [0., 42., 0.],
    };
    format!("{} {}", record_data(4096, &path), record_data(32, &end))
}

#[test]
fn paths_copy_snapshots_and_invalid_replacement_preserves_accepted_geometry() {
    let body = r#"
        i64.const 2 call $keep i32.const 0 i32.ne if unreachable end
        i32.const 4096 i32.const 152 i32.const 0 i32.const 2 call $path
        i32.const 0 i32.ne if unreachable end
        i64.const 2 call $drop i32.const 0 i32.ne if unreachable end
        i32.const 32 f64.const 0 f64.store
        i32.const 4096 i32.const 152 i32.const 0 i32.const 2 call $path
        i32.const -3 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(PATH_IMPORTS, &path_data(), body));
    controller.observer_origin = [10_i128.pow(25); 3];
    run(&mut controller, input(0.)).unwrap();
    let path = &controller.state.spatial.paths[&1];
    assert_eq!(path.vertices[1].time_s, 60.);
    assert_eq!(
        spatial::relative(path.at(60.).unwrap(), controller.observer_origin),
        Some([0., 42., 0.])
    );
}

#[test]
fn omissions_retain_leased_geometry_and_expiration_runs_when_callback_cannot() {
    let data = format!("{} (global $done (mut i32) (i32.const 0))", path_data());
    let body = r#"
        global.get $done i32.eqz if
            i32.const 4096 i32.const 152 i32.const 0 i32.const 2 call $path drop
            i32.const 1 global.set $done
        end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(PATH_IMPORTS, &data, body));
    run(&mut controller, input(0.)).unwrap();
    assert!(!controller.state.spatial.paths.is_empty());
    let revision = controller.state.spatial.revision;
    run(&mut controller, input(1.)).unwrap();
    assert_eq!(controller.state.spatial.revision, revision);
    run(&mut controller, input(10.)).unwrap();
    assert!(controller.state.spatial.paths.is_empty());
}

#[test]
fn trapped_callback_discards_spatial_instrument_and_hardware_publications() {
    let imports = format!(
        r#"{PATH_IMPORTS}
        (import "ship_v29" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
        (import "ship_v29" "instrument_attitude_put" (func $attitude (param i32 i32) (result i32)))
    "#
    );
    let attitude = abi::AttitudeState {
        valid_until_s: 10.,
        ..Default::default()
    };
    let data = format!("{} {}", path_data(), record_data(2048, &attitude));
    let body = r#"
        i32.const 4096 i32.const 152 i32.const 0 i32.const 2 call $path drop
        i32.const 2048 i32.const 64 call $attitude drop
        i32.const 1024 f64.const 0.75 f64.store
        i64.const 1 i64.const 0 i32.const 1024 i32.const 8 call $write drop
        unreachable
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(&imports, &data, body));
    assert!(run(&mut controller, input(0.)).is_err());
    assert!(controller.state.spatial.paths.is_empty());
    assert!(controller.state.attitude.is_none());
    assert!(controller.telemetry.is_none());
}

struct CountedSource(Arc<AtomicUsize>);

impl ScanSource for CountedSource {
    fn scan(&self, _: f64, maximum: usize) -> Vec<SensorContact> {
        self.0.fetch_add(1, Ordering::Relaxed);
        assert_eq!(maximum, 256);
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
fn scan_reserves_full_cost_and_suspends_before_repeating_provider_work() {
    let imports =
        r#"(import "ship_v29" "sensor_scan" (func $scan (param i64 i32 i32 i32) (result i32)))"#;
    let body = r#"(local $status i32)
        (loop $again
            i32.const 8192 i32.const 123 i32.store
            i64.const 1 i32.const 256 i32.const 8192 i32.const 18432
            call $scan local.tee $status i32.const 0 i32.ge_s br_if $again)
        local.get $status i32.const -1 i32.ne if unreachable end
        i32.const 8192 i32.load i32.const 123 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(imports, "", body));
    Arc::make_mut(&mut controller.catalogue)[0].kind = DeviceKind::Sensor { range_m: 1000. };
    let mut observation = input(0.);
    observation.devices[0].reading = DeviceReading::Sensor { range_m: 1000. };
    let count = Arc::new(AtomicUsize::new(0));
    let reservation = 256 * abi::SCAN_GAS_PER_OBJECT;
    for tick in 1..=3 {
        let slice = controller
            .run_slice(
                observation.clone(),
                Some(Arc::new(CountedSource(count.clone()))),
                FUEL_PER_TICK,
                FUEL_PER_TICK,
            )
            .unwrap();
        assert!(!slice.callback_completed);
        assert_eq!(count.load(Ordering::Relaxed), tick);
        assert!(controller.last_gas_used >= reservation);
        assert!(controller.last_gas_used <= FUEL_PER_TICK);
    }
    assert!(controller.state.spatial.tracks.contains_key(&7));
}

#[test]
fn fresh_requests_submitted_during_boot_are_delivered_after_startup() {
    let imports = r#"
        (import "ship_v29" "request_info" (func $info (param i32 i32 i32) (result i32)))
        (import "ship_v29" "request_reply" (func $reply (param i64 i64 i32 i32) (result i32)))
    "#;
    let body = r#"
        i32.const 0 i32.const 0 i32.const 24 call $info i32.const 0 i32.ne if unreachable end
        i32.const 0 i64.load i64.const 42 i64.ne if unreachable end
        i64.const 42 i64.const 0 i32.const 0 i32.const 0 call $reply drop
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(imports, "", body));
    controller.reboot();
    let mut observation = input(0.);
    observation.commands.push(Request {
        id: 42,
        command: Command::HoldAttitude,
    });
    assert!(
        !controller
            .run_slice(observation, None, 0, FUEL_PER_TICK)
            .unwrap()
            .callback_completed
    );
    boot(&mut controller);
    let out = run(&mut controller, input(5.)).unwrap();
    assert_eq!(out.replies.len(), 1);
    assert_eq!(out.replies[0].id, 42);
}

#[test]
fn unfinished_screen_frame_is_discarded_and_completed_frame_is_independent() {
    let imports = r#"
        (import "ship_v29" "screen_define" (func $define (param i32 i32) (result i32)))
        (import "ship_v29" "screen_begin" (func $begin (param i32 i32) (result i32)))
        (import "ship_v29" "screen_end" (func $end (param i64) (result i32)))
    "#;
    let definition = abi::ScreenDefinition {
        id: 0,
        width: 512,
        height: 256,
        title: abi::Text64::new("Test"),
    };
    let data = record_data(0, &definition);
    let body = r#"
        i32.const 0 i32.const 96 call $define i32.const 0 i32.ne if unreachable end
        i32.const 1024 i32.const 16 call $begin i32.const 0 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut unfinished = ready(&mut runtime, &guest(imports, &data, body));
    assert!(run(&mut unfinished, input(0.)).unwrap().screens.is_empty());
    assert_eq!(unfinished.screens.len(), 1);
    let mut complete = ready(
        &mut runtime,
        &guest(
            imports,
            &data,
            &(body.to_owned() + " i64.const 0 call $end drop"),
        ),
    );
    let out = run(&mut complete, input(0.)).unwrap();
    assert_eq!(out.screens.len(), 1);
    assert_eq!(out.screens[0].height, 256);
}

#[test]
fn borrowed_snapshot_expires_and_retained_snapshot_quota_is_explicit() {
    let imports = r#"
        (import "ship_v29" "tick_read" (func $tick (param i32 i32) (result i32)))
        (import "ship_v29" "snapshot_keep" (func $keep (param i64) (result i32)))
        (import "ship_v29" "snapshot_drop" (func $drop (param i64) (result i32)))
    "#;
    let body = r#"
        i32.const 0 i32.const 96 call $tick drop
        i32.const 8 i64.load i64.const 10 i64.le_u
        if
            i32.const 8 i64.load call $keep i32.const 0 i32.ne if unreachable end
            i32.const 8 i64.load call $keep i32.const 0 i32.ne if unreachable end
        else
            i32.const 8 i64.load call $keep i32.const -5 i32.ne if unreachable end
            i64.const 3 call $drop i32.const 0 i32.ne if unreachable end
            i32.const 8 i64.load call $keep i32.const 0 i32.ne if unreachable end
            i64.const 3 call $keep i32.const -6 i32.ne if unreachable end
        end
    "#;
    let data = "(global $skip (mut i32) (i32.const 0))";
    // Callback one borrows its snapshot without retaining it.
    let body = format!(
        r#"
        global.get $skip i32.eqz if
            i32.const 1 global.set $skip
        else
            i64.const 2 call $keep i32.const -6 i32.ne if unreachable end
            {body}
        end
    "#
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = ready(&mut runtime, &guest(imports, data, &body));

    for tick in 0..10 {
        run(&mut computer, input(tick as f64)).unwrap();
    }
}

#[test]
fn suspended_callback_discards_expired_fire_and_publications_without_faulting() {
    let imports = format!(
        r#"{PATH_IMPORTS}
        (import "ship_v29" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
        (import "ship_v29" "instrument_attitude_put" (func $attitude (param i32 i32) (result i32)))
        (import "ship_v29" "instrument_navigation_put" (func $navigation (param i32 i32) (result i32)))
        (import "ship_v29" "instrument_weapons_put" (func $weapons (param i32 i32 i32 i32) (result i32)))
        (import "ship_v29" "instrument_contacts_put" (func $contacts (param i32 i32) (result i32)))
        "#
    );
    let data = [
        record_data(
            0,
            &abi::WeaponSetting {
                aim_direction: [0., 0., -1.],
                valid_until_s: 0.1,
                trigger: 1,
                ..Default::default()
            },
        ),
        record_data(
            128,
            &abi::AttitudeState {
                valid_until_s: 2.,
                ..Default::default()
            },
        ),
        record_data(
            256,
            &abi::NavigationState {
                valid_until_s: 2.,
                ..Default::default()
            },
        ),
        record_data(
            640,
            &abi::WeaponsState {
                valid_until_s: 2.,
                mode: abi::WEAPONS_FIRING,
                ..Default::default()
            },
        ),
        record_data(
            1024,
            &abi::WeaponInstrument {
                device: 1,
                aim_marker: 2,
                ..Default::default()
            },
        ),
        record_data(
            1280,
            &abi::SpatialPath {
                meta: abi::SpatialMeta {
                    id: 1,
                    valid_until_s: 2.,
                    ..Default::default()
                },
                frame: abi::SpatialFrame {
                    reference: 2,
                    ..Default::default()
                },
                kind: abi::PATH_TIMED,
                ..Default::default()
            },
        ),
        record_data(1536, &abi::SpatialVertex::default()),
        record_data(
            1568,
            &abi::SpatialVertex {
                time_s: 10.,
                position_m: [0., 0., -100.],
            },
        ),
        record_data(
            1664,
            &abi::SpatialMarker {
                meta: abi::SpatialMeta {
                    id: 2,
                    valid_until_s: 2.,
                    ..Default::default()
                },
                frame: abi::SpatialFrame {
                    kind: abi::FRAME_PATH,
                    reference: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        record_data(
            1920,
            &abi::ContactsState {
                valid_until_s: 2.,
                selected_contact: 42,
            },
        ),
        record_data(
            2048,
            &abi::NavigationState {
                valid_until_s: 10.,
                throttle: 0.75,
                ..Default::default()
            },
        ),
    ]
    .join("\n");
    let publish = r#"
        i64.const 1 i64.const 5 i32.const 0 i32.const 72 call $write
        i32.const 0 i32.ne if unreachable end
        i32.const 1280 i32.const 152 i32.const 1536 i32.const 2 call $path
        i32.const 0 i32.ne if unreachable end
        i32.const 1664 i32.const 176 call $marker
        i32.const 0 i32.ne if unreachable end
        i32.const 128 i32.const 64 call $attitude
        i32.const 0 i32.ne if unreachable end
        i32.const 256 i32.const 368 call $navigation
        i32.const 0 i32.ne if unreachable end
        i32.const 640 i32.const 288 i32.const 1024 i32.const 1 call $weapons
        i32.const 0 i32.ne if unreachable end
    "#;
    let body = format!(
        r#"
        (local $remaining i32)
        {publish}
        i32.const 10000 local.set $remaining
        loop $delay
            local.get $remaining i32.const 1 i32.sub local.tee $remaining br_if $delay
        end
        i32.const 2048 i32.const 368 call $navigation
        i32.const 0 i32.ne if unreachable end
        {publish}
        i32.const 1920 i32.const 16 call $contacts
        i32.const 0 i32.ne if unreachable end

        i32.const 312 f64.const nan f64.store
        i32.const 256 i32.const 368 call $navigation
        i32.const -3 i32.ne if unreachable end
        i32.const 1576 f64.const nan f64.store
        i32.const 1280 i32.const 152 i32.const 1536 i32.const 2 call $path
        i32.const -3 i32.ne if unreachable end
        i32.const 1024 i64.const 0 i64.store
        i32.const 640 i32.const 288 i32.const 1024 i32.const 1 call $weapons
        i32.const -6 i32.ne if unreachable end
        i32.const 0 f64.const nan f64.store
        i64.const 1 i64.const 5 i32.const 0 i32.const 72 call $write
        i32.const -3 i32.ne if unreachable end
    "#
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(&imports, &data, &body));
    let mut catalogue = controller.catalogue.to_vec();
    catalogue[0].kind = DeviceKind::Weapon;
    controller.catalogue = catalogue.into();

    let first = controller
        .run_slice(input(0.), None, 5000, FUEL_PER_TICK)
        .unwrap();
    assert!(!first.callback_completed);
    assert_eq!(first.output.devices.len(), 1);
    assert!(controller.state.attitude.is_some());
    assert!(controller.state.weapons.is_some());
    assert_eq!(controller.state.spatial.paths.len(), 1);
    assert_eq!(controller.state.spatial.markers.len(), 1);

    let last = controller
        .run_slice(input(3.), None, FUEL_PER_TICK, FUEL_PER_TICK)
        .unwrap();
    assert!(last.callback_completed);
    assert!(last.output.devices.is_empty());
    assert!(controller.state.attitude.is_none());
    assert!(controller.state.weapons.is_none());
    assert!(controller.state.contacts.is_none());
    assert!(controller.state.spatial.paths.is_empty());
    assert!(controller.state.spatial.markers.is_empty());
    assert_eq!(controller.state.navigation.unwrap().throttle, 0.75);
    assert!(controller.fault.is_none());
}

#[test]
fn weapon_syscalls_reject_invalid_records_and_discard_staged_fire_on_fault() {
    let valid = abi::WeaponSetting {
        aim_direction: [0.0, 0.0, -1.0],
        maximum_pointing_error_rad: 0.01,
        valid_until_s: 0.1,
        trigger: 1,
        ..Default::default()
    };
    for (setting, status, trap) in [
        (valid, 0, true),
        (
            abi::WeaponSetting {
                aim_direction: [0.0; 3],
                ..valid
            },
            abi::ERR_ARGUMENT,
            false,
        ),
        (
            abi::WeaponSetting {
                valid_until_s: 100.0,
                ..valid
            },
            abi::ERR_ARGUMENT,
            false,
        ),
        (
            abi::WeaponSetting {
                trigger: 2,
                ..valid
            },
            abi::ERR_ARGUMENT,
            false,
        ),
    ] {
        let bytes = guest(
            r#"(import "ship_v29" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))"#,
            &record_data(0, &setting),
            &format!(
                "i64.const 1 i64.const {} i32.const 0 i32.const 72 call $write i32.const {status} i32.ne if unreachable end {}",
                abi::SET_WEAPON,
                if trap { "unreachable" } else { "" }
            ),
        );
        let mut runtime = ControllerRuntime::new().unwrap();
        let mut computer = ready(&mut runtime, &bytes);
        let mut catalogue = computer.catalogue.to_vec();
        catalogue[0].kind = DeviceKind::Weapon;
        computer.catalogue = catalogue.into();
        let result = run(&mut computer, input(0.0));
        if trap {
            assert!(result.is_err());
            assert!(computer.is_booting());
        } else {
            assert!(result.unwrap().devices.is_empty());
            assert!(!computer.is_booting());
        }
    }
}

#[test]
fn micropulse_engine_metadata_names_its_charge_resource_without_electrical_input() {
    use toy_sim_ships::{Catalogue, EXAMPLE_CONTROLLER, starter};

    let catalogue = Catalogue::builtin();
    let mut blueprint = starter(EXAMPLE_CONTROLLER.to_vec());
    blueprint.parts.truncate(1);
    blueprint.parts[0].prototype = "micropulse_engine_8m".into();
    blueprint.parts[0].tanks.clear();
    let design = blueprint.compile(&catalogue).unwrap();
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = runtime.instantiate(EXAMPLE_CONTROLLER).unwrap();
    controller.configure_hardware(&design, &catalogue);
    let device = design.part_devices[0].unwrap();
    let spec = abi::EngineSpec::read(&controller.device_specs[device]).unwrap();
    let charge_index = catalogue
        .resources
        .iter()
        .position(|resource| resource.id == "micropulse_charge")
        .unwrap();
    assert_eq!(spec.propellant_resource, charge_index as u64 + 1);
    assert_eq!(spec.max_power_w, 0.);
    assert_eq!(spec.max_thrust_n, 10_000_000.);
    assert!((spec.max_thrust_n / spec.propellant_units_s - 5000. * 9.80665).abs() < 1e-6);
}

#[test]
fn durable_guest_data_survives_checkpoints_and_reboots() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let bytes = guest(
        r#"(import "ship_v29" "persistent_read" (func $read (param i32 i32) (result i32)))
           (import "ship_v29" "persistent_write" (func $write (param i32 i32) (result i32)))"#,
        r#"(data (i32.const 64) "hello")"#,
        r#"i32.const 128 i32.const 5 call $read
           if
             i32.const 128 i32.load8_u i32.const 104 i32.ne if unreachable end
           else
             i32.const 64 i32.const 5 call $write drop
           end"#,
    );
    let mut controller = ready(&mut runtime, &bytes);
    run(&mut controller, input(0.)).unwrap();
    let saved = controller.checkpoint();
    assert_eq!(saved.persistent_data, b"hello");
    let mut restored = runtime.restore(&saved).unwrap();
    boot(&mut restored);
    run(&mut restored, input(0.1)).unwrap();
    restored.reboot();
    assert_eq!(restored.checkpoint().persistent_data, b"hello");
}

#[test]
fn failed_or_out_of_bounds_callbacks_do_not_commit_durable_writes() {
    for body in [
        "i32.const 64 i32.const 5 call $write drop unreachable",
        "i32.const 65535 i32.const 5 call $write drop",
    ] {
        let mut runtime = ControllerRuntime::new().unwrap();
        let bytes = guest(
            r#"(import "ship_v29" "persistent_write" (func $write (param i32 i32) (result i32)))"#,
            r#"(data (i32.const 64) "hello")"#,
            body,
        );
        let mut controller = ready(&mut runtime, &bytes);
        assert!(run(&mut controller, input(0.)).is_err());
        assert!(controller.checkpoint().persistent_data.is_empty());
        assert!(controller.is_booting());
    }
}

#[test]
fn world_query_rejects_bad_output_memory_before_calling_world_service() {
    struct CountingSource(Arc<AtomicUsize>);
    impl ScanSource for CountingSource {
        fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
            Vec::new()
        }
        fn query(
            &self,
            _: toy_sim_model::ProgramQuery,
            _: bool,
            _: usize,
        ) -> anyhow::Result<toy_sim_model::ProgramReply> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(toy_sim_model::ProgramReply::Pose(Default::default()))
        }
    }

    let request = postcard::to_allocvec(&toy_sim_model::ProgramQuery::Travel).unwrap();
    let escaped: String = request.iter().map(|byte| format!("\\{byte:02x}")).collect();
    let imports = format!(
        r#"(import "{}" "world_query" (func $query (param i32 i32 i32 i32) (result i32)))"#,
        abi::IMPORT_MODULE
    );
    let data = format!(r#"(data (i32.const 0) "{escaped}")"#);
    let body = format!(
        "i32.const 0 i32.const {} i32.const 65520 i32.const 64 call $query drop",
        request.len()
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(&imports, &data, &body));
    let count = Arc::new(AtomicUsize::new(0));
    assert!(
        controller
            .run_slice(
                input(0.),
                Some(Arc::new(CountingSource(count.clone()))),
                FUEL_PER_TICK,
                FUEL_PER_TICK
            )
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[test]
fn repeated_screen_removal_stays_bounded_and_acknowledges_each_event_once() {
    let imports = format!(
        r#"(import "{}" "screen_remove" (func $remove (param i64) (result i32)))"#,
        abi::IMPORT_MODULE
    );
    let body = "i64.const 0 call $remove drop\n".repeat(500);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(&imports, "", &body));
    let mut observation = input(0.);
    observation.screen_events = (1..=256)
        .map(|id| abi::ScreenEvent {
            id,
            screen: 0,
            kind: abi::EVENT_BEZEL,
            ..Default::default()
        })
        .collect();
    let output = run(&mut controller, observation).unwrap();
    assert_eq!(output.cleared_screens, vec![0]);
}

#[test]
fn executable_version_negotiation_is_rejected_without_running_guest_code() {
    let bytes = wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (func (export "ship_api_version") (result i32)
                i32.const {} i32.const 0 i32.add)
            (func (export "ship_tick")))"#,
        abi::VERSION,
    ))
    .unwrap();
    let error = ControllerRuntime::new()
        .unwrap()
        .validate_program(&bytes)
        .unwrap_err();
    assert!(error.to_string().contains("literal i32 constant"));
}

#[test]
fn unfinished_screen_draft_survives_suspension_until_screen_end() {
    let imports = r#"
        (import "ship_v29" "screen_define" (func $define (param i32 i32) (result i32)))
        (import "ship_v29" "screen_begin" (func $begin (param i32 i32) (result i32)))
        (import "ship_v29" "screen_end" (func $end (param i64) (result i32)))
    "#;
    let definition = abi::ScreenDefinition {
        id: 0,
        width: 512,
        height: 256,
        title: abi::Text64::new("Suspended frame"),
    };
    let body = r#"
        (local $remaining i32)
        i32.const 0 i32.const 96 call $define drop
        i32.const 1024 i32.const 16 call $begin drop
        i32.const 2000 local.set $remaining
        loop $work
            local.get $remaining i32.const 1 i32.sub local.tee $remaining br_if $work
        end
        i64.const 0 call $end i32.const 0 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(
        &mut runtime,
        &guest(imports, &record_data(0, &definition), body),
    );
    for tick in 0..100 {
        let slice = controller
            .run_slice(input(tick as f64), None, 500, FUEL_PER_TICK)
            .unwrap();
        if slice.callback_completed {
            assert!(tick > 1);
            assert_eq!(slice.output.screens.len(), 1);
            assert_eq!(slice.output.screens[0].height, 256);
            return;
        }
        assert!(slice.output.screens.is_empty());
    }
    panic!("screen callback never completed");
}
