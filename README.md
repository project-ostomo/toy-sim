# toy-sim

toy-sim is a prototype space-flight simulator written in Rust with Bevy. Ships are built from box-shaped parts on a 0.1 m grid. A flight computer runs each ship. It is a sandboxed WebAssembly program that reads sensors and commands hardware through a fixed, allocation-free syscall interface. The simulation runs at a fixed 10 Hz. It covers Keplerian star systems, per-ship gravity, continuous collision detection, shields that radiate waste heat and consume coolant reserves, hull damage from heat, projectile weapons, docking, gates, slipdrives, a sky rendered from an embedded catalogue of one million Gaia DR3 stars, and an orbital navigation overlay.

The simulation runs only in an authoritative server process. Every window, including the local debug application, is a network client that connects to a server over authenticated TCP ([docs/server-client.md](docs/server-client.md)). The server keeps all state in memory; there is no persistence.

The project is in early prototyping. Interfaces change without compatibility layers (see [AGENTS.md](AGENTS.md)).

## Workspace map

The Cargo workspace ([Cargo.toml](Cargo.toml)) includes every package under `apps/` and `crates/`. `cargo run` with no package flag runs `apps/toy-sim-debug`.

### Applications

| Package | Path | Purpose |
| --- | --- | --- |
| `toy-sim-debug` | [apps/toy-sim-debug](apps/toy-sim-debug) | Local debug launcher. It starts `toy-sim-server` as a child process with a temporary configuration, connects to it over loopback TCP with a debug account, and opens the client UI. |
| `toy-ship-editor` | [apps/toy-ship-editor](apps/toy-ship-editor) | Interactive ship editor that saves `.ship` files and can launch `toy-sim-debug` with the current design. |
| `toy-star-query` | [apps/toy-star-query](apps/toy-star-query) | Headless loader and query benchmark for the star catalogue. |

The server binary `toy-sim-server` and the remote client binary `toy-sim-client` live in their library crates below.

### Libraries

| Package | Path | Purpose |
| --- | --- | --- |
| `toy-sim-space` | [crates/toy-sim-space](crates/toy-sim-space) | `GalacticPosition`: signed 128-bit integer micrometre coordinates. |
| `toy-sim-stars` | [crates/toy-sim-stars](crates/toy-sim-stars) | Star records, the flat `.stars` file format, luminosity-bucketed KD-tree queries and the embedded Gaia catalogue. |
| `toy-sim-ship-api` | [crates/toy-sim-ship-api](crates/toy-sim-ship-api) | `no_std` ship ABI version 13: fixed C records, constants, raw imports (including the postcard-based `world_query`/`world_command`) and a small SDK. Also holds the generated C header and AssemblyScript bindings. |
| `toy-sim-ships` | [crates/toy-sim-ships](crates/toy-sim-ships) | Part catalogue, ship blueprints (`.ship`), design compilation, device and thermal models, weapon mechanisms, and `ShipState`, the hardware state record used to bootstrap and snapshot a ship. |
| `toy-sim-ship-wasm` | [crates/toy-sim-ship-wasm](crates/toy-sim-ship-wasm) | Wasmtime host for flight computers: gas metering, booting, syscalls, world services, spatial publications, screen frames and separate `ship_display` instances. |
| `toy-sim-ship-view` | [crates/toy-sim-ship-view](crates/toy-sim-ship-view) | Bevy 3D presentation used by the client and the editor: part meshes, plumes, shield fields, tracers and explosions. |
| `toy-sim-ui` | [crates/toy-sim-ui](crates/toy-sim-ui) | Shared egui theme, embedded fonts, Bevy integration, instruments and programmable screen widgets. |
| `toy-sim-example-controller` | [crates/toy-sim-example-controller](crates/toy-sim-example-controller) | Source of the standard flight computer firmware: hardware discovery, control allocation, braking rendezvous guidance, forecasts, weapons control and a travel planner with a multi-gate route search. |
| `toy-sim-model` | [crates/toy-sim-model](crates/toy-sim-model) | Shared serde types for the server, client and firmware: IDs, poses, tags, tracks, queries, frames, actions, debug commands, presentation records, travel and screen drawing lists. |
| `toy-sim-protocol` | [crates/toy-sim-protocol](crates/toy-sim-protocol) | `TSF1` application message framing (protocol version 3), sections and validation limits. |
| `toy-sim-net` | [crates/toy-sim-net](crates/toy-sim-net) | Authenticated X25519/Ed25519 handshake, ChaCha20-Poly1305 records, Zstd compression and picomux multiplexing. |
| `toy-sim-intel` | [crates/toy-sim-intel](crates/toy-sim-intel) | Measurements, immutable track snapshots and metered track queries. |
| `toy-sim-universe` | [crates/toy-sim-universe](crates/toy-sim-universe) | Celestial definitions, Keplerian solver, system index, atmosphere tables and replicated system assets; independent of Bevy. |
| `toy-sim-server` | [crates/toy-sim-server](crates/toy-sim-server) | The authoritative Bevy ECS simulation in private modules under `src/sim`, the 10 Hz simulation loop, TCP listener, asset streams, configuration, demo key provisioning, the server binary and the network benchmark example. |
| `toy-sim-client` | [crates/toy-sim-client](crates/toy-sim-client) | Network client library and playback buffer. With the `ui` feature it adds the Bevy/egui client UI and the `toy-sim-client` binary. |

### Other directories

- [assets/](assets/README.md): runtime assets (universe and star system TOML files, the bundled starter ship, models).
- [docs/](docs): topic guides, listed below.
- [tools/](tools): Python scripts for Gaia downloads, Gaia conversion and ABI binding generation, and a standalone compression experiment.
- [vendor/picomux](vendor/picomux): picomux 0.2.1 with project-local stream, frame and queue bounds, patched in through `[patch.crates-io]` ([docs/server-client.md](docs/server-client.md#multiplexing-vendored-picomux)).
- [tests/fixtures/](tests/fixtures): a remote star system used by tests and a synthetic Gaia-shaped CSV.
- [.cargo/config.toml](.cargo/config.toml): linker arguments for `wasm32-unknown-unknown` builds (64 KiB guest stack, 1 MiB maximum memory).

## Setup

You need a Rust toolchain that supports edition 2024 and resolver 3. Bevy 0.19 is built with the `wayland`, `file_watcher`, `embedded_watcher` and `jpeg` features, so the usual Bevy system requirements for your platform apply to the client and the editor. The server uses Bevy without rendering.

The dev profile compiles workspace code at `opt-level = 1` and dependencies at `opt-level = 3`.

Optional tools:

- `python3` for the scripts in `tools/`. The `toy-sim-stars` integration test `python_converter_fixture_matches_portable_loader` runs `tools/import_gaia.py`.
- The `wasm32-unknown-unknown` Rust target if you build controller firmware in Rust.
- A C compiler that targets `wasm32` if you build C controllers against [ship.h](crates/toy-sim-ship-api/include/ship.h).

## Build and run

`toy-sim-debug` starts the server executable that sits next to its own executable, so build both packages in the same profile first.

```sh
# Build the server and the debug launcher
cargo build -p toy-sim-server -p toy-sim-debug

# Start a local server and open the client UI with debug access
cargo run

# Same, with a custom ship blueprint for the first ship
cargo run -- --ship assets/ships/starter.ship

# Use a specific server executable
cargo run -- --server target/debug/toy-sim-server

# Connect, wait for the first state frame and exit without opening a window
cargo run -- --check

# Ship editor, optionally opening a file
cargo run -p toy-ship-editor
cargo run -p toy-ship-editor -- path/to/design.ship

# Editor command-line utilities
cargo run -p toy-ship-editor -- --example starter.ship   # write the unarmed starter design
cargo run -p toy-ship-editor -- --validate assets/ships/starter.ship

# Dedicated server and remote clients (see docs/server-client.md)
cargo run -p toy-sim-server -- --init demo          # writes demo/*.toml containing private keys
cargo run -p toy-sim-server -- demo/server.toml
cargo run -p toy-sim-client --features ui -- demo/client-a.toml

# Network benchmark (see docs/server-client.md#benchmark)
cargo build -p toy-sim-server
cargo run -p toy-sim-server --example benchmark -- --ships 16 --sessions 4

# Star catalogue benchmark
cargo run -p toy-star-query --release

# Tests
cargo test --workspace
```

`toy-sim-debug` accepts `--ship PATH`, `--server EXECUTABLE` and `--check`, and rejects anything else. It:

1. Creates a private temporary directory and writes a server configuration with a random server key, one random account, `debug_account` set to that account, `listen = "127.0.0.1:0"`, and the canonical `--ship` path if given.
2. Starts the server with `--ready-file` and `--shutdown-on-stdin-close`, and waits up to 60 s for the file to contain the listening address.
3. Connects with `toy_sim_client::connect`, the same TCP path a remote client uses.
4. With `--check`, waits up to 10 s for the first state frame, fails if it contains no controlled ship, and prints the tick and ship count. Otherwise it opens the client UI with a one-frame playback buffer.

When the launcher exits, it closes the server's standard input, waits up to 5 s for the server to stop, kills it if needed, and removes the temporary directory.

The client UI reads assets from the repository's `assets/` directory through a path fixed at compile time.

## What happens at startup

The server builds its world in [bootstrap.rs](crates/toy-sim-server/src/sim/bootstrap.rs) from the scenario constants in [scenario.rs](crates/toy-sim-server/src/sim/scenario.rs):

- The universe is the Helion system ([assets/stars/helion.star.toml](assets/stars/helion.star.toml)).
- The first ship, "Patrol ship", starts on the day side of Helion I Neris, in a circular orbit 40,000 km above the surface and facing along its velocity. Its tangential direction comes from a seeded sequence (seed 42), and the client initially places the camera on the illuminated side. Its design is the `ship` blueprint from the server configuration, or `assets/ships/micropulse-patrol.ship`.
- A second patrol ship, "Hostile patrol 001", spawns 1 km away with the same initial orbital velocity, pointed at the player. It aims and fires after its computer boots.
- The first configured account controls the explorer. Each further account gets an "Explorer *n*" ship 1,000 m further along +Y. The hostile patrol belongs to a separate account that no configuration knows.
- Every player ship has a slipdrive. The default single-player scenario contains the two patrol ships.
- Every ship receives a test loadout: a full battery, generator fuel, ammunition and propellant filling the remaining storage.
- Each flight computer spends its first 5 simulated seconds booting (a 50-tick startup reserve) before it runs.

When the explorer engages another ship with its weapons, the server orders that ship to engage the explorer in return, using the same engagement request a client sends.

## Controls

### Client

| Input | Effect |
| --- | --- |
| Left Shift / Left Ctrl (hold) | Raise / lower manual throttle at 50% per second of real time |
| W / S | Pitch (S is the positive X steering axis) |
| A / D | Yaw (A is the positive Y steering axis) |
| Q / E | Roll (Q is the positive Z steering axis) |
| Left or right mouse drag | Orbit the camera around its focus |
| Mouse wheel | Zoom |
| O | Toggle trajectories in the orbit overlay |
| Escape | Return the active camera to the controlled ship |
| `+` or `=` / `-` | Exposure up / down by half a stop |

Keyboard input is ignored while an egui widget has keyboard focus. Mouse drags that start over a window do not move the camera. Flight keys act on the automatically selected controlled ship. Manual throttle and steering are sent to the server as `Manual` commands, which pause travel and reach the flight computer as requests ([docs/rendezvous.md](docs/rendezvous.md)).

The client currently shows the 3D scene, contact and celestial HUD labels, orbit overlays, and one "Hello world" egui window. It automatically opens a view of the first controlled ship. The former control, debug, browser and MFD windows have been removed as the starting point for a new UI ([docs/server-client.md](docs/server-client.md#the-client-ui)).

### Ship editor

Assembly mode: click to place or select a part, right-drag to orbit, middle-drag to pan, wheel to zoom, R to rotate the placement, Delete to remove the selected part, Esc to cancel placement. See [docs/ship-editor.md](docs/ship-editor.md).

## Guides

| Guide | Topic |
| --- | --- |
| [docs/server-client.md](docs/server-client.md) | The server process, debug launcher, configuration, wire format, handshake, intelligence model, presentation, display instances, docking and travel, client playback and UI, and the benchmark |
| [docs/ships.md](docs/ships.md) | Parts, the catalogue, blueprints, compiled designs, hardware simulation, avionics and the standard firmware |
| [docs/ship-editor.md](docs/ship-editor.md) | Using the editor, its command-line modes and launching the debug client |
| [docs/ship-abi.md](docs/ship-abi.md) | Writing flight computer firmware against ABI version 13, including world services and the `ship_display` entry point |
| [docs/mfds.md](docs/mfds.md) | Programmable screens, input events, display subscriptions and the MFD renderer |
| [docs/weapons.md](docs/weapons.md) | Weapon parts, charging, firing, interlocks and the engagement request |
| [docs/collisions.md](docs/collisions.md) | Continuous collision detection, impacts, shields and destruction |
| [docs/rendezvous.md](docs/rendezvous.md) | The navigation request contract, the braking guidance law, states and tests |
| [docs/pursuit-trajectory-design.md](docs/pursuit-trajectory-design.md) | Historical proposal for trajectory presentation, written for the earlier pursuit law |
| [docs/orbital-navigation.md](docs/orbital-navigation.md) | The two-body orbit overlay, camera framing and published paths |
| [docs/gaia-catalogue.md](docs/gaia-catalogue.md) | The star catalogue format, import tools and sky rendering |
| [docs/asset-workflow.md](docs/asset-workflow.md) | Editing universe, star system, catalogue, model and generated assets |
| [docs/ship-step-profile.md](docs/ship-step-profile.md) | Historical ship-step measurements and the current fleet profiling test |

Directory notes: [assets/README.md](assets/README.md), [assets/models/parts/README.md](assets/models/parts/README.md), [crates/toy-sim-stars/data/README.md](crates/toy-sim-stars/data/README.md), [crates/toy-sim-ui/data/fonts/README.md](crates/toy-sim-ui/data/fonts/README.md).

## Architecture overview

- **Processes.** `toy-sim-server` owns the only simulation. Clients connect over TCP, authenticate with an account key, pin the server's public key, and exchange input and state frames ([docs/server-client.md](docs/server-client.md)). `toy-sim-debug` is a client that starts its own server process. A debug account is an ordinary account with extra protocol capabilities; its commands use the same session and transport paths as any other command.
- **Coordinates.** Authoritative positions are `GalacticPosition` values in integer micrometres ([toy-sim-space](crates/toy-sim-space/src/lib.rs)). Code subtracts positions before converting to `f64`. The client renders with a floating origin.
- **Schedule.** The server's Bevy `App` runs one fixed 10 Hz step per update ([simulation.rs](crates/toy-sim-server/src/sim/simulation.rs)). `FixedFirst` advances travel. `FixedUpdate` activates star systems, publishes world-service indexes, runs ship controllers and hardware (`PrepareBodies`), then gravity and drag (`Forces`). `FixedPostUpdate` applies firmware world actions, integrates bodies and collisions (`Integrate`), then advances celestial ephemerides (`Celestials`). `FixedLast` rebuilds the spatial index, runs sensor scans, and then runs intelligence: acquisition, coasting, fusion and snapshot publication.
- **Ship hardware.** Inventory, hull, thermal state, avionics, device settings and sensor range are ECS components on the ship entity. Each installed part is its own entity with typed device components, and hardware systems step them ([hardware.rs](crates/toy-sim-server/src/sim/hardware.rs)). `toy_sim_ships::ShipState` builds those components when a ship spawns or resets, and snapshots them for presentation and collision damage.
- **Orrery.** Each star system is a fixed star plus Keplerian bodies. Systems are activated when a ship's motion segment enters their gravitational influence radius ([orrery/](crates/toy-sim-server/src/sim/orrery)). Gravity from a system applies only inside that radius.
- **Integration.** Plain rigid bodies use symplectic Euler with a split rotational integrator ([rotation.rs](crates/toy-sim-server/src/sim/physics/rotation.rs)). Ships and projectiles are integrated inside the event-driven collision solver ([docs/collisions.md](docs/collisions.md)).
- **Firmware.** Flight programs run in Wasmtime with a gas budget. They see their own flight state, device readings, fused contacts from their ship's information group, and world services for travel ([docs/ship-abi.md](docs/ship-abi.md)).
- **Intelligence.** Clients receive fused tracks from information groups they have joined, exact reports shared by group members or IFF broadcasts, private telemetry and presentation for ships they control, and presentation for tracks whose identity the group already knows ([docs/server-client.md](docs/server-client.md#observations-and-intelligence)).
- **Presentation.** The client buffers state frames, plays them back on a steady presentation clock, and interpolates hulls, the camera, effects and instruments between frames. Instruments come from records the firmware publishes. Screens come from a separate `ship_display` instance that runs only while a client subscribes ([docs/mfds.md](docs/mfds.md)).

## Known limitations

- There is no save-game format and no persistence. The startup scenario is fixed in code, and the server keeps accounts' group keys, tracks and sessions in memory. Every server start creates a new world ID.
- Accounts are a fixed list in the server configuration. There is no registration, revocation or key rotation.
- No protocol action transfers control of a ship to another account.
- Only the Helion system is loaded. [assets/stars/sol.star.toml](assets/stars/sol.star.toml) exists but is not used by the server.
- Navigation guidance brakes to arrive within 2 m and 0.5 m/s of its aim point. It does not keep station afterwards ([docs/rendezvous.md](docs/rendezvous.md)).
- Aerodynamic drag acts at the centre of mass with a constant coefficient and produces no lift or torque.
- Sensor measurements have range-dependent noise but no light-speed delay. Sensors are omnidirectional and use sphere occlusion.
- The benchmark has not been used to show scalability beyond its small cases ([docs/server-client.md](docs/server-client.md#benchmark)).
