use osg_ship_api::abi;
use osg_ship_wasm::{
    CallbackKind, CallbackSchedule, Command, ControllerRuntime, FUEL_PER_TICK, Input, Observation,
    Request,
};

fn input(tick: u64) -> Input {
    Input {
        tick,
        observation: Observation {
            time_s: tick as f64 * 0.1,
            flight: abi::FlightState {
                rotation: [0., 0., 0., 1.],
                mass_kg: 1_000.,
                inertia: [100., 0., 0., 0., 100., 0., 0., 0., 100.],
                radius_m: 2.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn stock_status_reports_commands_and_shared_missiles_stay_responsive_each_tick() {
    let mut computer = ControllerRuntime::new()
        .unwrap()
        .instantiate(osg_ships::EXAMPLE_CONTROLLER)
        .unwrap();
    for _ in 0..80 {
        if !computer.is_booting() {
            break;
        }
        computer
            .run_slice(input(0), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
    }
    assert!(!computer.is_booting());

    let mut schedule = CallbackSchedule::default();
    let mut tick = 0;
    loop {
        tick += 1;
        let slice = computer
            .run_slice(input(tick), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
        if slice.callback_completed && slice.output.tick_interval_seconds == Some(0.) {
            schedule.completed(slice.output.tick_interval_seconds);
            break;
        }
        assert!(tick < 30, "idle controller did not finish discovery");
    }

    let mut callbacks = 0;
    for _ in 0..100 {
        tick += 1;
        schedule.advance(0.1);
        if schedule.ready(false) {
            let slice = computer
                .run_slice(input(tick), None, FUEL_PER_TICK, FUEL_PER_TICK)
                .unwrap();
            assert!(slice.callback_completed);
            assert_eq!(slice.output.tick_interval_seconds, Some(0.));
            schedule.completed(slice.output.tick_interval_seconds);
            callbacks += 1;
        }
    }
    assert_eq!(callbacks, 100);
    schedule.advance(0.1);
    assert!(schedule.ready(false));
    assert!(schedule.ready(true));

    let mut command = input(tick + 1);
    command.commands.push(Request {
        id: 1,
        command: Command::Manual {
            throttle: 0.5,
            steering: [0.; 3],
        },
    });
    let slice = computer
        .run_slice(command, None, FUEL_PER_TICK, FUEL_PER_TICK)
        .unwrap();
    assert!(
        slice
            .output
            .replies
            .iter()
            .any(|reply| reply.id == 1 && reply.result == abi::REPLY_ACCEPTED)
    );
    assert_eq!(slice.output.tick_interval_seconds, Some(0.));
    schedule.completed(slice.output.tick_interval_seconds);
    for _ in 0..10 {
        tick += 1;
        schedule.advance(0.1);
        assert!(schedule.ready(false));
        let slice = computer
            .run_slice(input(tick), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
        assert_eq!(slice.output.tick_interval_seconds, Some(0.));
        schedule.completed(slice.output.tick_interval_seconds);
    }

    let mut command = input(tick + 1);
    command.commands.push(Request {
        id: 2,
        command: Command::Manual {
            throttle: 0.,
            steering: [0.; 3],
        },
    });
    let slice = computer
        .run_slice(command, None, FUEL_PER_TICK, FUEL_PER_TICK)
        .unwrap();
    assert_eq!(slice.output.tick_interval_seconds, Some(0.));
    schedule.completed(slice.output.tick_interval_seconds);
    assert!(schedule.ready(false));

    let slice = computer
        .run_callback_slice(
            CallbackKind::Missile(7),
            input(tick + 2),
            None,
            Some(abi::MissileObservation {
                handle: 7,
                rotation: [0., 0., 0., 1.],
                ..Default::default()
            }),
            FUEL_PER_TICK,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(slice.callback_completed);
    assert_eq!(slice.callback, Some(CallbackKind::Missile(7)));
    assert_eq!(slice.output.missiles.len(), 1);
    assert!(schedule.ready(false));
    schedule.wake();
    assert!(schedule.ready(false));
}
