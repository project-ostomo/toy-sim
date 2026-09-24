# Ship controller ABI

A flight computer runs a WebAssembly module whose callbacks are scheduled by the host.
Imports use the `ship` module and fixed little-endian `#[repr(C)]` Rust records,
scalar arguments, and caller-owned arrays or byte buffers. The shared
`GAME_VERSION` controls firmware, network and save compatibility.

The standard flight program executes the server's current travel command.
Persistent guest data survives checkpoints; execution stacks and linear memory
restart. The runtime meters guest execution and native host work.

The Rust definitions are the ABI source. C and TypeScript bindings are not maintained.

- Record definitions and constants: [crates/osg-ship-api/src/abi.rs](../crates/osg-ship-api/src/abi.rs)
- Rust helpers: [crates/osg-ship-api/src/sdk.rs](../crates/osg-ship-api/src/sdk.rs)
- Host implementation: [crates/osg-ship-wasm](../crates/osg-ship-wasm)

For the hardware that devices represent, see [ships.md](ships.md). Screen drawing is covered in [mfds.md](mfds.md), and weapons in [weapons.md](weapons.md).

## Module requirements

`ControllerRuntime::compile` accepts a module when all of the following hold:

- It is at most 1 MiB.
- Every import comes from module `ship` and is one of the names in `abi::IMPORTS`.
- It exports `memory`: 32-bit, not shared, with an initial size of at most 128 pages.
- It exports `ship_tick` with no parameters and no results.
- It exports `game_version` as a defined function with no parameters, one `i32` result and no locals. Its body is exactly `i32.const GAME_VERSION; end`, allowing the host to verify the ABI without running guest code.

`ship_display` is optional and not checked at compile time. A display instance requires it to exist, with no parameters and no results.

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
- **Faults.** Invalid guest memory, traps and rejected hardware commands still reboot the computer. The instance is dropped and session state is cleared: instruments, spatial publications, tracks, pinned snapshots, screens and pending screen events. Work already consumed remains charged. The fault message is kept until the next successful boot. The server disables autopilot and retains its itinerary with a failure reason; staged actions, slip preparation and docking reservations are cleared. New requests submitted during startup are delivered after boot.

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
| Gun | 9 | `GunSpec` (160; nested mount `WeaponSpec` is 120) | `WeaponReading` (72) | `SET_WEAPON` |
| Laser | 11 | `LaserSpec` (152; nested mount `WeaponSpec` is 120) | `WeaponReading` (72) | `SET_WEAPON` |
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

## Sensor observations

`sensor_scan(sensor, maximum, contacts, bytes)` fills up to `maximum` (at most 256) `Contact` records (72 bytes each), and `bytes` must equal `maximum × 72`. It returns the number written. The sensor device must be operational and powered with a non-zero range, otherwise the call returns `ERR_UNAVAILABLE`. It charges `maximum × 3000` gas (`SCAN_GAS_PER_OBJECT`) before querying.

The server answers from the ship's current sensor snapshot, selecting the nearest detected ships within the requested range. Measurements are exact and published once per simulation tick. The range is limited by the powered sensor. Debug accounts can override range and occlusion.

`Contact` fields are `id` (an opaque per-ship handle for a current detection), `kind` (`CONTACT_SHIP` 0), `radius_m`, and position and velocity relative to the ship. Celestial bodies are excluded from sensor contacts; use explicit celestial world queries for navigation. Contact loss immediately invalidates live access. Reacquisition creates a fresh handle.

The standard firmware starts with a 32-contact scan buffer, grows it when full and reduces it for sparse results. It leaves room for flight control and publication before admitting optional scans, forecasts or catalogue pages.

Every successful scan is admitted into host-side tracks, up to 512. Each track keeps its latest and previous estimates. Tracks expire 2 s after their latest measurement. Historical samples support trajectory prediction only. `contact_label(id, out, bytes)` requires a current detection and returns its current advertised label or an anonymous label as `Text64`.

`contact_iff(id, out, bytes)` returns the fixed `ContactIff` record: presence, target UUID, advertised owner, optional faction and up to 16 `Text64` labels. A detected target with IFF disabled has presence zero and cleared identity fields. A lost contact returns `ERR_UNAVAILABLE`. IFF has no separate range.

## World services

Typed imports connect firmware to the authoritative travel, beacon and sensor services. Records live in `ship-api/src/world.rs` and `beacons.rs`; these Rust C-ABI definitions specify their layouts and constants. UUIDs use 16 bytes, galactic positions use six `u64` words, and enum choices use explicit integer tags.

| Import | Notes |
| --- | --- |
| `contact_get`, `destination_resolve`, `slip_eligibility` | Read fixed output records. |
| `travel_read` | Fills autopilot metadata plus caller-owned itinerary, fuel and location-hierarchy arrays; status contains fixed-capacity plan markers. |
| `orrery_read`, `orrery_system_read` | Fill caller-owned celestial obstacle records and a page header; the system query predicts a named system at a future offset. |
| `slip_eligibility_batch` | Evaluates an array of future-time slip probes against one published snapshot and fills corresponding results. |
| `beacon_read`, `beacons_read` | Fill record arrays, page metadata, and a separate byte arena for variable-length fields. |
| `route_request`, `route_poll` | Fill route metadata plus separate itinerary and fuel-requirement arrays. |
| `travel_use_route`, `travel_fail`, `travel_publish_status`, `travel_complete`, `travel_slip`, `travel_cancel_slip`, `travel_reserve_bay`, `travel_dock`, `travel_undock` | Stage typed itinerary publications or physical actions. |

### Query buffers and gas

**Buffers.** Array capacities count records; arena capacities count bytes. The program allocates enough storage for the requested result. Host admission validates memory ranges before service work. Fixed results require their declared layout; atomic results that cannot fit return `ERR_BUFFER`. Bounded collection queries report completion, truncation or continuation explicitly. Track pages retain unreturned records for a later page; this is pagination, not automatic buffer resizing or re-execution. Variable fields use offsets into the returned arena, never host pointers. Unused caller capacity is not charged as copied data.

**Gas.**

1. The call cost (100) plus one gas per 8 input bytes.
2. Admission for bounded query work and the output-copy allowance. `Tracks` and `Continue` work is capped by the physical tick ceiling after allowing for the call and input/output copies. The remaining query prices are `100 + 1008 × min(limit, 256)` for `Beacons`, 131,072 for `SlipEligibility`, and 1000 for other queries. A call that fits the physical ceiling can suspend until its slice can pay; a call exceeding that ceiling returns `ERR_LIMIT`.
3. The actual bounded native work.
4. One gas per 8 reply bytes.

`Beacon` and `Beacons` also admit 100 gas per inspected docking bay. One call may
inspect at most 4096 bays across its results; a larger page returns `ERR_LIMIT`
without silently truncating its bay lists.

**Errors.** Invalid record values or a failed query return `ERR_ARGUMENT`. A host with no world provider returns `ERR_UNAVAILABLE`. Invalid guest memory traps.

| `ProgramQuery` | Reply |
| --- | --- |
| `Travel` | Complete `AutopilotState`, own pose, presence, cached location, tick, exotic fuel, slip readiness and drive axis. |
| `RouteRequest(Request)` | Enqueues an idempotent server planning job and returns `Route { id, status }`. The request contains a nonzero ID, the requested directive list and planning preferences. |
| `RoutePoll { id }` | Returns the scoped job's `Unknown`, `Pending { progress }`, `Ready { plan }` or `Failed { reason }` status. |
| `Orrery { reference }`, `OrrerySystem { system, after_seconds }` | Current nearby or predicted named-system celestial geometry. |
| `SlipEligibilityBatch(probes)` | Per-candidate readiness, timing or error against a shared snapshot. |
| `Beacon(entity)` | `Beacons` with zero or one beacon |
| `Beacons { after, limit }` | `Beacons` in entity ID order, with `limit` from 1 to 256 |
| `SlipEligibility { origin, destination, departure_after_seconds, arrival_after_seconds, navigation_beacon }` | `SlipEligibility { ready, preparation_s, duration_s }`: departure clearance, guidance availability, remaining preparation time, and flight duration at the fixed guided or unguided speed. |
| `Resolve { destination, after_seconds }` | Predicted `Pose` of a galactic position, beacon, or offset from a beacon or celestial body. Celestials use locally resolved ephemerides; other objects extrapolate current linear and angular motion. |

Celestial queries provide physical and slip-exclusion radii from public ephemerides.
Firmware chooses maneuvers from these observations. Batch slip probes return one
result per candidate against the same published geometry snapshot.

Route request and polling each admit 8192 work gas in addition to the normal
call and copy costs. Search runs outside the non-preemptible syscall;
the public routing service does not debit the owner's account for search work.
Requests and ready plans are bounded to 256 directives. Submission
checks reply capacity before accepting or changing a job. Jobs are scoped to the
current world, ship, owner and control revision. Commit also requires the
expected directive revision. Preview age, ordinary movement and unrelated global
topology changes do not expire the plan. Its topology revision records the
planning context. The executor resolves current geometry within each strategic
command. Jobs do not reveal another ship's route or observations.

Prediction offsets are relative to the current query epoch and must be finite,
nonnegative and at most one Julian year (`365.25 × 86400` seconds). A slip arrival
offset must be at least its departure offset. `SlipEligibility` checks current
drive readiness and departure clearance.
Preparation estimates include the remaining energy and minimum preparation delay,
rounded to future simulation ticks because charging runs before the firmware
callback. Natural capture resolves the intersection time within a simulation tick.

Track queries are metered as described in [server-client.md](server-client.md#metered-queries). A cursor expires 10 ticks after its query started. Each ship keeps separate cursor stores for its flight instance and its display instance.

### Travel actions

A slice can stage at most eight actions. Display instances cannot stage flight
actions. Successful slices commit actions in ship ID and staging order.
Completion, failure and status publications carry a directive generation;
stale publications are ignored. Physical actions validate ownership, resources
and physical admission independently of itinerary indices.

| `ProgramAction` | Effect |
| --- | --- |
| `UseRoute { id, directive_revision, engage }` | Commits an authorized ready itinerary. |
| `Fail { directive_revision, reason }` | Disables autopilot and retains its itinerary and failure reason. |
| `PublishStatus { directive_revision, status }` | Publishes phase, timing, capture geometry, velocity change, risk and plan markers. |
| `Complete { directive_revision }` | Removes the completed head directive and advances the generation. |
| `Slip { destination, navigation_beacon, arrival_velocity, not_before_tick }` | Starts or updates charging toward a galactic aim, with optional arrival velocity and earliest departure tick. |
| `CancelSlip` | Cancels unfinished preparation without refunding energy. |
| `ReserveBay { station, bay }`, `Dock { station, bay }`, `Undock` | Performs the corresponding physical bay operation. |

The host rules are in [server-client.md](server-client.md#docking-and-travel).
Firmware privately plans each `SlipToSystem` or `DockAt` directive. Concrete
aim points are physical requests, rather than persisted itinerary waypoints.
Updating a charging candidate preserves accumulated work. Transit ends at
natural capture, collision or fuel exhaustion. Velocity change is limited by
actual distance traveled and consumes exotic fuel; partial transit earns only
its corresponding velocity allowance. Cancellation does not refund energy.

The flight instance uses ordinary WASM suspension during planning. A long
search may delay control across ticks. Firmware rechecks its directive and
geometry before executing a completed search. Waiting has no timeout.

### Availability

Every production flight computer uses the server's per-ship sensor scan and world-service provider, including ships viewed through `osg-debug`. Travel action imports stage validated actions for dispatch on the server. A standalone runtime invocation without a provider returns `ERR_UNAVAILABLE` for world queries.

## Display entry point

`ship_display` is a second entry point for drawing screens. `ControllerRuntime::instantiate_display(bytes)` creates a display controller:

- It fails if the module has no `ship_display` export.
- It boots at once and starts with `CALLBACK_START_GAS`.
- Each callback calls `ship_display` instead of `ship_tick`.

A display instance is a separate WebAssembly instance with its own memory, session, screens and query cursors. Its boot and execution consume the ship's shared physical tick allowance and the same owner's gas account. It shares no memory with the flight instance. The same imports are linked, with these differences:

- `device_write` rejects writes, and travel action imports return `ERR_UNAVAILABLE`.
- Intelligence queries use the display cursor store.

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

Depend on `osg-ship-api`. `abi::raw` declares the imports for `wasm32` targets. `sdk` wraps them with `Result<_, i32>` helpers: `tick`, `budget`, `flight`, `resources`, `device`, `device_spec`, `device_read`, `device_write`, `scan`, `request`, `request_read`, `request_reply`, `marker`, `path`, `attitude`, `navigation`, `contacts`, `weapons`, the screen calls, and generic `read`/`write` over any `Record`.

The minimal `no_std` example is [examples/embedded.rs](../crates/osg-ship-api/examples/embedded.rs). The standard firmware in [osg-example-controller](../crates/osg-example-controller) exports `game_version`, `ship_tick` and the drawing-only `ship_display`. Its flight `Computer` privately plans and executes directives read through `travel_read`; typed imports publish status, completion, failure and physical actions. Directive generations protect publications against stale searches. The display instance reads the flight publication. Model helpers convert C records into Rust values without serialization. Strategic route search is provided by the [server routing service](server-client.md#autopilot-itineraries-and-firmware-planning).

[examples/custom_screen.rs](../crates/osg-example-controller/examples/custom_screen.rs) exports `ship_tick`, which runs the standard `Computer`, and `ship_display`, which draws the "Custom diagnostics" screen. It is built with `--no-default-features` so the library does not export the entry points a second time. Its screen is drawn by a display instance when a remote or debug client subscribes.

The repository has no build script for firmware. These commands follow from the manifests and the comment in `custom_screen.rs`:

```sh
rustup target add wasm32-unknown-unknown

# Standard firmware (cdylib): target/wasm32-unknown-unknown/release/osg_example_controller.wasm
cargo build -p osg-example-controller --release --target wasm32-unknown-unknown

# Custom screen example: target/wasm32-unknown-unknown/release/examples/custom_screen.wasm
cargo build -p osg-example-controller --release --target wasm32-unknown-unknown \
    --example custom_screen --no-default-features
```

The simulator embeds the standard firmware from `crates/osg-ships/data/example-controller.wasm`. After changing the firmware source, copy the new build over that file and rebuild. The test fixtures in `crates/osg-ship-wasm/tests/fixtures/` are also prebuilt binaries.

## Host API

For isolated ABI tests or tools that embed the runtime:

- `ControllerRuntime::new()`, `compile(bytes)`, `validate_program(bytes)`, `instantiate(bytes)` and `instantiate_display(bytes)` (both return a booting controller), `cached_modules()`
- `Controller::configure_hardware(design, catalogue)` installs the device and resource directories.
- `is_booting()`, `boot_progress()`, `boot_remaining_gas()`, `execution_status()`, `is_suspended()`, `minimum_to_progress()` and `memory_bytes()` expose state without replenishing any gas.
- `run_slice(input, source, grant, gas_per_tick)` advances paid boot, starts a callback or resumes its continuation. It returns `SliceOutput { output, callback_completed }`. `last_gas_used` records actual consumption even if execution traps.
- `ScanSource` has `scan(range_m, n)`, `query_work(&ProgramQuery)` and `query(ProgramQuery, display, reply_capacity)`. The provider supplies a bounded admission price before executing a query.
- `reboot()`, `revoke_authority()` (drops pending requests and screen events, then reboots), `fail(message)`, `enqueue_screen_event(event)`, `has_pending_input()`
- State fields: `state` (the committed `Session`), `fault`, `contacts`, `scan_time`, `telemetry`, `screens`, `trajectory_revision`, `instrument_interest`, `observer_origin`
- `CallbackSchedule` implements the interval logic. Call `advance(dt)` once per tick, check `ready(has_input)`, and call `completed(output.tick_interval_seconds)` after a successful callback.
- `Input` carries tick, dt, physics dt, `Observation` (time, flight state, resources, inventory), device statuses, requests, screen events and requested screens. `Output` carries staged world actions, device commands, replies, screen frames, cleared screens and the interval.
- `screens` re-exports `osg_model::drawing`, where the validated screen frame types now live.

## Tests

```sh
cargo test -p osg-ship-wasm
```

[tests/sandbox.rs](../crates/osg-ship-wasm/tests/sandbox.rs) covers:

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
