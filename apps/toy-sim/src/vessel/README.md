# Vessel module

[vessel.rs](../vessel.rs) is the ECS adapter between Bevy and the ship crates. Hardware rules live in [toy-sim-ships](../../../../crates/toy-sim-ships), the WebAssembly host lives in [toy-sim-ship-wasm](../../../../crates/toy-sim-ship-wasm), and presentation lives in [toy-sim-ship-view](../../../../crates/toy-sim-ship-view). This module spawns ships, feeds each flight computer once per simulation tick, applies its commands to hardware, and turns hardware output into forces for physics.

For the ship model itself, see [docs/ships.md](../../../../docs/ships.md). For the controller interface, see [docs/ship-abi.md](../../../../docs/ship-abi.md).

## Components

| Component | Contents |
| --- | --- |
| `Vessel` | Class name (the blueprint name) and vessel name. It requires `RigidBody`, `VesselControlState` and `CollisionBody`. |
| `ControlledVessel` | Marker for the player ship. The GUI, keyboard input and instruments use it. |
| `ShipDesign` | `Arc<CompiledShipDesign>`, immutable. |
| `ShipHardware` | `ShipState`: inventory, device and weapon state, hull, thermal state, settings and tick counter. |
| `ShipSoftware` | The `Controller`, the request inbox, screen frames, the latest request replies, pending impact energy, the reset flag, the callback schedule and per-step timings. |
| `VesselControlState` | Raw manual throttle and steering from the keyboard. |

## Resources

| Resource | Purpose |
| --- | --- |
| `ShipCatalogue` | `Catalogue::builtin()`. |
| `WasmRuntime` | The shared `ControllerRuntime` (engine, linker, compiled-module cache). `main.rs` creates it and pre-validates a `--ship` controller. |
| `ShipLaunch` | The optional `--ship` path. |
| `ShipRecovery` | Flags set by "Reset encounter" buttons. |

## Systems and schedule

| Schedule | System | Behaviour |
| --- | --- | --- |
| `OnEnter(Game)`, after `LoadOrrery` | `spawn` | Spawns the player and traffic from `INITIAL_SCENARIO`. |
| `Update`, when recovery is requested | `spawn` | Respawns. An encounter reset first despawns all vessels, projectiles and explosions and clears combat history. |
| `PreUpdate` | `read_controls` | Reads Shift/Ctrl, W/S, A/D, Q/E and queues one `Command::Manual` sample when input changed (or after a boot or reset). |
| `FixedUpdate`, set `PrepareBodies` | `retaliation`, then `run` | See below. |
| `Update` | `update_engine_plumes`, `update_shields`, `add_weapon_visuals`, `update_weapons` | Presentation only. |
| `Startup` | `prepare_visuals` | Builds part meshes, materials and plume assets. |

`PrepareBodies` runs before gravity and drag, so hardware forces from this tick join the same force accumulation that the collision solver integrates in `FixedPostUpdate`.

## Spawning

`spawn` compiles the armed starter and, if present, the `--ship` blueprint. It then creates one entity per state from `InitialScenario::fleet_states`:

- Index 0 is the player ("Orbital explorer"). It uses the selected design, faces along its velocity, and receives `ControlledVessel` and `CameraFocus`.
- Later indices are traffic ("Traffic 001", …) using the armed starter. The first traffic ship is repositioned 100 km from the player along a direction between radial and prograde. It faces the player. Its inbox receives `SelectTarget(player)` and `EngageNavigation { throttle_limit: 1.0, stand_off_m: 1000.0 }`.

Each ship gets `configure_hardware` (device and resource specs for the ABI), `ShipState::new` plus `test_loadout`, mass properties, an `AeroModel` from the design's semi-axes, a `SpatialBody` (radius, occludes) and a default `Sensor`. Part visuals are spawned as children.

## The per-tick `run` system

1. **Resets and boots.** For each ship with `reset` set, hardware state is rebuilt and the controller reboots. Manual samples stay in the inbox, and other requests are dropped. Powered computers accumulate gas (`Controller::advance`). At most `MAX_BOOTS_PER_TICK` (64) booting computers with a full `BOOT_GAS` reserve are started in one tick. While a computer is booting, hardware commands revert to defaults and its screens are cleared.
2. **Scan scene.** If the spatial index exists and any computer can run, one shared `ScanScene` is built. It holds a clone of the `SpatialIndex` and per-entity metadata: position, velocity, name truncated to 64 characters, radius and contact kind (ship, projectile, celestial, other).
3. **Parallel ship step** (`par_iter_mut`), for each ship:
   - Deposit pending test impact energy into the hull and shield.
   - Advance the callback schedule. If the computer is running, has no fault, can run, and is due (or has pending input), build the `Observation` and device snapshot. Accelerometer readings are sampled at each device mount from `AccelerometerState`. Screen requests are only sent for the controlled vessel.
   - Call `run_with_scan`. Valid output commands are applied atomically with `ShipState::apply_commands`. Invalid commands reset hardware settings and fault the controller. Replies, screen frames and cleared screens are stored.
   - A controller error clears hardware commands. When the computer is not running, commands also reset.
   - Expire leased instrument state. Copy the controller's latest contacts into `SensorContacts` for the GUI.
   - Step hardware with `ShipState::step`. Write mass and inertia to `MassProps`, and add the rotated force and torque to the accumulators.
   - Record `ShipStepTimings` and `last_seconds`.

Non-focused vessels have `instrument_interest` forced to zero, so their firmware sees no request for optional markers or paths.

## Timings

`ShipStepTimings` records wall-clock durations for the most recent update of each ship:

| Field | Measures |
| --- | --- |
| `prepare` | Start of the ship step until the callback starts (impact deposit, mass properties, observation and device snapshot). If no callback ran, it covers the whole pre-hardware section. |
| `callback` | The `run_with_scan` call, including syscalls and the native scan. |
| `scan` | Native scene query time inside the callback. |
| `publish` | End of the callback until the hardware step (expiry and contact copying). |
| `hardware` | `ShipState::step`. |

The Hardware diagnostics window shows these for the controlled vessel. [docs/ship-step-profile.md](../../../../docs/ship-step-profile.md) covers profiling.

## Retaliation

`retaliation` watches the controlled vessel's published weapons state. When the player enters `WEAPONS_ENGAGE` against a contact that is an uncontrolled ship, that ship receives `Command::EngageWeapons { contact: player, maximum_flight_time_s: 2.0 }` once per player and target pair.

## Tests

[tests.rs](tests.rs) covers: manual thrust on a tumbling ship without automatic attitude hold; the boot wait, fault clearing and automatic recovery; fault isolation between computers; sleeping computers woken by commands; and the per-tick boot limit. It also contains the ignored `profile_default_fleet` benchmark.

```sh
cargo test -p toy-sim vessel::tests
```
