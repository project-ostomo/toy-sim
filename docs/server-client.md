# Authoritative server and network client

The simulation runs only in `osg-server`. Every UI is a network client: the standalone `osg-client`, and `osg-debug`, which starts its own server process and connects to it over loopback TCP. This guide describes the server process, its configuration, the transport and application protocol, the sensor and visual observations available to each ship, presentation data, display instances, docking and travel, client playback, the client UI, and the benchmark.

The server restores durable world state from SQLite checkpoints, including ownership, ships and installed WASM programs. Sessions reconnect after a restart. A debug reset creates a new world ID; inputs carrying another world ID are discarded so existing connections can survive a reset. Checkpoints default to every 900 seconds, with initial and graceful-shutdown saves. Unsupported or corrupt newest checkpoints stop startup explicitly.

## Crates

| Package | Path | Role |
| --- | --- | --- |
| `osg-model` | [crates/osg-model](../crates/osg-model) | Shared serde types: IDs, poses, sensor observations, queries, frames, actions, debug commands, presentation records, travel orders and drawing lists; explicit conversions to the WASM C ABI records |
| `osg-protocol` | [crates/osg-protocol](../crates/osg-protocol) | Application message framing and validation of client requests |
| `osg-net` | [crates/osg-net](../crates/osg-net) | TCP handshake, record encryption, Zstd compression and picomux multiplexing |
| `osg-spatial-bvh` | [crates/osg-spatial-bvh](../crates/osg-spatial-bvh) | Shared immutable BVHs for brightness, radius, nearest-neighbour, segment and metered cursor queries |
| `osg-universe` | [crates/osg-universe](../crates/osg-universe) | Shared astronomical catalogue, lazy deterministic generation, Keplerian solver and initial population recipe |
| `osg-server` | [crates/osg-server](../crates/osg-server) | The Bevy ECS simulation in private modules under [src/sim](../crates/osg-server/src/sim), the simulation loop, TCP listener, asset streams, configuration, key provisioning and the benchmark example |
| `osg-client` | [crates/osg-client](../crates/osg-client) | `connect`, asset fetching, the `Playback` buffer, and the Bevy/egui UI behind the `ui` feature |
| `osg-debug` | [apps/osg-debug](../apps/osg-debug) | Local launcher: server child process plus the client UI |
| picomux | [crates.io](https://crates.io/crates/picomux) | Published picomux 0.3.1 with independent stream read and write shutdown |

The server modules are private to `osg-server`. Its public library interface includes `launch::run`, `provision::demo`, `run`, `listen`, `scenario`, `assets`, the shared `AppearanceAssets` store and `key_bytes`.

## Running

```sh
# Build the server and the debug launcher in the same profile
cargo build -p osg-server -p osg-debug

# Local server with the client UI and debug access
cargo run
cargo run -- --ship assets/ships/starter.ship
cargo run -- --server target/debug/osg-server
cargo run -- --check

# Generate demo keys and configuration
cargo run -p osg-server -- --init demo

# Dedicated server (stops on Ctrl-C)
cargo run -p osg-server -- demo/server.toml

# Remote client
cargo run -p osg-client --features ui -- demo/client-a.toml
```

### Server command line

```
osg-server --init <directory>
osg-server <config.toml> [--ready-file PATH] [--shutdown-on-stdin-close]
```

- `--init <directory>` creates the directory if needed and writes `server.toml`, `client-a.toml` and `client-b.toml`. It generates a server Ed25519 key and two accounts with random UUIDs and Ed25519 keys. The files are created with mode `0600` on Unix and contain private keys. The command refuses to run if any of the three files exists. The addresses are `127.0.0.1:23000`. It does not write `debug_account` or `ship`.
- `--ready-file PATH` writes the actual listening address (for example `127.0.0.1:41234`) to `PATH` once the scenario is built and the listener is bound. The file is removed on shutdown.
- `--shutdown-on-stdin-close` starts a thread that reads standard input and stops the server when it reaches end of file or fails. A parent process uses this to tie the server's lifetime to its own.

Any other argument is an error. The server also stops on Ctrl-C, SIGTERM on Unix, and when the simulation thread ends. Graceful shutdown writes a final checkpoint. SIGUSR1 requests a checkpoint on Unix.

### Configuration files

Configuration files reject unknown fields.

| File | Fields |
| --- | --- |
| Server | `listen` (a socket address; port 0 picks a free port); `server_secret` (64 hex characters, the Ed25519 secret key); `[[accounts]]` entries with `id` (UUID) and `public_key` (64 hex); optional `debug_account` (UUID); optional `ship` (path to a `.ship` blueprint, relative paths resolved against the configuration file's directory); optional `[persistence]` with `enabled` (default true), `path` (default `world.sqlite`, relative to this directory) and `interval_seconds` (default 900) |
| Client | `address`; `server_public_key` (64 hex); `account` (UUID); `account_secret` (64 hex) |

`debug_account` must be one of the configured accounts, or startup fails with "debug account must be authorized". That account gets the debug capabilities described under [Debug accounts](#debug-accounts).

### Debug launcher

`osg-debug [--ship PATH] [--server EXECUTABLE] [--state-dir PATH] [--ephemeral] [--check]` ([main.rs](../apps/osg-debug/src/main.rs)):

- Without `--server`, the server executable is `osg-server` in the same directory as the launcher's executable. If it is not a file, the launcher fails with "build osg-server first, or pass --server EXECUTABLE".
- It keeps credentials and checkpoints under `$XDG_STATE_HOME/openspacegame/debug`, or `~/.local/state/openspacegame/debug`. `--state-dir` chooses another world; `--ephemeral` creates a disposable directory. Credentials are reused on later launches, and a directory lock prevents concurrent launchers from sharing the same state. The generated configuration listens on `127.0.0.1:0` and designates the local account as `debug_account`. A saved world restores its stored blueprint even if the original `--ship` file has disappeared.
- It starts the server with `--ready-file` and `--shutdown-on-stdin-close` and a piped standard input. It waits up to 60 s for a parseable address in the ready file, and fails if the server exits first.
- It connects with `osg_client::connect`. The debug session uses the same TCP, handshake, encryption, compression, multiplexing and session code as a remote client.
- With `--check`, it waits up to 10 s for a state frame, fails with "server did not provision the debug ship" if the frame has no ship telemetry, and prints the address, tick and ship count. Otherwise it runs the client UI with a playback target depth of 1.
- On exit, it drops the server's standard input and waits up to 120 s for shutdown and its final checkpoint. A server still running after that deadline is killed. Only disposable state directories are removed.

The ship editor's "Launch sim" runs `osg-debug --ship <snapshot>` ([ship-editor.md](ship-editor.md#launching-the-debug-client)).

### Scenario

[bootstrap.rs](../crates/osg-server/src/sim/bootstrap.rs) builds the world from the Bevy application in [sim/mod.rs](../crates/osg-server/src/sim/mod.rs):

- **Universe.** The astronomical catalogue covers the full bundled sky, with enriched stellar records and authored overrides. Systems resolve through one deterministic generator when needed. The initial population recipe selects 3,000 systems for infrastructure; later public inhabitation follows actual operating directory equipment. The starting encounter is in Helion. See [Celestial generation](../crates/osg-universe/GENERATION.md).
- **Explorer.** "Patrol ship" starts in a circular orbit 1,000 km outside Helion I Neris's slip exclusion radius, on the day side, with the tangential direction selected from seed 42 ([scenario.rs](../crates/osg-server/src/sim/scenario.rs)). Its design is the configured `ship` blueprint, or the bundled micropulse patrol ship. If the blueprint fails to load or compile, the server logs "Cannot load ship" and spawns no ships at all, so scenario setup fails.
- **Hostile patrol.** "Hostile patrol 001" uses the bundled micropulse patrol design, starts 1 km from the player with the same initial velocity, and points at it. It remains passive until commanded; there is no autonomous NPC behavior.
- **Players.** The first configured account controls the explorer. Account *n* (from 1) gets "Explorer *n+1*", with the explorer's design and velocity, offset by *n* × 1,000 m along galactic +Y. Every player ship gets a default slipdrive.
- **Ownership and infrastructure.** The hostile patrol belongs to Terminus Privateers. Neris Anchorage and initial directory/navigation installations have real equipment and inventories. Their continued public presence depends on power, damage, position, and transponder settings.
- Every ship gets a test loadout, and every flight computer boots for 5 s.

After spawning, bootstrap rebuilds identity and spatial indexes and publishes current sensor observations for the first frame.

## Server loop

`osg_server::launch::run` ([launch.rs](../crates/osg-server/src/launch.rs)) reads the configuration, binds the listener, and builds the scenario on a thread named `simulation`. It locks and reads the configured checkpoint database before choosing a bootstrap design, restores any saved world, and commits the initial checkpoint for a new world before publishing readiness. Once the world and its asset map exist, it writes the ready file and runs the listener. `osg_server::run` ([lib.rs](../crates/osg-server/src/lib.rs)) owns the Bevy `App` on that thread and loops every 100 ms of wall-clock time:

1. Accepts new connections into sessions. If 1024 sessions already exist, the connection is dropped.
2. Applies up to 4 queued input frames per session. A frame that fails validation, or a closed input channel, disconnects that session. Valid frames for another world are discarded before checking session sequence numbers.
3. If a debug reset was requested, rebuilds the scenario from the same configuration and reconnects every existing connection to the new world.
4. Applies queued debug requests ([Debug accounts](#debug-accounts)).
5. Runs simulation ticks. It adds the positive rate to a tick credit and runs the whole number of ticks in the credit, so a rate of 10 runs 10 ticks in one loop iteration. Each tick is one `App::update`, and its wall-clock duration is recorded for diagnostics.
6. Updates display instances once ([Display instances](#display-instances)).
7. Builds one `Frame` per session and queues the complete encoded frame in a FIFO. Each connection has a 64 MiB outbound byte budget, including the frame currently being written. Exhausting that budget disconnects the slow connection.
8. Collects completed checkpoint writes and captures a requested or due checkpoint for the bounded background writer. Then it sleeps until the next 100 ms deadline. If it is already late, the next deadline starts from now.

The loop exits when the listener side has closed and no sessions remain, or when the stop flag is set.

The listener accepts at most 1024 concurrent TCP connections through a semaphore. Connections beyond that are closed immediately. Each connection must complete the handshake within 10 s and open its `main` stream within a further 10 s. A state frame write that takes more than 10 s ends the connection.

### Simulation tick

One tick runs these Bevy schedules ([sim/mod.rs](../crates/osg-server/src/sim/mod.rs), [simulation.rs](../crates/osg-server/src/sim/simulation.rs)):

- **`FixedFirst`.** Travel advances: docked orders, slip intersections and slip preparation ([Docking and travel](#docking-and-travel)).
- **`FixedUpdate`.** Star systems are activated for ships inside their influence radius. World-service indexes (beacons, celestial poses, the public snapshot, slip apertures) are published and each ship's world source is prepared. Then, in order: history, `PrepareBodies` (flight programs and typed hardware systems), `Forces` (gravity and drag).
- **`FixedPostUpdate`.** Firmware world actions are applied in ship ID order. Then `Integrate` (rigid bodies and the collision solver) and `Celestials`.
- **`FixedLast`.** The spatial index is rebuilt, sensor scans run, and the tick counter increments. Then travel geometry and celestial identities are refreshed, sensor observations are published, and travel events are recorded.

Ship hardware lives in ECS components ([hardware.rs](../crates/osg-server/src/sim/hardware.rs)): `ShipInventory`, `Hull`, `ShipThermal`, `Avionics`, `DeviceSettings`, `HardwareClock` and `SensorRange` on the ship, and one entity per installed part with `InstalledPart`, `Device`, typed device components such as generators, engines, RCS, torquers and shields ([devices.rs](../crates/osg-server/src/sim/hardware/devices.rs)), and `Weapon` where present. `osg_ships::ShipState` is used to build these components when a ship spawns, resets or is recovered, and `hardware::snapshot` assembles one from them for presentation and collision damage. The simulation does not step a `ShipState`.

Flight programs run in parallel across ships. A program is called only when the interval it set with `tick_set_interval` has elapsed or requests are waiting ([ship-abi.md](ship-abi.md#scheduling)). At most `MAX_BOOTS_PER_TICK` computers boot per tick.

## Transport stack

```
TCP (TCP_NODELAY)
└─ handshake: X25519 + Ed25519 server identity + Ed25519 account proof
   └─ encrypted records: ChaCha20-Poly1305, implicit sequence numbers
      └─ one Zstd stream per direction (level 3, window log 21)
         └─ picomux
            ├─ "main" stream: TSF1 Input frames ⇄ TSF1 State frames
            └─ "assets" streams: 32-byte hash → asset bytes until EOF
```

Integers in the fixed-layout transport and protocol fields below (handshake hello, record length, key epoch, nonce and associated data, picomux frame header and `MORE` body, application message length) are little-endian. Message values are postcard-encoded, which uses its own variable-length integer encoding.

### Handshake

The handshake is in [crypto.rs](../crates/osg-net/src/crypto.rs). Both `connect` and `accept` enforce a 10 s timeout.

A hello is 66 bytes: a `u16` shared `GAME_VERSION`, 32 random bytes, then a 32-byte X25519 ephemeral public key.

1. The client sends its hello.
2. The server checks the version and sends its own hello, followed by a 64-byte Ed25519 signature over the transcript hash:
   `transcript = BLAKE3 derive_key("OpenSpaceGame transport v1 handshake", client_hello ‖ server_hello)`
3. The client checks the version. It verifies the signature with `verify_strict` against the server key pinned in its configuration. A mismatch fails with "server identity mismatch".
4. Both sides compute the X25519 shared secret and reject a non-contributory result. Key material is `shared_secret ‖ transcript`:
   - client-to-server root key: `BLAKE3 derive_key("OpenSpaceGame transport v1 c2s", material)`
   - server-to-client root key: `BLAKE3 derive_key("OpenSpaceGame transport v1 s2c", material)`
5. The client sends the first encrypted record (sequence 0). It holds 80 bytes: the 16-byte account ID, then an Ed25519 signature over
   `BLAKE3 derive_key("OpenSpaceGame transport v1 account authentication", account_id ‖ transcript)`.
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

[pipe.rs](../crates/osg-net/src/pipe.rs) wraps the record layer in `AsyncRead`/`AsyncWrite`:

- Each direction has one Zstd stream that lasts the whole connection. The encoder uses level 3 and window log 21 (2 MiB). The decoder sets `WindowLogMax(21)` and rejects streams that need a larger window. History carries across writes, so repeated frame content compresses against earlier frames.
- Each `poll_write` takes up to 32,768 bytes. The pipe compresses the chunk, flushes the encoder so the peer can decode it immediately, and splits the output into records of at most 65,535 data bytes.
- Decoding one record may produce at most 256 KiB of plaintext. More is a protocol error.
- Streaming compression and decompression run directly in the async reader and writer tasks, in bounded chunks. Network I/O and downstream backpressure provide the await points; there is no blocking task pool or compression semaphore.
- Writes are pipelined. `poll_write` queues the chunk and returns before it is sent. A write error is reported on the next write, flush or shutdown. Decoded bytes pass through a 64 KiB in-memory pipe to the reader.

### Multiplexing (picomux)

`osg-net` runs picomux over the pipe with debloat mode on. A picomux frame has an 8-byte header followed by the body:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | Version, which must be 1 |
| 1 | 1 | Command: `SYN` 0 (body is the stream metadata), `FIN` 1, `PSH` 2, `NOP` 3, `MORE` 4 (body is a `u16` window increase), `PING` 0xa0 (JSON `{"next_ping_in_ms":…}`), `PONG` 0xa1 |
| 2 | 2 | Body length (`u16`) |
| 4 | 4 | Stream ID (`u32`, chosen at random by the opener) |

The published picomux 0.3.1 crate provides per-stream flow control and concurrent streams:

- Pending accepts have a 100-stream queue. Closed stream IDs remain reserved for 3600 s. A `SYN` for an open or reserved ID is a protocol error.
- Outgoing data is split into frames of at most 8192 bytes (the MSS).
- Flow control is per stream and counted in frames. The initial window is 10 and the maximum is 1500; debloat mode adjusts the receive window using measured throughput.
- The outgoing queue accommodates concurrent senders. Each data writer waits until fewer than 10 frames are queued before continuing.
- A `FIN` closes one direction of a stream. Shutdown drains buffered writes and queues `FIN` after the data; the opposite direction remains readable until its own EOF. Both peers need half-close support for response-after-EOF transfers.

The default liveness settings are a ping every 1800 s with a 30 s timeout, plus a forced ping whenever a stream is opened.

### Streams

- **`main`.** The client opens it first. The server requires the first accepted stream to have metadata `main`. It carries application `Input` messages from the client and `Session`/`State` messages from the server. The server sends a universe descriptor before state for a world. The server's per-connection input queue holds 16 frames. If it is full, the connection closes with "input rate exceeded".
- **`assets`.** Each download uses a separate stream with metadata `assets`.
- **`blueprint-upload`.** Each private shipyard upload uses a separate stream with this metadata. Unknown labels end the connection. Transfers run independently on both peers, with no fixed transfer-count limit; dropping the connection cancels outstanding tasks.

**Asset transfer.** The client opens a fresh stream, writes exactly the asset's 32-byte BLAKE3 hash and shuts down its write direction. The server streams the complete asset bytes and shuts down its write direction. EOF delimits the response. The client reads to EOF and verifies the complete asset's BLAKE3 hash. There are no application chunk messages, offsets, length headers or acknowledgements. Picomux and TCP provide transport framing and flow control. An unknown hash gets an empty response, which fails verification for a nonempty expected asset. Download and hash failures are delivered to the requesting UI through that asset's result; other transfers and the main stream continue.

**Blueprint upload.** The client writes the 32-byte BLAKE3 hash followed by the
complete canonical `.ship` file, then closes its write direction. EOF delimits
the file. After checking its size and hash, the server stores the file privately
in that authenticated session and sends one Postcard `BlueprintUploadAck`, then
EOF. `Ready` contains the matching hash; `Rejected` contains a UTF-8 reason of
at most 256 bytes. The complete acknowledgement is limited to 512 bytes. The
client waits for `Ready` before sending a hash-only `BuildShip` command on `main`.

Files are limited to 16 MiB. Receiving and staged file allocations share limits
of 64 MiB per session, 128 MiB per account and 256 MiB per listener. Transfers
have a 60-second deadline. Quota leases remain attached to bytes held by a
validator after a disconnect. Uploads are not public assets and cannot be
looked up by another session. A successful upload does not authorize construction:
the subsequent command still checks authority, design, firmware, materials and
facility capacity. Accepted jobs retain their full blueprint in world snapshots.

The server and network tasks share a dynamic asset store. Its entries contain immutable bytes under their BLAKE3 hashes; newly published assets become available to existing connections immediately. A transfer holds a shared byte allocation after lookup and releases the store lock before awaiting network writes. Debug reset preserves the shared store and installs the new scenario's assets into it. The store holds:

- **Ship appearances.** A TOML `ShipAppearance` containing the catalogue revision and visible part prototypes, IDs, positions and rotations. It contains no firmware, tank allocations or operational loadout. Its BLAKE3 hash is the `appearance` field of an optical observation or a `Destroyed` combat event. Sensor observations do not publish appearance hashes.
- **The inhabited directory.** A Postcard-encoded `InhabitedDirectory`, referenced by `presentation.navigation.directory`. It contains system IDs, sovereignty ownership, and public sovereignty names and blocs. Ownership follows the organization with a strict majority of broadcasting installations.

## Application messages

The `main` stream carries a four-byte little-endian body length followed by one
Postcard-encoded `Message`: `State`, `Input`, or `Session`. There are no section
headers or per-message versions. The initial authenticated handshake checks the
shared `GAME_VERSION`, also used by firmware and saved worlds. Field and enum
ordering are part of that game version's wire format.

Server states and assets are trusted after decoding. The client uses their
sequence numbers, timestamps, geometry and identities directly. A `Session`
describes the world, orbital epoch and simulation-time origin. A world reset
sends a new descriptor before the new world's state.

`PresentationFrame.navigation` contains the inhabited-directory asset hash and
relevant live infrastructure. Clients resolve names and positions from their
catalogue. Membership changes publish a new hash; orbital motion updates live
poses independently. Obsolete asynchronous downloads cannot overwrite a newer
publication.

**Input frame limits.**

- At most 256 actions, with unique command IDs.
- Subscription queries: `limit` from 1 to 256, `work` at most 10,000,000, at most 32 tags in total and valid tags. A search sphere needs a valid centre and a radius from 0 to 1e22 m.
- `ScreenSubscribe`: slot below 8 and `hz` from 1 to 10. `ScreenUnsubscribe`: slot below 8.
- `MarkTarget`: maximum flight time from 0 to 3600 s; stock firmware further restricts this to [0.01, 60] s.
- `Flight(AimDirection)`: finite and not zero length. `Flight(EngageNavigation)`: throttle limit from 0 to 1 and stand-off from 0 to 1e22 m.
- `SetIff`: at most 16 valid labels, and range from 0 to 1e12 m.
- `SetTravel`: at most 256 orders. Galactic and relative destinations need valid positions.
- `ScreenInput`: slot below 8, kind at most 7, text at most 64 bytes, and coordinates at most 1e6 in magnitude.
- Debug commands: `SetRate` finite, greater than zero and at most 100, `ConfigureSensor` range from 0 to 1e22 m, `Relocate` a valid pose, and heat injections from 0 to 1e30 J.

## Sessions

Each connection gets a `Session` entity ([session.rs](../crates/osg-server/src/sim/session.rs)). It receives observations from ships the account is authorized to inspect.

### Input frames

After protocol validation, a frame whose `world` differs from the current world is discarded without applying its actions or changing sequence state. This handles inputs already in transit when the debug server resets. For frames in the current world, any of these violations disconnects the session:

- `sequence` must be strictly greater than the previous input frame's.

Actions are applied in order before the next simulation tick, and each result's `effective_tick` is the current tick. The session remembers every command ID it has seen and ignores repeats. After 65,536 IDs, further input is refused ("session command history exhausted; reconnect"). An action error does not close the session. It is reported in `CommandResult.error`.

### Results and events

- Each command result is included once, in the next published frame. Its delivery uses the same reliable FIFO as the snapshot.
- A frame includes new events whose `subject` is either an entity with a known ID in one of the frame's sensor observations, or a ship the account controls. Events with no subject are not sent. Each session advances a server-local publication cursor when building a frame. Event staging is pruned after sessions publish; its lifetime does not depend on client acknowledgements or network progress.
- Event kinds with a subject: `docked`, `undocked`, `destroyed`, `slip-departed`, `slip-arrived` and `relocated`. Collision and weapon records use the kind `combat` with no subject; they reach clients only as presentation combat events.

### Actions

A session can request detailed data for at most eight distinct ships across focused views, instruments and MFD subscriptions. Lightweight owned-ship telemetry remains available for up to 64 ships. Detailed hardware publication is limited to subscribed ships.

| Action | Effect |
| --- | --- |
| `Subscribe(ViewSubscription)` | Adds or replaces view `id`. A replacement must have a higher `revision`. At most 8 views. The account must have observe permission for the focused ship. |
| `Unsubscribe(id)` | Removes a view |
| `InstrumentSubscribe { ship }` / `InstrumentUnsubscribe` | Requests instrument presentation for a controlled ship. At most 8. |
| `ScreenSubscribe { ship, slot, hz }` / `ScreenUnsubscribe` | Requests display frames for a controlled ship. At most 8 subscriptions. |
| `IndustrySubscribe(IndustrySubscription)` / `IndustryUnsubscribe` | Requests an authorized directory page, selected inventories and optional manufacturing catalogue. Replacement revisions must increase. |
| `Industry(IndustryCommand)` | Starts or cancels production, orders a ship, transfers cargo or refills tanks; permissions and physical transfer constraints are checked when applied. |
| `Debug(command)` | Requires a debug account ([Debug accounts](#debug-accounts)) |
| `Ship { ship, authority_revision, command }` | The account must control the ship, and `authority_revision` must equal the ship's current revision |

Views with a focused ship, instrument subscriptions and screen subscriptions together may name at most 8 distinct ships ("detailed ship subscription limit").

Player commands call `sim::commands::execute`
function. It validates command payloads, checks the account's current permission
and authority revision, and queues ordinary ship-computer requests. Network
sessions also enforce ship permissions and display subscriptions.
Target commands resolve a `ContactRef` only in the commanded ship's current sensor
snapshot; knowing a physical entity UUID does not authorize a target. Two-request
autopilot changes reserve both queue slots before changing pending travel state.
Accepted external navigation commands wake the flight computer and pause any
standing freight assignment.

Director observation tools use the same owned-ship telemetry and hardware
presentation as clients. Their `ScanSource` is rebuilt from the ship's current
state, including its host-relative pose while docked, and exposes the existing
current sensor snapshot and public navigation queries. Each new observation and
command checks the officer account's authority; no network session or global
entity-lookup tool is required.

| `ShipCommand` | Effect |
| --- | --- |
| `SetTransponderEnabled(bool)` | Switches the IFF transponder |
| `SetIff(IffIdentity)` | `owner` must identify the requesting controller. Configure permission is required; `faction` must be an organization the account belongs to or administers. Range is at most 1e8 m. |
| `Flight(FlightCommand)` | `HoldAttitude`, `StopGuidance`, `AimDirection`, `SelectTarget(ContactRef)` or `EngageNavigation { throttle_limit, stand_off_m }`, queued to the flight computer as requests. `SelectTarget` resolves its contact like `Aim`. |
| `MarkTarget { target, maximum_flight_time_s }`, `Aim { target }` | The contact must belong to the commanded ship and exist in its current sensor snapshot. |
| `UnmarkTarget` | Clears the marked target and stops firing |
| `StartFiring` | Enables weapons against the marked target; firmware rejects the request without a mark |
| `StopFiring` | Stops firing while retaining the mark and aiming solutions |
| `SetTravel { engage, expected_revision, orders }` | Requires the current travel revision. Replaces orders and increments the revision. `engage` enables autopilot; otherwise its current state is preserved. Status becomes `Planning` when enabled, or `Paused` when disabled. |
| `SetAutopilot(bool)` | Disabling pauses the queue, cuts thrust and holds attitude. Enabling resumes planning. |
| `SetThrottle(f64)` | Sets manual throttle in [0, 1]; rejected while autopilot is enabled. |
| `Dock { station, bay }` / `Undock` | Direct docking operations ([Docking and travel](#docking-and-travel)) |
| `ScreenInput { slot, revision, kind, code, modifiers, xy, text }` | Requires a subscription to the slot and a live display instance. `revision` must match the displayed frame. Queues a `ScreenEvent` on the display instance. |

The flight computer's request queue accepts a command only while it holds fewer than 255 entries ("ship command queue full").

### Industry and cargo subscriptions

`IndustrySubscription` selects a directory page using `directory` and the
exclusive `directory_after` ID, up to eight inventory IDs in priority order, and
whether the manufacturing catalogue is needed. An optional `HangarSubscription`
selects a focused ship and local docked-ship cursor. The server resolves its
current host and filters host storage and docked ships by authority before
pagination. This local list is independent of the global directory. The directory contains at most
128 authorized summaries and a `directory_next` cursor. Selected facility views
include ownership, permissions, installed capabilities, cargo stacks and jobs.
The server rechecks access on every publication, so revocation removes private
contents without requiring the client to resubscribe.

`Frame.industry = None` means there is no industry update. A received snapshot
replaces the directory and selected facility views for its
`subscription_revision`. Its optional catalogue replaces the cached recipes and
blueprints only when present; catalogue revisions are content hashes. Sessions
send a catalogue once per subscription or catalogue change. Closing the windows
unsubscribes; reconnecting or changing worlds clears retained industry data.
Playback retains every industry update when consuming two snapshots to catch up,
including a catalogue delivered in the earlier snapshot.

Industry updates are limited to 512 KiB. Entire inventory views that do not fit
are listed in `omitted_inventories`; their stack lists are never truncated.
The client must clear omitted details and report the capacity limit. If one
inventory or the requested catalogue alone cannot fit, a bounded `error` with
the subscription revision replaces the details. Build commands carry a private
upload hash within the normal 64 KiB input frame; recipe requests accept
1–10,000 batches.

`ShipPresentation.inventory` contains consumables. Its separate `cargo` list
uses the same `CargoStack` records as facility views: a resource or part-kit ID,
integer total and reserved quantities, display name, unit mass and unit volume.
Reserved quantities remain physically stored and are unavailable to other jobs,
transfers and refills. Cargo transfers use `IndustryCommand::Transfer` for both
resources and kits. Refill requests identify the source inventory and target
ship. Remote production management does not permit remote movement of cargo;
the server checks both inventories' permissions and physical location.

### State frames

A session frame contains:

- **Clock.** `world`, a per-session `sequence` starting at 1, `tick`, `sim_time_ns` (the fixed clock's elapsed time), and `rate`, the current debug clock rate (1 unless a debug account changed it).
- **Views.** Each view names an authorized focused ship and reports its origin. Views and detail subscriptions are removed when observation permission is revoked.
- **Contacts.** Current sensor detections for the authorized ships named by views and detail subscriptions, keyed by observing ship UUID.
- **Ships.** Private telemetry for up to 64 ships the account controls. Ships named by a view focus, a screen subscription or an instrument subscription come first, then the rest, each part in ship ID order. Each record holds IFF identity, authority revision, presence, exact pose (only while in space), battery energy, hull heat, shield temperature, coolant reserve and travel state.
- **Screens.** One update per subscribed slot: the latest display frame, or, if there is none, an update with no frame and the error "Display unavailable".
- **Events and results**, as described above.
- **Presentation** ([Presentation](#presentation)), plus per-view **optical observations** ([Optical replication](#optical-replication)).
- **Society.** Ownership hierarchy, standing overrides and permitted asset access information.
- **Calendar.** Authoritative real UTC plus 400 Gregorian years, independent of accelerated simulation time.

Network views have no continuation. Each frame runs a fresh query, so a view that stops at `WorkLimit` or `ResultLimit` shows only its first page.

## Inhabited map and generated systems

The initial political population spans approximately 250 light-years around Sol.
Its placement recipe creates actual facilities and organizations. After initial
creation, public inhabitation is derived from operating directory transmitters
with lit transponders inside each system's gravitational influence. Hull category
and ordinary ship transponders do not determine membership. Destroying, disabling,
darkening, or moving the final transmitter removes the system from the public
directory. A new transmitter can add any catalogue system.

Server and client independently resolve the same deterministic body definitions
and orbital poses. Generation uses identity-separated ChaCha20 streams and
shared astronomical inputs. Generated planets and companion orbits are fictional
simulation parameters. Authored systems use the same resolver and lifecycle.

The server runs periodic celestial work only around actual noncelestial objects
outside slip. Dark stations and wrecks still require simulation. Read-only
queries and slipping ships may resolve definitions without activating systems.
Clients select their own detail from views, inspection, and route previews.
Celestial definitions, activation lists, and ephemeris subscriptions are absent
from the wire protocol. Both processes use bounded definition caches.

The universe fingerprint depends on catalogue data, authored inputs, and generator
revision, independently of which definitions have been requested. Incompatible
saved worlds fail explicitly; see [Persistence](persistence.md#universe-definition-changes).

## Shared spatial queries

[osg-spatial-bvh](../crates/osg-spatial-bvh/src/service.rs) owns the shared spatial service. Positions remain signed 128-bit micrometres until a query needs relative floating-point distances. Radii use metres; indexed luminosity is optical watts. Photometric catalogue values use 220 lumens per optical watt at the boundary.

The spatial service contains three `LuminosityBvh` trees: an unchanged static tree, an instantaneous dynamic tree, and a swept dynamic tree. Both dynamic trees rebuild at 10 Hz. Leaf payloads contain only IDs, distinguished by proxy role, and both dynamic trees share one object lookup table. Sphere, nearest, visibility and segment queries use the instantaneous tree; motion and collision queries use the swept tree. Exact geometry, brightness and permission predicates remain with callers. Cursors budget both node visits and leaf tests and borrow the service until traversal finishes. Previous tick trees are not retained. Published asynchronous aperture checks own a separate service containing only their aperture bodies.

Consumers share the trees:

- Server observations retain active ship and celestial metadata. Celestials block sensors but are excluded from ship sensor results. Projectiles have collision records but no sensor records; dormant ships have neither.
- Immutable sensor snapshots hold at most 256 current detections per observing ship.
- The universe catalogue and client sky share the static tree. One source leaf covers each star and its system envelope, followed by the appropriate exact predicate.
- Travel geometry queries physical extents and natural exclusion spheres for departure and swept capture.
- Collisions use the swept tree; shield clearance and beams use the instantaneous tree in the same service. Parry retains its shape queries and compound acceleration structures; see [collisions.md](collisions.md#broad-phase).

The collision step rebuilds the current service from tick-start poses and predicted motion before solving contacts. That state stays authoritative until the next tick. Travel clearance and route planning use the current service. Published aperture predictions own an index of their eligible bodies for asynchronous use. New projectiles and changed trajectories query the tick-start scene without modifying its trees. `LuminosityForest` is not used.

## Sensor and visual observations

Each ship publishes its own immutable snapshot of current sensor detections in
[sensors.rs](../crates/osg-server/src/sim/sensors.rs). A powered sensor selects
the nearest 256 candidates within range, then applies geometric occlusion.
Blocked candidates are not replaced with farther objects. Position, velocity,
rotation, angular velocity and radius are exact. Loss of detection removes the
contact on the next publication.

A `ContactRef { observer, contact }` contains the observing ship's UUID and an
opaque nonzero `u64` handle. Handles are independent between ships and survive
only continuous detection within the same spatial lifetimes. Reacquisition,
relocation and restore produce fresh handles. Commands verify that the observer
is the commanded ship and that the handle is currently detected.

`IffIdentity` contains advertised owner, optional faction, labels and enabled
state. A detection includes IFF and the target UUID while its transponder is
enabled. Disabling IFF clears both immediately; geometry remains available while
detected. IFF has no independent range. Actual ownership, control and permission
checks remain authoritative even when advertised identity differs.

Frames contain `contacts` keyed by authorized observing ship. Focused views,
instrument subscriptions and screen subscriptions determine which ships are
included. There are no information groups, shared tracks, fusion, sensor noise,
coasting, tags or query cursors.

Optical visibility is evaluated independently from each focused ship using light
and line of sight. Visible objects carry current IFF even outside sensor range.
Optical IDs are session-local; `(view, id)` identifies client render objects.
An optical observation has a sensor contact reference only when that same ship
currently detects it. Optical-only labels do not permit targeting commands.
Combat records use optical IDs and current visibility, with the existing brief
destruction grace.

Public celestial navigation and infrastructure directories are separate world
services. Directory entries do not create sensor contacts or targeting access.
Firmware `sensor_scan` reads only its ship's bounded current snapshot; positions
and velocities are relative to the ship. `contact_label` and `contact_iff`
require a current handle. Sensors are required for contact guidance and weapons.
## Presentation

Section 8 of a state frame is a `PresentationFrame` ([presentation.rs](../crates/osg-model/src/presentation.rs), built in [sim/presentation.rs](../crates/osg-server/src/sim/presentation.rs)). It carries what the shared client needs for the original rendering, flight instruments and windows, without exposing true state beyond what the session may already see.

- **`ships`.** One `ShipPresentation` per controlled ship that a view focuses, a screen subscribes or an instrument subscription names. It holds the authority revision, flight environment (altitude, airspeed, density, pressure), health, execution timings, mass, inertia, control rotation, heat and battery capacities, power flow, inventory, per-device telemetry, computer status and screen definitions. `instruments` (attitude, navigation, weapons, trajectories and markers published by the firmware) is included only for instrument subscriptions. Targets in instruments are `ContactRef`s.
- **`combat`.** Shots, projectiles, impacts and destruction since the session's preceding publication. Publication requires optical visibility of its source or target. Destruction may use visibility from the preceding publication so removing a destroyed body does not erase its final effect. This grace is cleared when a view changes focus or spatial lifetime. The record names an opaque optical ID.
- **`navigation`.** The dynamic inhabited-directory hash and bounded live public infrastructure for relevant views and destinations. Each live record carries containing-system IDs, pose, and docking/navigation capabilities. Usable docking bays and guidance permissions are checked for the requesting ship.
- **`capabilities`** and **`diagnostics`** for debug sessions ([Debug accounts](#debug-accounts)).

### Optical replication

Section 11 contains `OpticalObservation` records built in [session/optical.rs](../crates/osg-server/src/sim/session/optical.rs). A view needs an authorized focused ship. Visibility is evaluated at that ship's position, independently of its sensor detections and the client's orbit-camera position. The focused ship is included for its own presentation. A dormant focused ship has no surrounding space observations.

For other active ships, the server queries brightness buckets, computes the observer-dependent luminosity, and requires received flux of at least `1e-12 W/m²`. It rejects a body only when an intervening optical blocker covers its entire conservative angular disc; a body partly visible around a planetary limb remains eligible. Candidates are ordered by received flux. Each view receives an equal share of the frame's 8192-observation and 4 MiB optical budgets, with its focused ship first. Oversized records are skipped. Multiple views cannot let one dense scene consume every other view's allowance.

Each record carries its view, anonymous optical ID, spatial-lifetime token, optional current IFF, UUID and sensor contact, exact visual pose, conservative radius, equivalent optical luminosity, optional appearance hash, and `ShipVisual` engine/turret/shield state. Appearance and live geometry are absent from sensor-only contacts. A visible ship can therefore appear without an IFF identity or sensor detection, while a distant sensor contact can remain in the Overview without acquiring a mesh.

#### Brightness model and approximations

The current CPU model in [spatial/lighting.rs](../crates/osg-server/src/sim/spatial/lighting.rs) combines:

- Reflected starlight. It uses inverse-square irradiance, a spherical target with geometric albedo 0.3, and a Lambert phase function for the observer. A fully eclipsed light source contributes no reflection. The indexed value is the full-phase upper bound under current illumination; the observer-specific phase is applied after candidate discovery. Geometric albedo describes full-phase brightness relative to a diffuse reference disc; it does not specify the total fraction of incident energy absorbed by the material. See [JPL’s albedo definition](https://ssd.jpl.nasa.gov/glossary/albedo.html).
- Shield/radiator thermal emission, using the current emitting area and a numerical integral of the blackbody spectrum over 380–780 nm.
- Engine emission from delivered thrust and exhaust speed. The optical fraction of `0.5 × thrust × exhaust_speed` is 0.001 for thermal/conventional engines and RCS, and 0.01 for micropulse engines. Disabled or unpowered engines contribute zero.

Buckets therefore follow current thermal state, engine output and planetary shadows. Stellar luminosity uses a fixed 220 lumens per optical watt conversion. These are gameplay photometric approximations: materials, directional exhaust radiation, specular hull reflections, indirect planet light and partial-eclipse attenuation are not modeled. Ship bounding spheres are excluded as optical blockers because they can enclose large empty spaces; celestial blockers use spheres. The index remains conservative for partial target occultation.

#### Distant ship glints

The client uses the projected diameter of each optical observation's bounding sphere to choose its representation. [glints.rs](../crates/osg-client/src/ui/scene/glints.rs) crossfades between detailed geometry and a sprite over approximately 3–8 physical pixels, with 0.5-pixel hysteresis around mesh creation/removal. The decision accounts for viewport height, field of view and camera depth. Engine plumes and shields fade with the mesh.

Glints share one additive billboard batch per view. Their brightness follows `L / (4πr²)` with a finite near-distance clamp, the view's exposure and a nominal magnitude-based display gain. A bounded attitude-dependent modulation gives gentle glints without random atmospheric twinkling. The shader draws a compact core and halo; it adds no lights, shadows or prepass. The client updates the batch geometry each display frame. The sprite's appearance and nominal magnitude scale are display approximations, while its visibility and baseline brightness remain supplied by the server.

## Debug accounts

A debug account is a configured account named by `debug_account`. It has no separate connection path. Its session frames list seven `DebugCapability` values: `Clock`, `Reset`, `Relocate`, `Recover`, `InjectHeat`, `Inspect` and `ConfigureSensor`. `DebugCommand::capability` maps each command to the capability it needs. Any other account's `Debug` action fails with "debug access denied".

| `DebugCommand` | Effect |
| --- | --- |
| `SetRate(rate)` | Sets a finite positive clock rate of at most 100 ticks per 100 ms loop. |
| `Reset` | Rebuilds the scenario on the next loop, with a new world ID |
| `Inspect(enabled)` | Switches server diagnostics on or off for all debug sessions |
| `Relocate { ship, pose }` | Writes the pose and velocities, clears force accumulators, renews the spatial instance and queues `StopGuidance` |
| `RelocateToBody { ship, body }` | Uses a containing-system/body reference, places the ship near a non-star body, cancels pending slip preparation, renews its spatial instance and queues `StopGuidance` |
| `Recover { ship }` | See below |
| `InjectHeat { ship, joules }` / `InjectShieldHeat` | Adds heat to the hull or shield of the ship's flight computer state |
| `ConfigureSensor { ship, range_m, occlusion }` | Overrides the ship's sensor range and celestial occlusion from then on |
| `InspectBody { body }` | Keeps a body's system active for inspection, or clears it |

Commands other than clock, reset and inspect go into a queue of at most 256 and are applied before the next ticks. They name ships by UUID and are not limited to ships the debug account controls. A command naming an unknown ship is ignored.

**Dormant ships.** Docked, in-transit, stored and destroyed ships are dormant. `Relocate` and `RelocateToBody` ignore dormant ships. `Recover` accepts only a ship in space or a destroyed ship; a docked, in-transit or stored ship is refused. A destroyed ship is recovered only if its stored-ship inventory is empty and it holds no stored mass. Recovery rebuilds the hardware with a fresh test loadout, returns the ship to space, cancels pending travel operations, sets travel to `Paused`, reboots the computer and clears its requests and world actions. Recovery keeps the ship's UUID.

**Diagnostics.** While inspection is on, debug frames include entity count, active and dormant ship counts, the last tick's duration, and per-stage times: ship preparation, WASM callbacks, sensor queries, publication, hardware, and collision indexing, queries, solving and total, plus collision counters.

## Display instances

Screens for network clients come from a separate WebAssembly instance of the ship's firmware running its `ship_display` export ([displays.rs](../crates/osg-server/src/sim/displays.rs), [ship-abi.md](ship-abi.md#display-entry-point)). Display transport is drawing-only and on demand: nothing runs until a session subscribes.

- **One instance per ship.** All sessions subscribed to a ship share one instance. The refresh rate of each slot is the highest `hz` among its subscribers.
- **Creation.** An instance is created for a ship when at least one subscriber controls it, the ship is not dormant and its computer is powered. It is instantiated with `instantiate_display` and configured with the ship's hardware. Boot work is paid from the actual owner's gas account using the ship's physical CPU allowance left after flight execution. Display creation does not grant free boot or extra gas.
- **Input.** On each update it receives a copy of the flight computer's latest input with commands and screen events removed. `requested_screens` holds the subscribed slots that are due: a slot is due when it has no frame yet, or when `tick × hz / 10` has crossed an integer since its last frame. Paid boot, an unfinished callback or pending input can also require a slice when no new slot is due.
- **Frames.** Frames for requested slots are stored as `ScreenUpdate { ship, slot, revision, tick, frame, error }`. `revision` is the instance revision, a server-wide counter that is new for every instance. If the callback fails, every requested slot gets the error text, truncated to 512 bytes, and no frame. Screens the firmware clears lose their stored frame.
- **Limits.** It has its own memory, query cursors and screen event queue. Flight and display slices share one physical per-ship tick ceiling and the same owner account. An unfinished display callback resumes with later grants. It cannot write devices or submit world actions. It does not share memory with the flight instance.
- **Screen definitions.** A ship presentation lists the display instance's screen definitions, or, if it has none, the flight computer's.
- **Expiry and revocation.** An instance is removed on the first update after its ship becomes dormant, its computer loses power, its control revision changes, or 10 ticks pass with no subscriber. Its frames are withheld immediately in those cases. A new instance gets a new revision, so input aimed at the old frames is rejected, and queued input is dropped with the old instance.
- **Screen input.** `ScreenInput` requires the ship to be active and powered, the instance's control revision to match, the slot's stored frame to exist with the given revision, a valid event kind (0 to 7) and at most 64 bytes of text. The event gets the next event ID for that instance and is queued as a `ScreenEvent` with its code, modifiers, coordinates and text. Pointer moves, presses and releases, key presses and releases, text, bezel and reset kinds are all forwarded.

The stock firmware's `ship_display` defines each requested slot as a 512 × 256 screen titled "Ship status" and draws five text lines: `SHIP STATUS`, simulation time, speed, mass and battery energy. It does not read screen events. Firmware without a `ship_display` export cannot be instantiated as a display, so its subscribed slots report "Display unavailable".

## Docking and travel

Docking and travel are implemented in [travel.rs](../crates/osg-server/src/sim/travel.rs).

### Presence

`Presence` is one of `Space`, `Docked { host, bay }`, `SlipTransit(id)`, `StoredInWreck(host)` or `Destroyed`. Private telemetry includes an appearance hash, radius and presentation pose in space, docking storage and slip transit. Docked poses follow the host bay; transit poses follow the declared slip segment. Only space presence participates in normal physics.

Leaving space makes a ship dormant. Its hardware is shut down (default device settings, avionics unpowered, sensor range 0), and its velocity, rigid body, collision body and spatial body are removed and remembered. Dormant ships take no part in physics, sensing or displays. Ordinary flight callbacks and world actions stop. Their hull and shield thermal state advances once per simulated second. Returning to space restores the remembered components.

### Bays and docking

A bay belongs to a host ship. It has a centre and rotation in host axes, a radius, a mass capacity, public or allow-list access, an optional reservation. Docked ships are stored through the ECS `DockedIn` / `StoredShips` relationship; a stored ship releases the bay for the next arrival.

- **`reserve_bay`** requires:
  - the ship is not the host, both are in space, and the ship's containment tree is less than 8 deep with no cycles
  - access: the bay is public, the visiting ship's current owning principal has access through ownership or a Dock grant, or the ship's current player owner is on the bay allow list
  - a bay that fits the ship's radius and mass
  - no unexpired reservation by another ship

  A reservation lasts 600 ticks.
- **`dock`** reserves a compatible bay, then requires the ship to be within 100 m surface clearance of the station (centre distance minus both bounding radii) and moving at no more than 10 m/s relative to the station. Capture works from any direction and does not require matching orientation or entering the hangar. On success the ship becomes dormant with presence `Docked`, the host's stored mass and mass increase by the ship's mass, the ship joins the host's stored inventory and releases the reservation, a current `Dock` leg completes its order, and `docked` is emitted.
- **`undock`** requires the ship not to be destroyed, the host to be in space, and departure access. The ship leaves along the bay's −Z axis, offset from the host centre by host radius + ship radius + 10 m, and inherits the host's velocity plus ω × r at that point. The exit point must be clear of other active ships and bodies. `undocked` is emitted.
- Destroying a host changes its docked ships to `StoredInWreck`. Their inventory stays inside the wreck.

A docked ship with an `Undock` or due `WaitUntil` order completes it without running its program. A queued space order automatically undocks the ship and then resumes planning the same order; docking at the current host completes immediately.

### Slipdrive

A slipdrive commits the ship to a trajectory at 0.3 ly/s with navigation guidance
or 0.03 ly/s without it. Short journeys are slowed to last at least three seconds.
Its first natural exclusion intersection ends transit. The initial drive draws
up to 500 MW. Preparation requires space presence, working equipment, available
exotic fuel, and clearance from all natural exclusion spheres.

Each physical celestial body contributes an exclusion radius of
`0.08 AU × cbrt(mass / solar_mass)`. Barycentres add no sphere. Artificial objects
create no slip exclusions. Ordinary propulsion clears enclosing regions and
moves around obstructions before the next slip.

Charging requires 5 kJ/kg per light-year and at least ten seconds. Retargeting updates the energy requirement. The ship may coast while
charging. Firmware updates its moving-target aim while preserving charge work
and the original start time. Departure requires every ring axis to align within one degree of the aim and freezes nominal aim, departure mass,
and galactic velocity. There is no additional cooldown or manual disengagement.

The controller's supplied aim is authoritative. The server applies physical
departure rules but does not correct that aim, require a predicted capture, or
reject it for exceeding the itinerary's planning risk allowance. A faulty
controller can commit to a trajectory that ends in fuel exhaustion.

Transit draws independent Gaussian transverse displacements during flight.
At forward progress `s`, a step `ds` adds per-axis variance
`sigma² * ds * (2*s + ds)`, where `sigma = 1.0743925808301219e-7 / B`.
`B` is 36 with authenticated navigation guidance and 1 blind. The total variance
is `(sigma * distance)²`, preserving the planner's Gaussian arrival spread.
Actual direction is normalized to retain the fixed cruise speed; the correction
to the distribution is negligible at these angles. A blind solar capture at
10 ly has 50% probability. There is no speed/dispersion tradeoff.
The HUD estimates conditional capture failure from the observed transverse
displacement and remaining variance at the predicted target plane. It integrates
the displaced Gaussian over the capture disk; actual captures still sweep moving
spheres, so this remains an estimate for moving or unexpected bodies.

An operating navigation beacon requires `Navigate` permission. Guidance is
checked throughout transit. Its first loss switches future variance increments
to blind dispersion and reduces speed by ten. Returning guidance does not
restore assistance during that transit. Realized motion and loss state survive
save/load; future noise is sampled only as the ship advances.

Slipspace is a 2,500 K thermal environment. Radiative exchange heats operating
shields, drives continuous ablation and reserve consumption, and heats exposed
hulls. Radiators exchange heat with the same environment. Transit thermal
integration uses the actual elapsed slip time, including partial arrival ticks.

Exotic consumption is cumulative: `1e-5 × departure_mass_kg × distance_ly^1.2`
kilograms. Inventory uses grams with fractional accounting across updates.
Starter ships receive approximately 1,000 ly of uninterrupted endurance, including
the fuel's own mass. Refuelling uses ordinary inventory transfers.

Transit sweeps through moving natural exclusions in active and dormant systems.
The earliest capture restores the retained galactic velocity and activates
required systems before ordinary physics. A surface collision destroys the
ship; a farther capture cannot rescue a ship that already exhausted its fuel.
Arrival integrates only the remaining portion of the tick. Controllers resume
at the next boundary. Unexpected captures replan from the actual emergence
state and retain the itinerary's spent risk allowance.

### Travel orders and server planning

Player requests contain a complete order queue. Orders include
`TravelTo(Destination)`, `Sublight(Destination)`,
`Slip { destination, navigation_beacon }`, `Dock(station)`,
`Undock`, `WaitUntil(tick)` and guidance.
Destinations may refer to public beacons, galactic positions, or offsets from
beacons and celestial bodies. Guidance contact targets must belong to the
ship's current sensor detections. Celestial references carry their containing
system and stable local body identity.

The server owns the queue, its revision, active index and route search. The
standard WASM flight program receives only `CurrentOrder` and executes that
single strategic command. It does not load the navigation graph, expand later
orders or publish replacement routes. The client receives the complete queue
for route and waypoint displays.

Fine manoeuvres belong to command execution. The ship program generates local
waypoints, avoids known obstacles, escapes slip-exclusion volumes and corrects
its course without adding these manoeuvres to the server queue. The public
`LocalSpace` observation query supplies nearby known volumes; it does not choose
waypoints or steering directions for the program.

Route preview uses
`RouteRequest { ship, authority_revision, request }` and
`RoutePoll { ship, authority_revision, id }`, with `RouteCancel` for abandoned
previews. A request carries a nonzero caller
ID, all requested orders and `PlanningPreferences`. Both actions return
`Reply::Route { id, status }`: unknown, pending with progress, ready with a plan,
or failed with a bounded explanation. The client polls and offers an explicit
Engage action for a ready preview. `ShipCommand::UseRoute` commits it against the
expected travel revision. Current authority is checked at every operation;
changed ownership or control cannot inherit another caller's job.

A ready plan contains its creation tick, travel and topology revisions, the
strategic route queue, per-stage time and propellant estimates, and the
whole-route fuel budget, estimated destruction risk, exotic requirement, and
navigation-beacon assumptions. Requests and plans contain at most 256 orders;
serialized plans are limited to 48 KiB. Job enqueue and polling are bounded
operations. Planning work runs separately from the ship callback and debits the
owner's global gas account. A full requested queue is expanded before it is
committed. The server checks current control and queue revisions when committing a plan.
Its age, ordinary ship movement and changes elsewhere in public infrastructure
do not invalidate it. The topology revision records the planning context; it is
not a commit expiry condition. The ship resolves current geometry while
executing each strategic command, and the host checks physical admission when
applying that command.

The player sets maximum whole-itinerary ship-destruction risk in decimal ppm,
defaulting to 100 ppm (1 in 10,000). The planner chooses capture stops and evaluates their fixed-speed edges, rejecting captures above the remaining risk allowance. It accumulates
risk logarithmically and retains the remaining allowance through automatic
replanning. Missed intended captures count conservatively as losses. Estimates
cover slip travel and state their guidance assumptions.

Search considers up to 32 slip legs through spatial candidate queries with a
shared computation budget. It compares feasible routes using estimated time,
conventional propulsion, charging, and exotic consumption. A bounded search
reports whether its budget was exhausted; it does not establish a global
optimum. Rated hardware estimates can change with gravity and available power.

Each stage retains an optional duration and propulsion fuel estimate. The flight
program updates only the active stage's remaining time and propellant; the host
combines it with later stages and current tank balances. Resource shortages are
checked separately. Unknown and continuous stages make the budget partial.
Slip preserves departure velocity, so estimates include matching destination
motion after arrival. Budgets are advisory and require operating margin.

| `ProgramQuery` | Reply |
| --- | --- |
| `Travel` | Current order, revision, index, status, preferences, active arrival estimate, exact own pose, slip readiness and the drive ring's axis in ship coordinates. |
| `RouteRequest(request)`, `RoutePoll { id }` | The same scoped asynchronous preview service available to clients. Each call admits 8192 work gas plus its normal call and copy costs. |
| `Contact(reference)` | Current sensor pose, radius and opaque firmware handle. |
| `Orrery { reference }` | Up to 1,024 local celestial obstacle/exclusion records, resolved through the shared universe. |
| `Resolve { destination, after_seconds }` | Predicted public destination pose. |
| `Beacon(id)`, `Beacons { after, limit }` | Public beacon facts and currently authorized docking bays. |
| `SlipEligibility { origin, destination, departure_after_seconds, arrival_after_seconds, navigation_beacon }` | Current drive readiness, departure clearance, guidance availability, preparation time and flight duration. |

Local-space observations spend 262144 work gas per query, plus normal call and
copy costs, with a hard 2048-unit index/ephemeris work budget. Query range is
finite and at most 10¹² metres. The observations contain no undetected private
objects, and a truncated reply cannot be treated as an empty or complete scene.

Prediction offsets are finite, nonnegative and bounded to one Julian year.
Slip arrival cannot precede departure. Orbital bodies use their
public ephemerides; other beacons extrapolate current motion. The host rechecks
physical admission during charging, departure and arrival.

World actions are committed after each successful execution slice. Every action
that operates on an active stage carries both its travel revision and order
index. The host checks these and the current command type, so a delayed
suspended callback cannot dock, undock, launch slip or block a later stage.
Stale actions are ignored; a real failure of the matching active command blocks
that command.

| `ProgramAction` | Server behaviour |
| --- | --- |
| `UseRoute { id, revision, engage }` | Commits a ready preview using the current travel revision. |
| `Block { revision, order, reason }` | Blocks the matching command and cancels unfinished slip preparation. |
| `Estimate { revision, order, remaining_ticks, remaining_propellant_kg }` | Updates only the matching active stage; future estimates remain server-owned. |
| `CompleteOrder { revision, order }` | Advances the authoritative cursor after the current stage completes. |
| `Slip { revision, order, destination, navigation_beacon }` | Updates the current slip command's charging aim without resetting work or start time. Departure commits its trajectory. |
| `ReserveBay`, `Dock`, `Undock` | Executes the corresponding current command with the same revision and index checks. |

The standard executor resolves the current destination each callback. Sublight
guidance uses relative position and velocity with the economical navigation
law. Moving slip destinations are led through preparation and transit time,
and their charging candidates are refreshed until departure. Bay requests use
only bays reported as usable. Execution errors block the command for an explicit
replan; the VM does not replace the queue itself. Server and firmware share `GAME_VERSION` and must be rebuilt together.

## Client playback

`osg_client::connect` ([connection.rs](../crates/osg-client/src/connection.rs)) opens the `main` stream and returns an `Endpoint` with state and input channels plus a cloneable `AssetClient`:

- incoming states: an unbounded FIFO, drained into the jitter buffer by the client update loop
- outgoing inputs: 16 frames
- asset requests: 8 pending requests; each `AssetClient::fetch(hash)` awaits its own response containing complete bytes or an error

Asset requests start independent transfers as they leave the request channel. A slow transfer does not hold up later requests; the channel bounds pending requests rather than active downloads.

`Playback` buffers bursty arrivals. Bevy runs the client at a fixed presentation cadence of 10 Hz.

**Receiving frames.**

- The protocol decoder reads complete messages. Playback trusts server sequence numbers and timestamps.
- A new `world` resets buffered snapshots and retained publications. At the next fixed update, one reset system removes entities marked `WorldMember`, resets session metadata and interpolation time, and triggers `SessionReset`. Observers clear UI resources, subscription bookkeeping, pending actions and celestial indexes. The UI then subscribes again.
- Snapshots retain their events and command results until playback consumes them. Every queued snapshot is consumed in order; the client does not discard old snapshots to reduce backlog or impose a hard receive-buffer limit. Generic events are retained in session metadata for UI consumers.

**Output.**

- `PreUpdate` drains received states into the buffer. Each `FixedUpdate` consumes one snapshot normally, or two while catching up. Publications from both consumed snapshots are processed in order, and the final snapshot supplies the resulting ECS state. Playback starts once the buffer holds the target depth: 3 frames for `osg-client`, 1 for `osg-debug`.
- On an empty buffer, `underruns` increments once and the target grows by one, up to 10. Consumption resumes when the reserve has rebuilt. The previous and current ECS samples remain unchanged during this wait, so each fixed interval repeats the last movement. A new world restores the initial target and clears the underrun count.
- Catch-up starts when queued depth exceeds the target by more than six frames. It consumes two frames per tick until depth returns to the target, then resumes one per tick. No queued snapshot is dropped.
- In `Update`, the interpolation fraction is `Time<Fixed>::overstep_fraction_f64()`. Pose and hardware components interpolate from the preceding displayed snapshot to the final snapshot consumed in the tick; turret yaw follows the shortest arc. The first sample initializes both endpoints to the arrival pose.
- `RenderTime` contains only `previous_ns`, `current_ns` and `display_ns`. The display timestamp interpolates the two snapshots' explicit `sim_time_ns` values using the same fraction. Celestial positions are evaluated analytically at that time. Accelerated simulation and skipped publications use their actual timestamp intervals. Repeated timestamps remain valid when no simulation tick elapsed between publications at a positive fractional rate. The buffer does not synthesize snapshots or infer timestamps from the advertised clock rate.

**Spatial lifetimes.** Tracks and private ship telemetry carry a `spatial_instance` token. Relocation, departure and arrival replace this token while preserving the ship's UUID. A changed track token replaces its client observation entity and associated render instances; new samples start at the arrival pose. Controlled ships retain their telemetry entity, removing pose components while absent and initializing fresh samples on arrival. The token changes even when departure and arrival occur between two publications. Anonymous track tokens are scoped to the opaque track ID. Interpolation and tracer history use these lifetimes without travel-specific discontinuity events.

Publications are delivered when playback consumes their frame, including the intermediate frame of a catch-up tick. Combat visibility still follows each event's simulation timestamp. The session keeps the latest 128 delivered command results and generic events for the UI.

**Input.** `FixedPostUpdate` sends an `InputFrame` every 100 ms once the UI knows the world, even when there are no actions. Inputs carry no server snapshot reference or acknowledgement; received actions apply before the next available simulation tick. The command queue supplies command IDs and ship authority revisions, and preserves their order without coalescing. If the input channel is full, queued actions retain their IDs for the next attempt and the status reads "Input queue busy".

## Client ECS presentation

The client separates transport ingestion, replication and interpolation into the [state modules](../crates/osg-client/src/state). `SessionInfo` owns the world, generation, applied tick and sequence, capabilities, diagnostics and delivered command results. Network reception, buffered playback, outgoing commands and session metadata have separate resources. Only snapshot ingestion reads the buffered frames. It reconciles stable entities for contacts, controlled ships, optical observations, views and combat publications. Contact identity includes the observing ship, so each ship's detections remain distinct. ID-to-entity maps are derived lookup indexes.

Ship, contact and optical entities hold pose samples and interpolated display components. Optical entities also interpolate luminosity. Radio contacts supply HUD/Overview entries; live ship geometry is sourced from the optical entities for that view, with private rendering for the focused ship in a hangar. Each view owns its camera, origin, exposure and sky state. `CameraOptions` holds the camera focus separately from the orbit data used to construct trajectories. Render instances relate to both their source observation and their view, allowing either lifetime to remove the associated visuals. Bevy asset handles own immutable downloaded assets; sky baking keeps its separate work budget. HUD overlays query presentation components, and scene alignment gestures enqueue navigation actions through the outgoing resource.

Reception runs in `PreUpdate`, snapshot application in `FixedUpdate`, and input publication in `FixedPostUpdate`. `Update` then runs interpolation, celestial evaluation, view updates and rendering updates. World changes remove the old observations and reset selection. Missing observations remove their entities; a changed spatial lifetime creates fresh presentation samples. Docked ships retain private telemetry while their space pose is absent. MFD publications remain available in the wire protocol; the current UI does not subscribe to screens or replicate them into display entities.

CPU skybox bakes start at most once per ten seconds of real time across all views.
The first bake can start immediately. Only one bake runs at a time; later work
uses the latest camera position rather than queuing intermediate slip positions.
Completed work and texture sharing continue during the cooldown.

The compact full-sky catalogue is bundled locally. View position, inspection, and
queued celestial references select detailed definitions for local background
generation. Receiving the inhabited directory updates map membership without
generating those systems. The server's active set has no client counterpart.

Generated definitions contain stable identities, parent relationships, orbital
elements, rotation, mass, radius, luminosity, and atmosphere parameters. Views
share resolved definitions. The local solver evaluates them using the agreed
MJD epoch and simulation clock. Virtual barycentres organize orbits without
adding physical bodies or duplicate gravitational mass. Releasing local
consumers permits cache eviction; later resolution reproduces the same result.

The [asset source](../crates/osg-client/src/assets.rs) registers canonical
`server://<hash>` paths for ship appearances and inhabited-directory downloads.
It returns bytes through the existing Tokio transport and typed Bevy
loaders. Celestial generation uses the shared resolver directly.

A public [ship appearance](../crates/osg-ships/src/appearance.rs) contains only the catalogue revision and each visible part's catalogue prototype, animation ID, position and rotation relative to the ship's physical origin. The server exports these final transforms from its compiled design, preserving the center of mass used by rendering without publishing tank allocations, resource types, starting fills, names, device groups, avionics settings or firmware. The client resolves public catalogue models and derives visual bounds directly from those transforms; it does not compile a gameplay blueprint to render a ship. Authorized ship telemetry and construction blueprints use their separate permission checks.

Directory downloads have explicit loading and failure states. The loader
installs a directory only when its hash and session generation still match,
preventing late completion from restoring stale membership. Live infrastructure
replication continues independently. A world reset clears the directory handle
and prior session state.

The default ship starts in sunlight. Its initial camera faces the illuminated hull, and direct stellar illumination uses luminosity divided by spherical area at the view’s distance. Ambient fill is disabled, and the default camera exposure is EV100 15 with per-view adjustment. The original star-disc, Gaia cubemap, atmosphere and exposure algorithms remain the rendering reference.

## The client UI

The shared egui theme, embedded fonts, Phosphor icons and desktop toolkit come from [osg-ui](../crates/osg-ui/README.md). Both client launchers use the same [Bevy/egui app](../crates/osg-client/src/ui.rs). A toolbar runs down the left edge, location and autopilot information sit at the upper left, and Selected Item controls sit above the Overview on the right. The bottom HUD displays the controlled ship's reserves and operating state.

The toolbar opens Overview, Selected Item, Inventory, Industry, Local Chat, Navigation, the map and Society & Ownership. It also toggles orbital paths, returns the camera to the controlled ship, and opens Interface settings. Windows can be dragged, resized, closed, snapped and locked; Interface can reset their layout. They do not collapse, and can overlap the bottom HUD. Layouts last for the application session. The footer shows the game calendar (real UTC plus 400 years), connection status, jitter-buffer depth and display FPS; its timestamp tooltip includes simulation T+ time.

The location indicator names the subscribed system and nearby gravitational reference, shows altitude, and displays the remaining autopilot orders with stage ETAs. Docked ships get a hangar view, a selector for controlled ships at that station, and an Undock button. Changing ships updates the focused view and instrument subscriptions.

Overview combines sensor contacts, public beacons and orrery bodies. Planets come from celestial definitions, never sensor detections. Rows support sorting, text search and All/Ships/Celestials filters, and show distance and relative speed at the displayed simulation time. Contact and HUD colors use friendly, neutral, hostile or unknown standings derived from advertised IFF and the observer's relationship hierarchy. Personal standing overrides neither grant permissions nor confer command authority.

Click a row or HUD marker to select it. Selected Item offers Align, Approach, Keep range and Stop guidance, plus beacon Jump/Dock actions where applicable. Mark/Unmark and Fire/Hold fire are separate weapon controls; selecting an object or issuing navigation does not start firing. Look at, including an Overview double-click, changes camera focus only when the object is eligible; remote ships require optical visibility and must be within 100 km. Right-drag orbits the camera with smoothing, scrolling zooms, and Escape returns to the controlled ship. With autopilot off, double-clicking unobstructed space aligns the ship along the camera ray while preserving throttle. This is disabled while docked or in slip transit. O toggles trajectories and +/− adjusts exposure.

The searchable map combines the local full-sky catalogue, public inhabited
membership, and route highlights. **Plan destination** requests a preview;
**Add waypoint** includes the remaining goals. The preview shows estimated risk,
the selected maximum risk in ppm, guidance assumptions, time, and fuel.
Changing preferences cancels the old preview. **Engage route** commits a plan
matching the visible preferences and current ship authority. The flight computer
generates local manoeuvres during execution. Navigation shows guidance telemetry
and queue controls; the scene HUD marks queued waypoints.

Inventory separates movable cargo stacks from installed consumable tanks. Cargo shows counts and reservations, with quantity selection and drag/drop between authorized, colocated inventories. Consumables show only installed storage, provide refill controls using colocated cargo, and include IFF and dock-service controls. Industry lists authorized facilities for remote inspection and operation, including capabilities, power, recipes, jobs and ship construction. Society & Ownership exposes affiliations, personal standings, asset permissions, authorized gas-account balances and searchable organization histories, objectives and relationships. Local Chat follows the focused controlled ship's 500 AU channel, with timestamps, advertised affiliation colors and a virtualized scrollback.

The bottom HUD shows pooled propulsion reserves, battery, reactor or charge fuel, shield reserve, delivered thrust and torque, power, hull integrity, stored heat and shield temperature/coverage. Its CPU meter reports the last tick's gas consumption against the physical computer budget, with distinct suspended, insufficient-gas, boot and fault states. Clicking the thrust meter sets throttle when manual control is available; holding Shift raises it and Ctrl lowers it. The display continues to show delivered forces under autopilot.

[shell.rs](../crates/osg-client/src/ui/shell.rs) builds a view model from ECS presentation components and turns UI interactions into outgoing intents. Commands carry the current authority revision; the UI does not mutate authoritative ship state. Feedback distinguishes pending, accepted and rejected requests. Acceptance does not mean a maneuver has completed. A world reset clears selection, subscriptions, previews and feedback before subscribing again.

Initial focus uses the per-session `ShipTelemetry.can_control` permission flag and excludes destroyed hulls and ships stored in wrecks. The smallest UUID is only the tie-breaker among eligible living ships. Read-only access to Neris Anchorage does not make it the player's controlled ship. Explicit later inspection can keep a station focused. Under the 64-ship telemetry cap, explicitly subscribed objects retain priority, followed by living controllable ships and then other observable assets.

## Tests

| Location | Covers |
| --- | --- |
| `osg-protocol` unit tests | Whole-message round trips, framing, and validation of untrusted client commands |
| `osg-net` unit tests | Compressed stream history across flushes, clean shutdown, rejection of a wrong server pin and a wrong account key, replayed records, epoch rekeying |
| `osg-spatial-bvh` unit tests | Brute-force agreement for spatial and brightness queries, large signed coordinates, boundary cases, retained snapshots, collision pairs and cursor budgets |
| [session/optical.rs](../crates/osg-server/src/sim/session/optical.rs) tests | Focused-vantage visibility, anonymous objects, sensor-independent visibility, occlusion, per-view budgets and optical IDs |
| [session.rs](../crates/osg-server/src/sim/session.rs) tests | Views require access to the focused ship, a control change rejects old-revision commands without rewriting IFF, a non-debug account cannot change the clock, repeated command IDs are idempotent and replayed frames are rejected |
| [displays.rs](../crates/osg-server/src/sim/displays.rs) tests | Subscribers share one instance released 10 ticks after the last viewer, authority and power changes revoke frames and queued input, every ABI input kind is forwarded |
| [services.rs](../crates/osg-server/src/sim/services.rs) tests | Scans exclude celestials and use stable opaque ship handles, handles differ between observers and after reacquisition, slip aperture checks, beacon pages hide inaccessible bays, celestial references resolve without active bodies |
| [travel.rs](../crates/osg-server/src/sim/travel.rs) tests | Docking and undocking motion, nested inventory surviving host destruction, capture not unlocking a private bay, blocked slip arrival and retry, slip energy and cancellation, inactive bodies blocking slip arrival, debug recovery rules, simultaneous arrivals |
| [router_tests.rs](../crates/osg-server/src/sim/travel/router_tests.rs) | `travel_order_runs_in_stock_wasm_and_brakes_at_destination` and other travel orders flown by the stock firmware |
| [routing](../crates/osg-server/src/sim/routing) tests | Public server route search and full queue expansion |
| [tests/network.rs](../crates/osg-server/tests/network.rs) | Starts the real `osg-server` binary with a ready file. A wrong server key and a wrong account key fail to connect. One account receives telemetry, a view with sensor contacts, a stock MFD frame with at least five primitives and an appearance asset. A second account cannot inspect or command the first account's ship. Reset keeps the connection usable, discards delayed inputs for the previous world and accepts a new view subscription. The server shuts down cleanly when standard input closes. |
| `osg-client` tests | Buffer ordering, reserve refill, positive rate changes and repeated timestamps, ordered publications during catch-up, command ordering and backpressure, concurrent asset transfers, shared Bevy asset handles, explicit retry and unloading; UI tests cover fixed scheduling, spatial lifetimes, celestial loading, orbital projection, effects and selection |

```sh
cargo test -p osg-protocol -p osg-net -p osg-server -p osg-client -p osg-example-controller
```

Enable `osg-client/ui` to run the client ECS, asset-pipeline and rendering-math tests. These tests do not launch a graphics window; visual verification uses `osg-debug`.

## Spatial CPU benchmarks

```sh
cargo bench -p osg-spatial-bvh --bench gaia
```

The Gaia benchmark checks visibility against exhaustive scans and measures construction and queries over the million-star catalogue. The server benchmark exercises complete ticks and multiple sessions; both measurements are needed to assess a spatial change.

These are CPU index measurements. They exclude simulation, illumination updates, per-session serialization, network traffic and client rendering. Full observer batches and collision workloads must be measured separately; build contention and the number of returned objects materially affect timings.

## Benchmark

[examples/benchmark.rs](../crates/osg-server/examples/benchmark.rs) measures a real server process with real TCP clients. Build the server first; by default the example runs the `osg-server` executable from the same target profile directory.

```sh
cargo build -p osg-server
cargo run -p osg-server --example benchmark
cargo run -p osg-server --example benchmark -- --ships 128 --sessions 8 --frames 50

cargo build --release -p osg-server
cargo run --release -p osg-server --example benchmark -- --ships 512 --sessions 16
```

| Option | Default | Range |
| --- | --- | --- |
| `--ships` | 16 | 1 to 1024 |
| `--sessions` | 4 | 1 to `ships` |
| `--frames` | 20 | at least 2 |
| `--warmup-ticks` | 70 | any |
| `--server` | `osg-server` next to the target profile directory | an existing file |

The benchmark:

1. Writes a server configuration in a private temporary directory with one account per ship, `listen = "127.0.0.1:0"` and the first account as `debug_account`. The scenario therefore spawns one player ship per account, plus initial traffic and infrastructure ([Scenario](#scenario)).
2. Starts the server with `--ready-file` and `--shutdown-on-stdin-close`, and waits up to 120 s for readiness.
3. Connects `sessions` clients, one per account, through `osg_client::connect`. Each subscribes one view focused on its ship. The first client also enables debug inspection.
4. Acknowledges every frame until `warmup-ticks` ticks after its first frame, then waits for all clients.
5. Samples the server's CPU time from `/proc/<pid>/stat`, then each client receives `frames` frames. For each frame it re-encodes the received frame as a `State` message and compresses it through its own Zstd encoder (level 3, window log 21) that keeps history across frames, and it records skipped ticks and the server's reported tick duration.
6. Prints one CSV row after reading `/proc/<pid>/status`.

| Column | Meaning |
| --- | --- |
| `ships`, `sessions`, `frames_per_session` | Options |
| `contacts_per_frame` | Mean contacts per received frame |
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
- `recompressed_zstd_bytes` and `recompress_ms` are isolated measurements of the benchmark's encoder. The real transport splits writes into 32,768-byte chunks, compresses directly in its async transport tasks, and adds encryption, picomux and TCP framing, so these are not the wire size or the server's compression cost.
- All clients and the server share one machine and loopback networking. There is no network latency, packet loss or bandwidth limit.
- The ships are the scenario's ships in orbit near one planet. They fly no travel orders and fight no battles.
- It reads `/proc` and `getconf CLK_TCK`, so it runs only on Linux.

A local run on 2026-09-16 used an AMD Ryzen 9 5900XT, the optimized development profile, 16 player ships, four sessions, 70 warmup ticks and 30 measured frames per session:

| Measurement | Result |
| --- | --- |
| Mean simulation tick | 1.989 ms |
| Server CPU | 19.0% of one logical core |
| Server RSS / peak RSS | 89.1 MiB / 89.1 MiB |
| Mean contacts per frame | 4.8 |
| Mean uncompressed frame | 4187.5 bytes |
| Mean recompressed frame | 455.4 bytes |
| Re-encode / recompress time | 0.028 ms / 0.044 ms |
| Encoder context per session | 3.49 MiB |
| Delivery | 10.00 Hz, zero skipped ticks |

This three-second sample checks the small idle scenario. Dense views, sustained combat, many display subscriptions and larger fleets need separate measurements before capacity estimates.

## Firmware scan budgets

Native sensor scans cost 3000 gas per requested contact. The standard firmware starts with 32 contacts, grows when its buffer fills and shrinks when results are sparse. It budgets scans and forecasts against remaining gas, preserving flight control, publication and the next callback. Dense-sensor regression tests exercise repeated callbacks under the existing runtime limits.


## Station and navigation expansion

The client replicates moving public infrastructure into ECS pose samples so its
markers interpolate on the same clock as ships. The map combines the local
astronomical catalogue with current public inhabitation. Selected destinations
become `TravelTo` requests, and the planner exposes maximum destruction risk in
ppm, route estimates, and guidance assumptions. Queue controls remove, reorder,
pause, and resume commands. Offscreen destinations use edge markers; distances
of at least 0.1 light-years use ly.

Owned ships retain their meshes independently of sensor contacts.
Docking opens a client-rendered hangar with orbit-camera controls; slip transit
adds long neutral-colored light streaks to the ordinary space view. Up to 192
filaments are generated and projected once per view per frame, then rendered as
thin strips with analytic glow and scene-depth occlusion. Their distant endpoints
project to infinity along the slip direction. The background remains clear of
diffuse tunnel glow. Entry eases in streak brightness and motion; there is no enclosing tunnel mesh. The camera
follows the ship's actual galactic position throughout transit; no frozen departure
view or private transit scene is used. The fixed 16 m slip ring brightens and pulses during preparation. Transit
telemetry supplies the departure time, direction and destination. During transit,
a green HUD circle marks the destination with remaining distance and ETA, including
an edge marker when it is offscreen. Filaments have independent random birth
positions and lifetimes, with thin tails extending toward infinity.
Failure odds appear as “1 in X” using the planner's exact risk colors; they reflect
the active transit's risk estimate, including increases after beacon loss.
The last HUD line reads `[BEACON LOCKED]` in green or `[NO BEACON]` in red.
Entry and exit envelop the hull over
1.75 seconds without delaying physics; external transitions are directional,
coloured ruptures with a two-second decay and depth-aware background distortion.
The departing ship sees a local rupture flash. Its nearby wake is rendered in a
stable frame at the interpolated ship position, avoiding jumps from clipped
interstellar segments and new per-tick noise seeds. Other observers continue to
see the recorded physical trail.

Slipping ships deposit anonymous wakes along their actual swept trajectory,
including passages completed within one tick. Each portion retains ordinary
inertial drift and fades over 300 simulation seconds. Shared history survives
source destruction and world checkpoints. Later observers can see remaining
light without a sensor contact, IFF or destination disclosure.

Presentation carries a per-view slip snapshot independently of combat-event
retention. The server clips spans to 10 million km around each observer, applies
brightness and optical occlusion, and publishes at most 128 visible spans and
16 transitions per view. The renderer selects four nearby rupture volumes and
projects wakes into screen-space ribbons using double-precision camera-relative
clipping. Up to 96 ribbons retain perspective width, scene-depth occlusion and
age-based fading; Gaussian profiles are integrated over pixel footprints so
subpixel tails remain continuous. Transit streaks use the same pixel filtering.
Wake rendering has no cylinder geometry or ray marching. Five-minute history uses
coalesced straight spans rather than per-tick particles. The passage fades smoothly over the existing sky.

See [stations-navigation.md](stations-navigation.md) for the controls.

## Diagnosing stalls

The client bottom bar shows smoothed display FPS, queued jitter-buffer ticks, and buffering or catch-up status. For detailed console output, start the client and server with:

```sh
RUST_LOG=info,osg_client::diagnostics=debug,osg_server::timing=debug
```

Client warnings identify display frames lasting at least 250 ms, snapshot reception gaps of at least 500 ms, and jitter-buffer underruns. Five-second debug summaries include queue depths, buffered simulation time, maximum frame duration, and received tick and sequence numbers. Reception and display logging come from separate tasks, helping distinguish missing server updates from a stalled renderer.

Server warnings identify simulation ticks or display/snapshot publication taking at least 100 ms. Tick records include collision index, query and solve times; candidate, query, impact and contact-review counts; and firmware work totals with the slowest ship's preparation, callback and gas usage. Firmware totals sum work across parallel workers and can exceed elapsed tick time. Debug logging emits these records for ordinary ticks too. The server's optional `profile` feature enables Bevy Chrome tracing for deeper investigation.

## Bottom ship console

A dedicated egui ECS system draws the bottom console above the timing strip. Floating windows may overlap this console; it does not reserve desktop workspace. Consumable reserves appear on the left, with propulsion and thermal status on the right. Cargo remains in the inventory window. Installed equipment identifies propellants, reactor fuel and pulse charges.

The thrust fill uses server-reported actuator force projected onto the ship control frame, divided by installed forward thrust capacity. Torque meters use installed capacity in each direction. Gravity and collision forces are excluded. The throttle marker comes from the flight computer; a separate pending marker shows requests awaiting confirmation. Click or drag the gauge, or hold Shift/Control to increase/decrease throttle by 25 percentage points per second. Text entry suppresses these shortcuts. Autopilot locks manual controls; navigation actions enable it, while queue edits preserve its state.

Computer telemetry publishes per-tick computer gas usage and its configured positive tick limit. CPU percentage is the gas spent on guest execution and host services divided by that limit; usage cannot exceed the limit. A running computer reports `Ready`, `Suspended` or `WaitingForGas`. The console labels a suspended continuation `SUSPENDED` and an insufficient owner-account balance `NO GAS`; a positive balance can still be too small for the next indivisible operation. Booting, unpowered, paused and faulted computers remain distinct states. Gas suspension preserves the running callback and does not initiate a reboot.

`SocietySnapshot.gas_accounts` carries integer available, reserved and spent amounts, each keyed by its owning `Principal`. Only the caller's player account and organizations or sovereignties the caller administers are included. Ordinary membership does not disclose a group's balance. The Society window displays those authorized accounts; individual computer telemetry does not duplicate the shared balance. Billing follows actual asset ownership, independently of IFF or delegated control.

A computer reset clears pending requests, instruments, marks, firing state and forecasts. The server also clears the autopilot toggle, orders, ETA, staged world actions, slip preparation and docking reservations, and advances the travel revision. Successful reboot starts with idle navigation. Commands explicitly submitted after the reset may be queued during startup. Fault messages remain visible until boot succeeds. The countdown pauses without computer power and follows simulation time on the client.

The society snapshot describes ownership and permissions. Sovereignties,
organizations, player affiliations, private personal standings, and authorized
asset permissions use stable UUIDs. The Society window exposes the hierarchy,
organization membership and officer management, standing overrides, asset grants,
and transfers between principals the player administers. Ownership and IFF are
separate. Delegated flight control does not grant configuration or access
management rights.

Private docking uses the ship's current owning principal and Dock permission.
Automatic station resupply requires ownership or a TransferCargo grant. A previous
controller assignment does not preserve either right after a transfer.

The signed millisecond calendar timestamp represents real UTC
plus 146097 days, exactly 400 Gregorian years. The calendar advances independently
of simulation speed. The client samples it at network reception, outside
the presentation jitter buffer. The bottom strip displays UTC date and time; its
hover text retains simulation T+. See [Persistence](persistence.md) for saved
worlds, debug identities, checkpoint configuration and recovery behavior.


### Local chat

Local chat reaches physical ships within an inclusive 500 AU sphere around the
sender. The server resolves both positions; a docked ship uses its host's
position. Destroyed ships have no transmitter. Delivery is instantaneous within the simulation publication cycle.
It uses the shared spatial service's geometric range query, independently of light,
occlusion, transponder range, or sensor power.

`ChatSubscribe { revision, view }` binds one session subscription to a focused
view. The account must have `Control` permission for that view's ship. Every send
and publication checks this authority again. `ChatSend` carries the subscription
revision and text; it cannot supply an origin or sender identity. Changing focus
starts a new cursor at the present, and `ChatUnsubscribe` stops publication.
Reopening a window does not replay messages received while it was closed.

A `ChatMessage` has an opaque message ID, a recipient-local sequence, simulation
tick, real UTC plus 400 years, text, and the sender's advertised identity. Enabled
IFF supplies its advertised owner, organization, and bounded label. Disabled IFF
produces “Unidentified transmission” with no owner or organization. The message
contains no physical ship UUID, position, or authoritative ownership record.

Player input and ship-computer calls use the same service. Text must contain
non-whitespace content, fit in 1024 UTF-8 bytes, and contain no control characters.
Each ship can send three messages per rolling thirty simulation ticks across all
callers. Computer requests deduplicate the latest 128 scoped request IDs; retrying
an ID with different text fails. An accepted send enters a FIFO of at most 256
messages. A full queue fails without consuming sender quota or a request ID.
Native admission covers bounded enqueue work, while ECS publication routes the
messages before updating the indexed position snapshot.

Each ship retains at most 128 received messages. Mailboxes never consult a global
regional history, so ships arriving later cannot read earlier broadcasts. A page
contains at most 32 messages and an exact count of received messages lost to
bounded history before its cursor. Session updates include subscription and view
revisions. Clients preserve all updates consumed during interpolation catch-up,
and reject updates for an obsolete channel. Chat histories are transient and
start empty after server restart.
