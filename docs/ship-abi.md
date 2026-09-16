# Ship controller ABI (version 11)

Every ship runs a flight computer program: a WebAssembly module that the host calls once per scheduled callback. The program talks to the host only through the imports of module `ship_v11`. It exchanges fixed-size little-endian C records and never allocates or serializes across the boundary.

- Record definitions and constants: [crates/toy-sim-ship-api/src/abi.rs](../crates/toy-sim-ship-api/src/abi.rs)
- Rust helpers: [crates/toy-sim-ship-api/src/sdk.rs](../crates/toy-sim-ship-api/src/sdk.rs)
- Generated C header: [crates/toy-sim-ship-api/include/ship.h](../crates/toy-sim-ship-api/include/ship.h)
- Generated AssemblyScript bindings: [crates/toy-sim-ship-api/bindings/ship.ts](../crates/toy-sim-ship-api/bindings/ship.ts)
- Host implementation: [crates/toy-sim-ship-wasm](../crates/toy-sim-ship-wasm)

For the hardware that devices represent, see [ships.md](ships.md). Screen drawing is covered in [mfds.md](mfds.md), and weapons in [weapons.md](weapons.md).

## Module requirements

`ControllerRuntime::compile` accepts a module when all of the following hold:

- It is at most 1 MiB.
- Every import comes from module `ship_v11` and is one of the names in `abi::IMPORTS`.
- It exports `memory`: 32-bit, not shared, with an initial size of at most 16 pages.
- It exports `ship_tick` with no parameters and no results.
- It exports `ship_api_version` with no parameters and one result.

Instantiation (at boot, or in `validate_program`) also calls `ship_api_version` and requires it to return `10`. Store limits: one instance, one memory up to 1 MiB, 4096 table elements, and a 128 KiB WebAssembly stack. Compiled modules are cached by their bytes, so identical programs share one compiled module.

For `wasm32-unknown-unknown` builds, [.cargo/config.toml](../.cargo/config.toml) passes `-zstack-size=65536` and `--max-memory=1048576` to the linker.

## Lifecycle

```
instantiate ──► booting ──(50,000,000 gas accumulated, boot slot)──► running
                   ▲                                                   │
                   └──────────── fault or reboot (gas reset to 0) ─────┘
```

- **Gas.** A computer accrues 10,000,000 gas per simulated second while its avionics are powered. While booting, the reserve can reach `BOOT_GAS` (50,000,000, which is 5 s or 50 ticks). While running, it is capped at `RESERVE_CAPACITY` (4,000,000).
- **Boot.** When the reserve is full, the host spends the whole `BOOT_GAS` and instantiates the module. At most 64 boots happen per simulation tick. A failed instantiation still spends the gas and records a fault.
- **Callback admission.** A running computer is called when it holds at least `CALLBACK_START_GAS` (2,000,000) and either its interval has elapsed or it has pending requests or screen events.
- **Instruction limit.** Each callback runs with `min(gas, 1,000,000)` instruction fuel (`FUEL_PER_TICK`). Syscalls charge the same account and can lower the remaining fuel. Running out of fuel traps.
- **Faults.** A trap, a host error, or device commands the hardware rejects will reboot the computer. The instance is dropped, gas returns to zero, and all session state is cleared: instruments, spatial publications, tracks, pinned snapshots, screens and pending screen events. The fault message is kept until the next successful boot. Pending requests survive the reboot.

### Atomic callbacks

Each callback edits a private copy of the session. Only when `ship_tick` returns normally are these committed: the device writes, request replies, screen frames, instrument records, spatial publications and admitted scans. If the callback traps, all of it is discarded and the hardware's command settings are reset.

Hardware keeps the last committed settings between callbacks. A sleeping program therefore keeps its throttles and torques.

### Scheduling

`tick_set_interval(seconds)` sets how long the host waits after this callback completes before calling again. The value persists until changed. Zero means every tick. New requests or screen events wake the program early. The simulation tick is 0.1 s.

## Calling conventions

- Pointers are `u32` offsets into the exported memory. Every buffer length must equal the record size exactly, or the call returns `ERR_BUFFER`. A failed call writes nothing.
- Every import costs `CALL_GAS` (100). Copying records into or out of guest memory costs one gas per 8 bytes. Some calls add their own charges, listed below.
- Imports return `0` (or a count) on success and a negative error otherwise:

| Code | Name | Meaning |
| --- | --- | --- |
| −1 | `ERR_GAS` | Not enough gas for the call |
| −2 | `ERR_BUFFER` | Wrong length or out-of-bounds pointer |
| −3 | `ERR_ARGUMENT` | Invalid value, index or combination |
| −4 | `ERR_UNAVAILABLE` | Resource or data not available now |
| −5 | `ERR_LIMIT` | A quota would be exceeded |
| −6 | `ERR_HANDLE` | Unknown device, resource, snapshot or marker |
| −7 | `ERR_UNSUPPORTED` | Device or request kind does not match |

Device IDs are `1..=device_count`, the dense device index plus one. Resource IDs are `1..=resource_count`, where resource 1 is propellant and 2 is generator fuel. Text fields (`Text64`, `Text256`) are a `u64` length followed by UTF-8 bytes, with every byte past the length set to zero.

## Tick context and budget

| Import | Record | Notes |
| --- | --- | --- |
| `tick_read(out, bytes)` | `TickContext` (96 bytes) | See fields below |
| `budget_read(out, bytes)` | `BudgetInfo` (40) | Gas remaining, instructions remaining in this callback, gas capacity, refill per second, instruction limit |
| `flight_read(out, bytes)` | `FlightState` (168) | Body-to-world rotation (xyzw), angular velocity, velocity (inertial m/s), mass, inertia tensor (column array), hull radius |
| `ship_resources_read(out, bytes)` | `ShipResources` (80) | Hull HP and maximum, internal heat and storage budget, shield temperature, reserve mass and capacity, shield strength, stored energy and shield state |
| `tick_set_interval(seconds)` | f64 argument | Finite and non-negative |

`TickContext` fields:

| Field | Meaning |
| --- | --- |
| `tick` | Hardware tick counter |
| `snapshot` | ID of this callback's observation snapshot |
| `time_s` | Simulation time at the start of the tick |
| `dt_s` | Simulation time since the previous delivered observation |
| `physics_dt_s` | Hardware step length (0.1 s) |
| `device_count`, `resource_count` | Directory sizes |
| `request_count`, `screen_event_count` | Pending inputs visible in this callback |
| `interest` | Client interest bits: `INTEREST_MARKERS` (1), `INTEREST_PATHS` (2) |
| `requested_screens` | Bit mask of screen IDs the client wants frames for |
| `flags` | `FIRST_CALLBACK` (1) on the first callback of a new instance |

The simulator sets `interest` only for the controlled ship, and only while the orbit overlay is enabled and the computer is online. It sends `requested_screens` only for the controlled ship. Firmware should treat both as optional presentation hints.

## Devices and resources

| Import | Notes |
| --- | --- |
| `device_info(index, out, bytes)` | `DeviceInfo` (168): id, part id (0 for avionics), kind, flags (`CONTROL_ENABLED` = 1), position relative to dry centre of mass, rotation from device to ship axes, alias, group count. `index` is 0-based. |
| `device_group_read(device, index, out, bytes)` | One group label as `Text64` |
| `device_spec(device, expected_kind, out, bytes)` | Static specification for the kind. Wrong kind returns `ERR_UNSUPPORTED`. |
| `device_read(device, expected_kind, out, bytes)` | Current reading for this callback |
| `device_write(device, setting, in, bytes)` | Stage a setting. A later write to the same device in the same callback replaces it. |
| `resource_info(index, out, bytes)` | `ResourceInfo` (96): id, key, unit mass, unit volume |
| `resource_read(resource, out, bytes)` | `ResourceAmount` (8): units held |

`CONTROL_ENABLED` reflects the design's actuator exclusions. It is advisory: the host accepts writes to excluded devices, and the standard firmware chooses to skip them.

### Device kinds

| Kind | Code | Spec record (bytes) | Reading record (bytes) | Settings |
| --- | --- | --- | --- | --- |
| Accelerometer | 0 | none (0) | `AccelerometerReading` (48) | |
| Computer | 1 | none (0) | `DeviceStatus` (8) | |
| Storage | 2 | `StorageSpec` (8) | `DeviceStatus` (8) | |
| Battery | 3 | `BatterySpec` (8) | `DeviceStatus` (8) | |
| Engine | 4 | `EngineSpec` (32) | `EngineReading` (16) | `SET_THROTTLE` |
| Torquer | 5 | `TorquerSpec` (16) | `TorquerReading` (16) | `SET_TORQUE` |
| Generator | 6 | `GeneratorSpec` (32) | `GeneratorReading` (16) | `SET_GENERATOR_DEMAND` |
| Shield | 7 | `ShieldSpec` (40) | `ShieldReading` (72) | `SET_SHIELD_ENABLED` |
| Sensor | 8 | `SensorSpec` (16) | `SensorReading` (16) | `SET_SENSOR_ENABLED` |
| Weapon | 9 | `WeaponSpec` (152) | `WeaponReading` (72) | `SET_WEAPON` |
| RCS | 10 | `RcsSpec` (32) | `RcsReading` (32) | `SET_RCS` |

Every reading begins with a `DeviceStatus` whose flags are `OPERATIONAL` (1) and `POWERED` (2). A sensor reports powered only while its range is non-zero.

The accelerometer reading holds an ideal specific-force sample at the mount, in device axes. It includes thrust, drag, collision impulses and rotational terms, and excludes gravity. `sample_present` is 0 until the simulator has integrated at least one tick.

### Settings

| Setting | Code | Record | Validation |
| --- | --- | --- | --- |
| `SET_THROTTLE` | 0 | `ThrottleSetting { fraction }` | Fraction in [0, 1] |
| `SET_TORQUE` | 1 | `TorqueSetting { torque_nm[3] }` | Finite. Hardware clamps each axis. |
| `SET_GENERATOR_DEMAND` | 2 | `GeneratorDemandSetting { fraction }` | Fraction in [0, 1] |
| `SET_SHIELD_ENABLED` | 3 | `EnabledSetting { enabled }` | 0 or 1 |
| `SET_SENSOR_ENABLED` | 4 | `EnabledSetting { enabled }` | 0 or 1 |
| `SET_WEAPON` | 5 | `WeaponSetting` | See [weapons.md](weapons.md) |
| `SET_RCS` | 6 | `RcsSetting { thrust_n[3] }` | Finite. Hardware clamps each axis. |

An unknown setting code returns `ERR_ARGUMENT`. A setting that does not match the device kind returns `ERR_UNSUPPORTED`.

## Sensors and tracks

`sensor_scan(sensor, maximum, contacts, bytes)` fills up to `maximum` (at most 256) `Contact` records (72 bytes each), and `bytes` must equal `maximum × 72`. It returns the number written. The sensor device must be operational and powered with a non-zero range, otherwise the call returns `ERR_UNAVAILABLE`. It charges `maximum × 1000` gas (`SCAN_GAS_PER_OBJECT`) before querying.

The simulator answers with the nearest `maximum` indexed objects within range, measured from centre to centre. Objects hidden behind occluding bodies are then removed, and they are not replaced by more distant objects. The range is the smaller of the sensor reading and the ship's `Sensor` component range (adjustable in the Sensor debug window). The index holds ships and the bodies of active star systems. Projectiles are not indexed.

`Contact` fields are `id` (a stable entity identifier), `kind` (`CONTACT_SHIP` 0, `CONTACT_CELESTIAL` 1, `CONTACT_OTHER` 2, `CONTACT_PROJECTILE` 3), `radius_m`, and position and velocity relative to the ship.

Every successful scan is admitted into host-side tracks, up to 512. Each track keeps its latest and previous estimates. Tracks expire 2 s after their latest measurement. `contact_label(id, out, bytes)` returns the contact's name as `Text64` while its track is current.

## Requests

Players and other host code send requests. Each has an ID, a kind and a fixed payload.

| Import | Notes |
| --- | --- |
| `request_info(index, out, bytes)` | `RequestInfo` (24): id, kind, payload size |
| `request_read(index, expected_kind, out, bytes)` | Payload bytes. Wrong kind returns `ERR_UNSUPPORTED`. |
| `request_reply(id, result, message, bytes)` | `REPLY_ACCEPTED` 0, `REPLY_REJECTED` 1, `REPLY_UNSUPPORTED` 2. Message is UTF-8, at most 256 bytes. |

| Kind | Code | Payload |
| --- | --- | --- |
| `REQUEST_MANUAL` | 0 | `ManualRequest { throttle, steering[3] }` (32) |
| `REQUEST_HOLD_ATTITUDE` | 1 | none |
| `REQUEST_STOP_GUIDANCE` | 2 | none |
| `REQUEST_AIM_DIRECTION` | 3 | `DirectionRequest { direction[3] }` (24) |
| `REQUEST_AIM_CONTACT` | 4 | `ContactRequest { contact }` (8) |
| `REQUEST_SELECT_TARGET` | 5 | `ContactRequest { contact }` (8) |
| `REQUEST_ENGAGE_NAVIGATION` | 6 | `NavigationRequest { throttle_limit, stand_off_m }` (16) |
| `REQUEST_ENGAGE_WEAPONS` | 7 | `EngageWeaponsRequest { contact, maximum_flight_time_s }` (16) |
| `REQUEST_HOLD_FIRE` | 8 | none |

A request stays pending, and is presented in every callback, until a callback that replies to it commits. The host queue holds at most 256 requests. A new manual request replaces any queued manual request.

## Snapshots and spatial publications

Each callback has an observation snapshot. It records the callback ID, the simulation time, the ship's exact galactic position, its velocity and its rotation. Positions in `SNAPSHOT` frames are metres relative to that exact origin, so publications stay precise at any galactic coordinate.

| Import | Notes |
| --- | --- |
| `snapshot_keep(id)` | Pin the current snapshot, or keep an already pinned one. Costs 100 gas. At most 8 pins (`ERR_LIMIT`). Unknown IDs return `ERR_HANDLE`. |
| `snapshot_drop(id)` | Release a pin |
| `spatial_marker_put(marker, bytes)` | `SpatialMarker` (176) |
| `spatial_path_put(path, bytes, vertices, count)` | `SpatialPath` (152) plus `count` × `SpatialVertex` (32). `count` is 2 to 128. Costs 100 gas per vertex. |
| `spatial_remove(id)` | Remove a marker or path |
| `spatial_clear()` | Remove all markers and paths |

A snapshot that is not pinned can only be referenced during its own callback.

`SpatialMeta` holds `id` (non-zero, one namespace shared by markers and paths), `role`, `valid_until_s` (a lease that must be later than the current time) and a `label`. Publications disappear when their lease expires. Firmware refreshes them by publishing again.

`SpatialFrame` holds `kind`, `reference` and `origin_velocity_m_s`:

| Frame | Code | Reference | Notes |
| --- | --- | --- | --- |
| `FRAME_SNAPSHOT` | 0 | Current or pinned snapshot ID | Its origin moves at `origin_velocity_m_s` from the snapshot epoch |
| `FRAME_SHIP` | 1 | 0 | Ship position at presentation time, world axes |
| `FRAME_SHIP_BODY` | 2 | 0 | Ship position and rotation at presentation time |
| `FRAME_CONTACT` | 3 | Contact ID | Track estimate; the track must be current |
| `FRAME_PATH` | 4 | Path ID | Markers only; the path must be timed and active |

`origin_velocity_m_s` must be zero for every frame except `FRAME_SNAPSHOT`.

**Markers.** Roles are `MARKER_ANNOTATION` 0, `MARKER_TARGET` 1, `MARKER_WAYPOINT` 2, `MARKER_AIM` 3 and `MARKER_EVENT` 4. `offset_m` is added in frame axes. `time_mode` is `TIME_CURRENT` (0, with `time_s` = 0) or `TIME_FIXED` (1, snapshot or path frames only). A fixed time cannot precede the snapshot epoch, and on a path frame it must lie within the path. A marker on a path is tied to that path's revision; republishing the path hides the marker until it is published again. At most 64 markers.

**Paths.** `kind` is `PATH_POLYLINE` 0 (all vertex times zero) or `PATH_TIMED` 1 (requires a snapshot frame; times at or after the snapshot epoch and strictly increasing). Roles: `PATH_REFERENCE` 0 and `PATH_ROUTE` 3 (subject 0), `PATH_OWN_FORECAST` 1 (timed, subject 0), `PATH_CONTACT_FORECAST` 2 (timed, non-zero `subject_contact`). At most 8 paths and 1024 vertices in total. A rejected replacement leaves the previous path unchanged.

## Instruments

Instruments are fixed records that the client renders natively. Each carries a `valid_until_s` lease.

| Import | Record | Validation |
| --- | --- | --- |
| `instrument_attitude_put` | `AttitudeState` (64) | `mode` ≤ `ATTITUDE_GUIDANCE` (0 manual, 1 hold, 2 guidance). `present` 0 (reference all zero) or `ATTITUDE_REFERENCE` 1 (unit quaternion). `control_error` ≥ 0. |
| `instrument_navigation_put` | `NavigationState` (368) | `status` ≤ `NAV_UNAVAILABLE` (0 idle, 1 active, 2 suspended, 3 unavailable). `throttle` and `throttle_limit` in [0, 1]. `present` bits: `NAV_STAND_OFF` 1, `NAV_SPEED_LIMIT` 2, `NAV_BRAKING_DISTANCE` 4, `NAV_ARRIVAL` 8, `NAV_FUEL` 16. Each measurement is finite and non-negative, and zero when its bit is clear. `reason` is valid text. |
| `instrument_contacts_put` | `ContactsState` (16) | Requires a scan within the last 2 s. Publishes that scan's contact list with `selected_contact`. |
| `instrument_weapons_put(state, bytes, rows, count)` | `WeaponsState` (288) plus `count` × `WeaponInstrument` (128) | `mode` 0 hold or 1 engage. Each row names a distinct weapon device, `solution_flags` ≤ 1, finite non-negative times and errors, and `aim_marker` 0 or an existing marker. Costs 100 gas per row. |
| `instrument_clear(kind)` | | `INSTRUMENT_ATTITUDE` 0, `INSTRUMENT_NAVIGATION` 1, `INSTRUMENT_CONTACTS` 2, `INSTRUMENT_WEAPONS` 3 |

The navigation record also names `target_contact`, `own_path` and `target_path`. The orbit overlay uses these to find the plan and target forecast ([orbital-navigation.md](orbital-navigation.md)).

## Screens

`screen_define`, `screen_remove`, `screen_begin`, `screen_draw`, `screen_button`, `screen_end`, `screen_event_read` and `screen_event_ack` are documented in [mfds.md](mfds.md).

## Writing firmware

### Rust

Depend on `toy-sim-ship-api`. `abi::raw` declares the imports for `wasm32` targets. `sdk` wraps them with `Result<_, i32>` helpers: `tick`, `budget`, `flight`, `resources`, `device`, `device_spec`, `device_read`, `device_write`, `scan`, `request`, `request_read`, `request_reply`, `marker`, `path`, `attitude`, `navigation`, `contacts`, `weapons`, the screen calls, and generic `read`/`write` over any `Record`.

The minimal `no_std` example is [examples/embedded.rs](../crates/toy-sim-ship-api/examples/embedded.rs). It publishes a two-vertex forecast and sets every engine to 25% throttle. The standard firmware in [toy-sim-example-controller](../crates/toy-sim-example-controller) uses `std` collections and exports its entry points from [firmware.rs](../crates/toy-sim-example-controller/src/firmware.rs) behind the default `firmware` feature. [examples/custom_screen.rs](../crates/toy-sim-example-controller/examples/custom_screen.rs) wraps that firmware's `Computer` and adds a screen. It is built with `--no-default-features` so the library does not export the entry points a second time.

The repository has no build script for firmware. These commands follow from the manifests and the comment in `custom_screen.rs`:

```sh
rustup target add wasm32-unknown-unknown

# Standard firmware (cdylib): target/wasm32-unknown-unknown/release/toy_sim_example_controller.wasm
cargo build -p toy-sim-example-controller --release --target wasm32-unknown-unknown

# Custom screen example: target/wasm32-unknown-unknown/release/examples/custom_screen.wasm
cargo build -p toy-sim-example-controller --release --target wasm32-unknown-unknown \
    --example custom_screen --no-default-features
```

The simulator embeds the standard firmware from `crates/toy-sim-ships/data/example-controller.wasm`. After changing the firmware source, copy the new build over that file and rebuild. The test fixtures in `crates/toy-sim-ship-wasm/tests/fixtures/` are also prebuilt binaries.

### C

Include [ship.h](../crates/toy-sim-ship-api/include/ship.h). It declares `ship_<name>` imports with the correct import module and names, `ship_*_record` structs with layout assertions, and `SHIP_*` constants. [tests/fixtures/controller.c](../crates/toy-sim-ship-wasm/tests/fixtures/controller.c) is a freestanding example with no libc: it provides its own `memset`, exports `ship_api_version` and `ship_tick`, writes a throttle, and publishes an attitude record and a timed path. The repository does not record the compiler flags used to build `controller.wasm`.

### AssemblyScript

[ship.ts](../crates/toy-sim-ship-api/bindings/ship.ts) declares the imports with `@external("ship_v11", …)` and exports constants plus `<RECORD>_<FIELD>` byte offsets and `<RECORD>_SIZE` values for working with raw buffers.

### Regenerating bindings

After changing `abi.rs`, regenerate both binding files:

```sh
python3 tools/generate_ship_bindings.py
```

The script parses the record structs, `Text` sizes, integer constants and the `raw` import declarations in `abi.rs`. It computes offsets assuming 8-byte fields, then writes `include/ship.h` and `bindings/ship.ts`.

## Host API

For embedding the runtime elsewhere, as `toy-ship-bench` does:

- `ControllerRuntime::new()`, `compile(bytes)`, `validate_program(bytes)`, `instantiate(bytes)` (returns a booting `Controller`), `boot(&mut controller)`, `cached_modules()`
- `Controller::configure_hardware(design, catalogue)` installs the device and resource directories.
- `advance(dt)`, `can_run()`, `is_booting()`, `boot_progress()`, `gas_remaining()`, `memory_bytes()`
- `run(input)` and `run_with_scan(input, Option<Arc<dyn ScanSource>>)` return `Ok(None)` when the computer cannot run yet.
- `reboot()`, `fail(message)`, `enqueue_screen_event(event)`, `has_pending_input()`
- State fields: `state` (the committed `Session`), `fault`, `contacts`, `scan_time`, `telemetry`, `screens`, `trajectory_revision`, `instrument_interest`, `observer_origin`
- `CallbackSchedule` implements the interval logic. Call `advance(dt)` once per tick, check `ready(has_input)`, and call `completed(output.tick_interval_seconds)` after a successful callback.
- `Input` carries tick, dt, physics dt, `Observation` (time, flight state, resources, inventory), device statuses, requests, screen events and requested screens. `Output` carries device commands, replies, screen frames, cleared screens and the interval.

## Tests

```sh
cargo test -p toy-sim-ship-wasm
```

[tests/sandbox.rs](../crates/toy-sim-ship-wasm/tests/sandbox.rs) covers:

- rejection of old versions and foreign imports
- C and Rust programs sharing the ABI with isolated memory
- memory growth to 1 MiB and reset on reboot
- runaway loops faulting and requiring a full reboot
- exact buffer lengths
- path snapshots and rejected replacements
- lease expiry
- trap rollback
- scan gas reservation
- request persistence across reboot
- unfinished screen frames
- snapshot quotas
- the custom screen firmware
- bundled firmware forecasts, weapons engagement and pursuit within budget
- weapon setting validation
- the contacts instrument on a rotated ship
