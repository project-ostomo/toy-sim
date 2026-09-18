# Authoritative server and network client

The simulation runs only in `toy-sim-server`. Every UI is a network client: the standalone `toy-sim-client`, and `toy-sim-debug`, which starts its own server process and connects to it over loopback TCP. This guide describes the server process, its configuration, the transport and application protocol, the intelligence model that decides what each account can see, presentation data, display instances, docking and travel, client playback, the client UI, and the benchmark.

There is no persistence. The world, accounts' information-group keys, tracks and sessions exist only in memory. Every server start and every debug reset creates a new world ID. Input frames that carry another world ID are discarded, allowing connections to survive inputs already in transit during a reset.

## Crates

| Package | Path | Role |
| --- | --- | --- |
| `toy-sim-model` | [crates/toy-sim-model](../crates/toy-sim-model) | Shared serde types: IDs, poses, tags, tracks, queries, frames, actions, debug commands, presentation records, travel orders, drawing lists, and the program query/action types used by `world_query`/`world_command` |
| `toy-sim-protocol` | [crates/toy-sim-protocol](../crates/toy-sim-protocol) | Application message framing, sections and validation limits |
| `toy-sim-net` | [crates/toy-sim-net](../crates/toy-sim-net) | TCP handshake, record encryption, Zstd compression and picomux multiplexing |
| `toy-sim-intel` | [crates/toy-sim-intel](../crates/toy-sim-intel) | Measurements, immutable track snapshots and metered queries |
| `toy-sim-universe` | [crates/toy-sim-universe](../crates/toy-sim-universe) | Orrery configuration, Keplerian solver, star catalogue and atmosphere tables |
| `toy-sim-server` | [crates/toy-sim-server](../crates/toy-sim-server) | The Bevy ECS simulation in private modules under [src/sim](../crates/toy-sim-server/src/sim), the simulation loop, TCP listener, asset streams, configuration, key provisioning and the benchmark example |
| `toy-sim-client` | [crates/toy-sim-client](../crates/toy-sim-client) | `connect`, asset fetching, the `Playback` buffer, and the Bevy/egui UI behind the `ui` feature |
| `toy-sim-debug` | [apps/toy-sim-debug](../apps/toy-sim-debug) | Local launcher: server child process plus the client UI |
| picomux (vendored) | [vendor/picomux](../vendor/picomux) | picomux 0.2.1 with project-local changes, patched in through `[patch.crates-io]` in the workspace manifest |

The server modules are private to `toy-sim-server`. Its public library interface is `launch::run`, `provision::demo`, `run`, `listen`, `scenario`, `assets` and `key_bytes`.

## Running

```sh
# Build the server and the debug launcher in the same profile
cargo build -p toy-sim-server -p toy-sim-debug

# Local server with the client UI and debug access
cargo run
cargo run -- --ship assets/ships/starter.ship
cargo run -- --server target/debug/toy-sim-server
cargo run -- --check

# Generate demo keys and configuration
cargo run -p toy-sim-server -- --init demo

# Dedicated server (stops on Ctrl-C)
cargo run -p toy-sim-server -- demo/server.toml

# Remote client
cargo run -p toy-sim-client --features ui -- demo/client-a.toml
```

### Server command line

```
toy-sim-server --init <directory>
toy-sim-server <config.toml> [--ready-file PATH] [--shutdown-on-stdin-close]
```

- `--init <directory>` creates the directory if needed and writes `server.toml`, `client-a.toml` and `client-b.toml`. It generates a server Ed25519 key and two accounts with random UUIDs and Ed25519 keys. The files are created with mode `0600` on Unix and contain private keys. The command refuses to run if any of the three files exists. The addresses are `127.0.0.1:23000`. It does not write `debug_account` or `ship`.
- `--ready-file PATH` writes the actual listening address (for example `127.0.0.1:41234`) to `PATH` once the scenario is built and the listener is bound. The file is removed on shutdown.
- `--shutdown-on-stdin-close` starts a thread that reads standard input and stops the server when it reaches end of file or fails. A parent process uses this to tie the server's lifetime to its own.

Any other argument is an error. The server also stops on Ctrl-C and when the simulation thread ends.

### Configuration files

Configuration files reject unknown fields.

| File | Fields |
| --- | --- |
| Server | `listen` (a socket address; port 0 picks a free port); `server_secret` (64 hex characters, the Ed25519 secret key); `[[accounts]]` entries with `id` (UUID) and `public_key` (64 hex); optional `debug_account` (UUID); optional `ship` (path to a `.ship` blueprint, relative paths resolved against the configuration file's directory) |
| Client | `address`; `server_public_key` (64 hex); `account` (UUID); `account_secret` (64 hex) |

`debug_account` must be one of the configured accounts, or startup fails with "debug account must be authorized". That account gets the debug capabilities described under [Debug accounts](#debug-accounts).

### Debug launcher

`toy-sim-debug [--ship PATH] [--server EXECUTABLE] [--check]` ([main.rs](../apps/toy-sim-debug/src/main.rs)):

- Without `--server`, the server executable is `toy-sim-server` in the same directory as the launcher's executable. If it is not a file, the launcher fails with "build toy-sim-server first, or pass --server EXECUTABLE".
- It creates a temporary directory (mode `0700` on Unix) and writes `server.toml` (mode `0600`) with `listen = "127.0.0.1:0"`, a random server key, one random account that is also `debug_account`, and the canonicalized `--ship` path.
- It starts the server with `--ready-file` and `--shutdown-on-stdin-close` and a piped standard input. It waits up to 60 s for a parseable address in the ready file, and fails if the server exits first.
- It connects with `toy_sim_client::connect`. The debug session uses the same TCP, handshake, encryption, compression, multiplexing and session code as a remote client.
- With `--check`, it waits up to 10 s for a state frame, fails with "server did not provision the debug ship" if the frame has no ship telemetry, and prints the address, tick and ship count. Otherwise it runs the client UI with a playback target depth of 1.
- On exit, it drops the server's standard input, waits up to 5 s, kills the server if it is still running, and removes the temporary directory.

The ship editor's "Launch sim" runs `toy-sim-debug --ship <snapshot>` ([ship-editor.md](ship-editor.md#launching-the-debug-client)).

### Scenario

[bootstrap.rs](../crates/toy-sim-server/src/sim/bootstrap.rs) builds the world from the Bevy application in [sim/mod.rs](../crates/toy-sim-server/src/sim/mod.rs):

- **Universe.** The Helion system from `toy_sim_universe::example_config()`. Celestial bodies get stable UUIDs derived from their names.
- **Explorer.** "Patrol ship" starts in a circular orbit 40,000 km above Helion I Neris, on the day side, with the tangential direction selected from seed 42 ([scenario.rs](../crates/toy-sim-server/src/sim/scenario.rs)). Its design is the configured `ship` blueprint, or the bundled micropulse patrol ship. If the blueprint fails to load or compile, the server logs "Cannot load ship" and spawns no ships at all, so scenario setup fails.
- **Hostile patrol.** "Hostile patrol 001" uses the bundled micropulse patrol design, starts 1 km from the player with the same initial velocity, and points at it. Its computer receives aim and engage requests after initial sensor publication and fires once booted.
- **Players.** The first configured account controls the explorer. Account *n* (from 1) gets "Explorer *n+1*", with the explorer's design and velocity, offset by *n* × 1,000 m along galactic +Y. Every player ship gets a default slipdrive.
- **Ownership.** The hostile patrol belongs to a separate random account. The default scenario has no additional station or gate ships.
- Every ship gets a test loadout, and every flight computer boots for 5 s.

After spawning, bootstrap runs identity, acquisition, coasting, fusion and publication once, so the first state frame already has tracks.

## Server loop

`toy_sim_server::launch::run` ([launch.rs](../crates/toy-sim-server/src/launch.rs)) reads the configuration, binds the listener, and builds the scenario on a thread named `simulation`. Once the scenario and its asset map exist, it writes the ready file and runs the listener. `toy_sim_server::run` ([lib.rs](../crates/toy-sim-server/src/lib.rs)) owns the Bevy `App` on that thread and loops every 100 ms of wall-clock time:

1. Accepts new connections into sessions. If 1024 sessions already exist, the connection is dropped.
2. Applies up to 4 queued input frames per session. A frame that fails validation, or a closed input channel, disconnects that session. Valid frames for another world are discarded before checking session sequence numbers or acknowledgements.
3. If a debug reset was requested, rebuilds the scenario from the same configuration and reconnects every existing connection to the new world.
4. Applies queued debug requests ([Debug accounts](#debug-accounts)).
5. Runs simulation ticks. At a clock rate of 0, it runs one queued single step if there is one. Otherwise it adds the rate to a tick credit and runs the whole number of ticks in the credit, so a rate of 10 runs 10 ticks in one loop iteration. Each tick is one `App::update`, and its wall-clock duration is recorded for diagnostics.
6. Updates display instances once ([Display instances](#display-instances)).
7. Builds one `Frame` per session and publishes it with `watch::Sender::send_replace`. Only the newest frame is kept. A client that cannot keep up misses frames and sees gaps in `sequence`. A session whose frame cannot be built is disconnected.
8. Sleeps until the next 100 ms deadline. If it is already late, the next deadline starts from now.

The loop exits when the listener side has closed and no sessions remain, or when the stop flag is set.

The listener accepts at most 1024 concurrent TCP connections through a semaphore. Connections beyond that are closed immediately. Each connection must complete the handshake within 10 s and open its `main` stream within a further 10 s. A state frame write that takes more than 10 s ends the connection.

### Simulation tick

One tick runs these Bevy schedules ([sim/mod.rs](../crates/toy-sim-server/src/sim/mod.rs), [simulation.rs](../crates/toy-sim-server/src/sim/simulation.rs)):

- **`FixedFirst`.** Travel advances: docked orders, due gate transfers, slip arrivals and slip preparation ([Docking and travel](#docking-and-travel)).
- **`FixedUpdate`.** Star systems are activated for ships inside their influence radius. World-service indexes (beacons, celestial poses, the public snapshot, slip apertures) are published and each ship's world source is prepared. Then, in order: history, `PrepareBodies` (flight programs and typed hardware systems), `Forces` (gravity and drag).
- **`FixedPostUpdate`.** Firmware world actions are applied in ship ID order. Then `Integrate` (rigid bodies and the collision solver) and `Celestials`.
- **`FixedLast`.** The spatial index is rebuilt, sensor scans run, and the tick counter increments. Then intelligence runs: travel geometry refresh, celestial identities, index cleanup, acquisition, coasting, fusion, publication, and recording of travel events.

Ship hardware lives in ECS components ([hardware.rs](../crates/toy-sim-server/src/sim/hardware.rs)): `ShipInventory`, `Hull`, `ShipThermal`, `Avionics`, `DeviceSettings`, `HardwareClock` and `SensorRange` on the ship, and one entity per installed part with `InstalledPart`, `Device`, typed device components such as generators, engines, RCS, torquers and shields ([devices.rs](../crates/toy-sim-server/src/sim/hardware/devices.rs)), and `Weapon` where present. `toy_sim_ships::ShipState` is used to build these components when a ship spawns, resets or is recovered, and `hardware::snapshot` assembles one from them for presentation and collision damage. The simulation does not step a `ShipState`.

Flight programs run in parallel across ships. A program is called only when the interval it set with `tick_set_interval` has elapsed or requests are waiting ([ship-abi.md](ship-abi.md#scheduling)). At most `MAX_BOOTS_PER_TICK` computers boot per tick.

## Transport stack

```
TCP (TCP_NODELAY)
└─ handshake: X25519 + Ed25519 server identity + Ed25519 account proof
   └─ encrypted records: ChaCha20-Poly1305, implicit sequence numbers
      └─ one Zstd stream per direction (level 3, window log 21)
         └─ picomux (vendored)
            ├─ "main" stream: TSF1 Input frames ⇄ TSF1 State frames
            └─ "assets" streams: 32-byte hash → asset bytes until EOF
```

Integers in the fixed-layout transport and protocol fields below (handshake hello, record length, key epoch, nonce and associated data, picomux frame header and `MORE` body, application message and section headers) are little-endian. Section values are postcard-encoded, which uses its own variable-length integer encoding.

### Handshake

The handshake is in [crypto.rs](../crates/toy-sim-net/src/crypto.rs). Both `connect` and `accept` enforce a 10 s timeout.

A hello is 66 bytes: a `u16` transport version (1), 32 random bytes, then a 32-byte X25519 ephemeral public key.

1. The client sends its hello.
2. The server checks the version and sends its own hello, followed by a 64-byte Ed25519 signature over the transcript hash:
   `transcript = BLAKE3 derive_key("toy-sim transport v1 handshake", client_hello ‖ server_hello)`
3. The client checks the version. It verifies the signature with `verify_strict` against the server key pinned in its configuration. A mismatch fails with "server identity mismatch".
4. Both sides compute the X25519 shared secret and reject a non-contributory result. Key material is `shared_secret ‖ transcript`:
   - client-to-server root key: `BLAKE3 derive_key("toy-sim transport v1 c2s", material)`
   - server-to-client root key: `BLAKE3 derive_key("toy-sim transport v1 s2c", material)`
5. The client sends the first encrypted record (sequence 0). It holds 80 bytes: the 16-byte account ID, then an Ed25519 signature over
   `BLAKE3 derive_key("toy-sim transport v1 account authentication", account_id ‖ transcript)`.
6. The server looks up the account's public key, verifies the signature with `verify_strict`, and replies with the encrypted record `authenticated` (sequence 0). An unknown account or a bad signature closes the connection without a reply.

The account signature is bound to this connection's transcript, so it cannot be replayed on another connection.

### Encrypted records

Each direction uses its root key and its own sequence counter. Sequence 0 is the authentication record. Data records start at 1. Sequence numbers are never sent on the wire. A counter overflow is an error.

| Part | Size | Contents |
| --- | --- | --- |
| Length | 4 | `u32`: ciphertext length including the 16-byte tag, from 17 to 65,552 |
| Ciphertext | Length | ChaCha20-Poly1305 over the plaintext |

- Plaintext: one type byte, `0` for data or `1` for close, followed by the data. Data is shorter than 65,536 bytes. A close record carries no data.
- Key: `BLAKE3 keyed_hash(root_key, epoch as u64)`, where `epoch = sequence / 2^20`. The cipher key changes every 1,048,576 records.
- Nonce: 4 zero bytes followed by the `u64` sequence.
- Associated data: the `u32` length followed by the `u64` sequence.

A replayed, reordered, truncated or modified record fails authentication, and the connection closes. Shutdown sends a close record, then shuts down the TCP write half.

### Compression

[pipe.rs](../crates/toy-sim-net/src/pipe.rs) wraps the record layer in `AsyncRead`/`AsyncWrite`:

- Each direction has one Zstd stream that lasts the whole connection. The encoder uses level 3 and window log 21 (2 MiB). The decoder sets `WindowLogMax(21)` and rejects streams that need a larger window. History carries across writes, so repeated frame content compresses against earlier frames.
- Each `poll_write` takes up to 32,768 bytes. The pipe compresses the chunk, flushes the encoder so the peer can decode it immediately, and splits the output into records of at most 65,535 data bytes.
- Decoding one record may produce at most 256 KiB of plaintext. More is a protocol error.
- Compression and decompression run on blocking threads. A process-wide semaphore limits them to `available_parallelism()` jobs at a time.
- Writes are pipelined. `poll_write` queues the chunk and returns before it is sent. A write error is reported on the next write, flush or shutdown. Decoded bytes pass through a 64 KiB in-memory pipe to the reader.

### Multiplexing (vendored picomux)

`toy-sim-net` runs picomux over the pipe with debloat mode on. A picomux frame has an 8-byte header followed by the body:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | Version, which must be 1 |
| 1 | 1 | Command: `SYN` 0 (body is the stream metadata), `FIN` 1, `PSH` 2, `NOP` 3, `MORE` 4 (body is a `u16` window increase), `PING` 0xa0 (JSON `{"next_ping_in_ms":…}`), `PONG` 0xa1 |
| 2 | 2 | Body length (`u16`) |
| 4 | 4 | Stream ID (`u32`, chosen at random by the opener) |

The vendored copy ([vendor/picomux](../vendor/picomux)) provides per-stream flow control and concurrent streams:

- The header version must be 1.
- Stream metadata is at most 256 bytes.
- Active streams and pending accepts have no fixed count limit. Closed stream IDs remain reserved for 3600 s. A `SYN` for an open or reserved ID is a protocol error.
- A `PSH` or `FIN` body is at most 8192 bytes (the MSS).
- Flow control is per stream and counted in frames. The initial window is 10 and the maximum is 16. A stream holding 32 or more unconsumed incoming frames fails with "stream receive budget exceeded".
- The outgoing queue accommodates concurrent senders. Each data writer waits until fewer than 10 frames are queued before continuing. The writer flushes after every frame.
- A `FIN` closes one direction of a stream. Shutdown drains buffered writes and queues `FIN` after the data; the opposite direction remains readable until its own EOF.
- The buffer-table registry removes a table as soon as the table is dropped. It does not accumulate one entry per connection.

The source also contains diagnostic logging for stalled writers and missed pongs. The default liveness settings are unchanged: a ping every 1800 s with a 30 s timeout, plus a forced ping whenever a stream is opened.

### Streams

- **`main`.** The client opens it first. The server requires the first accepted stream to have metadata `main`. It carries application `Input` messages from the client and `State` messages from the server. The server's per-connection input queue holds 16 frames. If it is full, the connection closes with "input rate exceeded".
- **`assets`.** Every later stream must have metadata `assets`. Any other label ends the connection. Each asset transfer runs in its own task on both peers, with no fixed concurrency limit. Dropping the connection cancels its outstanding tasks.

**Asset transfer.** The client opens a fresh stream, writes exactly the asset's 32-byte BLAKE3 hash and shuts down its write direction. The server streams the complete asset bytes and shuts down its write direction. EOF delimits the response. The client reads to EOF and verifies the complete asset's BLAKE3 hash. There are no application chunk messages, offsets, length headers or acknowledgements. Picomux and TCP provide transport framing and flow control. An unknown hash gets an empty response, which fails verification for a nonempty expected asset. Download and hash failures are delivered to the requesting UI through that asset's result; other transfers and the main stream continue.

The asset map is built once when the scenario is built and is immutable afterwards. It holds ship appearances, the universe catalogue and celestial system definitions:

- **Ship appearances.** The TOML of a ship blueprint with firmware reset to standard, and with avionics, the ship name, and part names, aliases and groups removed. Its BLAKE3 hash is the `appearance` field of a track and of a `Destroyed` combat event.
- **The universe catalogue.** A postcard-encoded `UniverseCatalogue`: every system's ID, name, position and influence radius, and each body's ID, name, kind (`star` or `planet`), radius, mass and parent. Its hash is `presentation.universe.catalogue`. `validate_catalogue` limits it to 65,536 systems and 262,144 bodies, with unique IDs and valid parents.
- **Celestial systems.** TOML system definitions with the orbital and physical parameters needed by the client orrery. Views reference their hashes in `presentation.celestial_systems`.

## Application messages

Messages are defined in [toy-sim-protocol](../crates/toy-sim-protocol/src/lib.rs). The `main` stream carries a sequence of messages. Each message is a 12-byte header followed by a body:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | Magic `TSF1` |
| 4 | 2 | Protocol version, which must be 5 (`VERSION`) |
| 6 | 2 | Kind: 1 `State`, 2 `Input`. Any other kind is rejected. |
| 8 | 4 | Body length: at most 8 MiB for `State`, 64 KiB for `Input` |

The body is a list of sections, with at most 32 per message:

| Size | Field |
| --- | --- |
| 2 | Section ID |
| 2 | Flags: `0` optional, `1` required. Values above 1 are rejected. |
| 4 | Length |
| Length | [postcard](https://docs.rs/postcard)-encoded value |

A reader ignores unknown optional sections. It rejects unknown required sections, duplicate known sections, missing known sections, truncated sections and trailing bytes inside a section. The encoder marks every section it writes as required. Because the values are postcard-encoded, field order and enum variant order in `toy-sim-model` are part of the wire format.

| Kind | Section | Value |
| --- | --- | --- |
| `State` | 1 | Clock: `world`, `sequence`, `tick`, `sim_time_ns`, `event_watermark`, `rate` |
| `State` | 2 | `Vec<ViewState>` |
| `State` | 3 | `BTreeMap<GroupId, Vec<Track>>` |
| `State` | 4 | `Vec<ShipTelemetry>` |
| `State` | 5 | `Vec<ScreenUpdate>` |
| `State` | 6 | `Vec<Event>` |
| `State` | 7 | `Vec<CommandResult>` |
| `State` | 8 | `PresentationFrame` |
| `Input` | 1 | `InputFrame` |

All eight `State` sections are required. A version 1 message, or a state message without section 8, is rejected. Encoding and decoding both validate the message.

**State frame limits.**

- `rate` is finite and from 0 to 100.
- At most 8 views, 16 track groups, 64 ship telemetry records and 64 screen updates.
- At most 8192 tracks in total, and at most 8192 track IDs per view. View IDs are unique.
- At most 16,384 events and 4096 command results.
- Every position component is at most 2^110 µm in magnitude. Velocities, rotations and angular velocities are finite, and rotations are unit quaternions to within 1e-5.
- Track IDs are unique within a group. A track has at most 64 tags, and uncertainties are finite and non-negative.
- `Kind`, `Advertised` and `Annotation` tag strings are 1 to 64 bytes with no control characters.
- Ship resources are finite and non-negative. Travel states have at most 256 orders and 256 legs, and a blocked reason is at most 1024 bytes.
- Event kinds are at most 64 bytes, and an event's sequence cannot exceed `event_watermark`. Command errors are at most 1024 bytes.
- A screen update has a slot below 8 and a valid drawing list whose `screen_id` equals the slot. The whole update encodes to at most 65,536 bytes, and its error text is at most 1024 bytes.

**Presentation limits** ([presentation.rs](../crates/toy-sim-protocol/src/presentation.rs)):

- At most 64 ship presentations, 8192 track visuals, 16,384 combat events, 256 celestial system references and 7 capabilities.
- Every ship presentation needs a telemetry record for the same ship in the frame, with at most one presentation per ship. Every track visual must name a track present in the frame, with at most one visual per track.
- Hardware totals, device readings, health, environment and execution metrics are finite and non-negative where they are physical amounts. Fractions such as throttle, weapon progress, shield strength and shield coverage are within 0 to 1. At most 256 inventory entries, 4096 devices and 8 screen definitions per ship; screen definitions have a slot below 8, a size from 1 to 4096 and a title of at most 64 bytes.
- Instruments have at most 4096 weapon rows, 64 paths with 16,384 vertices in total, and 256 markers. Timed paths have strictly increasing vertex times.
- Combat events have a sequence no greater than `event_watermark`, valid positions and poses, and a projectile's end time is not before its start.

**Input frame limits.**

- At most 256 actions, with unique command IDs.
- Subscription queries: `limit` from 1 to 256, `work` at most 10,000,000, at most 32 tags in total and valid tags. A search sphere needs a valid centre and a radius from 0 to 1e22 m.
- `ScreenSubscribe`: slot below 8 and `hz` from 1 to 10. `ScreenUnsubscribe`: slot below 8.
- `EngageWeapons`: maximum flight time from 0 to 3600 s.
- `Manual`: throttle from 0 to 1, and steering components from −1 to 1.
- `Flight(AimDirection)`: finite and not zero length. `Flight(EngageNavigation)`: throttle limit from 0 to 1 and stand-off from 0 to 1e22 m.
- `SetIff`: at most 16 valid labels, and range from 0 to 1e12 m.
- `SetTravel`: at most 256 orders. Galactic and relative destinations need valid positions.
- `ScreenInput`: slot below 8, kind at most 7, text at most 64 bytes, and coordinates at most 1e6 in magnitude.
- Debug commands: `SetRate` finite from 0 to 100, `ConfigureSensor` range from 0 to 1e22 m, `Relocate` a valid pose, and heat injections from 0 to 1e30 J.

## Sessions

Each connection gets a `Session` entity ([session.rs](../crates/toy-sim-server/src/sim/session.rs)). It starts as a member of the account's default information group and the public group.

### Input frames

After protocol validation, a frame whose `world` differs from the current world is discarded without applying its actions or changing sequence and acknowledgement state. This handles inputs already in transit when the debug server resets. For frames in the current world, any of these violations disconnects the session:

- `sequence` must be strictly greater than the previous input frame's.
- `acknowledged_event` must not go backwards, and it must not exceed the last `event_watermark` the server sent.
- `acknowledged_frame` must not exceed the last frame sequence the server sent.

Actions are applied in order before the next simulation tick, and each result's `effective_tick` is the current tick. The session remembers every command ID it has seen and ignores repeats. After 65,536 IDs, further input is refused ("session command history exhausted; reconnect"). An action error does not close the session. It is reported in `CommandResult.error`.

### Results and events

- Each result is repeated in every state frame until the client acknowledges a frame sequence at or after the first frame that carried it. At most 4096 results can be pending ("command results backlogged").
- The server keeps the last 8192 events. A frame includes every event after the session's acknowledged event whose `subject` is either an entity with a known ID in one of the frame's tracks, or a ship the account controls. Events with no subject are not sent. Events repeat until acknowledged. If the history no longer reaches back to the acknowledged event, building the frame fails and the session closes ("event history expired; reconnect").
- Event kinds with a subject: `docked`, `undocked`, `destroyed`, `gate-transferred`, `slip-departed`, `slip-arrived` and `relocated`. Collision and weapon records use the kind `combat` with no subject; they reach clients only as presentation combat events.

### Actions

A session can request detailed data for at most eight distinct ships across focused views, instruments and MFD subscriptions. Lightweight owned-ship telemetry remains available for up to 64 ships. Detailed hardware publication is limited to subscribed ships.

| Action | Effect |
| --- | --- |
| `JoinGroup(key)` | Joins the information group for `key`, creating it if needed. Replies `JoinedGroup(group_id)`. At most 16 groups per session. |
| `Subscribe(ViewSubscription)` | Adds or replaces view `id`. The session must belong to `group`. A replacement must have a higher `revision`. At most 8 views. A `focused_ship` must be controlled by this account. |
| `Unsubscribe(id)` | Removes a view |
| `InstrumentSubscribe { ship }` / `InstrumentUnsubscribe` | Requests instrument presentation for a controlled ship. At most 8. |
| `ScreenSubscribe { ship, slot, hz }` / `ScreenUnsubscribe` | Requests display frames for a controlled ship. At most 8 subscriptions. |
| `Debug(command)` | Requires a debug account ([Debug accounts](#debug-accounts)) |
| `Ship { ship, authority_revision, command }` | The account must control the ship, and `authority_revision` must equal the ship's current revision |

Views with a focused ship, instrument subscriptions and screen subscriptions together may name at most 8 distinct ships ("detailed ship subscription limit").

| `ShipCommand` | Effect |
| --- | --- |
| `SetTransponderEnabled(bool)` | Switches the IFF transponder |
| `SetIff(IffIdentity)` | `owner` must be the controlling account. `faction` must be one of the account's factions (no code path adds factions, so only `None` succeeds). Range is at most 1e8 m. |
| `SetGroup(key)` | Sends the ship's future reports to another information group, creating it if needed |
| `Flight(FlightCommand)` | `HoldAttitude`, `StopGuidance`, `AimDirection`, `SelectTarget(ContactRef)` or `EngageNavigation { throttle_limit, stand_off_m }`, queued to the flight computer as requests. `SelectTarget` resolves its contact like `Aim`. |
| `EngageWeapons { group, track, maximum_flight_time_s }`, `Aim { group, track }` | `group` must be both the ship's reporting group and a group the session belongs to, and the track must exist in that group's snapshot. The track becomes a contact handle ([Contact handles](#contact-handles)) and is queued as a request. |
| `HoldFire` | Queues the hold-fire request |
| `Manual { throttle, steering }` | The ship must be in space. Sets travel to `Paused` and queues a manual request. |
| `SetTravel { expected_revision, orders }` | Requires the current travel revision. Replaces orders, increments the revision and sets status `Planning`. |
| `PauseTravel` / `ResumeTravel` | Pause sets `Paused` and queues a zero manual request. Resume sets `Planning`. |
| `Dock { station, bay }` / `Undock` | Direct docking operations ([Docking and travel](#docking-and-travel)) |
| `ScreenInput { slot, revision, kind, code, modifiers, xy, text }` | Requires a subscription to the slot and a live display instance. `revision` must match the displayed frame. Queues a `ScreenEvent` on the display instance. |

The flight computer's request queue accepts a command only while it holds fewer than 255 entries ("ship command queue full").

### State frames

A session frame contains:

- **Clock.** `world`, a per-session `sequence` starting at 1, `tick`, `sim_time_ns` (the fixed clock's elapsed time), `event_watermark`, and `rate`, the current debug clock rate (1 unless a debug account changed it).
- **Views.** Each view runs its query against the current snapshot of its group ([Metered queries](#metered-queries)). The frame shares a total budget of 2,000,000 work units across views, in view ID order. If a view has a `focused_ship` and a sphere, the sphere is centred on that ship's current position. `ViewState` reports `origin`, the returned track IDs and `completion`. Views, instrument subscriptions and screen subscriptions on ships the account no longer controls are removed.
- **Tracks.** The union of the tracks returned by all views, grouped by group ID. Every group the session belongs to has an entry, even when empty.
- **Ships.** Private telemetry for up to 64 ships the account controls. Ships named by a view focus, a screen subscription or an instrument subscription come first, then the rest, each part in ship ID order. Each record holds the information-group key, IFF identity, authority revision, presence, exact pose (only while in space), battery energy, hull heat, shield temperature, coolant reserve and travel state.
- **Screens.** One update per subscribed slot: the latest display frame, or, if there is none, an update with no frame and the error "Display unavailable".
- **Events and results**, as described above.
- **Presentation** ([Presentation](#presentation)).

Network views have no continuation. Each frame runs a fresh query, so a view that stops at `WorkLimit` or `ResultLimit` shows only its first page.

## Observations and intelligence

Clients never receive the true world state for ships they do not control. They receive fused tracks from information groups. The implementation is in [intelligence.rs](../crates/toy-sim-server/src/sim/intelligence.rs), [identity.rs](../crates/toy-sim-server/src/sim/identity.rs) and [toy-sim-intel](../crates/toy-sim-intel/src/lib.rs).

### Identities

Two kinds of identifier are kept apart:

- **Physical UUIDs.** Ships, celestial bodies, accounts and groups have 16-byte UUIDs. A ship's UUID is its identity for control, commands, telemetry, beacons, docking and IFF. A track exposes it in `entity` only when the observing group has an authenticated measurement of that ship.
- **Track IDs and contact references.** A `TrackId` is random per group, so the same ship has unrelated track IDs in different groups, and a sensor-only track carries no UUID. A `ContactRef { group, track }` names a track as seen by one group. Presentation, targeting and instruments refer to other ships through `ContactRef`, not through UUIDs. Flight programs see only opaque `u64` contact handles ([Contact handles](#contact-handles)).

### Information groups

- An `InfoGroupKey` is a random 32-byte bearer secret. Anyone who presents it with `JoinGroup` can read the group's tracks. The server maps each key to a random `GroupId`. `Debug` output redacts the key.
- Each account gets its own default key when the account is created. Each ship reports into the group of its own key, which starts as its owner's default key and can be changed with `SetGroup`. The key appears in the controlling account's ship telemetry, which is how a player can share it.
- Membership only grants reading. It does not grant control of ships, and group-based targeting still requires controlling the firing ship.
- Session membership ends when the connection ends. Groups themselves are not removed while the server runs.

### Private observations

After physics, every ship contributes measurements to its group:

- **Itself.** One exact authenticated measurement with provenance `GroupMember`, including its IFF tags, radius and appearance hash.
- **Other ships.** If its sensor range is positive, the ship takes the nearest 256 objects within range from the spatial index. Celestial occlusion is then applied to those 256, and hidden candidates are not replaced by more distant ones. Ships in the observer's own group are skipped. There is no light-speed delay: every measurement describes the target's state at the current tick.
  - If the target's transponder is enabled and the target is within its own `iff.range_m`, the measurement is exact and authenticated. Its provenance is `Transponder`. It carries the UUID and the tags `Kind("ship")`, `IffOwner`, `IffFaction` if set, and `Advertised` labels.
  - Otherwise it is a sensor measurement. It has no UUID, only the tag `Kind("ship")`, identity rotation and zero angular velocity. Position and velocity carry noise with σ = √(1 + (distance × 1e-5)²) metres. On each axis the error is σ × (√0.9 × bias + √0.1 × per-tick noise). The Gaussian samples come from BLAKE3 keyed with a per-world seed, the sensing platform, the target, the axis and the tick. The bias is the same on every tick, so repeating a query or a measurement does not average the noise away.
  - Both kinds of measurement carry the target's radius and appearance hash.

Acquisition runs in parallel across groups.

### Public observations

The public group (`PUBLIC_GROUP`, ID `ff…ff`) is joined by every session. It receives:

- every ship with a beacon emitter, as an exact measurement with provenance `Beacon`, the beacon's IFF tags and `Kind("beacon")`

In the scenario the public group holds three beacons: the demo station and the two demo gates. Celestial bodies are excluded from sensor acquisition and fused tracks. Their positions and HUD labels come from the subscribed system definitions and client orrery. They still occlude sensors and remain available through explicit celestial queries.

### Fusion

Once per tick:

1. **Coasting.** Tracks not observed for more than 600 ticks (60 s) are removed. Other tracks are extrapolated by their velocity, their position uncertainty grows, and their provenance becomes `Extrapolated`.
2. **Grouping.** Measurements are grouped by physical entity and sorted by σ, then platform. The best measurement is the lowest σ.
3. **Association.** The best measurement keeps the entity's existing track ID if it is authenticated with the same UUID, or if it lies within 3 × (track σ + measurement σ) of the track. Otherwise the track gets a new random ID. A known UUID is kept. When the best measurement is an unauthenticated sensor measurement, IFF owner and faction tags from the continued track are kept, so a ship that switches off its transponder keeps its last identified owner.
4. **Weighting.** When the best measurement is noisy, one measurement per platform is combined by inverse-variance weighting. The fused σ is at least max(1 m, best σ / 4), and the velocity σ equals the position σ.
5. **Publication.** Each group gets a new immutable `Snapshot` (`Arc`) indexed by tag and by cubic spatial cells 100,000 km on a side. Queries and flight programs read snapshots and never block the next update.

Grouping in step 2 uses the server's true entity identity. Clients never see that identity for sensor-only tracks.

### IFF

`IffIdentity` has `owner` (account), optional `faction`, up to 16 advertised `labels`, `enabled` and `range_m`. A new ship starts with its owner, no faction, no labels, transponder enabled and a range of 1e8 m. The transponder is the only way for an observer outside the ship's group to learn its UUID and owner.

A ship's IFF identity and group are separate from control. Control is the `Control { account, revision }` component, and commands must carry the current revision. If control changes, the ship keeps its IFF identity and its information-group key until the new controller sends `SetIff` and `SetGroup`. No protocol action or server system transfers control; the session tests change `Control` directly to check that commands with the old revision fail and IFF is unchanged.

## Metered queries

`toy_sim_intel::query::Queries` runs a `TrackQuery` against a snapshot and returns a `QueryPage`.

A query has an optional direct `track`, an optional `sphere`, tag sets `all`, `any` and `exclude`, an optional `max_age_ticks`, a result `limit` of 1 to 256, and a `work` budget.

**Candidate source.** The first rule that applies decides where candidates come from:

1. The direct track ID.
2. The smallest `all` tag index, when there is no sphere or that index holds at most 256 tracks.
3. The spatial cells overlapping the sphere's bounding box.
4. The union of the `any` tag indexes.
5. All tracks.

Every candidate is then filtered against all conditions.

**Work charges.**

| Charge | Amount |
| --- | --- |
| Page call | 100 |
| Each index step | 8 (plus 8 per tag for an `any` union) |
| Each candidate examined | 1000 |
| Each returned track | 1 per 8 bytes of its postcard encoding |

**Completion.** `Complete` means the source is exhausted. `ResultLimit` means `limit` tracks were returned. `WorkLimit` means the budget ran out. `gas_used` reports the work spent, and an empty result can still spend the whole budget. An incomplete page returns a `continuation` cursor. The cursor reads the same snapshot, and `revision` reports the snapshot's tick. A cursor expires 10 ticks after the query started. One `Queries` holds at most 8 live cursors.

**Who uses it.**

| Caller | Budget | Cursors |
| --- | --- | --- |
| Network views | Shared 2,000,000 per frame | None; a fresh query every frame |
| Flight programs (`world_query` `Tracks` and `Continue`) | At most 1,000,000 per call, paid from the computer's gas | Kept per ship, with separate stores for `ship_tick` and `ship_display` |
| `sensor_scan` | `n × 1200 + 100` per group snapshot | None |

### Contact handles

Flight programs see `u64` contact IDs instead of track IDs ([services.rs](../crates/toy-sim-server/src/sim/services.rs)). Each ship keeps a table from `(group, track)` to a random non-zero handle. The table is replaced when the ship changes group. Entries unused for 600 ticks expire, and when the table holds 8192 entries the oldest is evicted. A handle therefore says nothing about the ship's UUID, and the same target has different handles on different ships.

`sensor_scan(range, n)` runs a sphere query on the ship's own group snapshot and on the public snapshot, removes the ship itself and duplicate UUIDs, sorts by distance and returns at most `min(n, 256)` contacts. The query selects ship tracks only. Contacts have kind `CONTACT_SHIP` and are named by UUID when the track has one, or "Unknown contact". Position and velocity are relative to the ship.

Instruments translate handles back into `ContactRef`s only for tracks that still exist in the ship's group or the public group.

## Presentation

Section 8 of a state frame is a `PresentationFrame` ([presentation.rs](../crates/toy-sim-model/src/presentation.rs), built in [sim/presentation.rs](../crates/toy-sim-server/src/sim/presentation.rs)). It carries what the shared client needs for the original rendering, flight instruments and windows, without exposing true state beyond what the session may already see.

- **`ships`.** One `ShipPresentation` per controlled ship that a view focuses, a screen subscribes or an instrument subscription names. It holds the authority revision, flight environment (altitude, airspeed, density, pressure), health, execution timings, mass, inertia, control rotation, heat and battery capacities, power flow, inventory, per-device telemetry, computer status and screen definitions. `instruments` (attitude, navigation, weapons, trajectories and markers published by the firmware) is included only for instrument subscriptions. Targets in instruments are `ContactRef`s.
- **`visuals`.** Engine thrust, turret angles and shield state for tracks in the frame that have a UUID and were observed on this tick.
- **`combat`.** Shots, projectiles, impacts and destruction after the session's acknowledged event. A record is included only if it can be attached to a track in the frame that has a UUID and is either exact or was observed within one tick of the record. The record names that `ContactRef`. Spatial lifetime changes are represented by the token in the track and owned ship telemetry.
- **`celestial_systems`.** Per-view system IDs, definition asset hashes and an explicit epoch/time origin. Complete system definitions arrive through asset streams. The client evaluates the shared orrery solver at presentation time for positions, velocities and rotations. System selection uses the view’s location independently of sensor coverage and server ECS activation.
- **`universe`.** The catalogue asset hash for every session. Debug sessions also receive the active systems (with the reason `ships` or `debug inspection`) and the inspected body.
- **`capabilities`** and **`diagnostics`** for debug sessions ([Debug accounts](#debug-accounts)).

## Debug accounts

A debug account is a configured account named by `debug_account`. It has no separate connection path. Its session frames list seven `DebugCapability` values: `Clock`, `Reset`, `Relocate`, `Recover`, `InjectHeat`, `Inspect` and `ConfigureSensor`. `DebugCommand::capability` maps each command to the capability it needs. Any other account's `Debug` action fails with "debug access denied".

| `DebugCommand` | Effect |
| --- | --- |
| `SetRate(rate)` | Sets the clock rate from 0 to 100 ticks per 100 ms loop. 0 pauses. |
| `Step` | Requires a rate of 0 ("pause before stepping"). Queues one tick, up to 100 queued. |
| `Reset` | Rebuilds the scenario on the next loop, with a new world ID |
| `Inspect(enabled)` | Switches server diagnostics on or off for all debug sessions |
| `Relocate { ship, pose }` | Writes the pose and velocities, clears force accumulators, renews the spatial instance and queues `StopGuidance` |
| `RelocateToBody { ship, body }` | Places the ship at an arrival pose near a non-star body, cancels pending slip preparation and gate transfer, renews the spatial instance and queues `StopGuidance` |
| `Recover { ship }` | See below |
| `InjectHeat { ship, joules }` / `InjectShieldHeat` | Adds heat to the hull or shield of the ship's flight computer state |
| `ConfigureSensor { ship, range_m, occlusion }` | Overrides the ship's sensor range and celestial occlusion from then on |
| `InspectBody { body }` | Keeps a body's system active for inspection, or clears it |

Commands other than clock, reset and inspect go into a queue of at most 256 and are applied before the next ticks. They name ships by UUID and are not limited to ships the debug account controls. A command naming an unknown ship is ignored.

**Dormant ships.** Docked, in-transit, stored and destroyed ships are dormant. `Relocate` and `RelocateToBody` ignore dormant ships. `Recover` accepts only a ship in space or a destroyed ship; a docked, in-transit or stored ship is refused. A destroyed ship is recovered only if its stored-ship inventory is empty and it holds no stored mass. Recovery rebuilds the hardware with a fresh test loadout, returns the ship to space, cancels pending travel operations, sets travel to `Paused`, reboots the computer and clears its requests and world actions. Recovery keeps the ship's UUID.

**Diagnostics.** While inspection is on, debug frames include entity count, active and dormant ship counts, the last tick's duration, and per-stage times: ship preparation, WASM callbacks, sensor queries, publication, hardware, and collision indexing, queries, solving and total, plus collision counters.

## Display instances

Screens for network clients come from a separate WebAssembly instance of the ship's firmware running its `ship_display` export ([displays.rs](../crates/toy-sim-server/src/sim/displays.rs), [ship-abi.md](ship-abi.md#display-entry-point)). Display transport is drawing-only and on demand: nothing runs until a session subscribes.

- **One instance per ship.** All sessions subscribed to a ship share one instance. The refresh rate of each slot is the highest `hz` among its subscribers.
- **Creation.** An instance is created for a ship when at least one subscriber controls it, the ship is not dormant and its computer is powered. It is instantiated with `instantiate_display`, configured with the ship's hardware, and booted. Creation and booting share `MAX_BOOTS_PER_TICK` per update. The instance advances its own gas clock by elapsed ticks.
- **Input.** On each update it receives a copy of the flight computer's latest input with commands and screen events removed. `requested_screens` holds the subscribed slots that are due: a slot is due when it has no frame yet, or when `tick × hz / 10` has crossed an integer since its last frame. If no slot is due, the instance is not called.
- **Frames.** Frames for requested slots are stored as `ScreenUpdate { ship, slot, revision, tick, frame, error }`. `revision` is the instance revision, a server-wide counter that is new for every instance. If the callback fails, every requested slot gets the error text, truncated to 512 bytes, and no frame. Screens the firmware clears lose their stored frame.
- **Limits.** It has its own memory, gas account, query cursors and screen event queue. It cannot write devices or submit world actions. It does not share memory with the flight instance.
- **Screen definitions.** A ship presentation lists the display instance's screen definitions, or, if it has none, the flight computer's.
- **Expiry and revocation.** An instance is removed on the first update after its ship becomes dormant, its computer loses power, its control revision changes, or 10 ticks pass with no subscriber. Its frames are withheld immediately in those cases. A new instance gets a new revision, so input aimed at the old frames is rejected, and queued input is dropped with the old instance.
- **Screen input.** `ScreenInput` requires the ship to be active and powered, the instance's control revision to match, the slot's stored frame to exist with the given revision, a valid event kind (0 to 7) and at most 64 bytes of text. The event gets the next event ID for that instance and is queued as a `ScreenEvent` with its code, modifiers, coordinates and text. Pointer moves, presses and releases, key presses and releases, text, bezel and reset kinds are all forwarded.

The stock firmware's `ship_display` defines each requested slot as a 512 × 256 screen titled "Ship status" and draws five text lines: `SHIP STATUS`, simulation time, speed, mass and battery energy. It does not read screen events. Firmware without a `ship_display` export cannot be instantiated as a display, so its subscribed slots report "Display unavailable".

## Docking and travel

Docking and travel are implemented in [travel.rs](../crates/toy-sim-server/src/sim/travel.rs).

### Presence

`Presence` is one of `Space`, `Docked { host, bay }`, `SlipTransit(id)`, `StoredInWreck(host)` or `Destroyed`. Private telemetry includes an appearance hash, radius and presentation pose in space, docking storage and slip transit. Docked poses follow the host bay; transit poses follow the declared slip segment. Only space presence participates in normal physics.

Leaving space makes a ship dormant. Its hardware is shut down (default device settings, avionics unpowered, sensor range 0), and its velocity, rigid body, collision body and spatial body are removed and remembered. Dormant ships take no part in physics, sensing, programs, world services or displays. Their hull and shield thermal state advances once per simulated second. Returning to space restores the remembered components.

### Bays and docking

A bay belongs to a host ship. It has a centre and rotation in host axes, a radius, a mass capacity, public or allow-list access, an optional reservation. Docked ships are stored through the ECS `DockedIn` / `StoredShips` relationship; a stored ship releases the bay for the next arrival.

- **`reserve_bay`** requires:
  - the ship is not the host, both are in space, and the ship's containment tree is less than 8 deep with no cycles
  - access: the bay is public, the host has the same owner, or the ship's owner is on the allow list
  - a bay that fits the ship's radius and mass
  - no unexpired reservation by another ship

  A reservation lasts 600 ticks.
- **`dock`** reserves the bay, then requires the whole ship to be inside the bay sphere, within 0.5 m/s of the bay's velocity and within 5° of its orientation. On success the ship becomes dormant with presence `Docked`, the host's stored mass and mass increase by the ship's mass, the ship joins the host's stored inventory and releases the reservation, a current `Dock` leg completes its order, and `docked` is emitted.
- **`undock`** requires the ship not to be destroyed, the host to be in space, and departure access. The ship leaves along the bay's −Z axis, offset by host radius + ship radius + 10 m, and inherits the host's velocity plus ω × r at that point. The exit point must be clear of other active ships and bodies. `undocked` is emitted.
- Destroying a host changes its docked ships to `StoredInWreck`. Their inventory stays inside the wreck.

A docked ship with an `Undock` or due `WaitUntil` order completes it without running its program. A queued space order automatically undocks the ship and then resumes planning the same order; docking at the current host completes immediately.

### Gates

A gate is fixed navigation infrastructure with a `Gate` record paired to another gate. It has no rigid body. The demo gates follow prescribed circular ephemerides, so they remain near their local traffic frame without consuming rigid-body physics. Their apertures are spherical and accept entry from any direction. `ProgramAction::Gate(entry)` validates entry and schedules the transfer one tick later. The transfer re-validates everything.

**Entry requirements:**

- both the ship and the entry gate are in space
- the entry gate is enabled, and public, allowed, or owned by the ship's owner
- the ship is entirely inside the aperture radius
- relative speed is at most 10 m/s
- the exit gate is in space, enabled, paired back to the entry, and grants the same access
- the ship's offset fits inside the exit aperture

**Exit requirements:**

- tidal curvature 2GM/d³, summed over the bodies of systems containing the point, is at most 1e-8 s⁻² at both the entry position and the exit position
- no other ship overlaps the exit position

Position, velocity, rotation and angular velocity relative to the entry gate are carried over to the exit gate. The travel leg advances and `gate-transferred` is emitted. A failure sets travel status `Blocked`.

### Slipdrive

A `SlipDrive` defaults to 100 MW and is ready at once. The scenario gives one to each player ship.

**Preparation** requires:

- the ship is in space and moving at most 1 m/s in the galactic frame
- both the departure and destination points are clear of ships and bodies, have tidal curvature of at most 1e-8 s⁻², and lie outside every enabled gate's exclusion radius
- the drive is ready and not already preparing

**Energy.** The drive needs 1e5 J/kg × mass × (1 + distance in light-years / 1000). Each tick it draws min(power × 0.1 s, remaining) from the ship's stored energy, and 20% of the energy drawn becomes waste heat. Preparation is cancelled with "Slip preparation invalidated" if the conditions stop holding or the mass grows by more than 0.1%.

**Departure** happens once the energy is complete and at least 100 ticks have passed. The ship becomes dormant with presence `SlipTransit`. Arrival is scheduled (30 + 8.64 × light-years) s later, the drive is ready again 60 s after arrival, and `slip-departed` is emitted.

**Arrival** places the ship at the destination if the destination is still admissible, returns it to space, and emits `slip-arrived`. A ship whose hull has failed is destroyed instead. Otherwise the ship stays in transit, retries every 10 ticks, and reports `Blocked("Arrival obstructed")`.

### Travel orders and firmware planning

Player orders are `TravelTo(Destination)`, `Jump(entry_gate)`, `Dock(station)`, `Undock`, `WaitUntil(tick)` and `Guidance { mode, target, range_m }`. Guidance modes are align, approach, keep range and engage. The latter two remain active until interrupted or removed. Targets are destinations or authorized fused contacts. A destination is a beacon, a galactic position, or an offset from a celestial body or beacon in galactic or body-fixed axes. Body-fixed offsets add ω × r to the resolved velocity.

The server holds the authoritative travel shell: orders, revision, status, legs and the current leg, and the operations that change presence. It does not plan routes. The ship's flight program does, through `world_query` and `world_command` ([ship-abi.md](ship-abi.md#world-services)).

| `ProgramQuery` | Reply |
| --- | --- |
| `Contact(reference)` | Current fused pose, radius and opaque firmware handle; only the ship's group and public picture are accessible |
| `Travel` | Travel state, own pose and whether the slipdrive is ready |
| `Resolve(destination)` | The destination's pose. Celestial references resolve from the universe catalogue, so inactive systems work too. |
| `Beacon(id)`, `Beacons { after, limit }` | Beacons in UUID order, `limit` from 1 to 256. Each has pose, radius, IFF, the bays this ship could use now, and the paired exit if the ship may use the gate. |
| `SlipEligibility { destination }` | Whether the drive is ready and both apertures are admissible |
| `Tracks(query)`, `Continue { cursor, work }` | Metered track queries on the ship's group snapshot |

World actions are applied after all ships have run, in ship ID order. If an action fails, travel becomes `Blocked(error)` and the ship's later actions in the same batch are skipped.

| `ProgramAction` | Server behaviour |
| --- | --- |
| `Block { revision, reason }` | Requires the current revision. Sets `Blocked` with the first 256 characters of the reason. |
| `Route { revision, legs }` | Requires the current revision, travel not paused and at most 256 legs. Sets the legs, leg 0 and `Active`. |
| `CompleteLeg { revision, leg }` | Requires the current revision and leg while `Active`. Advances the leg; after the last leg it advances the order, to `Completed` or `Planning`. |
| `Slip(destination)`, `Gate(entry)` | Starts slip preparation or a gate transfer. The server advances the leg on arrival or transfer. |
| `ReserveBay`, `Dock`, `Undock` | The operations above. `Undock` also completes the order. |

The stock firmware's planner ([world.rs](../crates/toy-sim-example-controller/src/world.rs), [graph.rs](../crates/toy-sim-example-controller/src/world/graph.rs)) runs inside `ship_tick`:

- **Target.** A beacon destination becomes a point on the beacon's body-fixed −Z axis, beacon radius + own radius + 100 m away, plus 1e7 m for a gate. Other destinations are resolved directly.
- **Short trips.** If the target is less than 1e7 m away, the route is a single sublight leg, followed by a dock leg for a `Dock` order.
- **Beacon scan.** Otherwise the planner pages through beacons 16 per callback, collecting gates with a visible exit. It gives up with `ERR_UNAVAILABLE` beyond 4096 beacons or 256 gates.
- **Route graph.** It runs Dijkstra over the origin, the target and the collected gates. Edge cost is straight-line distance, except that a gate's edge to its paired exit costs at most 1e6 m. The search does at most 128 relaxations per callback and resumes on the next callback, so a large catalogue spreads over several ticks. Routes can chain any number of gate pairs.
- **Slip.** If the slipdrive is ready, the direct distance is more than 1e9 m, and the best graph route is longer than half the direct distance, the route is sublight to the current position, a slip leg, then sublight to the target.
- **Gate legs.** Otherwise, for each gate pair on the path, the route has a sublight leg to the entry gate's centre and a `Gate` leg, followed by a final sublight leg (and a dock leg for `Dock`).
- **Sublight.** The planner feeds the resolved relative position and velocity to the braking navigation law as a synthetic contact (ID `u64::MAX`). It completes the leg within 2 m and 0.5 m/s ([rendezvous.md](rendezvous.md)).
- **Slip and gate legs.** The planner sends `Slip` while the drive is ready, or `Gate` on each callback. The server advances the leg.
- **Dock.** The planner picks the lowest-numbered bay the beacon reports as usable, reserves it, and renews the reservation every 100 ticks. It first rendezvous with a point outside the mouth on the bay's −Z axis, then enters the bay. Within 2 m and 0.3 m/s of the berth it holds the bay's attitude, and within 5° it sends `Dock`.
- **Errors.** If a query or command fails while travel is active, the planner sends `Block` with "Routing query failed (code); retrying" and plans again 50 ticks later. It also retries from a `Blocked` state.

The planner uses straight-line distances rather than flight time or energy. Gate access and bay availability come from the beacon replies; enablement, obstruction and curvature are checked only by the server when the action is applied.

## Client playback

`toy_sim_client::connect` ([connection.rs](../crates/toy-sim-client/src/connection.rs)) opens the `main` stream and returns an `Endpoint` with state and input channels plus a cloneable `AssetClient`:

- incoming states: 12 frames. When the channel is full, the reader stops reading, which applies back-pressure through picomux.
- outgoing inputs: 16 frames
- asset requests: 8 pending requests; each `AssetClient::fetch(hash)` awaits its own response containing complete bytes or an error

Asset requests start independent transfers as they leave the request channel. A slow transfer does not hold up later requests; the channel bounds pending requests rather than active downloads.

`Playback` buffers bursty arrivals. Bevy runs the client at a fixed presentation cadence of 10 Hz.

**Receiving frames.**

- The protocol decoder validates every frame. Playback checks sequence and timestamp ordering.
- A new `world` resets buffered snapshots and retained publications. At the next fixed update, one reset system removes entities marked `WorldMember`, resets session metadata and interpolation time, and triggers `SessionReset`. Observers clear UI resources, subscription bookkeeping, pending actions and celestial indexes. The UI then subscribes again.
- `sequence` must increase and simulation time must not go backwards. A violation is reported to the UI as a status message.
- Typed combat publications and command results are retained before acknowledgement, independently of queued snapshots. Combat events are deduplicated by event watermark and results by command ID; up to 65,536 command IDs are remembered for the session. Each retained batch records the receiving frame sequence. Destruction events also retain the target spatial instance from that frame. Snapshot trimming therefore cannot lose acknowledged publications or apply an old destruction marker to a later arrival. Generic wire events currently have no UI consumer.
- If more than 12 frames are buffered, the oldest are dropped down to the target depth and prebuffering starts again.

**Output.**

- `PreUpdate` drains received states into the buffer. Each `FixedUpdate` calls `Playback::tick` once and applies at most one actual snapshot to the client ECS. Playback starts once the buffer holds the target depth: 3 frames for `toy-sim-client`, 1 for `toy-sim-debug`.
- On an empty buffer, `underruns` increments once and the target grows by one, up to 10. Consumption resumes when the reserve has rebuilt. The previous and current ECS samples remain unchanged during this wait, so each fixed interval repeats the last movement. A new world restores the initial target and clears the underrun count.
- In `Update`, the interpolation fraction is `Time<Fixed>::overstep_fraction_f64()`. Pose and hardware components interpolate between their previous and current samples; turret yaw follows the shortest arc. The first sample initializes both endpoints to the arrival pose.
- `RenderTime` contains only `previous_ns`, `current_ns` and `display_ns`. The display timestamp interpolates the two snapshots' explicit `sim_time_ns` values using the same fraction. Celestial positions are evaluated analytically at that time. Accelerated simulation, paused snapshots and skipped publications use their actual timestamp intervals. The buffer does not synthesize snapshots or infer timestamps from the advertised clock rate.

**Spatial lifetimes.** Tracks and private ship telemetry carry a `spatial_instance` token. Relocation, departure and arrival replace this token while preserving the ship's UUID. A changed track token replaces its client observation entity and associated render instances; new samples start at the arrival pose. Controlled ships retain their telemetry entity, removing pose components while absent and initializing fresh samples on arrival. The token changes even when departure and arrival occur between two publications. Anonymous track tokens are scoped to the opaque track ID. Interpolation and tracer history use these lifetimes without travel-specific discontinuity events.

Retained publications are delivered when playback reaches their receiving frame sequence, including when intervening snapshots were trimmed. Combat visibility still follows each event's simulation timestamp. The session keeps the latest 128 delivered command results for the UI.

**Input.** `FixedPostUpdate` sends an `InputFrame` every 100 ms once the UI knows the world, even when there are no actions. Each frame acknowledges the newest received frame sequence and event watermark. The command queue supplies command IDs and ship authority revisions, and coalesces pending manual inputs separately for each ship. If the input channel is full, queued actions retain their IDs for the next attempt and the status reads "Input queue busy".

## Client ECS presentation

The client separates transport ingestion, replication and interpolation into the [state modules](../crates/toy-sim-client/src/state). `SessionInfo` owns the world, generation, applied tick and sequence, capabilities, diagnostics and delivered command results. Network reception, buffered playback, outgoing commands and session metadata have separate resources. Only snapshot ingestion reads the buffered frames. It reconciles stable entities for contacts, controlled ships, views and combat publications. Contact identity includes the information group, so observations from different groups remain distinct. ID-to-entity maps are derived lookup indexes.

Ship and contact entities hold pose samples and interpolated display components. Each view owns its camera, origin, exposure and sky state. `CameraOptions` holds the camera focus separately from the orbit data used to construct trajectories. Render instances relate to both their source observation and their view, allowing either lifetime to remove the associated visuals. Bevy asset handles own immutable downloaded assets; sky baking keeps its separate work budget. HUD overlays query presentation components, and keyboard flight controls enqueue actions through the outgoing resource.

Reception runs in `PreUpdate`, snapshot application in `FixedUpdate`, and input publication in `FixedPostUpdate`. `Update` then runs interpolation, celestial evaluation, view updates and rendering updates. World changes remove the old observations and reset selection. Missing observations remove their entities; a changed spatial lifetime creates fresh presentation samples. Docked ships retain private telemetry while their space pose is absent. MFD publications remain available in the wire protocol; the current UI does not subscribe to screens or replicate them into display entities.

Complete system assets contain body IDs, parent relationships, orbital elements, rotation, mass, radius, stellar luminosity, colours and atmosphere parameters. Multiple views of one system share its definition and celestial entities. The assets use TOML, preserving the existing definition parser; their content hashes identify immutable bytes. Each system entity holds a typed definition handle and reports pending or failed loading in its subscribed views. The shared solver uses the explicit MJD epoch plus elapsed simulation time; it does not depend on the client’s wall-clock date.

The [asset source](../crates/toy-sim-client/src/assets.rs) registers canonical `server://<hash>` paths before Bevy's asset plugin. Its reader forwards requests to the existing Tokio transport and returns verified bytes to typed loaders for ship designs and system definitions. Decoding and compilation run through Bevy's asset pipeline. Ship observations and destruction publications hold design handles, and system entities hold definition handles. Shared handles reuse loading and decoded assets; releasing the final handle allows unloading. A failed load stays failed until an explicit retry reloads its path. Bevy reports loading failures in the log; the current UI has no asset status or retry window. Individual entities appear as their assets become available.

The default ship starts in sunlight. Its initial camera faces the illuminated hull, and direct stellar illumination uses luminosity divided by spherical area at the view’s distance. Ambient fill is disabled, and the default camera exposure is EV100 15 with per-view adjustment. The original star-disc, Gaia cubemap, atmosphere and exposure algorithms remain the rendering reference.

## The client UI

The shared egui theme, embedded fonts, Phosphor icons and desktop toolkit come from [toy-sim-ui](../crates/toy-sim-ui/README.md). `toy_sim_client::ui::run` ([ui.rs](../crates/toy-sim-client/src/ui.rs)) is the Bevy/egui app used by both `toy-sim-client` and `toy-sim-debug`. The desktop follows the layout hierarchy of EVE Online's interface: a narrow launcher, location information at the upper left, and selected-item controls above the Overview at the right. The center remains available to the 3D scene and orbit HUD.

The launcher opens or closes Overview, Selected Item, Ship Status and Navigation. It also toggles orbital paths, returns the camera to the controlled ship, and opens Interface settings. Windows support dragging, edge resizing, close, snapping, layout locking and reset. Window arrangements last for the current application session. The footer displays interpolated simulation time and transport status.

The location indicator uses the subscribed celestial definitions. It names the brightest star and the body with the strongest local gravitational acceleration, and shows altitude above that body's surface. The Overview combines tracks from the selected view with its orrery bodies. It never obtains planets from sensors. Rows show name, type, center-to-center distance and speed relative to the controlled ship, with sorting, text search and All/Ships/Celestials filters. Stable track or celestial identities resolve sort ties. Distances, relative speeds and alignment vectors use interpolated poses from the same presentation time.

Click an Overview row or a HUD contact to select it. Double-click a row to center the camera; a row's context menu also offers camera and flight actions. Selection is shared with the HUD even when filters hide the selected row. Selected Item provides Align, Approach, Keep range, Look at, Engage, Hold fire and Stop. Approach and Keep range request the existing firmware's rendezvous guidance; Keep range uses the editable stand-off distance. Guidance finishes on arrival. Stop cancels guidance without removing the ship's velocity. Celestials support Align and Look at; contact-based pursuit and weapons controls are disabled for them. There is no universal hostile/friendly classification inferred from IFF ownership.

Ship Status shows hull integrity, shield reserve, battery, heat, temperature, power, resource inventory and flight-computer state. It can toggle IFF broadcasts. Navigation shows published guidance telemetry and existing travel orders, with attitude hold, guidance cancellation and route pause/resume controls. Route planning remains the flight computer's responsibility. MFD, universe browser and debug-command panels are not part of this shell.

[shell.rs](../crates/toy-sim-client/src/ui/shell.rs) reads ECS presentation components and builds a temporary view model. Drawing produces intents; dispatch uses `Outgoing` and the current ship authority revision. It does not change authoritative ship state. Command feedback distinguishes pending requests, server acceptance and rejection, and retains errors across all commands in an action batch. Server acceptance does not mean a maneuver has completed.

On the first applied frame, [selection.rs](../crates/toy-sim-client/src/ui/selection.rs) selects the controlled ship with the smallest UUID and subscribes view 1 in the first non-public information group. The view follows that ship with a 1e8 m sphere, a limit of 256 and a work budget of 300,000. It also subscribes to that ship's instrument data for the HUD. A world reset clears selection, subscriptions, manual flight state and command feedback, then the UI subscribes again.

The HUD draws the native coast estimate, published navigation paths and markers, contact boxes, celestial labels and distances. Clicking a celestial label focuses the camera on that body; Escape returns the active view to its controlled ship. Mouse dragging and scrolling orbit and zoom the camera. O toggles trajectories, and +/− adjusts exposure in the active view. Keyboard flight controls remain in [input.rs](../crates/toy-sim-client/src/ui/input.rs); see the [README](../README.md#client) for their bindings.

## Tests

| Location | Covers |
| --- | --- |
| `toy-sim-protocol` unit tests | Round trips, skipped optional sections, rejected required sections, truncation at every length, oversized headers, non-finite input, paused clocks and debug capabilities, rejection of version 1 and of frames without the presentation section, invalid combat payloads and visuals for unobserved tracks, numeric limits of debug and flight commands |
| `toy-sim-net` unit tests | Compressed stream history across flushes, clean shutdown, rejection of a wrong server pin and a wrong account key, replayed records, epoch rekeying |
| `toy-sim-intel` unit tests | Measurements, noise stability, metered queries and cursors, snapshots |
| [session.rs](../crates/toy-sim-server/src/sim/session.rs) tests | A group key grants views without ship control, a control change rejects old-revision commands without rewriting IFF, a non-debug account cannot change the clock, repeated command IDs are idempotent and replayed frames are rejected |
| [displays.rs](../crates/toy-sim-server/src/sim/displays.rs) tests | Subscribers share one instance released 10 ticks after the last viewer, authority and power changes revoke frames and queued input, every ABI input kind is forwarded |
| [services.rs](../crates/toy-sim-server/src/sim/services.rs) tests | Scans exclude celestials and use stable opaque ship handles, handles differ between groups and after expiry and stay bounded, program and display cursors are separate, slip aperture checks, beacon pages hide inaccessible bays, celestial references resolve without active bodies |
| [travel.rs](../crates/toy-sim-server/src/sim/travel.rs) tests | Docking and undocking motion, nested inventory surviving host destruction, capture not unlocking a private bay, gate dwell and exit access, blocked slip arrival and retry, slip energy and cancellation, inactive bodies blocking slip arrival, debug recovery rules, simultaneous arrivals |
| [router_tests.rs](../crates/toy-sim-server/src/sim/travel/router_tests.rs) | `travel_order_runs_in_stock_wasm_and_brakes_at_destination` and other travel orders flown by the stock firmware |
| [graph.rs](../crates/toy-sim-example-controller/src/world/graph.rs) tests | Multi-gate routes, no detour through distant gates, missing exits, bounded work per callback |
| [tests/network.rs](../crates/toy-sim-server/tests/network.rs) | Starts the real `toy-sim-server` binary with a ready file. A wrong server key and a wrong account key fail to connect. One account receives telemetry, a view with tracks, a stock MFD frame with at least five primitives and an appearance asset. A second account joins the first ship's group, sees its track and cannot command it. Reset keeps the connection usable, discards delayed inputs for the previous world and accepts a new view subscription. The server shuts down cleanly when standard input closes. |
| `toy-sim-client` tests | Buffer ordering, reserve refill, skipped and paused timestamps, retained publications across trimming, command coalescing and backpressure, concurrent asset transfers, shared Bevy asset handles, explicit retry and unloading; UI tests cover fixed scheduling, spatial lifetimes, celestial loading, orbital projection, effects and selection |

```sh
cargo test -p toy-sim-protocol -p toy-sim-net -p toy-sim-intel -p toy-sim-server -p toy-sim-client -p toy-sim-example-controller
```

Enable `toy-sim-client/ui` to run the client ECS, asset-pipeline and rendering-math tests. These tests do not launch a graphics window; visual verification uses `toy-sim-debug`.

## Benchmark

[examples/benchmark.rs](../crates/toy-sim-server/examples/benchmark.rs) measures a real server process with real TCP clients. Build the server first; by default the example runs the `toy-sim-server` executable from the same target profile directory.

```sh
cargo build -p toy-sim-server
cargo run -p toy-sim-server --example benchmark
cargo run -p toy-sim-server --example benchmark -- --ships 128 --sessions 8 --frames 50

cargo build --release -p toy-sim-server
cargo run --release -p toy-sim-server --example benchmark -- --ships 512 --sessions 16
```

| Option | Default | Range |
| --- | --- | --- |
| `--ships` | 16 | 1 to 1024 |
| `--sessions` | 4 | 1 to `ships` |
| `--frames` | 20 | at least 2 |
| `--warmup-ticks` | 70 | any |
| `--server` | `toy-sim-server` next to the target profile directory | an existing file |

The benchmark:

1. Writes a server configuration in a private temporary directory with one account per ship, `listen = "127.0.0.1:0"` and the first account as `debug_account`. The scenario therefore spawns one player ship per account, plus the traffic ship, station and gates ([Scenario](#scenario)).
2. Starts the server with `--ready-file` and `--shutdown-on-stdin-close`, and waits up to 120 s for readiness.
3. Connects `sessions` clients, one per account, through `toy_sim_client::connect`. Each subscribes one view on its default group focused on its ship, with no sphere, a limit of 256 and a work budget of 1,000,000. The first client also enables debug inspection.
4. Acknowledges every frame until `warmup-ticks` ticks after its first frame, then waits for all clients.
5. Samples the server's CPU time from `/proc/<pid>/stat`, then each client receives `frames` frames. For each frame it re-encodes the received frame as a `State` message and compresses it through its own Zstd encoder (level 3, window log 21) that keeps history across frames, and it records skipped ticks and the server's reported tick duration.
6. Prints one CSV row after reading `/proc/<pid>/status`.

| Column | Meaning |
| --- | --- |
| `ships`, `sessions`, `frames_per_session` | Options |
| `tracks_per_frame` | Mean tracks per received frame |
| `frame_bytes` | Mean uncompressed `State` message size |
| `recompressed_zstd_bytes` | Mean output of the benchmark's own Zstd encoder per frame |
| `reencode_ms`, `recompress_ms` | Mean client-side time to re-encode and recompress one frame |
| `encoder_bytes_per_session` | Zstd compression context size |
| `server_tick_ms` | Mean of the tick duration reported in the debug client's diagnostics |
| `server_cpu_percent` | Server process CPU time over wall time during measurement; above 100 means several cores |
| `server_rss_kib`, `server_peak_rss_kib` | `VmRSS` and `VmHWM` of the server process after measurement |
| `received_hz` | Frames received per second of client measurement time |
| `skipped_ticks` | Ticks missing between consecutive received frames, summed over sessions |

Scope and interpretation:

- The CPU and memory figures are for the server process only. The clients run in the benchmark process and are not included.
- `server_tick_ms` is the simulation tick alone. Session frame building, display updates and network writes happen outside it, and are included only in `server_cpu_percent`.
- `recompressed_zstd_bytes` and `recompress_ms` are isolated measurements of the benchmark's encoder. The real transport splits writes into 32,768-byte chunks, compresses on blocking threads behind a semaphore, and adds encryption, picomux and TCP framing, so these are not the wire size or the server's compression cost.
- All clients and the server share one machine and loopback networking. There is no network latency, packet loss or bandwidth limit.
- The ships are the scenario's ships in orbit near one planet. They fly no travel orders and fight no battles.
- It reads `/proc` and `getconf CLK_TCK`, so it runs only on Linux.

A local run on 2026-09-16 used an AMD Ryzen 9 5900XT, the optimized development profile, 16 player ships, four sessions, 70 warmup ticks and 30 measured frames per session:

| Measurement | Result |
| --- | --- |
| Mean simulation tick | 1.989 ms |
| Server CPU | 19.0% of one logical core |
| Server RSS / peak RSS | 89.1 MiB / 89.1 MiB |
| Mean tracks per frame | 4.8 |
| Mean uncompressed frame | 4187.5 bytes |
| Mean recompressed frame | 455.4 bytes |
| Re-encode / recompress time | 0.028 ms / 0.044 ms |
| Encoder context per session | 3.49 MiB |
| Delivery | 10.00 Hz, zero skipped ticks |

This three-second sample checks the small idle scenario. Dense views, sustained combat, many display subscriptions and larger fleets need separate measurements before capacity estimates.

## Firmware scan budgets

Native fused scans cost 3000 gas per requested contact, covering both group and public queries. The standard firmware starts with 32 contacts, grows when its buffer fills and shrinks when results are sparse. It budgets scans and forecasts against remaining gas, preserving flight control, publication and the next callback. Dense-sensor regression tests exercise repeated callbacks under the existing runtime limits.

Unused information groups are collected periodically. Account defaults, ship memberships, public information and active session subscriptions keep their groups alive.

## Station and navigation expansion

Protocol version 7 adds the public navigation catalogue and integer cargo/consumable telemetry. The client replicates moving beacons into ECS pose samples so their markers interpolate on the same clock as ships. Gate topology supports a subway-style map and shortest-hop route previews; selected routes become explicit `Jump` orders for the flight computer. Queue controls can remove, reorder, pause and resume commands. The HUD shows the active action, destination markers and slip ETA. Ship camera focus is limited to visible nearby contacts within 100 km.

Docked and transiting ships have private meshes independent of sensor contacts. Docking opens a client-rendered hangar with orbit-camera controls; slip transit uses a procedural streak tunnel. Neither view reactivates ship physics. Gates use a sparse GLB frame, an animated spherical distortion material and a shadow-casting point light. See [stations-navigation.md](stations-navigation.md) for the catalogue, controls and verification commands.
