# Ship controller ABI (version 30)

A flight computer runs a WebAssembly module whose callbacks are scheduled by the host. The program talks to the host only through the imports of module `ship_v30`. Almost every import exchanges fixed-size little-endian C records without serialization. The exceptions are the two world-service imports added in ABI 12, `world_query` and `world_command`, which exchange postcard-encoded `toy-sim-model` values ([World services](#world-services)).

ABI 30 moves route search and the full order queue to the server. `Travel` now
returns `CurrentOrder`, containing the current order, its revision and index,
autopilot status, preferences and active arrival estimate. Programs can request a
server route preview and poll its asynchronous job, then commit it with
`UseRoute`. Physical actions carry both the revision and order index, so a
suspended callback cannot act on a later command. The changed Postcard enums and
import namespace require rebuilding programs and bindings together.

ABI 28 adds an optional `missile_tick(handle: u64)` export and the
`missile_read` and `missile_control` imports. Missile callbacks run in their
parent's flight instance, sharing its linear memory, durable store and gas
allowance. A suspended callback resumes before another callback begins.

ABI 27 replaces the accumulating local reserve and separate instruction budget
with one gas slice. `BudgetInfo` is now a 24-byte record containing
`gas_remaining`, `gas_limit` and `gas_per_tick`, in that order. Guest execution and
host calls spend the same allowance. Older import namespaces, API versions and
record layouts are rejected; rebuild programs and bindings together.

ABI 26 retains a typed `Destination` in each queued slip order and adds future
epochs to destination resolution and aperture queries. `Resolve` takes
`destination` and `after_seconds`. `SlipEligibility` takes separate departure and
arrival offsets and returns remaining preparation time separately from flight
duration. Programs using older ABI exports or import namespaces are rejected;
rebuild them and their language bindings together.

The public `Navigation` query remains available for programs that inspect gate
facts. The standard flight program executes one strategic command at a time and does not
load the navigation graph or search it. It generates local waypoints, collision
avoidance and slip-exclusion escape manoeuvres within that command's execution. The runtime permits 8 MiB of guest linear
memory while metering both guest execution and native host work.

ABI 24 adds a persistent byte store for each computer. `persistent_read(out, capacity)`
returns its length and copies the current data, or returns `ERR_BUFFER` if the
buffer is too small. `persistent_write(data, length)` replaces the whole store;
an empty write clears it. The maximum length is 65536 bytes. Copies are metered,
out-of-bounds memory accesses trap, and display callbacks cannot write. A write
is committed when an execution slice successfully yields or completes. The committed data survives a
computer reboot and is included with the program in server checkpoints.

Server recovery creates a fresh VM from the saved program and restores this byte
store. Native execution stacks and guest linear memory are not checkpointed.
Programs must use the persistent store for state that must survive a restart.

Travel preferences and propulsion fuel estimates are shared model values.
`QueuedOrder` contains an `action`, optional `estimated_duration_ticks` and
optional `estimated_propellant_kg`. The server computes the full `FuelBudget`;
the flight program reports only the active command's remaining time and
propellant. `CompleteOrder` advances the server's cursor.

ABI 19 adds request code 11, `REQUEST_THROTTLE`, with an eight-byte `ThrottleRequest { throttle: f64 }` payload. It changes manual throttle without replacing the direction target. Direction alignment preserves manual throttle.

ABI 18 separates target marking from firing: request codes 7–10 are Mark target, Stop firing, Unmark target and Start firing. The weapons instrument mode is hold or firing, independently of its marked contact. World guidance also accepts a galactic direction target. Rebuild firmware and generated bindings.

ABI 17 stores `ShipResources.energy_j`, `WeaponReading.battery_energy_j`, and `BatterySpec.capacity_j` as `u64` joules. Their offsets and record sizes are unchanged; their integer bit representation is different. Rebuild firmware and regenerate bindings. Shot-energy estimates and physical rates remain floating-point.

ABI 16 removed the explicit `ProgramAction::Gate` action: aperture crossings now belong to the physics solver. This changes the Postcard world-action enum discriminants; rebuild firmware.

ABI 15 introduced resource quantities as `u64` and appended `chemical: u64` to `WeaponSpec` at byte 168. A nonzero value describes cartridge-powered propulsion with no electrical shot cost or separate counterpropellant. The world-service enums now include queued guidance and exact authorized contact lookup. Rebuild firmware against the current ABI and `toy-sim-model`.

ABI 12 also adds an optional second entry point, `ship_display`. The authoritative server runs it in a separate instance to draw screens for network clients ([Display entry point](#display-entry-point)).

- Record definitions and constants: [crates/toy-sim-ship-api/src/abi.rs](../crates/toy-sim-ship-api/src/abi.rs)
- Rust helpers: [crates/toy-sim-ship-api/src/sdk.rs](../crates/toy-sim-ship-api/src/sdk.rs)
- Generated C header: [crates/toy-sim-ship-api/include/ship.h](../crates/toy-sim-ship-api/include/ship.h)
- Generated AssemblyScript bindings: [crates/toy-sim-ship-api/bindings/ship.ts](../crates/toy-sim-ship-api/bindings/ship.ts)
- Host implementation: [crates/toy-sim-ship-wasm](../crates/toy-sim-ship-wasm)

For the hardware that devices represent, see [ships.md](ships.md). Screen drawing is covered in [mfds.md](mfds.md), and weapons in [weapons.md](weapons.md).

## Module requirements

`ControllerRuntime::compile` accepts a module when all of the following hold:

- It is at most 1 MiB.
- Every import comes from module `ship_v30` and is one of the names in `abi::IMPORTS`.
- It exports `memory`: 32-bit, not shared, with an initial size of at most 128 pages.
- It exports `ship_tick` with no parameters and no results.
- It exports `ship_api_version` as a defined function with no parameters, one `i32` result and no locals. Its body is exactly `i32.const 28; end`, allowing the host to verify the ABI without running guest code.

`ship_display` is optional and not checked at compile time. A display instance requires it to exist, with no parameters and no results.

`missile_tick` is optional. When present, it must accept one `i64` handle and return
nothing. It belongs to the flight instance, so it cannot run as a display callback.

`validate_program` performs structural validation, including the literal version export, without executing guest code. Normal funded initialization runs the module's start function and version export under slice accounting and may suspend. Store limits: one instance, one memory up to 8 MiB, 4096 table elements, and a 128 KiB WebAssembly stack. Compiled modules are cached by their bytes, so identical programs share one compiled module.

For `wasm32-unknown-unknown` builds, [.cargo/config.toml](../.cargo/config.toml) passes `-zstack-size=65536` and `--max-memory=8388608` to the linker.

## Lifecycle

```
instantiate ──► paid boot work ──► initialization ──► ready
                     ▲                               │
                     │                   callback ──► suspended
                     │                               │
                     └──────── fault or reboot ──────┘
```

- **Gas.** Every computer draws from its actual owner's global account. The standard physical ceiling is 1,000,000 gas per simulation tick (`FUEL_PER_TICK`), shared by flight and display execution. Guest instructions and native calls consume one allowance. Unused grants return to the account at settlement.
- **Boot.** A new or rebooted instance consumes `BOOT_GAS` (50,000,000) across its grants before initialization. At the full standard allowance this fee takes 50 ticks, or 5 s; account scarcity and other work can extend it. Initialization code is also metered and may suspend. The displayed countdown estimates the unpaid fixed fee at the physical ceiling, so it can reach zero while initialization is still running.
- **Callbacks.** An idle computer starts a callback when its interval has elapsed or pending requests or screen events wake it. A suspended callback resumes where it stopped; the host does not start another `ship_tick` concurrently or queue missed tick calls.
- **Suspension.** Insufficient slice gas pauses execution. A host call waits until its entire bounded cost can be admitted, then runs without preemption. A call that cannot fit the physical tick ceiling returns `ERR_LIMIT`. Insufficient owner gas leaves the continuation waiting until the account can fund its next indivisible operation.
- **Faults.** Invalid guest memory, traps and rejected hardware commands still reboot the computer. The instance is dropped and session state is cleared: instruments, spatial publications, tracks, pinned snapshots, screens and pending screen events. Work already consumed remains charged. The fault message is kept until the next successful boot. The server clears the autopilot queue and toggle, staged actions, slip preparation and docking reservations. New requests submitted during startup are delivered after boot.

### Slice commits

Successful suspension and normal callback completion commit completed device writes, request replies, finished screen frames, instrument records, spatial publications, admitted scans, durable bytes and staged world actions. A trap discards changes from the current uncommitted slice. Earlier successful slices have already committed. A screen draft remains private until `screen_end` completes.

Hardware keeps the last committed settings while execution sleeps or suspends. On resume, host reads use refreshed observations, device readings, time and world-query sources. Guest locals and linear memory retain the values the program previously stored. The request and screen-event batch belongs to the current callback; newly queued inputs wait for the next callback.

### Scheduling

`tick_set_interval(seconds)` sets how long the host waits after this callback completes before calling again. The value persists until changed. Zero means every tick. New requests or screen events wake the program early. The simulation tick is 0.1 s.

The server honours the interval through `CallbackSchedule`: an idle flight program starts only when its interval has elapsed or requests are queued. A suspended callback resumes before any new callback starts. Display refresh requests follow the subscribers' refresh rate ([Display entry point](#display-entry-point)) and share the ship's physical CPU ceiling and owner account.

## Calling conventions

- Pointers are `u32` offsets into the exported memory. Every fixed-record length must equal its record size exactly, or the call returns `ERR_BUFFER`. Out-of-range memory accesses trap and reboot the computer. A failed call writes nothing.
- Every import costs `CALL_GAS` (100). Copying records into or out of guest memory costs one gas per 8 bytes. Some calls add their own charges, listed below.
- Imports return `0` (or a count) on success and a negative error otherwise:

| Code | Name | Meaning |
| --- | --- | --- |
| −2 | `ERR_BUFFER` | Wrong record length or insufficient output capacity |
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
| `budget_read(out, bytes)` | `BudgetInfo` (24) | Remaining slice gas, actual slice grant, physical per-ship tick ceiling |
| `flight_read(out, bytes)` | `FlightState` (168) | Body-to-world rotation (xyzw), angular velocity, velocity (inertial m/s), mass, inertia tensor (column array), hull radius |
| `ship_resources_read(out, bytes)` | `ShipResources` (80) | Hull HP and maximum, internal heat and storage budget, shield temperature, reserve mass and capacity, shield strength, stored energy and shield state |
| `tick_set_interval(seconds)` | f64 argument | Finite and non-negative |

`BudgetInfo` uses three little-endian `u64` fields with eight-byte alignment:

| Offset | Field | Meaning |
| --- | --- | --- |
| 0 | `gas_remaining` | Unspent gas in the current execution slice |
| 8 | `gas_limit` | Total gas granted to this slice |
| 16 | `gas_per_tick` | The computer's configured physical limit per simulation tick |

The owner's available gas may produce a grant smaller than `gas_per_tick`.
Unused gas is returned to the owner's shared account when the slice settles;
the computer does not accumulate a private reserve. Client CPU telemetry uses
the physical tick ceiling as its denominator, so a small grant does not appear
as a fully loaded computer merely because the owner has little gas left.

`TickContext` fields:

| Field | Meaning |
| --- | --- |
| `tick` | Hardware tick counter |
| `snapshot` | Borrowed snapshot ID returned by this `tick_read` |
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
| `resource_read(resource, out, bytes)` | `ResourceAmount` (8): unsigned 64-bit consumable units held |

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
| Weapon | 9 | `WeaponSpec` (176) | `WeaponReading` (72) | `SET_WEAPON` |
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

A well-formed `SET_WEAPON` command whose lease has already expired returns success
without changing the weapon command. Suspension can delay a callback beyond its
sample's lease; the host never extends that lease or reactivates its stale aim.
Device handles, record sizes, finite values, and aim constraints are still
validated. A future weapon lease cannot exceed one physics tick beyond the
current time.

## Sensors and tracks

`sensor_scan(sensor, maximum, contacts, bytes)` fills up to `maximum` (at most 256) `Contact` records (72 bytes each), and `bytes` must equal `maximum × 72`. It returns the number written. The sensor device must be operational and powered with a non-zero range, otherwise the call returns `ERR_UNAVAILABLE`. It charges `maximum × 3000` gas (`SCAN_GAS_PER_OBJECT`) before querying.

The server answers from the ship's fused information-group snapshot and the public beacon snapshot, selecting ship tracks only. It returns the nearest available tracks within range, excluding the observing ship and duplicate UUIDs. Measurements are acquired once per simulation tick; repeating a query cannot reroll sensor noise. The range is limited by the powered sensor. Debug accounts can override its range and occlusion setting.

`Contact` fields are `id` (an opaque per-ship handle for a group track), `kind` (`CONTACT_SHIP` 0), `radius_m`, and position and velocity relative to the ship. Celestial bodies are excluded from sensor contacts; use explicit celestial world queries for navigation. A previously authenticated ship may retain its IFF identity while its estimate coasts. See [server-client.md](server-client.md#contact-handles) for handle lifetime and query accounting.

The standard firmware starts with a 32-contact scan buffer, grows it when full and reduces it for sparse results. It leaves room for flight control and publication before admitting optional scans, forecasts or catalogue pages.

Every successful scan is admitted into host-side tracks, up to 512. Each track keeps its latest and previous estimates. Tracks expire 2 s after their latest measurement. `contact_label(id, out, bytes)` returns the contact's name as `Text64` while its track is current.

## Missile callbacks

The optional `missile_tick(handle: u64)` export executes in the parent's flight
instance. The handle identifies one launched missile within that parent and
remains stable across saved-world recovery. Missile and ship callbacks share
globals, linear memory, committed persistent bytes and the owner's paid gas.
They run serially; a suspended callback retains its kind and handle until it
finishes. The missile body does not receive a separate WASM instance.

| Import | Record | Cost and scope |
| --- | --- | --- |
| `missile_read(out, bytes)` | `MissileObservation` (192 bytes) | 124 gas; reads the current missile callback's observation |
| `missile_control(input, bytes)` | `MissileControl` (32 bytes) | 104 gas; stages steering for that same missile |

Both calls use the implicitly active handle. Outside a missile callback they
return `ERR_UNAVAILABLE`; a caller cannot pass another missile's handle to either
import. The full call cost is admitted before reading or staging its effect.

`MissileObservation` contains the handle, `target_visible`, target offset and
relative velocity, the missile's orientation and angular velocity, inertial
velocity, maximum acceleration, turn rate, integer fuel units, tick duration,
simulation time and target uncertainty. Vectors use metres, seconds and radians.
The target offset and steering direction use galactic axes. An unavailable target
does not become an exact server-side position merely because the missile was
launched at it.

`MissileControl` contains `direction: [f64; 3]` and `throttle: f64`. Throttle must
be finite and within zero to one. Direction must be a unit vector within a squared
length tolerance of `1e-6`; a zero direction is also accepted when throttle is
zero. The last control written in a slice replaces earlier writes for its active
missile. It becomes effective at a successful slice commit. Rust programs use
`sdk::missile()` and `sdk::control_missile()`.

## World services

ABI 12 adds two imports that connect firmware to the authoritative world's travel, beacon and intelligence services. Their payloads are [postcard](https://docs.rs/postcard) encodings of types in [toy-sim-model](../crates/toy-sim-model/src/lib.rs). They do not use fixed records, and the generated C header and AssemblyScript bindings declare the imports only. Firmware in those languages must produce postcard bytes itself.

| Import | Notes |
| --- | --- |
| `world_query(input, bytes, out, capacity)` | Decodes a `ProgramQuery` from `input` and writes a postcard `ProgramReply` into `out`. Returns the reply length. |
| `world_command(input, bytes)` | Decodes a `ProgramAction` and stages it. Returns 0. |

### `world_query`

**Buffers.** `bytes` and `capacity` are each at most 65,536. The reply must fit in `capacity`, otherwise the call returns `ERR_BUFFER`. Unlike record imports, `out` does not need to match the reply size exactly. Track pages stop at the encoded output capacity and retain the next unreturned track for continuation. If even one track and its page header cannot fit, the call returns `ERR_BUFFER` before changing its cursor.

**Gas.**

1. The call cost (100) plus one gas per 8 input bytes.
2. Admission for bounded query work and the output-copy allowance. `Tracks` and `Continue` work is capped by the physical tick ceiling after allowing for the call and input/output copies. The remaining query prices are `100 + 1008 × min(limit, 256)` for `Beacons`, `100 + 4096 × min(limit, 128)` for `Navigation`, 131,072 for `SlipEligibility`, and 1000 for other queries. Navigation prices are exposed as `NAVIGATION_GAS_BASE` and `NAVIGATION_GAS_PER_GATE`. A call that fits the physical ceiling can suspend until its slice can pay; a call exceeding that ceiling returns `ERR_LIMIT`.
3. The actual bounded native work, with track pages reporting their measured `gas_used`.
4. One gas per 8 reply bytes.

`Beacon` and `Beacons` also admit 100 gas per inspected docking bay. One call may
inspect at most 4096 bays across its results; a larger page returns `ERR_LIMIT`
without silently truncating its bay lists.

**Errors.** Undecodable input, a failed query or an invalid query returns `ERR_ARGUMENT`. A host with no world provider returns `ERR_UNAVAILABLE`.

| `ProgramQuery` | Reply |
| --- | --- |
| `Travel` | `Travel { state, pose, slip_ready }`: `CurrentOrder` with the active stage only, the ship's exact galactic pose, and whether its slipdrive is ready |
| `RouteRequest(Request)` | Enqueues an idempotent server planning job and returns `Route { id, status }`. The request contains a nonzero ID, the complete requested order list and planning preferences. |
| `RoutePoll { id }` | Returns the scoped job's `Unknown`, `Pending { progress }`, `Ready { plan }` or `Failed { reason }` status. |
| `LocalSpace { destination, range_m, after_seconds }` | `LocalSpace { obstacles, truncated }`: bounded known obstacle and slip-exclusion observations around the ship and destination. |
| `Tracks(TrackQuery)` | `Tracks(QueryPage)` from the ship's information group snapshot. Work is bounded by the physical tick ceiling after its call/copy envelope; other fields use the network limits. |
| `Continue { cursor, work }` | The next page of a retained cursor |
| `Beacon(entity)` | `Beacons` with zero or one beacon |
| `Beacons { after, limit }` | `Beacons` in entity ID order, with `limit` from 1 to 256 |
| `Navigation { after, limit, reference }` | Public enabled gate endpoints in entity ID order, with `limit` from 1 to 128 and an exclusive `after` cursor. Returns a topology revision, current poses, paired exits, systems, staging positions and spatial slip eligibility. |
| `SlipEligibility { origin, destination, departure_after_seconds, arrival_after_seconds }` | `SlipEligibility { ready, preparation_s, duration_s }`: current drive readiness and predicted aperture eligibility at the two future epochs, remaining preparation time, and flight duration alone. |
| `Resolve { destination, after_seconds }` | Predicted `Pose` of a galactic position, beacon, or offset from a beacon or celestial body. Celestials and orbital gates use ephemerides; other beacons extrapolate current linear and angular motion. |

`LocalSpace` admits 262144 work gas plus the normal call and copy envelope. It
examines at most 2048 weighted index/ephemeris work units and returns at most 32
obstacles. Range is finite and between zero and 10¹² metres; prediction time is
between zero and one Julian year. Volumes containing either query position are
included even when their centres lie beyond the range. A result carries an
observed or public reference, predicted pose, physical radius and slip-exclusion
radius. Observed contacts use the ship's fused information; public bodies use
catalogue ephemerides. Undetected private objects are not exposed.

The `truncated` flag reports either work exhaustion or result overflow. A program
must treat it as incomplete knowledge when choosing a safe manoeuvre. The
service supplies observations only: local waypoints and steering decisions
remain in the ship program. It does not append manoeuvres to the server queue.

Route request and polling each admit 8192 work gas in addition to the normal
call and serialization costs. Search runs outside the non-preemptible syscall;
its work is charged separately to the ship owner's account. Requests and ready
plans are bounded to 256 orders, and serialized plans to 48 KiB. Submission
checks reply capacity before accepting or changing a job. Jobs are scoped to the
current world, ship, owner and control revision. Commit also requires the
expected queue revision. Preview age, ordinary movement and unrelated global
topology changes do not expire the plan. Its topology revision records the
planning context. The executor resolves current geometry within each strategic
command. Jobs do not reveal another ship's route or observations.

Prediction offsets are relative to the current query epoch and must be finite,
nonnegative and at most one Julian year (`365.25 × 86400` seconds). A slip arrival
offset must be at least its departure offset. `Navigation` staging eligibility
excludes transient drive cooldown; `SlipEligibility` checks actual readiness.
Preparation estimates include the remaining energy and minimum preparation delay,
rounded to future simulation ticks because charging runs before the firmware
callback. Flight duration uses the same tick rounding as the transit schedule.

Track queries are metered as described in [server-client.md](server-client.md#metered-queries). A cursor expires 10 ticks after its query started. Each ship keeps separate cursor stores for its flight instance and its display instance.

### `world_command`

**Limits.** Input is at most 65,536 bytes, and a callback can stage at most 8 actions. The call costs 100, plus 1000, plus one gas per 8 input bytes. It returns `ERR_ARGUMENT` for undecodable input, for a display instance, or when the limit is reached.

**Application.** Staged actions are applied only after a successful slice commit. The world applies them after every ship's slice has run, grouped by ship ID and in staging order. A stale revision or order index is ignored without changing the current queue. Other rejected actions do not fault the computer; an active-command failure sets the ship's travel status to `Blocked(error)`, and the ship's remaining actions from that batch are skipped, so a `CompleteOrder` staged after a rejected `Dock` does not advance travel.

| `ProgramAction` | Effect |
| --- | --- |
| `UseRoute { id, revision, engage }` | Commits an authorized ready server plan against the current travel revision. |
| `Block { revision, order, reason }` | Blocks only the matching active command, cancels unfinished slip preparation and clears its ETA. |
| `Estimate { revision, order, remaining_ticks, remaining_propellant_kg }` | Updates the matching active stage. The server combines its remaining fuel estimate with later stages. |
| `CompleteOrder { revision, order }` | Completes the matching active stage and advances the server's cursor. |
| `Slip { revision, order, destination }` | Starts or updates the matching slip command's charging candidate, preserving work and start time; departure freezes the endpoint. |
| `ReserveBay { revision, order, station, bay }`, `Dock { revision, order, station, bay }`, `Undock { revision, order }` | Performs the bay operation only for the matching active command. |

The host rules for each action are in [server-client.md](server-client.md#docking-and-travel).

A queued `Order::Slip` holds a typed `Destination`, including beacon-relative and
celestial-relative references. Firmware predicts its future pose and supplies
concrete galactic candidates through `ProgramAction::Slip`. Updating a charging
candidate recomputes required energy; it does not restart preparation. The host
validates route geometry before changing the queue and checks aperture admission
again during preparation and arrival. Blocking cancels an unfinished charge.
Candidate changes and cancellation do not refund energy already spent.

### Availability

Every production flight computer uses the server's fused scan and world-service provider, including ships viewed through `toy-sim-debug`. `world_command` stages validated actions for dispatch on the server. A standalone runtime invocation without a provider returns `ERR_UNAVAILABLE` for world queries.

## Display entry point

`ship_display` is a second entry point for drawing screens. `ControllerRuntime::instantiate_display(bytes)` creates a display controller:

- It fails if the module has no `ship_display` export.
- It boots at once and starts with `CALLBACK_START_GAS`.
- Each callback calls `ship_display` instead of `ship_tick`.

A display instance is a separate WebAssembly instance with its own memory, session, screens and query cursors. Its boot and execution consume the ship's shared physical tick allowance and the same owner's gas account. It shares no memory with the flight instance. The same imports are linked, with these differences:

- `device_write` and `world_command` return `ERR_ARGUMENT`.
- `world_query` uses the display cursor store.

The authoritative server creates a display instance only while a network client subscribes to one of the ship's screens. It passes the flight computer's latest observation with requests removed, sets `requested_screens` to the due subscribed slots, delivers screen input from clients, and keeps only screen frames from the result. Other publications from a display instance are ignored. The lifecycle is described in [server-client.md](server-client.md#display-instances).

The server invokes `ship_display` only for subscribed screens. Production clients receive those display frames; the flight callback does not serve MFD windows.

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
| `REQUEST_MARK_TARGET` | 7 | `MarkTargetRequest { contact, maximum_flight_time_s }` (16) |
| `REQUEST_STOP_FIRING` | 8 | none |
| `REQUEST_UNMARK_TARGET` | 9 | none |
| `REQUEST_START_FIRING` | 10 | none |
| `REQUEST_THROTTLE` | 11 | `ThrottleRequest { throttle }` (8) |

A request stays pending until its reply commits at a successful slice boundary. The input batch is fixed for a callback, including its resumed slices; new inputs wait for the next callback. The host queue holds at most 256 requests. A new manual request replaces any queued manual request.

## Snapshots and spatial publications

Each successful `tick_read` lends the program one observation snapshot containing
the simulation time, the ship's exact galactic position, its velocity and its
rotation. That borrowed snapshot stays valid across implicit gas suspension until
the next explicit successful `tick_read` or callback completion. Host observations
refresh on resume; the borrowed snapshot retains its original epoch and pose.
Positions in `SNAPSHOT` frames are metres relative to that exact origin, so
publications stay precise at any galactic coordinate.

| Import | Notes |
| --- | --- |
| `snapshot_keep(id)` | Pin a valid borrowed snapshot, or keep an already pinned one. Costs 100 gas. At most 8 pins (`ERR_LIMIT`). Unknown IDs return `ERR_HANDLE`. |
| `snapshot_drop(id)` | Release a pin |
| `spatial_marker_put(marker, bytes)` | `SpatialMarker` (176) |
| `spatial_path_put(path, bytes, vertices, count)` | `SpatialPath` (152) plus `count` × `SpatialVertex` (32). `count` is 2 to 128. Costs 100 gas per vertex. |
| `spatial_remove(id)` | Remove a marker or path |
| `spatial_clear()` | Remove all markers and paths |

Pin a snapshot before another `tick_read` or callback completion when it must
remain available longer. There is one active borrowed snapshot and at most
eight explicit pins; suspended code does not accumulate an unbounded history.

`SpatialMeta` holds `id` (non-zero, one namespace shared by markers and paths), `role`, `valid_until_s` (a finite lease deadline) and a `label`. Publications disappear when their lease expires. Firmware refreshes them by publishing again.

An expired publication returns success without replacing existing data. The host
still validates record sizes, memory ranges, finite values, frame kinds, and
vertex ordering. It does not resolve expired publications' snapshot, contact,
path, or aim-marker references: those objects may have expired while the callback
was suspended. Live publications must resolve their references normally.

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

Well-formed expired instruments return success without publishing or replacing
newer state. Their scalar and device-handle validation still applies; an expired
contacts instrument does not require a current scan. Non-finite lease deadlines
remain invalid.

| Import | Record | Validation |
| --- | --- | --- |
| `instrument_attitude_put` | `AttitudeState` (64) | `mode` ≤ `ATTITUDE_GUIDANCE` (0 manual, 1 hold, 2 guidance). `present` 0 (reference all zero) or `ATTITUDE_REFERENCE` 1 (unit quaternion). `control_error` ≥ 0. |
| `instrument_navigation_put` | `NavigationState` (368) | `status` ≤ `NAV_UNAVAILABLE` (0 idle, 1 active, 2 suspended, 3 unavailable). `throttle` and `throttle_limit` in [0, 1]. `present` bits: `NAV_STAND_OFF` 1, `NAV_SPEED_LIMIT` 2, `NAV_BRAKING_DISTANCE` 4, `NAV_ARRIVAL` 8, `NAV_FUEL` 16. Each measurement is finite and non-negative, and zero when its bit is clear. `reason` is valid text. |
| `instrument_contacts_put` | `ContactsState` (16) | Requires a scan within the last 2 s. Publishes that scan's contact list with `selected_contact`. |
| `instrument_weapons_put(state, bytes, rows, count)` | `WeaponsState` (288) plus `count` × `WeaponInstrument` (128) | `mode` 0 hold or 1 firing. Each row names a distinct weapon device, `solution_flags` ≤ 1, finite non-negative times and errors, and `aim_marker` 0 or an existing marker. Costs 100 gas per row. |
| `instrument_clear(kind)` | | `INSTRUMENT_ATTITUDE` 0, `INSTRUMENT_NAVIGATION` 1, `INSTRUMENT_CONTACTS` 2, `INSTRUMENT_WEAPONS` 3 |

The navigation record also names `target_contact`, `own_path` and `target_path`. The orbit overlay uses these to find the plan and target forecast ([orbital-navigation.md](orbital-navigation.md)).

## Screens

`screen_define`, `screen_remove`, `screen_begin`, `screen_draw`, `screen_button`, `screen_end`, `screen_event_read` and `screen_event_ack` are documented in [mfds.md](mfds.md). They work from both `ship_tick` and `ship_display`. Production clients receive frames drawn by the separate `ship_display` instance.

## Writing firmware

### Rust

Depend on `toy-sim-ship-api`. `abi::raw` declares the imports for `wasm32` targets. `sdk` wraps them with `Result<_, i32>` helpers: `tick`, `budget`, `flight`, `resources`, `device`, `device_spec`, `device_read`, `device_write`, `scan`, `request`, `request_read`, `request_reply`, `marker`, `path`, `attitude`, `navigation`, `contacts`, `weapons`, the screen calls, and generic `read`/`write` over any `Record`.

The minimal `no_std` example is [examples/embedded.rs](../crates/toy-sim-ship-api/examples/embedded.rs). It publishes a two-vertex forecast and sets every engine to 25% throttle. The standard firmware in [toy-sim-example-controller](../crates/toy-sim-example-controller) uses `std` collections and exports `ship_api_version` and `ship_tick` from [firmware.rs](../crates/toy-sim-example-controller/src/firmware.rs) behind the default `firmware` feature. It also exports a drawing-only `ship_display` that draws a "Ship status" text screen (simulation time, speed, mass and battery energy) on every requested slot; it ignores screen events and does not call world services. On `wasm32`, its `Computer` runs the current-order executor in [world.rs](../crates/toy-sim-example-controller/src/world.rs). It uses `world_query` to read the active host-owned command and `world_command` to report estimates, completion, or physical actions guarded by the queue revision and order index. Route search is provided by the [server routing service](server-client.md#travel-orders-and-server-planning).

[examples/custom_screen.rs](../crates/toy-sim-example-controller/examples/custom_screen.rs) exports `ship_tick`, which runs the standard `Computer`, and `ship_display`, which draws the "Custom diagnostics" screen. It is built with `--no-default-features` so the library does not export the entry points a second time. Its screen is drawn by a display instance when a remote or debug client subscribes.

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

Include [ship.h](../crates/toy-sim-ship-api/include/ship.h). It declares `ship_<name>` imports with the correct import module and names, `ship_*_record` structs with layout assertions, and `SHIP_*` constants. [tests/fixtures/controller.c](../crates/toy-sim-ship-wasm/tests/fixtures/controller.c) is a freestanding example with no libc: it provides its own `memset`, exports `ship_api_version` and `ship_tick`, writes a throttle, and publishes an attitude record and a timed path. Run `tools/build_ship_firmware.sh` to rebuild the standard controller and all C/Rust firmware fixtures with the current ABI.

### AssemblyScript

[ship.ts](../crates/toy-sim-ship-api/bindings/ship.ts) declares the imports with `@external("ship_v30", …)` and exports constants plus `<RECORD>_<FIELD>` byte offsets and `<RECORD>_SIZE` values for working with raw buffers.

### Regenerating bindings

After changing `abi.rs`, regenerate both binding files:

```sh
python3 tools/generate_ship_bindings.py
```

The script parses the record structs, `Text` sizes, integer constants and the `raw` import declarations in `abi.rs`. It computes offsets assuming 8-byte fields, then writes `include/ship.h` and `bindings/ship.ts`.

## Host API

For isolated ABI tests or tools that embed the runtime:

- `ControllerRuntime::new()`, `compile(bytes)`, `validate_program(bytes)`, `instantiate(bytes)` and `instantiate_display(bytes)` (both return a booting controller), `cached_modules()`
- `Controller::configure_hardware(design, catalogue)` installs the device and resource directories.
- `is_booting()`, `boot_progress()`, `boot_remaining_gas()`, `execution_status()`, `is_suspended()`, `minimum_to_progress()` and `memory_bytes()` expose state without replenishing any gas.
- `run_slice(input, source, grant, gas_per_tick)` advances paid boot, starts a callback or resumes its continuation. It returns `SliceOutput { output, callback_completed }`. `last_gas_used` records actual consumption even if execution traps.
- `run_callback_slice(kind, input, source, missile, grant, gas_per_tick)` selects `CallbackKind::Ship`, `Display` or `Missile(handle)`. A missile callback requires the matching `MissileObservation`; `pending_callback()` identifies a suspended callback. `SliceOutput.callback` identifies the callback producing its committed output.
- `ScanSource` has `scan(range_m, n)`, `query_work(&ProgramQuery)` and `query(ProgramQuery, display, reply_capacity)`. The provider supplies a bounded admission price before executing a query.
- `reboot()`, `revoke_authority()` (drops pending requests and screen events, then reboots), `fail(message)`, `enqueue_screen_event(event)`, `has_pending_input()`
- State fields: `state` (the committed `Session`), `fault`, `contacts`, `scan_time`, `telemetry`, `screens`, `trajectory_revision`, `instrument_interest`, `observer_origin`
- `CallbackSchedule` implements the interval logic. Call `advance(dt)` once per tick, check `ready(has_input)`, and call `completed(output.tick_interval_seconds)` after a successful callback.
- `Input` carries tick, dt, physics dt, `Observation` (time, flight state, resources, inventory), device statuses, requests, screen events and requested screens. `Output` carries staged world actions, device commands, replies, screen frames, cleared screens and the interval.
- `screens` re-exports `toy_sim_model::drawing`, where the validated screen frame types now live.

## Tests

```sh
cargo test -p toy-sim-ship-wasm
```

[tests/sandbox.rs](../crates/toy-sim-ship-wasm/tests/sandbox.rs) covers:

- rejection of old versions and foreign imports
- C and Rust programs sharing the ABI with isolated memory
- memory growth to 1 MiB and reset on reboot
- bounded execution and continuation across gas slices
- exact buffer lengths
- path snapshots and rejected replacements
- lease expiry
- trap rollback
- scan gas reservation
- clearing old requests on reboot and delivering new requests submitted during startup
- unfinished screen frames
- snapshot quotas
- the custom screen firmware running as a display instance, with no attitude or navigation instruments published
- bundled firmware forecasts, weapons engagement and pursuit within budget
- weapon setting validation
- the contacts instrument on a rotated ship
