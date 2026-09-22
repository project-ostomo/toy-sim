# OpenSpaceGame

OpenSpaceGame is a prototype space-flight simulator written in Rust with Bevy. Ships and stations are assembled from parts with typed attachment nodes. The server compiles a coarse 1 m collision volume. A flight computer runs each ship. It is a sandboxed WebAssembly program that reads sensors and commands hardware through a fixed, allocation-free syscall interface. The simulation runs at a fixed 10 Hz. It covers Keplerian star systems, per-ship gravity, continuous collision detection, shields that radiate waste heat and consume coolant reserves, hull damage from heat, projectile weapons, docking, slip travel with natural capture, and an orbital navigation overlay. A shared catalogue supplies more than one million systems, with detailed celestial bodies generated when needed.

The simulation runs only in an authoritative server process. Every window, including the local debug application, is a network client that connects to a server over authenticated TCP ([docs/server-client.md](docs/server-client.md)). The server checkpoints the authoritative world, ship programs, ownership and account gas to SQLite; see [persistence](docs/persistence.md).

The project is in early prototyping. Interfaces change without compatibility layers (see [AGENTS.md](AGENTS.md)).

The default encounter has two patrol ships 1 km apart, one hostile, near Neris Anchorage. The player starts with an expedition patrol equipped for laser combat, micropulse propulsion and slip travel. The initial scenario places navigation installations in 3,000 systems across roughly 250 light-years. Public inhabitation changes as directory transmitters broadcast, go dark, move or are destroyed. The client includes a navigation map, navigation queue, weapon controls, industrial jobs, cargo and consumable inventory, and a docked hangar view. See [inhabited space](docs/inhabited-map.md), [industry](docs/industry.md), and [stations and navigation](docs/stations-navigation.md).

Travel planning accepts a maximum ship-destruction risk for the whole itinerary in parts per million. The default is 100 ppm, or one chance in 10,000. The planner chooses speeds and intermediate natural captures within that budget. Navigation beacons improve precision, while a substantial dispersion floor prevents arbitrarily slow travel from eliminating risk. Ships consume electrical charge and exotic fuel, and retain their galactic velocity after capture.

## Workspace map

The Cargo workspace ([Cargo.toml](Cargo.toml)) includes every package under `apps/` and `crates/`. `cargo run` with no package flag runs `apps/osg-debug`.

### Applications

| Package | Path | Purpose |
| --- | --- | --- |
| `osg-debug` | [apps/osg-debug](apps/osg-debug) | Local debug launcher. It starts `osg-server` as a child process, retains its state directory by default, connects over loopback TCP with a debug account, and opens the client UI. |
| `osg-ship-editor` | [apps/osg-ship-editor](apps/osg-ship-editor) | Interactive ship editor that saves `.ship` files and can launch `osg-debug` with the current design. |
| `osg-star-query` | [apps/osg-star-query](apps/osg-star-query) | Headless loader and query benchmark for the star catalogue. |

The server binary `osg-server` and the remote client binary `osg-client` live in their library crates below.

### Libraries

| Package | Path | Purpose |
| --- | --- | --- |
| `osg-spatial-bvh` | [crates/osg-spatial-bvh](crates/osg-spatial-bvh) | Shared immutable BVHs for visibility, sensors, collisions and world queries. |
| `osg-space` | [crates/osg-space](crates/osg-space) | `GalacticPosition`: signed 128-bit integer micrometre coordinates. |
| `osg-stars` | [crates/osg-stars](crates/osg-stars) | Star records, the flat `.stars` file format, shared BVH queries and the embedded Gaia catalogue. |
| `osg-ship-api` | [crates/osg-ship-api](crates/osg-ship-api) | `no_std` Rust C-ABI records, typed world/service imports, caller-owned output arrays and a small SDK. |
| `osg-ships` | [crates/osg-ships](crates/osg-ships) | Part catalogue, ship blueprints (`.ship`), design compilation, device and thermal models, weapon mechanisms, and `ShipState`, the hardware state record used to bootstrap and snapshot a ship. |
| `osg-ship-wasm` | [crates/osg-ship-wasm](crates/osg-ship-wasm) | Wasmtime host for flight computers: gas metering, booting, syscalls, world services, spatial publications, screen frames and separate `ship_display` instances. |
| `osg-ship-view` | [crates/osg-ship-view](crates/osg-ship-view) | Bevy 3D presentation used by the client and the editor: part meshes, plumes, shield fields, tracers and explosions. |
| `osg-ui` | [crates/osg-ui](crates/osg-ui) | Shared egui theme, embedded fonts, Bevy integration, instruments and programmable screen widgets. |
| `osg-example-controller` | [crates/osg-example-controller](crates/osg-example-controller) | Source of the standard flight computer firmware: hardware discovery, control allocation, braking rendezvous guidance, forecasts, weapons control and execution of the current host-owned navigation command. Strategic route search runs on the server. |
| `osg-model` | [crates/osg-model](crates/osg-model) | Shared serde types for the server, client and firmware: IDs, poses, tags, tracks, queries, frames, actions, debug commands, presentation records, travel and screen drawing lists. |
| `osg-protocol` | [crates/osg-protocol](crates/osg-protocol) | Application message framing and validation of client requests. |
| `osg-net` | [crates/osg-net](crates/osg-net) | Authenticated X25519/Ed25519 handshake, ChaCha20-Poly1305 records, Zstd compression and picomux multiplexing. |
| `osg-universe` | [crates/osg-universe](crates/osg-universe) | Shared deterministic system catalogue, lazy celestial generation, stable identities, Keplerian solver and atmosphere tables; independent of Bevy. |
| `osg-server` | [crates/osg-server](crates/osg-server) | The authoritative Bevy ECS simulation in private modules under `src/sim`, the 10 Hz simulation loop, TCP listener, asset streams, configuration, demo key provisioning, the server binary and the network benchmark example. |
| `osg-client` | [crates/osg-client](crates/osg-client) | Network client library and playback buffer. With the `ui` feature it adds the Bevy/egui client UI and the `osg-client` binary. |

### Other directories

- [assets/](assets/README.md): runtime assets (universe and star system TOML files, the bundled starter ship, models).
- [docs/](docs): topic guides, listed below.
- [tools/](tools): Scripts for Gaia downloads, catalogue conversion and firmware builds.
- [tests/fixtures/](tests/fixtures): a remote star system used by tests and a synthetic Gaia-shaped CSV.
- [.cargo/config.toml](.cargo/config.toml): linker arguments for `wasm32-unknown-unknown` builds (64 KiB guest stack, 8 MiB maximum memory).

## Setup

You need a Rust toolchain that supports edition 2024 and resolver 3. Bevy 0.19 is built with the `wayland`, `file_watcher`, `embedded_watcher` and `jpeg` features, so the usual Bevy system requirements for your platform apply to the client and the editor. The server uses Bevy without rendering.

The dev profile compiles workspace code at `opt-level = 1` and dependencies at `opt-level = 3`.

Optional tools:

- `python3` for the scripts in `tools/`. The `osg-stars` integration test `python_converter_fixture_matches_portable_loader` runs `tools/import_gaia.py`.
- The `wasm32-unknown-unknown` Rust target if you build controller firmware in Rust.

## Build and run

`osg-debug` starts the server executable that sits next to its own executable, so build both packages in the same profile first.

```sh
# Build the server and the debug launcher
cargo build -p osg-server -p osg-debug

# Start a local server and open the client UI with debug access
cargo run

# Same, with a custom ship blueprint for the first ship
cargo run -- --ship assets/ships/starter.ship

# Use a specific server executable
cargo run -- --server target/debug/osg-server

# Connect, wait for the first state frame and exit without opening a window
cargo run -- --check

# Ship editor, optionally opening a file
cargo run -p osg-ship-editor
cargo run -p osg-ship-editor -- path/to/design.ship

# Editor command-line utilities
cargo run -p osg-ship-editor -- --example starter.ship   # write the unarmed starter design
cargo run -p osg-ship-editor -- --validate assets/ships/starter.ship

# Dedicated server and remote clients (see docs/server-client.md)
cargo run -p osg-server -- --init demo          # writes demo/*.toml containing private keys
cargo run -p osg-server -- demo/server.toml
cargo run -p osg-client --features ui -- demo/client-a.toml

# Network benchmark (see docs/server-client.md#benchmark)
cargo build -p osg-server
cargo run -p osg-server --example benchmark -- --ships 16 --sessions 4

# Star catalogue benchmark
cargo run -p osg-star-query --release

# Tests
cargo test --workspace
```

`osg-debug` accepts `--ship PATH`, `--server EXECUTABLE`, `--state-dir PATH`, `--ephemeral` and `--check`. A persistent state directory retains the world and identity between runs; `--ephemeral` creates a disposable world. It:

1. Opens its state directory, creating the local server configuration and account keys when necessary. The server listens on loopback with an automatically allocated port.
2. Starts the server with `--ready-file` and `--shutdown-on-stdin-close`, and waits up to 60 s for the file to contain the listening address.
3. Connects with `osg_client::connect`, the same TCP path a remote client uses.
4. With `--check`, waits up to 10 s for the first state frame, fails if it contains no controlled ship, and prints the tick and ship count. Otherwise it opens the client UI.

When the launcher exits, it closes the server's standard input and allows up to 120 seconds for the final snapshot and shutdown. Persistent state directories are retained.

The client UI reads assets from the repository's `assets/` directory through a path fixed at compile time.

To try the current starting scenario independently of an older save, use a new
state directory:

```sh
cargo run -- --state-dir "$HOME/.local/state/openspacegame/mvp-world"
```

Subsequent launches with the same directory restore that world, including its
ships and exact saved firmware.

## What happens at startup

For a new world, the server builds the scenario in [bootstrap.rs](crates/osg-server/src/sim/bootstrap.rs). A saved world restores its authoritative data before accepting clients.

- The player starts on the day side of Helion I Neris, in a circular orbit 1,000 km outside Neris's slip exclusion radius. The client initially places the camera on the illuminated side.
- The first ship uses the configured blueprint, or `assets/ships/expedition-patrol.ship`. Additional configured accounts receive their own ships.
- A hostile patrol starts 1 km away with the same orbital velocity. Neris Anchorage is a nearby destination; navigation installations provide references for interstellar travel.
- Neris Anchorage provides manufacturing modules, cargo storage and starting industrial supplies. The player can manage its production remotely and dock for physical transfers.
- Initial scenario ships use their configured loadouts, with starter exotic fuel sized for approximately 300 light-years of uninterrupted slip. Ships manufactured later start empty and require fuel, coolant and battery charging.
- Flight computers boot and execute within their gas allowances. Exhausting an allowance suspends work until a later tick. Invalid accesses and other program faults still trigger a reboot.

Target marking, unmarking, starting fire and stopping fire are separate controls. Navigation commands never start weapons automatically.

## Controls

### Client

| Input | Effect |
| --- | --- |
| Double-click empty space | Align the controlled ship with the clicked direction |
| Right mouse drag | Orbit the camera around its focus, with smooth angular motion |
| Mouse wheel | Zoom |
| O | Toggle trajectories in the orbit overlay |
| Escape | Return the active camera to the controlled ship |
| `+` or `=` / `-` | Exposure up / down by half a stop |

Camera gestures that start over an egui window do not affect the scene. Double-click alignment changes the flight computer’s attitude command; navigation and weapon commands are issued through the selected-item controls and command queue.

The client combines the 3D scene and orbit HUD with a desktop interface: a left launcher, simulation clock, location indicator, sortable Overview, selected-item flight and weapon controls, and ship/navigation panels. The windows can be moved, resized and closed; Interface settings provide layout locking and reset. It automatically opens a view of the first controlled ship. See [the client UI guide](docs/server-client.md#the-client-ui).

### Ship editor

Assembly mode: click to place or select a part, right-drag to orbit, middle-drag to pan, wheel to zoom, R to rotate the placement, Delete to remove the selected part, Esc to cancel placement. See [docs/ship-editor.md](docs/ship-editor.md).

## Guides

| Guide | Topic |
| --- | --- |
| [docs/server-client.md](docs/server-client.md) | The server process, debug launcher, configuration, wire format, handshake, sensor and visual observations, presentation, display instances, docking and travel, client playback and UI, and the benchmark |
| [docs/industry.md](docs/industry.md) | Factories, material reservations, ship construction, cargo transfers and commissioning |
| [docs/persistence.md](docs/persistence.md) | SQLite world snapshots, restoration and durable ship programs |
| [docs/inhabited-map.md](docs/inhabited-map.md) | Political geography, dynamic public inhabitation and shared universe generation |
| [docs/stations-navigation.md](docs/stations-navigation.md) | Navigation controls, itinerary risk, natural capture, beacons, exotic fuel and docking |
| [docs/ships.md](docs/ships.md) | Parts, the catalogue, blueprints, compiled designs, hardware simulation, avionics and the standard firmware |
| [docs/ship-editor.md](docs/ship-editor.md) | Using the editor, its command-line modes and launching the debug client |
| [docs/ship-abi.md](docs/ship-abi.md) | Writing flight computer firmware, including world services and the `ship_display` entry point |
| [docs/mfds.md](docs/mfds.md) | Programmable screens, input events, display subscriptions and the MFD renderer |
| [docs/weapons.md](docs/weapons.md) | Weapon parts, target marking, firing, interlocks and control requests |
| [docs/collisions.md](docs/collisions.md) | Continuous collision detection, impacts, shields and destruction |
| [docs/rendezvous.md](docs/rendezvous.md) | The navigation request contract, the braking guidance law, states and tests |
| [docs/orbital-navigation.md](docs/orbital-navigation.md) | The two-body orbit overlay, camera framing and published paths |
| [docs/gaia-catalogue.md](docs/gaia-catalogue.md) | The star catalogue format, import tools and sky rendering |
| [docs/asset-workflow.md](docs/asset-workflow.md) | Editing universe, star system, catalogue, model and generated assets |

Directory notes: [assets/README.md](assets/README.md), [assets/models/parts/README.md](assets/models/parts/README.md), [crates/osg-stars/data/README.md](crates/osg-stars/data/README.md), [crates/osg-ui/data/fonts/README.md](crates/osg-ui/data/fonts/README.md).

## Architecture overview

`osg_ship_api::GAME_VERSION` is the single compatibility version, re-exported by
`osg-model`. It governs connection handshakes, firmware and saved worlds.
`osg-model` also defines `TICK_NS`, `TICK_DURATION`, `TICK_SECONDS` and
`TICK_RATE_HZ` for the shared 100 ms simulation cadence.

- **Processes.** `osg-server` owns the only simulation. Clients connect over TCP, authenticate with an account key, pin the server's public key, and exchange input and state frames ([docs/server-client.md](docs/server-client.md)). `osg-debug` is a client that starts its own server process. A debug account is an ordinary account with extra protocol capabilities; its commands use the same session and transport paths as any other command.
- **Coordinates.** Authoritative positions are `GalacticPosition` values in integer micrometres ([osg-space](crates/osg-space/src/lib.rs)). Code subtracts positions before converting to `f64`. The client renders with a floating origin.
- **Schedule.** The server's Bevy `App` runs one fixed 10 Hz step per update ([simulation.rs](crates/osg-server/src/sim/simulation.rs)). `FixedFirst` advances travel. `FixedUpdate` activates star systems, publishes world-service indexes, runs ship controllers and hardware (`PrepareBodies`), then gravity and drag (`Forces`). `FixedPostUpdate` applies firmware world actions, integrates bodies and collisions (`Integrate`), then advances celestial ephemerides (`Celestials`). `FixedLast` rebuilds the spatial index, runs sensor scans, and publishes current observations for each ship.
- **Ship hardware.** Inventory, hull, thermal state, avionics, device settings and sensor range are ECS components on the ship entity. Each installed part is its own entity with typed device components, and hardware systems step them ([hardware.rs](crates/osg-server/src/sim/hardware.rs)). `osg_ships::ShipState` builds those components when a ship spawns or resets, and snapshots them for presentation and collision damage.
- **Universe.** Clients and server share a compact catalogue of 1,001,760 system roots, authored definitions, deterministic generation and stable system/body keys. Each process resolves detailed systems independently and evaluates their Keplerian motion at the shared epoch. Clients receive the public inhabited set and gameplay state; celestial definitions are generated locally ([osg-universe](crates/osg-universe)).
- **Server activity.** Actual objects outside slip activate overlapping systems inside their gravitational influence bounds ([orrery/](crates/osg-server/src/sim/orrery)). Ships, stations, dark installations and wrecks can maintain activity. Inspection and slip intersection queries can resolve dormant definitions without activating ongoing simulation. Gravity queries use the relevant local systems.
- **Integration.** Plain rigid bodies use symplectic Euler with a split rotational integrator ([rotation.rs](crates/osg-server/src/sim/physics/rotation.rs)). Ships and projectiles are integrated inside the event-driven collision solver ([docs/collisions.md](docs/collisions.md)).
- **Firmware.** Flight programs run in Wasmtime with a gas budget. They see their own flight state, device readings, current contacts from their ship's sensors, and world services for travel ([docs/ship-abi.md](docs/ship-abi.md)).
- **Observations.** Clients receive exact current detections from authorized ships and independently filtered optical observations. Enabled IFF accompanies either detection; disabling IFF clears advertised identity immediately ([docs/server-client.md](docs/server-client.md#sensor-and-visual-observations)).
- **Presentation.** The client buffers state frames, consumes them in a 10 Hz client FixedUpdate, and interpolates hulls, the camera, effects and instruments between frames. Instruments come from records the firmware publishes. Screens come from a separate `ship_display` instance that runs only while a client subscribes ([docs/mfds.md](docs/mfds.md)).

## Known limitations

- Persistence restores durable ship data and programs, but native suspended VM stacks restart from boot after server recovery.
- Accounts are a fixed list in the server configuration. There is no registration, revocation or key rotation.
- Aerodynamic drag acts at the centre of mass with a constant coefficient and produces no lift or torque.
- Sensor measurements have range-dependent noise but no light-speed delay. Sensors are omnidirectional and use sphere occlusion.
- Client frame rate and crowded collision performance remain under investigation; the current build does not establish the intended MMO capacity. The network benchmark covers smaller workloads ([docs/server-client.md](docs/server-client.md#benchmark)).
