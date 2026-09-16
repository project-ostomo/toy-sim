//! Repeatable headless measurement of ship execution, excluding ECS, physics and sensors.
use std::time::{Duration, Instant};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{Command, ControllerRuntime, Input, Observation, Request};
use toy_sim_ships::*;
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let count: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(500);
    let ticks: usize = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(100);
    anyhow::ensure!(count > 0 && ticks > 0, "ship/tick counts must be positive");
    let start = Instant::now();
    let cat = Catalogue::builtin();
    let d = starter(EXAMPLE_CONTROLLER.to_vec()).compile(&cat)?;
    let mut runtime = ControllerRuntime::new()?;
    runtime.compile(d.blueprint.controller_bytes())?;
    let preparation = start.elapsed();
    let start = Instant::now();
    let mut fleet = Vec::with_capacity(count);
    for _ in 0..count {
        let mut h = ShipState::new(&d, &cat);
        h.test_loadout(&d, &cat);
        fleet.push((h, runtime.instantiate(d.blueprint.controller_bytes())?));
    }
    // Pay startup outside the steady-state measurement, without sleeping wall time.
    for (_, computer) in &mut fleet {
        computer.configure_hardware(&d, &cat);
        computer.advance(5.);
        runtime.boot(computer)?;
        computer.advance(0.2);
    }
    let instantiate = start.elapsed();
    let memory: usize = fleet.iter().map(|(_, c)| c.memory_bytes()).sum();
    let mut wasm = Duration::ZERO;
    let mut hardware = Duration::ZERO;
    let start = Instant::now();
    let mut callback_dt = vec![0.; count];
    let mut schedules = vec![toy_sim_ship_wasm::CallbackSchedule::default(); count];
    let mut directory_sent = vec![false; count];
    let mut completed = 0usize;
    let mut skipped = 0usize;
    for tick in 0..ticks {
        for (i, (h, c)) in fleet.iter_mut().enumerate() {
            callback_dt[i] += 0.1;
            let (mass, inertia) = h.mass_properties(&d, &cat);
            let obs = Observation {
                time_s: tick as f64 * 0.1,
                flight: abi::FlightState {
                    rotation: [0., 0., 0., 1.],
                    mass_kg: mass,
                    inertia: inertia.to_cols_array(),
                    radius_m: d.radius,
                    ..Default::default()
                },
                resources: h.resources(&d),
                inventory: h.inventory.quantities.clone(),
            };
            let a = Instant::now();
            let devices = h.snapshot(&d);
            hardware += a.elapsed();
            let input = Input {
                tick: tick as u64,
                dt: callback_dt[i],
                commands: if !directory_sent[i] {
                    vec![Request {
                        id: 0,
                        command: Command::Manual {
                            throttle: 0.25,
                            steering: [0.; 3],
                        },
                    }]
                } else {
                    vec![]
                },
                observation: obs.clone(),
                devices,
                ..Default::default()
            };
            let a = Instant::now();
            schedules[i].advance(0.1);
            c.advance(0.1);
            if runtime.boot(c)? {
                directory_sent[i] = false;
            }
            let output = if c.can_run()
                && schedules[i].ready(!input.commands.is_empty() || c.has_pending_input())
            {
                callback_dt[i] = 0.;
                directory_sent[i] = true;
                c.run(input)?
            } else {
                None
            };
            if let Some(output) = output {
                completed += 1;
                schedules[i].completed(output.tick_interval_seconds);
                h.apply_commands(&d, &output.devices)?;
            } else {
                skipped += 1;
            }
            wasm += a.elapsed();
            let a = Instant::now();
            let out = h.step(&d, &cat, 0.1);
            hardware += a.elapsed();
            std::hint::black_box(out);
        }
    }
    let elapsed = start.elapsed();
    println!(
        "{count} ships × {ticks} ticks; {} shared compiled WASM module",
        runtime.cached_modules()
    );
    println!(
        "Prepare/JIT {:.1} ms; instantiate {:.1} ms; guest linear memory {:.1} MiB (not process RSS)",
        preparation.as_secs_f64() * 1e3,
        instantiate.as_secs_f64() * 1e3,
        memory as f64 / 1048576.
    );
    println!(
        "Fleet tick {:.3} ms; controller {:.3} ms; hardware {:.3} ms",
        elapsed.as_secs_f64() * 1e3 / ticks as f64,
        wasm.as_secs_f64() * 1e3 / ticks as f64,
        hardware.as_secs_f64() * 1e3 / ticks as f64
    );
    println!("Excludes gravity, sensors, ECS, networking and rendering.");
    println!("{completed} completed callbacks; {skipped} booting/sleeping updates");
    Ok(())
}
