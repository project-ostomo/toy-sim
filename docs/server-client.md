# Authoritative server and network client

The simulation runs only in `toy-sim-server`. Every UI is a network client: the standalone `toy-sim-client`, and `toy-sim-debug`, which starts its own server process and connects to it over loopback TCP. This guide describes the server process, its configuration, the transport and application protocol, the intelligence model that decides what each account can see, presentation data, display instances, docking and travel, client playback, the client UI, and the benchmark.

The server restores durable world state from SQLite checkpoints, including ownership, information-group keys, ships and installed WASM programs. Sessions reconnect after a restart. A debug reset creates a new world ID; inputs carrying another world ID are discarded so existing connections can survive a reset. Checkpoints default to every 900 seconds, with initial and graceful-shutdown saves. Unsupported or corrupt newest checkpoints stop startup explicitly.

## Crates

| Package | Path | Role |
| --- | --- | --- |
| `toy-sim-model` | [crates/toy-sim-model](../crates/toy-sim-model) | Shared serde types: IDs, poses, tags, tracks, queries, frames, actions, debug commands, presentation records, travel orders and drawing lists; explicit conversions to the WASM C ABI records |
| `toy-sim-protocol` | [crates/toy-sim-protocol](../crates/toy-sim-protocol) | Application message framing, sections and validation limits |
| `toy-sim-net` | [crates/toy-sim-net](../crates/toy-sim-net) | TCP handshake, record encryption, Zstd compression and picomux multiplexing |
| `toy-sim-spatial` | [crates/toy-sim-spatial](../crates/toy-sim-spatial) | Shared spatial hash for brightness, radius, nearest-neighbour, segment and metered cursor queries |
| `toy-sim-intel` | [crates/toy-sim-intel](../crates/toy-sim-intel) | Measurements, immutable track snapshots and metered queries |
| `toy-sim-universe` | [crates/toy-sim-universe](../crates/toy-sim-universe) | Inhabited map, deterministic celestial generation, Keplerian solver, system catalogue and atmosphere parameters |
| `toy-sim-server` | [crates/toy-sim-server](../crates/toy-sim-server) | The Bevy ECS simulation in private modules under [src/sim](../crates/toy-sim-server/src/sim), the simulation loop, TCP listener, asset streams, configuration, key provisioning and the benchmark example |
| `toy-sim-client` | [crates/toy-sim-client](../crates/toy-sim-client) | `connect`, asset fetching, the `Playback` buffer, and the Bevy/egui UI behind the `ui` feature |
| `toy-sim-debug` | [apps/toy-sim-debug](../apps/toy-sim-debug) | Local launcher: server child process plus the client UI |
| picomux (vendored) | [vendor/picomux](../vendor/picomux) | picomux 0.2.1 with project-local changes, patched in through `[patch.crates-io]` in the workspace manifest |

The server modules are private to `toy-sim-server`. Its public library interface includes `launch::run`, `provision::demo`, `run`, `listen`, `scenario`, `assets`, the shared `AppearanceAssets` store and `key_bytes`.

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

Any other argument is an error. The server also stops on Ctrl-C, SIGTERM on Unix, and when the simulation thread ends. Graceful shutdown writes a final checkpoint. SIGUSR1 requests a checkpoint on Unix.

### Configuration files

Configuration files reject unknown fields.

| File | Fields |
| --- | --- |
| Server | `listen` (a socket address; port 0 picks a free port); `server_secret` (64 hex characters, the Ed25519 secret key); `[[accounts]]` entries with `id` (UUID) and `public_key` (64 hex); optional `debug_account` (UUID); optional `ship` (path to a `.ship` blueprint, relative paths resolved against the configuration file's directory); optional `[persistence]` with `enabled` (default true), `path` (default `world.sqlite`, relative to this directory) and `interval_seconds` (default 900) |
| Client | `address`; `server_public_key` (64 hex); `account` (UUID); `account_secret` (64 hex) |

`debug_account` must be one of the configured accounts, or startup fails with "debug account must be authorized". That account gets the debug capabilities described under [Debug accounts](#debug-accounts).

### Debug launcher

`toy-sim-debug [--ship PATH] [--server EXECUTABLE] [--state-dir PATH] [--ephemeral] [--check]` ([main.rs](../apps/toy-sim-debug/src/main.rs)):

- Without `--server`, the server executable is `toy-sim-server` in the same directory as the launcher's executable. If it is not a file, the launcher fails with "build toy-sim-server first, or pass --server EXECUTABLE".
- It keeps credentials and checkpoints under `$XDG_STATE_HOME/toy-sim/debug`, or `~/.local/state/toy-sim/debug`. `--state-dir` chooses another world; `--ephemeral` creates a disposable directory. Credentials are reused on later launches, and a directory lock prevents concurrent launchers from sharing the same state. The generated configuration listens on `127.0.0.1:0` and designates the local account as `debug_account`. A saved world restores its stored blueprint even if the original `--ship` file has disappeared.
- It starts the server with `--ready-file` and `--shutdown-on-stdin-close` and a piped standard input. It waits up to 60 s for a parseable address in the ready file, and fails if the server exits first.
- It connects with `toy_sim_client::connect`. The debug session uses the same TCP, handshake, encryption, compression, multiplexing and session code as a remote client.
- With `--check`, it waits up to 10 s for a state frame, fails with "server did not provision the debug ship" if the frame has no ship telemetry, and prints the address, tick and ship count. Otherwise it runs the client UI with a playback target depth of 1.
- On exit, it drops the server's standard input and waits up to 120 s for shutdown and its final checkpoint. A server still running after that deadline is killed. Only disposable state directories are removed.

The ship editor's "Launch sim" runs `toy-sim-debug --ship <snapshot>` ([ship-editor.md](ship-editor.md#launching-the-debug-client)).

### Scenario

[bootstrap.rs](../crates/toy-sim-server/src/sim/bootstrap.rs) builds the world from the Bevy application in [sim/mod.rs](../crates/toy-sim-server/src/sim/mod.rs):

- **Universe.** The bundled inhabited map contains 3,000 wormhole-connected systems within 125 light-years of Sol. Ten authored systems retain their names and celestial designs; 2,990 catalogue anchors receive deterministic stellar and planetary systems. System and celestial UUIDs derive from stable names; gate-mouth IDs derive from the two endpoint catalogue identities. The starting encounter is in Helion. See [Inhabited space](inhabited-map.md) and [Celestial generation](../crates/toy-sim-universe/GENERATION.md).
- **Explorer.** "Patrol ship" starts in a circular orbit 40,000 km above Helion I Neris, on the day side, with the tangential direction selected from seed 42 ([scenario.rs](../crates/toy-sim-server/src/sim/scenario.rs)). Its design is the configured `ship` blueprint, or the bundled micropulse patrol ship. If the blueprint fails to load or compile, the server logs "Cannot load ship" and spawns no ships at all, so scenario setup fails.
- **Hostile patrol.** "Hostile patrol 001" uses the bundled micropulse patrol design, starts 1 km from the player with the same initial velocity, and points at it. Its computer receives mark and start-firing requests after initial sensor publication and fires once booted.
- **Players.** The first configured account controls the explorer. Account *n* (from 1) gets "Explorer *n+1*", with the explorer's design and velocity, offset by *n* × 1,000 m along galactic +Y. Every player ship gets a default slipdrive.
- **Ownership and infrastructure.** The hostile patrol belongs to Terminus Privateers. Neris Anchorage and the generated gate network are present. Gate operating organizations belong to their systems’ sovereignties, and each bilateral map connection has two mouths. Political affiliation does not restrict transit: blocking passage requires physical action.
- Every ship gets a test loadout, and every flight computer boots for 5 s.

After spawning, bootstrap runs identity, acquisition, coasting, fusion and publication once, so the first state frame already has tracks.

## Server loop

`toy_sim_server::launch::run` ([launch.rs](../crates/toy-sim-server/src/launch.rs)) reads the configuration, binds the listener, and builds the scenario on a thread named `simulation`. It locks and reads the configured checkpoint database before choosing a bootstrap design, restores any saved world, and commits the initial checkpoint for a new world before publishing readiness. Once the world and its asset map exist, it writes the ready file and runs the listener. `toy_sim_server::run` ([lib.rs](../crates/toy-sim-server/src/lib.rs)) owns the Bevy `App` on that thread and loops every 100 ms of wall-clock time:

1. Accepts new connections into sessions. If 1024 sessions already exist, the connection is dropped.
2. Applies up to 4 queued input frames per session. A frame that fails validation, or a closed input channel, disconnects that session. Valid frames for another world are discarded before checking session sequence numbers.
3. If a debug reset was requested, rebuilds the scenario from the same configuration and reconnects every existing connection to the new world.
4. Applies queued debug requests ([Debug accounts](#debug-accounts)).
5. Runs simulation ticks. It adds the positive rate to a tick credit and runs the whole number of ticks in the credit, so a rate of 10 runs 10 ticks in one loop iteration. Each tick is one `App::update`, and its wall-clock duration is recorded for diagnostics.
6. Updates display instances once ([Display instances](#display-instances)).
7. Builds and validates one `Frame` per session and queues the complete encoded frame in a FIFO. Each connection has a 64 MiB outbound byte budget, including the frame currently being written. Exhausting that budget disconnects the slow connection. Invalid server-generated frames panic and abort the server.
8. Collects completed checkpoint writes and captures a requested or due checkpoint for the bounded background writer. Then it sleeps until the next 100 ms deadline. If it is already late, the next deadline starts from now.

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

- **Ship appearances.** A TOML `ShipAppearance` containing the catalogue revision and visible part prototypes, IDs, positions and rotations. It contains no firmware, tank allocations or operational loadout. Its BLAKE3 hash is the `appearance` field of an optical observation or a `Destroyed` combat event. Radio tracks do not publish appearance hashes.
- **The universe catalogue.** A postcard-encoded `UniverseCatalogue`: every system's ID, name, position and influence radius, and each body's ID, name, kind (`star`, `planet` or virtual `barycenter`), radius, mass and parent. Its hash is `presentation.universe.catalogue`. `validate_catalogue` limits it to 65,536 systems and 262,144 bodies, with unique IDs and valid parents.
- **Celestial systems.** TOML system definitions with the orbital and physical parameters needed by the client orrery. Views reference their hashes in `presentation.celestial_systems`.
- **The navigation catalogue.** A Postcard tuple `(asset_version, NavigationCatalogue)`, currently version 1, referenced by `presentation.navigation.catalogue`. It contains system anchors, sovereignty IDs, aggregate population, beacon metadata and reciprocal gate connections. Beacon poses are fixed reference poses captured when the catalogue is published. The decoder rejects assets larger than 32 MiB, unsupported versions, trailing bytes, invalid poses, duplicate IDs, unknown systems and nonreciprocal gate pairs. It accepts at most 65,536 systems and 65,536 beacons.

## Application messages

Messages are defined in [toy-sim-protocol](../crates/toy-sim-protocol/src/lib.rs). The `main` stream carries a sequence of messages. Each message is a 12-byte header followed by a body:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | Magic `TSF1` |
| 4 | 2 | Protocol version, which must be 27 (`VERSION`) |
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
| `State` | 1 | Clock: `world`, `sequence`, `tick`, `sim_time_ns`, `rate` |
| `State` | 2 | `Vec<ViewState>` |
| `State` | 3 | `BTreeMap<GroupId, Vec<Track>>` |
| `State` | 4 | `Vec<ShipTelemetry>` |
| `State` | 5 | `Vec<ScreenUpdate>` |
| `State` | 6 | `Vec<Event>` |
| `State` | 7 | `Vec<CommandResult>` |
| `State` | 8 | `PresentationFrame` |
| `State` | 9 | `SocietySnapshot` |
| `State` | 10 | `calendar_unix_ms` (`i64`, real UTC plus 400 Gregorian years) |
| `State` | 11 | `Vec<OpticalObservation>` |
| `State` | 12 | `Option<IndustrySnapshot>` |
| `State` | 13 | `Option<ChatUpdate>` |
| `Input` | 1 | `InputFrame` |

All thirteen `State` sections are required. Other protocol versions and messages missing any required section are rejected. Encoding and decoding both validate the message. Protocol 27 moves construction blueprints into private upload streams. Protocol 26 adds local hangar subscriptions; protocol 25 adds unloading reactor products into cargo. Protocol 24 adds server route requests, polling and plan commits. Protocol 23 added local chat subscriptions and recipient mailboxes. Protocol 22 added subscribed industry updates and unified cargo stacks. Protocol 21 added authorized gas-account balances and distinguished suspended execution from waiting for account gas. Protocol 20 added sovereignty and population fields to navigation systems and moved the static map into an asset.

`PresentationFrame.navigation` is an `Arc<NavigationSnapshot>` containing an optional catalogue hash, currently relevant live beacons and system-definition references needed by queued celestial destinations. `Arc` shares immutable data inside a process; Postcard serializes the value. Presentation state is a complete snapshot of its relevant live state; the separate industry section carries subscribed changes as described below. The navigation catalogue downloads separately through the existing asset stream and changes when topology or structural metadata changes. Ordinary orbital motion updates live beacon poses without replacing the catalogue.

This division keeps the map outside the continuous snapshot compression window. A prototype that repeated the 3,000-system catalogue produced roughly 2.84 MB state frames and about 1 MB per compressed frame, exceeding the configured 2 MiB Zstd history. Repeating that static graph at 10 Hz therefore remained expensive. The content hash lets clients retain the catalogue while normal state updates stay small.

**State frame limits.**

- `rate` is finite, greater than zero and at most 100.
- At most 8 views, 16 track groups, 64 ship telemetry records and 64 screen updates.
- At most 8192 tracks in total, and at most 8192 track IDs per view. View IDs are unique.
- At most 16,384 events and 4096 command results.
- Every position component is at most 2^110 µm in magnitude. Velocities, rotations and angular velocities are finite, and rotations are unit quaternions to within 1e-5.
- Track IDs are unique within a group. A track has at most 64 tags, and uncertainties are finite and non-negative.
- `Kind`, `Advertised` and `Annotation` tag strings are 1 to 64 bytes with no control characters.
- Ship resources are finite and non-negative. Travel states have at most 256 orders with a cursor within the queue, and a blocked reason is at most 1024 bytes.
- Event kinds are at most 64 bytes. Command errors are at most 1024 bytes.
- A screen update has a slot below 8 and a valid drawing list whose `screen_id` equals the slot. The whole update encodes to at most 65,536 bytes, and its error text is at most 1024 bytes.

**Presentation limits** ([presentation.rs](../crates/toy-sim-protocol/src/presentation.rs)):

- At most 64 ship presentations, 16,384 combat events, 256 celestial system references and 7 capabilities.
- Every ship presentation needs a telemetry record for the same ship in the frame, with at most one presentation per ship. Ship visuals are nested in optical observations in section 11.
- Hardware totals, device readings, health, environment and execution metrics are finite and non-negative where they are physical amounts. Fractions such as throttle, weapon progress, shield strength and shield coverage are within 0 to 1. At most 256 inventory entries, 4096 devices and 8 screen definitions per ship; screen definitions have a slot below 8, a size from 1 to 4096 and a title of at most 64 bytes.
- Instruments have at most 4096 weapon rows, 64 paths with 16,384 vertices in total, and 256 markers. Timed paths have strictly increasing vertex times.
- Combat events have valid positions and poses, and a projectile's end time is not before its start.
- Navigation contains at most 65,536 live beacons, with unique IDs and valid poses. Nonempty live navigation requires a catalogue hash. A live subset may omit the remote half of a gate; reciprocal topology is validated when the complete catalogue asset is decoded.
- Navigation ephemerides contain at most 256 system references per view. Each uses an existing view ID, a finite epoch and a unique `(view, system)` pair within that list. The same system may also occur in the view's visible celestial definitions; the client shares the loaded definition.

**Optical limits.** At most 8192 observations per frame, unique by `(view, id)`. Each names an existing view and carries a valid pose, positive finite radius, non-negative finite luminosity, and validated engine/turret/shield visual state. An optional contact reference must name a track in the same frame. An anonymous observation needs no radio track. The server additionally reserves at most 4 MiB of serialized observations, dividing both count and byte budgets equally among subscribed views. This leaves room within the 8 MiB state-message limit for other sections.

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

Each connection gets a `Session` entity ([session.rs](../crates/toy-sim-server/src/sim/session.rs)). It starts as a member of the account's default information group and the public group.

### Input frames

After protocol validation, a frame whose `world` differs from the current world is discarded without applying its actions or changing sequence state. This handles inputs already in transit when the debug server resets. For frames in the current world, any of these violations disconnects the session:

- `sequence` must be strictly greater than the previous input frame's.

Actions are applied in order before the next simulation tick, and each result's `effective_tick` is the current tick. The session remembers every command ID it has seen and ignores repeats. After 65,536 IDs, further input is refused ("session command history exhausted; reconnect"). An action error does not close the session. It is reported in `CommandResult.error`.

### Results and events

- Each command result is included once, in the next published frame. Its delivery uses the same reliable FIFO as the snapshot.
- A frame includes new events whose `subject` is either an entity with a known ID in one of the frame's tracks, or a ship the account controls. Events with no subject are not sent. Each session advances a server-local publication cursor when building a frame. Event staging is pruned after sessions publish; its lifetime does not depend on client acknowledgements or network progress.
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
| `IndustrySubscribe(IndustrySubscription)` / `IndustryUnsubscribe` | Requests an authorized directory page, selected inventories and optional manufacturing catalogue. Replacement revisions must increase. |
| `Industry(IndustryCommand)` | Starts or cancels production, orders a ship, transfers cargo or refills tanks; permissions and physical transfer constraints are checked when applied. |
| `Debug(command)` | Requires a debug account ([Debug accounts](#debug-accounts)) |
| `Ship { ship, authority_revision, command }` | The account must control the ship, and `authority_revision` must equal the ship's current revision |

Views with a focused ship, instrument subscriptions and screen subscriptions together may name at most 8 distinct ships ("detailed ship subscription limit").

Player commands and NPC officer tools call the same `sim::commands::execute`
function. It validates command payloads, checks the account's current permission
and authority revision, and queues ordinary ship-computer requests. Network
sessions also enforce their joined information groups and display subscriptions.
Target commands resolve a `TrackId` only in the controlled ship's actual sensor
group; knowing a physical entity UUID does not authorize a target. Two-request
autopilot changes reserve both queue slots before changing pending travel state.
Accepted external navigation commands wake the flight computer and pause any
standing freight assignment.

Director observation tools use the same owned-ship telemetry and hardware
presentation as clients. Their `ScanSource` is rebuilt from the ship's current
state, including its host-relative pose while docked, and exposes the existing
fused sensor snapshot and public navigation queries. Each new observation and
command checks the officer account's authority; no network session or global
entity-lookup tool is required.

| `ShipCommand` | Effect |
| --- | --- |
| `SetTransponderEnabled(bool)` | Switches the IFF transponder |
| `SetIff(IffIdentity)` | `owner` must identify the requesting controller. Configure permission is required; `faction` must be an organization the account belongs to or administers. Range is at most 1e8 m. |
| `SetGroup(key)` | Sends the ship's future reports to another information group, creating it if needed |
| `Flight(FlightCommand)` | `HoldAttitude`, `StopGuidance`, `AimDirection`, `SelectTarget(ContactRef)` or `EngageNavigation { throttle_limit, stand_off_m }`, queued to the flight computer as requests. `SelectTarget` resolves its contact like `Aim`. |
| `MarkTarget { group, track, maximum_flight_time_s }`, `Aim { group, track }` | `group` must be both the ship's reporting group and a group the session belongs to, and the track must exist in that group's snapshot. The track becomes a contact handle ([Contact handles](#contact-handles)) and is queued as a request. |
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
- **Views.** Each view runs its query against the current snapshot of its group ([Metered queries](#metered-queries)). The frame shares a total budget of 2,000,000 work units across views, in view ID order. If a view has a `focused_ship` and a sphere, the sphere is centred on that ship's current position. `ViewState` reports `origin`, the returned track IDs and `completion`. Views, instrument subscriptions and screen subscriptions on ships the account no longer controls are removed.
- **Tracks.** The union of the tracks returned by all views, grouped by group ID. Every group the session belongs to has an entry, even when empty.
- **Ships.** Private telemetry for up to 64 ships the account controls. Ships named by a view focus, a screen subscription or an instrument subscription come first, then the rest, each part in ship ID order. Each record holds the information-group key, IFF identity, authority revision, presence, exact pose (only while in space), battery energy, hull heat, shield temperature, coolant reserve and travel state.
- **Screens.** One update per subscribed slot: the latest display frame, or, if there is none, an update with no frame and the error "Display unavailable".
- **Events and results**, as described above.
- **Presentation** ([Presentation](#presentation)), plus per-view **optical observations** ([Optical replication](#optical-replication)).
- **Society.** Ownership hierarchy, standing overrides and permitted asset access information.
- **Calendar.** Authoritative real UTC plus 400 Gregorian years, independent of accelerated simulation time.

Network views have no continuation. Each frame runs a fresh query, so a view that stops at `WorkLimit` or `ResultLimit` shows only its first page.

## Inhabited map and generated systems

The current political map spans roughly 250 light-years across, centred on Sol. It contains the USE, six sovereign LFS member states, and three independent states, including permanently neutral Nova Partenia. Navigation system records carry sovereignty IDs that resolve through the Society directory and integer aggregate population values. Those populations describe settlements; they are not counts of spawned NPC ships or agents. Public affiliation and neutral status do not impose weapon or gate-access rules.

The original ten named systems form a retained corridor inside a larger connected graph. Regional expansion and additional links create a sparse USE trunk network and a more redundant LFS mesh. The initial generation caps each system at six connections. That construction bound is a map-generation choice. The map guide describes the political boundaries and infrastructure history; [stellar provenance](../crates/toy-sim-universe/data/README.md) records the catalogue selection and ICRS coordinate conversion.

Procedural generation runs on the CPU and uses ChaCha20 streams with BLAKE3-derived seeds separated by object identity and generation stage. Catalogue luminosity and temperature seed approximate stellar properties, companion hierarchies and planetary architectures. Planet/moon spacing, Hill and Roche limits, atmospheric pressure/density and surface parameter seeds are generated before the server constructs its immutable system assets. These are deterministic simulation parameters; the source observations do not establish that the generated planets or companion orbits exist. Surface terrain rendering is a separate feature.

All materialized definitions participate in the persistence fingerprint. Rebuilding against a different universe definition is an explicit saved-world boundary; see [Persistence](persistence.md#universe-definition-changes).

## Shared spatial queries

[toy-sim-spatial](../crates/toy-sim-spatial/src/lib.rs) supplies one spatial-index implementation to the server and supporting crates. Each entry contains an integer galactic position, a conservative radius and a luminosity coefficient. Occupied cells form a sparse hierarchy with compressed empty scales. Coordinates remain signed 128-bit micrometres until a query needs relative floating-point distances.

The index maintains geometry cells and additional cells grouped by powers of two in luminosity. A brightness query uses each bucket's upper luminosity bound to choose a conservative search radius, then checks individual entries. Changing luminosity moves an entry between buckets as needed; zero-luminosity objects remain available for geometric queries. Radius, sphere-intersection, nearest-neighbour, segment and resumable range queries use the same implementation. Cursor work is explicit and charged by the intelligence query layer.

Consumers keep separate instances for their data and lifetimes:

- Server spatial observations index active ships, celestial bodies and gates. Celestials block sensors but are excluded from ship sensor results. Projectiles and dormant ships are omitted from this observation index.
- Immutable intelligence snapshots index fused track estimates alongside their tag indexes.
- The star catalogue uses the hash for nearby-star and apparent-brightness queries.
- Travel geometry indexes physical extents and gate exclusions for departure/arrival checks.
- Collision `SweptIndex` indexes conservative motion envelopes and filters candidate capsules before continuous contact prediction. Parry retains its shape queries and internal compound acceleration structures; see [collisions.md](collisions.md#broad-phase).

Each instance has its own records and lifetime, and all use the same query implementation. The active observation index currently rebuilds at the simulation boundary; immutable readers may retain the preceding version. The hash itself supports incremental position and luminosity updates, which the collision adapter uses.

## Observations and intelligence

Clients receive private state for authorized ships, fused radio/sensor tracks from information groups, and separately filtered optical observations from each focused ship. Radio knowledge alone does not grant a ship mesh, its appearance asset or live visual state. The implementation is in [intelligence.rs](../crates/toy-sim-server/src/sim/intelligence.rs), [identity.rs](../crates/toy-sim-server/src/sim/identity.rs) and [toy-sim-intel](../crates/toy-sim-intel/src/lib.rs).

### Identities

Three kinds of identifier are kept apart:

- **Physical UUIDs.** Ships, celestial bodies, accounts and groups have 16-byte UUIDs. A ship's UUID is its identity for control, commands, telemetry, beacons, docking and IFF. A track exposes it in `entity` only when the observing group has an authenticated measurement of that ship.
- **Track IDs and contact references.** A `TrackId` is random per group, so the same ship has unrelated track IDs in different groups, and a sensor-only track carries no UUID. A `ContactRef { group, track }` names a track as seen by one group. Targeting, instruments and identified combat records use these references. Flight programs see opaque `u64` contact handles ([Contact handles](#contact-handles)).
- **Optical IDs.** The server assigns random session-local IDs to visible physical objects. The same ID can appear in several views; client identity is `(view, id)`. It persists through brief visibility losses for 100 simulation ticks. `known_entity` is populated only when the UUID is already known through authenticated tracks, control or the focused ship itself. A separate spatial-lifetime token prevents interpolation across a relocation or gate transit.

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
  - Internal measurements carry the target's radius and appearance hash. Session publication removes appearance hashes from all transmitted tracks; optical observations supply geometry separately.

Acquisition runs in parallel across groups.

### Public observations

The public group (`PUBLIC_GROUP`, ID `ff…ff`) is joined by every session. It receives:

- every ship with a beacon emitter, as an exact measurement with provenance `Beacon`, the beacon's IFF tags and `Kind("beacon")`

The public group includes the station and the generated map’s gate beacons. Full route topology is also available in the public navigation catalogue and paged firmware queries. Celestial bodies are excluded from sensor acquisition and fused tracks. Their positions and HUD labels come from the subscribed system definitions and client orrery. They still occlude sensors and remain available through explicit celestial queries.

### Fusion

Once per tick:

1. **Coasting.** Tracks not observed for more than 600 ticks (60 s) are removed. Other tracks are extrapolated by their velocity, their position uncertainty grows, and their provenance becomes `Extrapolated`.
2. **Grouping.** Measurements are grouped by physical entity and sorted by σ, then platform. The best measurement is the lowest σ.
3. **Association.** The best measurement keeps the entity's existing track ID if it is authenticated with the same UUID, or if it lies within 3 × (track σ + measurement σ) of the track. Otherwise the track gets a new random ID. A known UUID is kept. When the best measurement is an unauthenticated sensor measurement, IFF owner and faction tags from the continued track are kept, so a ship that switches off its transponder keeps its last identified owner.
4. **Weighting.** When the best measurement is noisy, one measurement per platform is combined by inverse-variance weighting. The fused σ is at least max(1 m, best σ / 4), and the velocity σ equals the position σ.
5. **Publication.** Each group gets an immutable `Snapshot` (`Arc`) indexed by tags and the shared `toy-sim-spatial` hash. Queries and flight programs retain their snapshot while the next update proceeds.

Grouping in step 2 uses the server's true entity identity. Clients never see that identity for sensor-only tracks.

### IFF

`IffIdentity` has `owner` (account), optional `faction`, up to 16 advertised `labels`, `enabled` and `range_m`. A new ship starts with its owner and current organization, with the transponder enabled and a range of 1e8 m. The transponder is the only way for an observer outside the ship's group to learn its UUID and owner.

A ship's advertised IFF identity remains separate from actual ownership and control. Control is the `Control { account, revision }` component, and commands carry its current revision. The server capture operation changes ownership, controller, and information group, clears prior access grants, and preserves IFF. It does not expose a player-facing capture action. Organization ownership can retain an assigned controller. Configure permission is required to reprogram IFF or change the information group.

## Metered queries

`toy_sim_intel::query::Queries` runs a `TrackQuery` against a snapshot and returns a `QueryPage`.

A query has an optional direct `track`, an optional `sphere`, tag sets `all`, `any` and `exclude`, an optional `max_age_ticks`, a result `limit` of 1 to 256, and a `work` budget.

**Candidate source.** The first rule that applies decides where candidates come from:

1. The direct track ID.
2. The smallest `all` tag index, when there is no sphere or that index holds at most 256 tracks.
3. A resumable spatial-hash range cursor over the sphere.
4. The union of the `any` tag indexes.
5. All tracks.

Every candidate is then filtered against all conditions.

**Work charges.**

| Charge | Amount |
| --- | --- |
| Page call | 100 |
| Each index step | 8 (plus 8 per tag for an `any` union); spatial cursors charge occupied-cell visits and candidate checks |
| Each candidate examined | 1000 |
| Each returned track | 1 per 8 bytes of its ABI record and variable-field arena |

**Completion.** `Complete` means the source is exhausted. `ResultLimit` means `limit` tracks were returned. `WorkLimit` means the budget ran out. `gas_used` reports the work spent, and an empty result can still spend the whole budget. An incomplete page returns a `continuation` cursor. The cursor reads the same snapshot, and `revision` reports the snapshot's tick. A cursor expires 10 ticks after the query started. One `Queries` holds at most 8 live cursors.

**Who uses it.**

| Caller | Budget | Cursors |
| --- | --- | --- |
| Network views | Shared 2,000,000 per frame | None; a fresh query every frame |
| Flight programs (`intel_tracks` and `intel_continue`) | At most 1,000,000 per call, paid from the computer's gas | Kept per ship, with separate stores for `ship_tick` and `ship_display` |
| `sensor_scan` | `n × 1200 + 100` per group snapshot | None |

### Contact handles

Flight programs see `u64` contact IDs instead of track IDs ([services.rs](../crates/toy-sim-server/src/sim/services.rs)). Each ship keeps a table from `(group, track)` to a random non-zero handle. The table is replaced when the ship changes group. Entries unused for 600 ticks expire, and when the table holds 8192 entries the oldest is evicted. A handle therefore says nothing about the ship's UUID, and the same target has different handles on different ships.

`sensor_scan(range, n)` runs a sphere query on the ship's own group snapshot and on the public snapshot, removes the ship itself and duplicate UUIDs, sorts by distance and returns at most `min(n, 256)` contacts. The query selects ship tracks only. Contacts have kind `CONTACT_SHIP` and are named by UUID when the track has one, or "Unknown contact". Position and velocity are relative to the ship.

Instruments translate handles back into `ContactRef`s only for tracks that still exist in the ship's group or the public group.

## Presentation

Section 8 of a state frame is a `PresentationFrame` ([presentation.rs](../crates/toy-sim-model/src/presentation.rs), built in [sim/presentation.rs](../crates/toy-sim-server/src/sim/presentation.rs)). It carries what the shared client needs for the original rendering, flight instruments and windows, without exposing true state beyond what the session may already see.

- **`ships`.** One `ShipPresentation` per controlled ship that a view focuses, a screen subscribes or an instrument subscription names. It holds the authority revision, flight environment (altitude, airspeed, density, pressure), health, execution timings, mass, inertia, control rotation, heat and battery capacities, power flow, inventory, per-device telemetry, computer status and screen definitions. `instruments` (attitude, navigation, weapons, trajectories and markers published by the firmware) is included only for instrument subscriptions. Targets in instruments are `ContactRef`s.
- **`combat`.** Shots, projectiles, impacts and destruction since the session's preceding publication. Publication requires both a suitable identified track and optical visibility of its source or target. Destruction may use visibility from the preceding publication so removing a destroyed body does not erase its final effect. This grace is cleared when a view changes focus or spatial lifetime. The record names a `ContactRef`; radio reports alone cannot reveal remote firing or destruction geometry.
- **`celestial_systems`.** Per-view system IDs, definition asset hashes and an explicit epoch/time origin. Complete system definitions arrive through asset streams. The client evaluates the shared orrery solver at presentation time for positions, velocities and rotations. System selection uses the view’s location independently of sensor coverage and server ECS activation.
- **`universe`.** The complete system/body catalogue asset hash for every session. Debug sessions also receive the active systems (with the reason `ships` or `debug inspection`) and the inspected body.
- **`navigation`.** The public navigation catalogue's asset hash and current beacon poses for the session's local systems and queued destinations. Local systems include subscribed view origins and controlled ships' positions. Orders referring to a beacon add that beacon even when remote. `ephemerides` supplies public system-definition hashes and epochs for celestial references in each focused ship's queue, including remote body-relative slip destinations. The static catalogue supplies all system anchors, sovereignty UUIDs, aggregate settlement populations, gate connectivity and docking-facility metadata. Its reference poses support the map; HUD and camera geometry use live beacon records. The docking flag advertises a facility; usable bays require a ship-specific beacon query.
- **`capabilities`** and **`diagnostics`** for debug sessions ([Debug accounts](#debug-accounts)).

### Optical replication

Section 11 contains `OpticalObservation` records built in [session/optical.rs](../crates/toy-sim-server/src/sim/session/optical.rs). A view needs an authorized focused ship. Visibility is evaluated at that ship's position, independently of its radio query and the client's orbit-camera position. The focused ship is included for its own presentation. A dormant focused ship has no surrounding space observations.

For other active ships, the server queries brightness buckets, computes the observer-dependent luminosity, and requires received flux of at least `1e-12 W/m²`. It rejects a body only when an intervening optical blocker covers its entire conservative angular disc; a body partly visible around a planetary limb remains eligible. Candidates are ordered by received flux. Each view receives an equal share of the frame's 8192-observation and 4 MiB optical budgets, with its focused ship first. Oversized records are skipped. Multiple views cannot let one dense scene consume every other view's allowance.

Each record carries its view, anonymous optical ID, spatial-lifetime token, optional already-known UUID/contact, exact visual pose, conservative radius, equivalent optical luminosity, optional appearance hash, and `ShipVisual` engine/turret/shield state. Appearance and live geometry are absent from radio-only tracks. A visible ship can therefore appear without an IFF identity or sensor track, while a distant radio contact can remain in the Overview without acquiring a mesh.

#### Brightness model and approximations

The current CPU model in [spatial/lighting.rs](../crates/toy-sim-server/src/sim/spatial/lighting.rs) combines:

- Reflected starlight and enabled-gate light. It uses inverse-square irradiance, a spherical target with geometric albedo 0.3, and a Lambert phase function for the observer. A fully eclipsed light source contributes no reflection. The indexed value is the full-phase upper bound under current illumination; the observer-specific phase is applied after candidate discovery. Geometric albedo describes full-phase brightness relative to a diffuse reference disc; it does not specify the total fraction of incident energy absorbed by the material. See [JPL’s albedo definition](https://ssd.jpl.nasa.gov/glossary/albedo.html).
- Shield/radiator thermal emission, using the current emitting area and a numerical integral of the blackbody spectrum over 380–780 nm.
- Engine emission from delivered thrust and exhaust speed. The optical fraction of `0.5 × thrust × exhaust_speed` is 0.001 for thermal/conventional engines and RCS, and 0.01 for micropulse engines. Disabled or unpowered engines contribute zero.

Buckets therefore follow current thermal state, engine output and planetary shadows. Stellar/gate luminosity uses a fixed 220 lumens per optical watt conversion. These are gameplay photometric approximations: materials, directional exhaust radiation, specular hull reflections, indirect planet light and partial-eclipse attenuation are not modeled. Ship bounding spheres are excluded as optical blockers because they can enclose large empty spaces; celestial blockers use spheres. The index remains conservative for partial target occultation.

#### Distant ship glints

The client uses the projected diameter of each optical observation's bounding sphere to choose its representation. [glints.rs](../crates/toy-sim-client/src/ui/scene/glints.rs) crossfades between detailed geometry and a sprite over approximately 3–8 physical pixels, with 0.5-pixel hysteresis around mesh creation/removal. The decision accounts for viewport height, field of view and camera depth. Engine plumes and shields fade with the mesh.

Glints share one additive billboard batch per view. Their brightness follows `L / (4πr²)` with a finite near-distance clamp, the view's exposure and a nominal magnitude-based display gain. A bounded attitude-dependent modulation gives gentle glints without random atmospheric twinkling. The shader draws a compact core and halo; it adds no lights, shadows or prepass. The client updates the batch geometry each display frame. The sprite's appearance and nominal magnitude scale are display approximations, while its visibility and baseline brightness remain supplied by the server.

## Debug accounts

A debug account is a configured account named by `debug_account`. It has no separate connection path. Its session frames list seven `DebugCapability` values: `Clock`, `Reset`, `Relocate`, `Recover`, `InjectHeat`, `Inspect` and `ConfigureSensor`. `DebugCommand::capability` maps each command to the capability it needs. Any other account's `Debug` action fails with "debug access denied".

| `DebugCommand` | Effect |
| --- | --- |
| `SetRate(rate)` | Sets a finite positive clock rate of at most 100 ticks per 100 ms loop. |
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
- **Creation.** An instance is created for a ship when at least one subscriber controls it, the ship is not dormant and its computer is powered. It is instantiated with `instantiate_display` and configured with the ship's hardware. Boot work is paid from the actual owner's gas account using the ship's physical CPU allowance left after flight execution. Display creation does not grant free boot or extra gas.
- **Input.** On each update it receives a copy of the flight computer's latest input with commands and screen events removed. `requested_screens` holds the subscribed slots that are due: a slot is due when it has no frame yet, or when `tick × hz / 10` has crossed an integer since its last frame. Paid boot, an unfinished callback or pending input can also require a slice when no new slot is due.
- **Frames.** Frames for requested slots are stored as `ScreenUpdate { ship, slot, revision, tick, frame, error }`. `revision` is the instance revision, a server-wide counter that is new for every instance. If the callback fails, every requested slot gets the error text, truncated to 512 bytes, and no frame. Screens the firmware clears lose their stored frame.
- **Limits.** It has its own memory, query cursors and screen event queue. Flight and display slices share one physical per-ship tick ceiling and the same owner account. An unfinished display callback resumes with later grants. It cannot write devices or submit world actions. It does not share memory with the flight instance.
- **Screen definitions.** A ship presentation lists the display instance's screen definitions, or, if it has none, the flight computer's.
- **Expiry and revocation.** An instance is removed on the first update after its ship becomes dormant, its computer loses power, its control revision changes, or 10 ticks pass with no subscriber. Its frames are withheld immediately in those cases. A new instance gets a new revision, so input aimed at the old frames is rejected, and queued input is dropped with the old instance.
- **Screen input.** `ScreenInput` requires the ship to be active and powered, the instance's control revision to match, the slot's stored frame to exist with the given revision, a valid event kind (0 to 7) and at most 64 bytes of text. The event gets the next event ID for that instance and is queued as a `ScreenEvent` with its code, modifiers, coordinates and text. Pointer moves, presses and releases, key presses and releases, text, bezel and reset kinds are all forwarded.

The stock firmware's `ship_display` defines each requested slot as a 512 × 256 screen titled "Ship status" and draws five text lines: `SHIP STATUS`, simulation time, speed, mass and battery energy. It does not read screen events. Firmware without a `ship_display` export cannot be instantiated as a display, so its subscribed slots report "Display unavailable".

## Missiles and shared computers

[missiles.rs](../crates/toy-sim-server/src/sim/missiles.rs) launches missiles as
ordinary simulated ships. A powered launcher consumes one packaged missile from
its magazine, creates the assembled body with its fuel and battery, and applies
the corresponding parent mass, recoil and angular-momentum changes. Launch
requires a current target contact in the parent's information group, range and
cooldown checks, and a working computer with the optional ABI 28
`missile_tick(u64)` callback. The launcher has no privileged target-position
lookup. The existing marked-target and firing state controls launcher fire.

Each missile has its own physical body, engine, steering devices, seeker and
integer resource inventory. It inherits the parent's actual owner and information
group, and launches with its IFF transponder disabled. Sensor contacts carry both
`Kind("ship")` and `Kind("missile")`; clients can distinguish them while general
ship queries still include them. Optical appearance remains subject to ordinary
visibility admission.

The missile's stable handle selects a callback in its parent's flight computer;
it does not create another WASM instance. All such callbacks share the parent's
memory, persistent store, owner gas account and physical CPU allowance. A pending
callback retains its kind and handle when gas runs out. The scoped
`missile_read` and `missile_control` imports expose its observation and steering
command, as described in [Ship controller ABI](ship-abi.md#missile-callbacks).
Targets are fused information-group contacts with uncertainty. An expired or
unavailable contact is reported as unavailable. The stock guidance coasts when it
cannot see its target or has no propellant, and uses proportional navigation to
correct a visible interception.

Guidance can continue while the parent is docked or in slip transit, provided its
missiles still have working electronics and energy. Computer telemetry reports
that shared execution, including paid boot, suspension and fault recovery. It does
not mark the dormant parent's physical devices as powered.

Destroying the parent hull retains the shared computer while guided missiles
remain. The destroyed parent contributes no hull, collision body, sensor source,
beacon or display. Its original ownership and information group still determine
billing and observations. The retained computer is released when its last guided
missile becomes inactive. Missile impacts use ordinary ship collision and hull
damage rules. Checkpoints preserve bodies, guidance controls, stable handles,
launcher state and the retained computer's committed data; suspended native
execution cold boots after recovery.

## Docking and travel

Docking and travel are implemented in [travel.rs](../crates/toy-sim-server/src/sim/travel.rs).

### Presence

`Presence` is one of `Space`, `Docked { host, bay }`, `SlipTransit(id)`, `StoredInWreck(host)` or `Destroyed`. Private telemetry includes an appearance hash, radius and presentation pose in space, docking storage and slip transit. Docked poses follow the host bay; transit poses follow the declared slip segment. Only space presence participates in normal physics.

Leaving space makes a ship dormant. Its hardware is shut down (default device settings, avionics unpowered, sensor range 0), and its velocity, rigid body, collision body and spatial body are removed and remembered. Dormant ships take no part in physics, sensing or displays. Ordinary flight callbacks and world actions stop. A destroyed parent can retain its shared computer to guide already launched missiles, using information-group observations without restoring the parent's sensor. Their hull and shield thermal state advances once per simulated second. Returning to space restores the remembered components.

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

### Gates

A gate is fixed navigation infrastructure with a `Gate` record paired to another gate. It has no rigid body. Gate mouths follow prescribed circular ephemerides in their assigned systems. Their spherical apertures accept entry from any direction.

The collision solver detects inward crossings of the aperture boundary along each body's swept trajectory, including projectiles created during the tick. The object's centre crossing the boundary triggers the interaction immediately. Relative speed above 100 m/s destroys the object at that time, even if it would cross the entire aperture between ticks.

At speeds up to 100 m/s, transfer requires both mouths to be enabled and reciprocally paired. The body's bounding radius must fit both mouths, and its exit must be clear of other simulated collision bodies. An unavailable, undersized, or obstructed exit reflects the approach velocity at the entry boundary. Gates have no ownership or transit-permission checks. Territorial control must be enforced physically by in-game ships, turrets, missiles, and drones.

The exit is placed along the rotated relative velocity, at exit aperture radius + body radius + 2 m from the exit centre. Velocity relative to the mouth, orientation, and angular momentum rotate into the exit frame. The remaining part of the physics tick runs there. This places the object fully outside the exit and moving away from it.

The stock flight computer guides a `Gate` leg toward the mouth. Crossing advances a matching gate leg and emits `gate-transferred`; manual crossings work without a travel order or firmware action. ABI 16 removes `ProgramAction::Gate`.

Gate apertures render as animated luminous spirals. Their point lights cast shadows within twice the aperture radius (440 m for the default gates).

### Slipdrive

A `SlipDrive` defaults to 100 MW and is ready at once. The scenario gives one to each player ship.

**Preparation** requires:

- the ship is in space; galactic velocity does not restrict slip preparation
- the departure point is currently clear, and the candidate exit is predicted to be clear at arrival; each aperture must avoid ships and bodies, have tidal curvature of at most 1e-8 s⁻², and lie outside every enabled gate's exclusion radius
- the drive's cooldown has ended

**Energy.** The drive needs 1e5 J/kg × mass × (1 + distance in light-years / 1000), rounded up to whole joules. Each tick it draws min(power × 0.1 s, remaining) from the ship's stored energy, and 20% of the energy drawn becomes waste heat. Losing a valid departure aperture, increasing mass by more than 0.1%, or predicting an obstructed exit blocks the route and cancels unfinished preparation. Entering `Blocked` clears the charging candidate and ETA. Cancellation and candidate changes do not refund spent energy.

The ship can coast during preparation. The queued order keeps its typed destination, while `ProgramAction::Slip(position)` supplies a concrete predicted exit. Repeating that action during charging updates the candidate and required energy while preserving the original start time and accumulated work. The standard computer resolves moving destinations at the estimated arrival epoch and refreshes its candidate each tick. Its bounded prediction iteration includes remaining preparation and flight time; failure to converge follows the normal blocked/retry path.

**Departure** happens once the energy is complete and at least 100 ticks have passed. Charging and departure run before that tick's firmware callback. Preparation estimates therefore include whole future charging ticks; flight duration is (30 + 8.64 × light-years) s, rounded up to a simulation tick. The ship becomes dormant with presence `SlipTransit`, which freezes the selected galactic exit point. Departure velocity is preserved through transit and restored on arrival. The drive is ready again 60 s after scheduled arrival, and `slip-departed` is emitted.

**Arrival** places the ship at the destination if the destination is still admissible, returns it to space, and emits `slip-arrived`. A ship whose hull has failed is destroyed instead. Otherwise the ship stays in transit, retries every 10 ticks, and reports `Blocked("Arrival obstructed")`.

### Travel orders and server planning

Player requests contain a complete order queue. Orders include
`TravelTo(Destination)`, `Sublight(Destination)`, `Slip { destination }`,
`Jump(entry_gate)`, `Dock(station)`, `Undock`, `WaitUntil(tick)` and guidance.
Destinations may refer to public beacons, galactic positions, or offsets from
beacons and celestial bodies. Guidance contact targets must belong to the
ship's authorized fused picture. Gate transit has no political permission check.

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

Protocol 24 exposes route preview through
`RouteRequest { ship, authority_revision, request }` and
`RoutePoll { ship, authority_revision, id }`. A request carries a nonzero caller
ID, all requested orders and `PlanningPreferences`. Both actions return
`Reply::Route { id, status }`: unknown, pending with progress, ready with a plan,
or failed with a bounded explanation. The client polls and offers an explicit
Engage action for a ready preview. `ShipCommand::UseRoute` commits it against the
expected travel revision. Current authority is checked at every operation;
changed ownership or control cannot inherit another caller's job.

A ready plan contains its creation tick, travel and topology revisions, the
strategic route queue, per-stage time and propellant estimates, and the
whole-route fuel budget. Requests and plans contain at most 256 orders;
serialized plans are limited to 48 KiB. Job enqueue and polling are bounded
operations. Planning work runs separately from the ship callback and debits the
owner's global gas account. A full requested queue is expanded before it is
committed. The server checks current control and queue revisions when committing a plan.
Its age, ordinary ship movement and changes elsewhere in global gate topology
do not invalidate it. The topology revision records the planning context; it is
not a commit expiry condition. The ship resolves current geometry while
executing each strategic command, and the host checks physical admission when
applying that command.

The fuel-priority slider ranges from 0.1 to 1000, with a default of 1. The cost
is time in seconds plus propellant kilograms multiplied by
`3600 × fuel_priority / ship_mass_kg`. The server compares gate, local transfer
and slip candidates using public navigation facts and the ship's own hardware.
Search optimizes this candidate graph and its estimated edge costs; it does not
prove an optimum over every physically possible trajectory. Estimates assume
rated acceleration and drive power. Turning, gravity, changing mass and power
shortages can add time or fuel consumption.

Each stage retains an optional duration and propulsion fuel estimate. The flight
program updates only the active stage's remaining time and propellant; the host
combines it with later stages and current tank balances. Resource shortages are
checked separately. Unknown and continuous stages make the budget partial.
Slip preserves departure velocity, so estimates include matching destination
motion after arrival. Budgets are advisory and require operating margin.

| `ProgramQuery` | Reply |
| --- | --- |
| `Travel` | Current order, revision, index, status, preferences, active arrival estimate, exact own pose and slip readiness. |
| `RouteRequest(request)`, `RoutePoll { id }` | The same scoped asynchronous preview service available to clients. Each call admits 8192 work gas plus its normal call and copy costs. |
| `Contact(reference)` | Authorized fused pose, radius and opaque firmware handle. |
| `LocalSpace { destination, range_m, after_seconds }` | Up to 32 public or observed obstacle/exclusion volumes around the ship and destination, including enclosing volumes. An explicit truncation flag reports incomplete results. |
| `Resolve { destination, after_seconds }` | Predicted public destination pose. |
| `Beacon(id)`, `Beacons { after, limit }` | Public beacon facts and currently authorized docking bays. |
| `Navigation { after, limit, reference }` | Paged public gate facts; still available to custom programs, unused by standard command execution. |
| `SlipEligibility { origin, destination, departure_after_seconds, arrival_after_seconds }` | Current drive readiness, future aperture eligibility, remaining preparation time and flight duration. |
| `Tracks(query)`, `Continue { cursor, work }` | Metered queries of the ship's fused information group. |

Local-space observations spend 262144 work gas per query, plus normal call and
copy costs, with a hard 2048-unit index/ephemeris work budget. Query range is
finite and at most 10¹² metres. The observations contain no undetected private
objects, and a truncated reply cannot be treated as an empty or complete scene.

Prediction offsets are finite, nonnegative and bounded to one Julian year.
Slip arrival cannot precede departure. Orbital bodies and gates use their
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
| `Slip { revision, order, destination }` | Updates the current slip command's concrete charging candidate without resetting work or start time. Departure freezes the exit. |
| `ReserveBay`, `Dock`, `Undock` | Executes the corresponding current command with the same revision and index checks. |

The standard executor resolves the current destination each callback. Sublight
guidance uses relative position and velocity with the economical navigation
law. Moving slip destinations are led through preparation and transit time,
and their charging candidates are refreshed until departure. Bay requests use
only bays reported as usable. Execution errors block the command for an explicit
replan; the VM does not replace the queue itself. Server and firmware use ABI 31
and must be rebuilt together.

## Client playback

`toy_sim_client::connect` ([connection.rs](../crates/toy-sim-client/src/connection.rs)) opens the `main` stream and returns an `Endpoint` with state and input channels plus a cloneable `AssetClient`:

- incoming states: an unbounded FIFO, drained into the jitter buffer by the client update loop
- outgoing inputs: 16 frames
- asset requests: 8 pending requests; each `AssetClient::fetch(hash)` awaits its own response containing complete bytes or an error

Asset requests start independent transfers as they leave the request channel. A slow transfer does not hold up later requests; the channel bounds pending requests rather than active downloads.

`Playback` buffers bursty arrivals. Bevy runs the client at a fixed presentation cadence of 10 Hz.

**Receiving frames.**

- The protocol decoder validates every frame. Playback checks sequence and timestamp ordering.
- A new `world` resets buffered snapshots and retained publications. At the next fixed update, one reset system removes entities marked `WorldMember`, resets session metadata and interpolation time, and triggers `SessionReset`. Observers clear UI resources, subscription bookkeeping, pending actions and celestial indexes. The UI then subscribes again.
- `sequence` must increase and simulation time must not go backwards. A violation is reported to the UI as a status message.
- Snapshots retain their events and command results until playback consumes them. Every queued snapshot is consumed in order; the client does not discard old snapshots to reduce backlog or impose a hard receive-buffer limit. Generic events are retained in session metadata for UI consumers.

**Output.**

- `PreUpdate` drains received states into the buffer. Each `FixedUpdate` consumes one snapshot normally, or two while catching up. Publications from both consumed snapshots are processed in order, and the final snapshot supplies the resulting ECS state. Playback starts once the buffer holds the target depth: 3 frames for `toy-sim-client`, 1 for `toy-sim-debug`.
- On an empty buffer, `underruns` increments once and the target grows by one, up to 10. Consumption resumes when the reserve has rebuilt. The previous and current ECS samples remain unchanged during this wait, so each fixed interval repeats the last movement. A new world restores the initial target and clears the underrun count.
- Catch-up starts when queued depth exceeds the target by more than six frames. It consumes two frames per tick until depth returns to the target, then resumes one per tick. No queued snapshot is dropped.
- In `Update`, the interpolation fraction is `Time<Fixed>::overstep_fraction_f64()`. Pose and hardware components interpolate from the preceding displayed snapshot to the final snapshot consumed in the tick; turret yaw follows the shortest arc. The first sample initializes both endpoints to the arrival pose.
- `RenderTime` contains only `previous_ns`, `current_ns` and `display_ns`. The display timestamp interpolates the two snapshots' explicit `sim_time_ns` values using the same fraction. Celestial positions are evaluated analytically at that time. Accelerated simulation and skipped publications use their actual timestamp intervals. Repeated timestamps remain valid when no simulation tick elapsed between publications at a positive fractional rate. The buffer does not synthesize snapshots or infer timestamps from the advertised clock rate.

**Spatial lifetimes.** Tracks and private ship telemetry carry a `spatial_instance` token. Relocation, departure and arrival replace this token while preserving the ship's UUID. A changed track token replaces its client observation entity and associated render instances; new samples start at the arrival pose. Controlled ships retain their telemetry entity, removing pose components while absent and initializing fresh samples on arrival. The token changes even when departure and arrival occur between two publications. Anonymous track tokens are scoped to the opaque track ID. Interpolation and tracer history use these lifetimes without travel-specific discontinuity events.

Publications are delivered when playback consumes their frame, including the intermediate frame of a catch-up tick. Combat visibility still follows each event's simulation timestamp. The session keeps the latest 128 delivered command results and generic events for the UI.

**Input.** `FixedPostUpdate` sends an `InputFrame` every 100 ms once the UI knows the world, even when there are no actions. Inputs carry no server snapshot reference or acknowledgement; received actions apply before the next available simulation tick. The command queue supplies command IDs and ship authority revisions, and preserves their order without coalescing. If the input channel is full, queued actions retain their IDs for the next attempt and the status reads "Input queue busy".

## Client ECS presentation

The client separates transport ingestion, replication and interpolation into the [state modules](../crates/toy-sim-client/src/state). `SessionInfo` owns the world, generation, applied tick and sequence, capabilities, diagnostics and delivered command results. Network reception, buffered playback, outgoing commands and session metadata have separate resources. Only snapshot ingestion reads the buffered frames. It reconciles stable entities for contacts, controlled ships, optical observations, views and combat publications. Contact identity includes the information group, so observations from different groups remain distinct. ID-to-entity maps are derived lookup indexes.

Ship, contact and optical entities hold pose samples and interpolated display components. Optical entities also interpolate luminosity. Radio contacts supply HUD/Overview entries; live ship geometry is sourced from the optical entities for that view, with private rendering for the focused ship in a hangar. Each view owns its camera, origin, exposure and sky state. `CameraOptions` holds the camera focus separately from the orbit data used to construct trajectories. Render instances relate to both their source observation and their view, allowing either lifetime to remove the associated visuals. Bevy asset handles own immutable downloaded assets; sky baking keeps its separate work budget. HUD overlays query presentation components, and scene alignment gestures enqueue navigation actions through the outgoing resource.

Reception runs in `PreUpdate`, snapshot application in `FixedUpdate`, and input publication in `FixedPostUpdate`. `Update` then runs interpolation, celestial evaluation, view updates and rendering updates. World changes remove the old observations and reset selection. Missing observations remove their entities; a changed spatial lifetime creates fresh presentation samples. Docked ships retain private telemetry while their space pose is absent. MFD publications remain available in the wire protocol; the current UI does not subscribe to screens or replicate them into display entities.

The global map and local celestial ephemerides have different lifetimes. The navigation catalogue asset contains all 3,000 systems and their public gate connectivity, including systems with no active simulation entities. The server retains all immutable system definitions and can resolve inactive bodies from their solvers. ECS celestial entities are created only for systems intersected by a vessel's motion over the current tick, or selected for debug inspection; they are removed when no longer needed. Gate landmarks do not activate every remote system merely by existing.

A view receives system-definition references based on its own position, independently of server ECS activation and sensor coverage. At most 32 overlapping systems are referenced per view; debug inspection takes priority, then distance. A truncated view reports `ResultLimit`. The client evaluates those systems using the explicit simulation epoch and renders physical stars, planets and moons. Virtual barycentres organize orbits and appear in the catalogue hierarchy, but add no rendered body, collider, light or duplicate gravitational mass.

Queued celestial destinations add `navigation.ephemerides` references to the definition loader's desired set. These use the same public assets and epochs as visible systems. They let the client resolve remote waypoints without a sensor observation or a list of every body's current pose. Loading a definition for navigation does not add that remote system to the view's rendering subscription.

Complete system assets contain body IDs, parent relationships, orbital elements, rotation, mass, radius, stellar luminosity, colours and atmosphere parameters. Multiple views of one system share its definition and celestial entities. The assets use TOML, preserving the existing definition parser; their content hashes identify immutable bytes. Each system entity holds a typed definition handle and reports pending or failed loading in its subscribed views. The shared solver uses the explicit MJD epoch plus elapsed simulation time; it does not depend on the client’s wall-clock date.

The [asset source](../crates/toy-sim-client/src/assets.rs) registers canonical `server://<hash>` paths before Bevy's asset plugin. Its reader forwards requests to the existing Tokio transport and returns verified bytes to typed loaders for ship appearances, system definitions and navigation catalogues. Decoding and preparation run through Bevy's asset pipeline. Ship observations and destruction publications hold appearance handles, and system entities hold definition handles. Shared handles reuse loading and decoded assets; releasing the final handle allows unloading. Individual entities appear as their assets become available.

A public [ship appearance](../crates/toy-sim-ships/src/appearance.rs) contains only the catalogue revision and each visible part's catalogue prototype, animation ID, position and rotation relative to the ship's physical origin. The server exports these final transforms from its compiled design, preserving the center of mass used by rendering without publishing tank allocations, resource types, starting fills, names, device groups, avionics settings or firmware. The client resolves public catalogue models and derives visual bounds directly from those transforms; it does not compile a gameplay blueprint to render a ship. Authorized ship telemetry and construction blueprints use their separate permission checks.

Navigation has explicit `Unavailable`, `Loading`, `Ready` and `Failed` states. A replacement hash immediately clears the previous catalogue and map layout, even if its topology revision matches. The loader installs a shared catalogue only when its hash and session generation still match, preventing a late completion from restoring stale data. The Gate Network window shows loading or failure text and offers **Retry download**, which reloads the current asset path. Live beacon ECS replication continues independently; downloading the full map never creates remote HUD objects. A world reset clears the handle and catalogue state.

The default ship starts in sunlight. Its initial camera faces the illuminated hull, and direct stellar illumination uses luminosity divided by spherical area at the view’s distance. Ambient fill is disabled, and the default camera exposure is EV100 15 with per-view adjustment. The original star-disc, Gaia cubemap, atmosphere and exposure algorithms remain the rendering reference.

## The client UI

The shared egui theme, embedded fonts, Phosphor icons and desktop toolkit come from [toy-sim-ui](../crates/toy-sim-ui/README.md). Both client launchers use the same [Bevy/egui app](../crates/toy-sim-client/src/ui.rs). A toolbar runs down the left edge, location and autopilot information sit at the upper left, and Selected Item controls sit above the Overview on the right. The bottom HUD displays the controlled ship's reserves and operating state.

The toolbar opens Overview, Selected Item, Inventory, Industry, Local Chat, Navigation, Gate Network and Society & Ownership. It also toggles orbital paths, returns the camera to the controlled ship, and opens Interface settings. Windows can be dragged, resized, closed, snapped and locked; Interface can reset their layout. They do not collapse, and can overlap the bottom HUD. Layouts last for the application session. The footer shows the game calendar (real UTC plus 400 years), connection status, jitter-buffer depth and display FPS; its timestamp tooltip includes simulation T+ time.

The location indicator names the subscribed system and nearby gravitational reference, shows altitude, and displays the remaining autopilot orders with stage ETAs. Docked ships get a hangar view, a selector for controlled ships at that station, and an Undock button. Changing ships updates the focused view and instrument subscriptions.

Overview combines sensor contacts, public beacons and orrery bodies. Planets come from celestial definitions, never sensor detections. Rows support sorting, text search and All/Ships/Celestials filters, and show distance and relative speed at the displayed simulation time. Contact and HUD colors use friendly, neutral, hostile or unknown standings derived from advertised IFF and the observer's relationship hierarchy. Personal standing overrides neither grant permissions nor change NPC orders.

Click a row or HUD marker to select it. Selected Item offers Align, Approach, Keep range and Stop guidance, plus beacon Jump/Dock actions where applicable. Mark/Unmark and Fire/Hold fire are separate weapon controls; selecting an object or issuing navigation does not start firing. Look at, including an Overview double-click, changes camera focus only when the object is eligible; remote ships require optical visibility and must be within 100 km. Right-drag orbits the camera with smoothing, scrolling zooms, and Escape returns to the controlled ship. With autopilot off, double-clicking unobstructed space aligns the ship along the camera ray while preserving throttle. This is disabled while docked or in slip transit. O toggles trajectories and +/− adjusts exposure.

Gate Network provides a searchable, pannable map with sovereignty colors and route highlights. **Plan destination** requests a preview from the public server routing service; **Add waypoint** submits the remaining queue plus the new destination. The preview shows the returned strategic commands, estimated durations and fuel requirements, including exhaustion warnings. **Engage route** commits that plan. The ship's flight computer generates local maneuvers during execution. Preview guards prevent replacing a newer queue or commanding a different ship or authority; ordinary movement and map updates do not invalidate it. Navigation shows guidance telemetry and the current queue, with command removal/reordering and pause/resume controls. The scene HUD also marks queued waypoints and connects them with dotted lines.

Inventory separates movable cargo stacks from installed consumable tanks. Cargo shows counts and reservations, with quantity selection and drag/drop between authorized, colocated inventories. Consumables show only installed storage, provide refill controls using colocated cargo, and include IFF and dock-service controls. Industry lists authorized facilities for remote inspection and operation, including capabilities, power, recipes, jobs and ship construction. Society & Ownership exposes affiliations, personal standings, asset permissions, authorized gas-account balances and searchable organization histories, objectives and relationships. Local Chat follows the focused controlled ship's 500 AU channel, with timestamps, advertised affiliation colors and a virtualized scrollback.

The bottom HUD shows pooled propulsion reserves, battery, reactor or charge fuel, shield reserve, delivered thrust and torque, power, hull integrity, stored heat and shield temperature/coverage. Its CPU meter reports the last tick's gas consumption against the physical computer budget, with distinct suspended, insufficient-gas, boot and fault states. Clicking the thrust meter sets throttle when manual control is available; holding Shift raises it and Ctrl lowers it. The display continues to show delivered forces under autopilot.

[shell.rs](../crates/toy-sim-client/src/ui/shell.rs) builds a view model from ECS presentation components and turns UI interactions into outgoing intents. Commands carry the current authority revision; the UI does not mutate authoritative ship state. Feedback distinguishes pending, accepted and rejected requests. Acceptance does not mean a maneuver has completed. A world reset clears selection, subscriptions, previews and feedback before subscribing again.

Initial focus uses the per-session `ShipTelemetry.can_control` permission flag and excludes destroyed hulls and ships stored in wrecks. The smallest UUID is only the tie-breaker among eligible living ships. Read-only access to Neris Anchorage does not make it the player's controlled ship. Explicit later inspection can keep a station focused. Under the 64-ship telemetry cap, explicitly subscribed objects retain priority, followed by living controllable ships and then other observable assets.

## Tests

| Location | Covers |
| --- | --- |
| `toy-sim-protocol` unit tests | Round trips, skipped optional sections, rejected required sections, truncation at every length, oversized headers, non-finite input, positive clock rates and debug capabilities, rejection of version 1 and of frames without the presentation section, invalid combat payloads, optical observations with missing views or contacts, duplicate optical IDs, anonymous optical observations and non-finite photometry, numeric limits of debug and flight commands |
| `toy-sim-net` unit tests | Compressed stream history across flushes, clean shutdown, rejection of a wrong server pin and a wrong account key, replayed records, epoch rekeying |
| `toy-sim-intel` unit tests | Measurements, noise stability, metered queries and cursors, snapshots |
| `toy-sim-spatial` unit tests | Brute-force agreement for spatial and brightness queries, large signed coordinates, boundary cases, updates/deletion, cursor budgets and invalidation |
| [session/optical.rs](../crates/toy-sim-server/src/sim/session/optical.rs) tests | Focused-vantage visibility, anonymous objects, remote radio tracks, occlusion, per-view budgets and optical IDs |
| [session.rs](../crates/toy-sim-server/src/sim/session.rs) tests | A group key grants views without ship control, a control change rejects old-revision commands without rewriting IFF, a non-debug account cannot change the clock, repeated command IDs are idempotent and replayed frames are rejected |
| [displays.rs](../crates/toy-sim-server/src/sim/displays.rs) tests | Subscribers share one instance released 10 ticks after the last viewer, authority and power changes revoke frames and queued input, every ABI input kind is forwarded |
| [services.rs](../crates/toy-sim-server/src/sim/services.rs) tests | Scans exclude celestials and use stable opaque ship handles, handles differ between groups and after expiry and stay bounded, program and display cursors are separate, slip aperture checks, beacon pages hide inaccessible bays, celestial references resolve without active bodies |
| [travel.rs](../crates/toy-sim-server/src/sim/travel.rs) tests | Docking and undocking motion, nested inventory surviving host destruction, capture not unlocking a private bay, blocked slip arrival and retry, slip energy and cancellation, inactive bodies blocking slip arrival, debug recovery rules, simultaneous arrivals |
| [router_tests.rs](../crates/toy-sim-server/src/sim/travel/router_tests.rs) | `travel_order_runs_in_stock_wasm_and_brakes_at_destination` and other travel orders flown by the stock firmware |
| [routing](../crates/toy-sim-server/src/sim/routing) tests | Public server route search and full queue expansion |
| [tests/network.rs](../crates/toy-sim-server/tests/network.rs) | Starts the real `toy-sim-server` binary with a ready file. A wrong server key and a wrong account key fail to connect. One account receives telemetry, a view with tracks, a stock MFD frame with at least five primitives and an appearance asset. A second account joins the first ship's group, sees its track and cannot command it. Reset keeps the connection usable, discards delayed inputs for the previous world and accepts a new view subscription. The server shuts down cleanly when standard input closes. |
| `toy-sim-client` tests | Buffer ordering, reserve refill, positive rate changes and repeated timestamps, ordered publications during catch-up, command ordering and backpressure, concurrent asset transfers, shared Bevy asset handles, explicit retry and unloading; UI tests cover fixed scheduling, spatial lifetimes, celestial loading, orbital projection, effects and selection |

```sh
cargo test -p toy-sim-protocol -p toy-sim-net -p toy-sim-intel -p toy-sim-server -p toy-sim-client -p toy-sim-example-controller
```

Enable `toy-sim-client/ui` to run the client ECS, asset-pipeline and rendering-math tests. These tests do not launch a graphics window; visual verification uses `toy-sim-debug`.

## Spatial CPU benchmarks

```sh
cargo run --release -p toy-sim-spatial --example benchmark
```

The standalone benchmark builds 100,000 entries in each of two deterministic distributions: a sparse volume and 100 dense clusters. It measures brightness queries, a 500 AU radius query, nearby geometry, segment candidates, and 10,000 position/luminosity updates. Output includes build time, process RSS growth on Linux, occupied cells, brightness buckets, time per query, visited cells, tested candidates and returned hits. The 500 AU case measures the spatial primitive; it does not imply that local-chat routing is already implemented.

These are CPU index measurements. They exclude simulation, illumination updates, per-session serialization, network traffic and client rendering. Full observer batches and collision workloads must be measured separately; build contention and the number of returned objects materially affect timings. Measured sprint results and their conditions are recorded in [DEVLOG.md](../DEVLOG.md).

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

Protocol version 7 adds the public navigation catalogue and integer cargo/consumable telemetry. The client replicates moving beacons into ECS pose samples so their markers interpolate on the same clock as ships. Gate topology supports a subway-style map and shortest-hop route previews; selected destinations become `TravelTo` requests for the server to compare gate and slip routes. Queue controls can remove, reorder, pause and resume commands. The location HUD lists all remaining stages without a scroll area, with estimated cumulative completion times. Amber numbered markers identify spatial waypoints, with dotted lines connecting them in queue order. Absolute and relative coordinates, slip arrivals, gates and docking destinations are supported. Offscreen destinations use edge markers; distances of at least 0.1 light-years use ly. Ship camera focus is limited to visible nearby contacts within 100 km.

Docked and transiting ships have private meshes independent of sensor contacts. Docking opens a client-rendered hangar with orbit-camera controls; slip transit uses a procedural streak tunnel. Neither view reactivates ship physics. Gates use a sparse GLB frame, an animated spherical distortion material and a shadow-casting point light. See [stations-navigation.md](stations-navigation.md) for the catalogue, controls and verification commands.

Protocol version 8 changes battery charge and capacity fields to `u64` joules, including ship telemetry, device readings, and weapon-instrument battery readings. Server-generated snapshots are validated during encoding; an invalid snapshot panics and aborts the server process. Network I/O failures remain connection errors.

Protocol version 9 removes input acknowledgements and the outgoing event watermark. Complete state frames use a bounded server FIFO and an unbounded client receive queue. TCP carries each queued event and result once; playback catches up by consuming two snapshots per fixed tick.

Protocol 10 removes direct manual flight input and separates target marking from the firing latch. `WeaponsInstrument.firing` reports that latch independently of `target`. The client sends directional Align orders with a finite galactic direction and zero stand-off; the stock firmware clears thrust when aligning. Server and client must be upgraded together.


## Diagnosing stalls

The client bottom bar shows smoothed display FPS, queued jitter-buffer ticks, and buffering or catch-up status. For detailed console output, start the client and server with:

```sh
RUST_LOG=info,toy_sim_client::diagnostics=debug,toy_sim_server::timing=debug
```

Client warnings identify display frames lasting at least 250 ms, snapshot reception gaps of at least 500 ms, and jitter-buffer underruns. Five-second debug summaries include queue depths, buffered simulation time, maximum frame duration, and received tick and sequence numbers. Reception and display logging come from separate tasks, helping distinguish missing server updates from a stalled renderer.

Server warnings identify simulation ticks or display/snapshot publication taking at least 100 ms. Tick records include collision index, query and solve times; candidate, query, impact and contact-review counts; and firmware work totals with the slowest ship's preparation, callback and gas usage. Firmware totals sum work across parallel workers and can exceed elapsed tick time. Debug logging emits these records for ordinary ticks too. The server's optional `profile` feature enables Bevy Chrome tracing for deeper investigation.

## Bottom ship console

A dedicated egui ECS system draws the bottom console above the timing strip. Floating windows may overlap this console; it does not reserve desktop workspace. Consumable reserves appear on the left, with propulsion and thermal status on the right. Cargo remains in the inventory window. Installed equipment identifies propellants, reactor fuel and pulse charges.

The thrust fill uses server-reported actuator force projected onto the ship control frame, divided by installed forward thrust capacity. Torque meters use installed capacity in each direction. Gravity and collision forces are excluded. The throttle marker comes from the flight computer; a separate pending marker shows requests awaiting confirmation. Click or drag the gauge, or hold Shift/Control to increase/decrease throttle by 25 percentage points per second. Text entry suppresses these shortcuts. Autopilot locks manual controls; navigation actions enable it, while queue edits preserve its state.

Protocol 16 accompanies ABI 23 and adds planning preferences and propulsion fuel estimates to the shared travel order queue. That release used ten connected systems, a fitted slipdrive part and the Peregrine laser/micropulse patrol; the current inhabited map expands the catalogue to 3,000 systems. Server and client must be rebuilt together.

Protocol 21 publishes per-tick computer gas usage and its configured positive tick limit. CPU percentage is the gas spent on guest execution and host services divided by that limit; usage cannot exceed the limit. A running computer reports `Ready`, `Suspended` or `WaitingForGas`. The console labels a suspended continuation `SUSPENDED` and an insufficient owner-account balance `NO GAS`; a positive balance can still be too small for the next indivisible operation. Booting, unpowered, paused and faulted computers remain distinct states. Gas suspension preserves the running callback and does not initiate a reboot.

`SocietySnapshot.gas_accounts` carries integer available, reserved and spent amounts, each keyed by its owning `Principal`. Only the caller's player account and organizations or sovereignties the caller administers are included. Ordinary membership does not disclose a group's balance. The Society window displays those authorized accounts; individual computer telemetry does not duplicate the shared balance. Billing follows actual asset ownership, independently of IFF or delegated control.

A computer reset clears pending requests, instruments, marks, firing state and forecasts. The server also clears the autopilot toggle, orders, ETA, staged world actions, slip preparation and docking reservations, and advances the travel revision. Successful reboot starts with idle navigation. Commands explicitly submitted after the reset may be queued during startup. Fault messages remain visible until boot succeeds. The countdown pauses without computer power and follows simulation time on the client.

Protocol 17 adds the society snapshot and ownership commands. Sovereignties,
organizations, player affiliations, private personal standings, and authorized
asset permissions use stable UUIDs. The Society window exposes the hierarchy,
organization membership and officer management, standing overrides, asset grants,
and transfers between principals the player administers. Ownership and IFF are
separate. Delegated flight control does not grant configuration or access
management rights.

Private docking uses the ship's current owning principal and Dock permission.
Automatic station resupply requires ownership or a TransferCargo grant. Historical
controller assignment does not preserve either right after a transfer. Known
information-group secrets remain bearer credentials until the group changes;
revoking an asset grant does not erase a secret somebody already learned.

Protocol 18 adds a signed millisecond calendar timestamp. It represents real UTC
plus 146097 days, exactly 400 Gregorian years. The calendar advances independently
of simulation speed. The client samples it at network reception, outside
the presentation jitter buffer. The bottom strip displays UTC date and time; its
hover text retains simulation T+. See [Persistence](persistence.md) for saved
worlds, debug identities, checkpoint configuration and recovery behavior.


### Local chat

Local chat reaches physical ships within an inclusive 500 AU sphere around the
sender. The server resolves both positions; a docked ship uses its host's
position, and a destroyed carrier retained only for missile computing has no
transmitter. Delivery is instantaneous within the simulation publication cycle.
It uses the shared spatial hash's geometric range query, independently of light,
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
