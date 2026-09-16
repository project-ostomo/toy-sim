# toy-sim

toy-sim is a prototype space-flight simulator written in Rust with Bevy. Ships are built from box-shaped parts on a 0.1 m grid. A flight computer runs each ship. It is a sandboxed WebAssembly program that reads sensors and commands hardware through a fixed, allocation-free syscall interface. The simulation runs at a fixed 10 Hz. It covers Keplerian star systems, per-ship gravity, continuous collision detection, shields that radiate waste heat and consume coolant reserves, hull damage from heat, projectile weapons, a sky rendered from an embedded catalogue of one million Gaia DR3 stars, and an orbital navigation overlay.

The project is in early prototyping. Interfaces change without compatibility layers (see [AGENTS.md](AGENTS.md)).

## Workspace map

The Cargo workspace ([Cargo.toml](Cargo.toml)) includes every package under `apps/` and `crates/`. `cargo run` with no package flag runs `apps/toy-sim`.

### Applications

| Package | Path | Purpose |
| --- | --- | --- |
| `toy-sim` | [apps/toy-sim](apps/toy-sim) | The simulator: orrery, physics, collisions, sensors, ships, GUI and rendering. |
| `toy-ship-editor` | [apps/toy-ship-editor](apps/toy-ship-editor) | Interactive ship editor that saves `.ship` files and can launch the simulator. |
| `toy-ship-bench` | [apps/toy-ship-bench](apps/toy-ship-bench) | Headless benchmark of controller and hardware execution for a fleet. |
| `toy-star-query` | [apps/toy-star-query](apps/toy-star-query) | Headless loader and query benchmark for the star catalogue. |

### Libraries

| Package | Path | Purpose |
| --- | --- | --- |
| `toy-sim-space` | [crates/toy-sim-space](crates/toy-sim-space) | `GalacticPosition`: signed 128-bit integer micrometre coordinates. |
| `toy-sim-stars` | [crates/toy-sim-stars](crates/toy-sim-stars) | Star records, the flat `.stars` file format, luminosity-bucketed KD-tree queries and the embedded Gaia catalogue. |
| `toy-sim-ship-api` | [crates/toy-sim-ship-api](crates/toy-sim-ship-api) | `no_std` ship ABI version 11: fixed C records, constants, raw imports and a small SDK. Also holds the generated C header and AssemblyScript bindings. |
| `toy-sim-ships` | [crates/toy-sim-ships](crates/toy-sim-ships) | Part catalogue, ship blueprints (`.ship`), design compilation, hardware interpretation, thermal model and weapon mechanisms. |
| `toy-sim-ship-wasm` | [crates/toy-sim-ship-wasm](crates/toy-sim-ship-wasm) | Wasmtime host for flight computers: gas metering, booting, syscalls, spatial publications and screen frames. |
| `toy-sim-ship-view` | [crates/toy-sim-ship-view](crates/toy-sim-ship-view) | Bevy/egui presentation shared by the simulator and editor: part meshes, plumes, shield fields, tracers, explosions, instruments and programmable screens. |
| `toy-sim-example-controller` | [crates/toy-sim-example-controller](crates/toy-sim-example-controller) | Source of the standard flight computer firmware: hardware discovery, control allocation, pursuit guidance, forecasts and weapons control. |

### Other directories

- [assets/](assets/README.md): runtime assets loaded by Bevy (universe and star system TOML files, the bundled starter ship, models).
- [docs/](docs): topic guides, listed below.
- [tools/](tools): Python scripts for Gaia downloads, Gaia conversion and ABI binding generation.
- [tests/fixtures/](tests/fixtures): a remote star system used by tests and a synthetic Gaia-shaped CSV.
- [.cargo/config.toml](.cargo/config.toml): linker arguments for `wasm32-unknown-unknown` builds (64 KiB guest stack, 1 MiB maximum memory).

## Setup

You need a Rust toolchain that supports edition 2024 and resolver 3. Bevy 0.19 is built with the `wayland`, `file_watcher`, `embedded_watcher` and `jpeg` features, so the usual Bevy system requirements for your platform apply. The renderer requests the `FLOAT32_FILTERABLE` wgpu feature for the HDR star skybox.

Native debug builds link Bevy dynamically through `bevy_dylib`. Release builds do not. The dev profile compiles workspace code at `opt-level = 1` and dependencies at `opt-level = 3`.

Optional tools:

- `python3` for the scripts in `tools/`. The `toy-sim-stars` integration test `python_converter_fixture_matches_portable_loader` runs `tools/import_gaia.py`.
- The `wasm32-unknown-unknown` Rust target if you build controller firmware in Rust.
- A C compiler that targets `wasm32` if you build C controllers against [ship.h](crates/toy-sim-ship-api/include/ship.h).

## Build and run

```sh
# Simulator with the built-in armed starter ship
cargo run

# Simulator with a ship file
cargo run -p toy-sim -- --ship assets/ships/starter.ship

# Ship editor, optionally opening a file
cargo run -p toy-ship-editor
cargo run -p toy-ship-editor -- path/to/design.ship

# Editor command-line utilities
cargo run -p toy-ship-editor -- --example starter.ship   # write the unarmed starter design
cargo run -p toy-ship-editor -- --validate assets/ships/starter.ship

# Headless benchmarks (see docs/ship-step-profile.md)
cargo run -p toy-ship-bench --release -- 500 100
cargo run -p toy-star-query --release

# Tests
cargo test --workspace
```

`toy-sim --ship <path>` loads the blueprint, compiles it against the built-in catalogue and validates its controller before the window opens. Both GUI applications read assets from the repository's `assets/` directory through a path fixed at compile time.

## What happens at startup

The initial scenario is defined in code ([scenario.rs](apps/toy-sim/src/scenario.rs)):

- The universe is [assets/universe.toml](assets/universe.toml), which lists one system, [Helion](assets/stars/helion.star.toml).
- The player ship, "Orbital explorer", starts in a circular orbit 40,000 km above the planet Helion I Neris. The orbit plane comes from a seeded sequence (seed 42). Without `--ship`, the design is the armed starter from `toy_sim_ships::armed_starter()`.
- One traffic ship, "Traffic 001", spawns 100 km from the player on the same orbit. It uses the armed starter with the standard firmware. It is commanded to select the player as its target and engage navigation, so it begins pursuing the player.
- Every ship receives a test loadout: a full battery, generator fuel, ammunition and propellant filling the remaining storage.
- Each flight computer spends its first 5 simulated seconds booting (a 50-tick startup reserve) before it runs.

## Controls

### Simulator

| Input | Effect |
| --- | --- |
| Left Shift / Left Ctrl (hold) | Raise / lower manual throttle at 50% per second of real time |
| W / S | Pitch (S is the positive X steering axis) |
| A / D | Yaw (A is the positive Y steering axis) |
| Q / E | Roll (Q is the positive Z steering axis) |
| Left mouse drag | Orbit the camera around its focus |
| Mouse wheel | Zoom |
| O | Toggle the orbital navigation overlay |
| `+` or `=` / `-` | Exposure up / down by half a stop |

Keyboard input is ignored while an egui widget has keyboard focus. Mouse drags that start over a window do not move the camera. Steering is relative to the ship's configured control orientation. Manual throttle and steering are sent to the flight computer as requests, and a change cancels active guidance ([docs/rendezvous.md](docs/rendezvous.md)).

Main windows: Ship, Camera target, Exposure, Time (pause, 1×, 10×), Diagnostics, Inventory, Hardware diagnostics, Flight computer, Contacts, Weapons, Navigation, Ship systems, Sensor debug, Universe, and one window per programmable screen the firmware defines.

### Ship editor

Assembly mode: click to place or select a part, right-drag to orbit, middle-drag to pan, wheel to zoom, R to rotate the placement, Delete to remove the selected part, Esc to cancel placement. See [docs/ship-editor.md](docs/ship-editor.md).

## Guides

| Guide | Topic |
| --- | --- |
| [docs/ships.md](docs/ships.md) | Parts, the catalogue, blueprints, compiled designs, hardware simulation, avionics and the standard firmware |
| [docs/ship-editor.md](docs/ship-editor.md) | Using the editor, its command-line modes and launching the simulator |
| [docs/ship-abi.md](docs/ship-abi.md) | Writing flight computer firmware against ABI version 11 |
| [docs/mfds.md](docs/mfds.md) | Programmable screens, input events and the MFD renderer |
| [docs/weapons.md](docs/weapons.md) | Weapon parts, charging, firing, interlocks and the engagement request |
| [docs/collisions.md](docs/collisions.md) | Continuous collision detection, impacts, shields and destruction |
| [docs/rendezvous.md](docs/rendezvous.md) | The navigation request contract, requirements, states and tests |
| [docs/pursuit-trajectory-design.md](docs/pursuit-trajectory-design.md) | The pursuit guidance law and its forecast |
| [docs/orbital-navigation.md](docs/orbital-navigation.md) | The two-body orbit overlay, camera framing and published paths |
| [docs/gaia-catalogue.md](docs/gaia-catalogue.md) | The star catalogue format, import tools and sky rendering |
| [docs/asset-workflow.md](docs/asset-workflow.md) | Editing universe, star system, catalogue, model and generated assets |
| [docs/ship-step-profile.md](docs/ship-step-profile.md) | Built-in timing and reproducible profiling commands |

Directory notes: [assets/README.md](assets/README.md), [assets/models/parts/README.md](assets/models/parts/README.md), [apps/toy-sim/src/vessel/README.md](apps/toy-sim/src/vessel/README.md), [crates/toy-sim-stars/data/README.md](crates/toy-sim-stars/data/README.md), [crates/toy-sim-ship-view/data/fonts/README.md](crates/toy-sim-ship-view/data/fonts/README.md).

## Architecture overview

- **Coordinates.** Authoritative positions are `GalacticPosition` values in integer micrometres ([toy-sim-space](crates/toy-sim-space/src/lib.rs)). Code subtracts positions before converting to `f64`. Rendering uses a floating origin ([precision.rs](apps/toy-sim/src/precision.rs)).
- **Schedule.** `Time<Fixed>` runs at 10 Hz ([simulation.rs](apps/toy-sim/src/simulation.rs)). `FixedUpdate` runs ship controllers and hardware (`PrepareBodies`), then gravity and drag (`Forces`), then dock aggregation (`GatherForces`). `FixedPostUpdate` integrates bodies and collisions (`Integrate`), then advances celestial ephemerides (`Celestials`). `FixedLast` rebuilds the spatial index and runs sensor scans.
- **Orrery.** Each star system is a fixed star plus Keplerian bodies. Systems are activated when a ship's motion segment enters their gravitational influence radius ([orrery/](apps/toy-sim/src/orrery)). Gravity from a system applies only inside that radius.
- **Integration.** Plain rigid bodies use symplectic Euler with a split rotational integrator ([physics/rotation.rs](apps/toy-sim/src/physics/rotation.rs)). Ships, projectiles and dock parents are integrated inside the event-driven collision solver ([docs/collisions.md](docs/collisions.md)).
- **Ships.** Hardware is simulated natively in `toy-sim-ships`. Firmware runs in Wasmtime with a gas budget and can only see its own flight state, device readings and sensor contacts ([docs/ship-abi.md](docs/ship-abi.md)).
- **Presentation.** Hulls, the camera and effects use interpolated `PresentationPose` values between ticks. Instruments, overlays and screens are drawn by the client from records the firmware publishes.

## Known limitations

- There is no save-game format. The startup scenario is fixed in code.
- Only one star system is listed in `assets/universe.toml`. [assets/stars/sol.star.toml](assets/stars/sol.star.toml) exists but is not loaded.
- Navigation guidance flies a full-thrust pursuit pass. It does not match velocity or hold a stand-off distance ([docs/rendezvous.md](docs/rendezvous.md)).
- Aerodynamic drag acts at the centre of mass with a constant coefficient and produces no lift or torque.
- Sensors are omnidirectional, report exact relative position and velocity, and use sphere occlusion.
