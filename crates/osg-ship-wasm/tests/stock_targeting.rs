use osg_ship_api::abi;
use osg_ship_wasm::{
    Command, Controller, ControllerRuntime, FUEL_PER_TICK, Input, Observation, Request, ScanSource,
    SensorContact,
};
use osg_ships::{DeviceDescriptor, DeviceHandle, DeviceKind, DeviceReading, DeviceStatus};
use std::sync::Arc;

struct Target;

impl ScanSource for Target {
    fn scan(&self, _: f64, maximum: usize) -> Vec<SensorContact> {
        if maximum == 0 {
            return Vec::new();
        }
        vec![SensorContact {
            iff: None,
            measured: abi::Contact {
                id: 7,
                kind: abi::CONTACT_SHIP,
                position_m: [0.0, 0.0, -1_000.0],
                radius_m: 10.0,
                ..Default::default()
            },
            name: "Observed target".into(),
        }]
    }
}

#[test]
fn stock_mark_start_stop_flow_publishes_targeting_intent_without_turret_handles() {
    let mut computer = ControllerRuntime::new()
        .unwrap()
        .instantiate(osg_ships::EXAMPLE_CONTROLLER)
        .unwrap();
    computer.catalogue = vec![DeviceDescriptor {
        handle: DeviceHandle(0),
        part_id: 1,
        control_enabled: true,
        alias: "sensor".into(),
        groups: Vec::new(),
        kind: DeviceKind::Sensor { range_m: 1e6 },
        position_m: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
    }]
    .into();
    assert!(
        computer
            .catalogue
            .iter()
            .all(|device| { !matches!(device.kind, DeviceKind::Weapon) })
    );

    for _ in 0..64 {
        if !computer.is_booting() {
            break;
        }
        computer
            .run_slice(input(0), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(!computer.is_booting());

    let mut tick = 1;
    for (id, command, target, mode) in [
        (
            1,
            Command::MarkTarget {
                contact: 7,
                maximum_flight_time_s: 30.0,
            },
            7,
            abi::WEAPONS_HOLD,
        ),
        (2, Command::StartFiring, 7, abi::WEAPONS_FIRING),
        (3, Command::StopFiring, 7, abi::WEAPONS_HOLD),
        (4, Command::UnmarkTarget, 0, abi::WEAPONS_HOLD),
    ] {
        issue(&mut computer, &mut tick, id, command, target, mode);
    }
}

fn issue(
    computer: &mut Controller,
    tick: &mut u64,
    id: u64,
    command: Command,
    target: u64,
    mode: u64,
) {
    let mut request = Some(Request { id, command });
    let mut accepted = false;
    for _ in 0..40 {
        let mut observation = input(*tick);
        *tick += 1;
        observation.commands.extend(request.take());
        let slice = computer
            .run_slice(
                observation,
                Some(Arc::new(Target)),
                FUEL_PER_TICK / 2,
                FUEL_PER_TICK,
            )
            .unwrap();
        for reply in slice.output.replies {
            assert_eq!(reply.id, id);
            assert_eq!(reply.result, abi::REPLY_ACCEPTED, "{}", reply.message);
            accepted = true;
        }
        assert!(slice.output.devices.is_empty());
        if accepted
            && computer
                .state
                .weapons
                .as_ref()
                .is_some_and(|state| state.target_contact == target && state.mode == mode)
        {
            return;
        }
    }
    panic!(
        "command {id} never reached targeting state: {:?}",
        computer.state.weapons
    );
}

fn input(tick: u64) -> Input {
    Input {
        tick,
        dt: 0.1,
        physics_dt: 0.1,
        observation: Observation {
            time_s: tick as f64 * osg_model::TICK_SECONDS,
            flight: abi::FlightState {
                rotation: [0.0, 0.0, 0.0, 1.0],
                mass_kg: 1_000.0,
                inertia: [100.0, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 0.0, 100.0],
                radius_m: 2.0,
                ..Default::default()
            },
            ..Default::default()
        },
        devices: vec![DeviceStatus {
            operational: true,
            powered: true,
            reading: DeviceReading::Sensor { range_m: 1e6 },
        }],
        ..Default::default()
    }
}
