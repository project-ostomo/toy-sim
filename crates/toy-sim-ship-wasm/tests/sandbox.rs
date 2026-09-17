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
        (func (export "ship_api_version") (result i32) i32.const 13)
        {data}
        (func (export "ship_tick") {body}))"#
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

#[test]
fn old_versions_and_foreign_imports_are_rejected() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let old = wat::parse_str(
        r#"(module
        (memory (export "memory") 1)
        (func (export "ship_api_version") (result i32) i32.const 12)
        (func (export "ship_tick")))"#,
    )
    .unwrap();
    assert!(runtime.validate_program(&old).is_err());
    assert!(
        runtime
            .compile(&guest(
                r#"(import "ship_v12" "tick_read" (func (param i32 i32) (result i32)))"#,
                "",
                "",
            ))
            .is_err()
    );
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

        let out = a.run(input(0.)).unwrap().unwrap();
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

        b.run(input(0.)).unwrap().unwrap();
        a.reboot();
        assert!(a.state.spatial.paths.is_empty());
        assert!(!b.state.spatial.paths.is_empty());
        assert_eq!(a.memory_bytes(), 0);
    }
}

#[test]
fn memory_grows_on_demand_up_to_one_mebibyte_and_reboot_resets_it() {
    let bytes = guest(
        "",
        "",
        r#"
        memory.size i32.const 1 i32.eq if
            i32.const 15 memory.grow i32.const 1 i32.ne if unreachable end
            i32.const 1048575 i32.const 42 i32.store8
        end
        memory.size i32.const 16 i32.ne if unreachable end
        i32.const 1 memory.grow i32.const -1 i32.ne if unreachable end
        i32.const 1048575 i32.load8_u i32.const 42 i32.ne if unreachable end
    "#,
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &bytes);
    assert_eq!(controller.memory_bytes(), 65536);

    for time in [0., 0.1] {
        controller.run(input(time)).unwrap().unwrap();
        assert_eq!(controller.memory_bytes(), MEMORY_LIMIT);
    }

    controller.reboot();
    controller.advance(4.9);
    assert!(!runtime.boot(&mut controller).unwrap());
    controller.advance(0.1);
    assert!(runtime.boot(&mut controller).unwrap());
    assert_eq!(controller.memory_bytes(), 65536);
}

#[test]
fn runaway_callback_faults_and_requires_a_full_reboot() {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest("", "", "(loop br 0)"));
    assert!(controller.run(input(0.)).is_err());
    assert!(controller.is_booting());
    assert_eq!(controller.gas_remaining(), 0);
    controller.advance(4.9);
    assert!(!runtime.boot(&mut controller).unwrap());
    controller.advance(0.1);
    assert!(runtime.boot(&mut controller).unwrap());
    assert!(controller.fault.is_none());
}

#[test]
fn exact_record_lengths_and_out_of_bounds_pointers_fail_without_writes() {
    let imports = r#"(import "ship_v13" "flight_read" (func $read (param i32 i32) (result i32)))"#;
    let body = r#"
        i32.const 0 i32.const 123 i32.store
        i32.const 0 i32.const 169 call $read i32.const -2 i32.ne if unreachable end
        i32.const 0 i32.load i32.const 123 i32.ne if unreachable end
        i32.const -16 i32.const 168 call $read i32.const -2 i32.ne if unreachable end
        i32.const 65520 i32.const 168 call $read i32.const -2 i32.ne if unreachable end
        i32.const 0 i32.const 168 call $read i32.const 0 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    ready(&mut runtime, &guest(imports, "", body))
        .run(input(0.))
        .unwrap()
        .unwrap();
}

const PATH_IMPORTS: &str = r#"
    (import "ship_v13" "spatial_path_put" (func $path (param i32 i32 i32 i32) (result i32)))
    (import "ship_v13" "spatial_marker_put" (func $marker (param i32 i32) (result i32)))
    (import "ship_v13" "snapshot_keep" (func $keep (param i64) (result i32)))
    (import "ship_v13" "snapshot_drop" (func $drop (param i64) (result i32)))
"#;

fn path_data() -> String {
    let path = abi::SpatialPath {
        meta: abi::SpatialMeta {
            id: 1,
            valid_until_s: 10.,
            ..Default::default()
        },
        frame: abi::SpatialFrame {
            reference: 1,
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
        i64.const 1 call $keep i32.const 0 i32.ne if unreachable end
        i32.const 4096 i32.const 152 i32.const 0 i32.const 2 call $path
        i32.const 0 i32.ne if unreachable end
        i64.const 1 call $drop i32.const 0 i32.ne if unreachable end
        i32.const 32 f64.const 0 f64.store
        i32.const 4096 i32.const 152 i32.const 0 i32.const 2 call $path
        i32.const -3 i32.ne if unreachable end
    "#;
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = ready(&mut runtime, &guest(PATH_IMPORTS, &path_data(), body));
    controller.observer_origin = [10_i128.pow(25); 3];
    controller.run(input(0.)).unwrap().unwrap();
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
    controller.run(input(0.)).unwrap().unwrap();
    let revision = controller.state.spatial.revision;
    controller.run(input(1.)).unwrap().unwrap();
    assert_eq!(controller.state.spatial.revision, revision);
    controller.run(input(10.)).unwrap();
    assert!(controller.state.spatial.paths.is_empty());
}

#[test]
fn trapped_callback_discards_spatial_instrument_and_hardware_publications() {
    let imports = format!(
        r#"{PATH_IMPORTS}
        (import "ship_v13" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))
        (import "ship_v13" "instrument_attitude_put" (func $attitude (param i32 i32) (result i32)))
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
    assert!(controller.run(input(0.)).is_err());
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
fn scan_reserves_full_cost_and_failure_never_calls_provider_or_overwrites_buffer() {
    let imports =
        r#"(import "ship_v13" "sensor_scan" (func $scan (param i64 i32 i32 i32) (result i32)))"#;
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
    let initial_gas = controller.gas_remaining();
    let reservation = 256 * abi::SCAN_GAS_PER_OBJECT;
    let expected_admissions = initial_gas / reservation;
    assert!(expected_admissions > 0);
    controller
        .run_with_scan(observation, Some(Arc::new(CountedSource(count.clone()))))
        .unwrap()
        .unwrap();
    assert_eq!(count.load(Ordering::Relaxed) as u64, expected_admissions);
    assert!(initial_gas - controller.gas_remaining() >= expected_admissions * reservation);
    assert!(controller.gas_remaining() < reservation);
    assert!(controller.state.spatial.tracks.contains_key(&7));
}

#[test]
fn requests_survive_reboot_until_committed_acknowledgement() {
    let imports = r#"
        (import "ship_v13" "request_info" (func $info (param i32 i32 i32) (result i32)))
        (import "ship_v13" "request_reply" (func $reply (param i64 i64 i32 i32) (result i32)))
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
    assert!(controller.run(observation).unwrap().is_none());
    controller.advance(5.);
    runtime.boot(&mut controller).unwrap();
    controller.advance(0.2);
    let out = controller.run(input(5.)).unwrap().unwrap();
    assert_eq!(out.replies.len(), 1);
    assert_eq!(out.replies[0].id, 42);
}

#[test]
fn unfinished_screen_frame_is_discarded_and_completed_frame_is_independent() {
    let imports = r#"
        (import "ship_v13" "screen_define" (func $define (param i32 i32) (result i32)))
        (import "ship_v13" "screen_begin" (func $begin (param i32 i32) (result i32)))
        (import "ship_v13" "screen_end" (func $end (param i64) (result i32)))
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
    assert!(
        unfinished
            .run(input(0.))
            .unwrap()
            .unwrap()
            .screens
            .is_empty()
    );
    assert_eq!(unfinished.screens.len(), 1);
    let mut complete = ready(
        &mut runtime,
        &guest(
            imports,
            &data,
            &(body.to_owned() + " i64.const 0 call $end drop"),
        ),
    );
    let out = complete.run(input(0.)).unwrap().unwrap();
    assert_eq!(out.screens.len(), 1);
    assert_eq!(out.screens[0].height, 256);
}

#[test]
fn borrowed_snapshot_expires_and_retained_snapshot_quota_is_explicit() {
    let imports = r#"
        (import "ship_v13" "tick_read" (func $tick (param i32 i32) (result i32)))
        (import "ship_v13" "snapshot_keep" (func $keep (param i64) (result i32)))
        (import "ship_v13" "snapshot_drop" (func $drop (param i64) (result i32)))
    "#;
    let body = r#"
        i32.const 0 i32.const 96 call $tick drop
        i32.const 8 i64.load i64.const 9 i64.le_u
        if
            i32.const 8 i64.load call $keep i32.const 0 i32.ne if unreachable end
            i32.const 8 i64.load call $keep i32.const 0 i32.ne if unreachable end
        else
            i32.const 8 i64.load call $keep i32.const -5 i32.ne if unreachable end
            i64.const 2 call $drop i32.const 0 i32.ne if unreachable end
            i32.const 8 i64.load call $keep i32.const 0 i32.ne if unreachable end
            i64.const 2 call $keep i32.const -6 i32.ne if unreachable end
        end
    "#;
    let data = "(global $skip (mut i32) (i32.const 0))";
    // Callback one borrows its snapshot without retaining it.
    let body = format!(
        r#"
        global.get $skip i32.eqz if
            i32.const 1 global.set $skip
        else
            i64.const 1 call $keep i32.const -6 i32.ne if unreachable end
            {body}
        end
    "#
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = ready(&mut runtime, &guest(imports, data, &body));

    for tick in 0..10 {
        computer.advance(0.2);
        computer.run(input(tick as f64)).unwrap().unwrap();
    }
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
            r#"(import "ship_v13" "device_write" (func $write (param i64 i64 i32 i32) (result i32)))"#,
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
        let result = computer.run(input(0.0));
        if trap {
            assert!(result.is_err());
            assert!(computer.is_booting());
        } else {
            assert!(result.unwrap().unwrap().devices.is_empty());
            assert!(!computer.is_booting());
        }
    }
}
