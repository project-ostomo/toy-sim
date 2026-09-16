# State compression benchmark

This standalone Rust program compares lossless encoding of synthetic 10 Hz game
observations. It does not modify the simulator or establish a network protocol.
The published crates are pinned in Cargo.lock.

Run from the repository root:

```sh
cargo test --manifest-path tools/net-compression-bench/Cargo.toml --offline
cargo build --manifest-path tools/net-compression-bench/Cargo.toml --release --offline
taskset -c 2 tools/net-compression-bench/target/release/net-compression-bench \
  > tools/net-compression-bench/results/measurements.csv
MPLCONFIGDIR=/tmp/toy-sim-matplotlib python3 tools/net-compression-bench/analyze.py
```

Omit `--offline` when the dependencies are not cached. `taskset` is optional on
systems without Linux CPU affinity. Set `BENCH_FRAMES` and `BENCH_REPEATS` to change
the defaults of 160 frames and three repetitions. Use at least 40 frames for a
useful measurement. The analysis script needs matplotlib.

## Workloads

Each corpus uses a fixed seed. Frames contain explicit simulation ticks and
nanosecond timestamps, view revision, focused object, eight own-ship measurements,
sixteen instrument strings, and an ordered list of visible objects. Each object
contains an ID, name, design reference, position, velocity, orientation, heat,
coolant reserve, throttle, turret angles, and flags. Immutable assets are represented
by design references rather than repeated model bytes.

| Scenario | Objects | Motion and changes |
| --- | ---: | --- |
| coast128 | 128 | Constant velocity; positions change every tick |
| coast1024 | 1,024 | Constant velocity; positions change every tick |
| battle1024 | 1,024 | One third maneuver and change thermal/weapon state every tick |
| battle8192 | 8,192 | Same battle behavior at larger scale |
| battle1024-assets | 1,024 | Battle plus 256 KiB of independent random asset bytes each second |

Battles replace one percent of objects each second and include four timed events
per frame. Asset bytes approximate content that has already been compressed. They
pass through the same compressor between game frames, exercising shared history.
The benchmark does not simulate picomux packet interleaving, flow control, TCP
congestion, encryption, socket latency, or rendering. All numbers preserve the
same values and precision across formats.

## Formats and compressors

**Snapshot** is the normal ciborium serialization of named Rust structs as CBOR
maps. **Compact** sends object records as CBOR arrays with fixed field positions.
Both send every object and every field in every frame.

**Delta** retains the previous observation. New objects and changed object metadata
are complete CBOR records. Removed IDs are listed explicitly. Existing objects use
ascending ID differences, a 16-bit changed-field mask, and unsigned variable-length
integers containing the XOR of old and new numeric bits. It preserves floating-point
bits exactly. The patch byte string sits inside a CBOR message with the complete
frame header. No motion prediction, quantization, periodic keyframes, or lossy
omissions are used. The first frame establishes the baseline; a reconnect needs a
fresh baseline. This is an experimental codec, with a decoder for generated valid
inputs, not a hardened protocol parser.

The codecs are raw bytes, linked-block LZ4, streaming Zstandard levels 1 and 3
with 1 MiB or 8 MiB history limits, and independent Zstandard level 1 frames.
`w20` and `w23` denote window-log settings. Larger windows permit matches farther
back but do not guarantee that a given compression level will find them.
Every packet is flushed immediately; continuous codecs retain history. Independent
Zstandard starts a new context for each packet. Compression uses one thread.

## Measurement and validation

Three repetitions rotate method order on a Ryzen 9 5900XT, pinned to logical CPU 2.
Other workstation processes remain active. Machine and toolchain details are in
[results/machine.json](results/machine.json). These are local measurements rather
than capacity predictions for a production server.

Each trial starts with a fresh encoder. The first 20 ticks warm its history and
are excluded from steady-state means. Initial frame size is recorded separately.
Timings include:

- serialization or delta construction;
- compression and flush;
- decompression;
- CBOR parsing or delta reconstruction into a complete owned frame.

Full-frame delta reconstruction includes maintaining a BTreeMap and cloning its
objects into the output vector. Equality checking, synthetic input generation,
and output-frame destruction are outside measured stages. This measures record
construction and parsing rather than ECS application or steady-state allocator
behavior of a tuned game client. Codec setup and final stream termination are
excluded from timing and steady-state sizes. The raw codec includes its output
buffer copy in the compression column.

Every frame and interleaved asset is checked after decompression, and every decoded
observation is compared with the source. Tests separately verify delivery of small
flushed packets before stream termination, and exact delta reconstruction across
removals, metadata changes, and numeric bit changes.

`summary.csv` reports medians across the three trial means. `server_p95_us` is the
median of the three per-trial 95th percentiles. Server cost is serialization plus
compression; client cost is decompression plus parsing/reconstruction. Main-stream
bandwidth excludes asset payloads, whose compressed bytes are recorded separately.
For mixed traffic, decoder buffering can move some work across packet boundaries;
the CPU columns are not a measurement of total asset-transfer cost.

## Interpretation limits

The floating-point workload deliberately changes exact position bits every tick.
Real observations may have more static objects, different motion, richer screen
content, different sensor accuracy, or more churn. Field selection and quantization
could change the ranking. Delta encoding also avoids repeated static metadata;
that benefit must not be attributed solely to its XOR technique. Compact snapshots
provide a control for some of the schema overhead.

Codec memory consumption and latency under packet loss are not measured here.
An 8 MiB history setting also entails per-connection memory costs beyond bandwidth
and CPU; actual encoder/decoder allocations should be measured before deployment.

See [results/summary.csv](results/summary.csv) for all combinations and
[results/bandwidth.svg](results/bandwidth.svg) for the bandwidth comparison.

## Recorded results

The September 16, 2026 run completed 180 trials (five workloads, twelve methods,
three repetitions). All 28,800 observations reconstructed successfully. Bandwidth
is decimal Mbit/s at ten observations per second; timings are milliseconds per
observation and include serialization/compression or decompression/reconstruction.

| Encoding | 1,024 battle objects, Mbit/s | Server ms | Client ms | 8,192 battle objects, Mbit/s |
| --- | ---: | ---: | ---: | ---: |
| CBOR maps + LZ4 | 8.57 | 0.620 | 1.030 | 68.37 |
| CBOR maps + Zstd 1, 8 MiB | 4.46 | 0.563 | 1.009 | 52.29 |
| CBOR maps + Zstd 3, 8 MiB | 4.11 | 0.753 | 1.007 | 38.09 |
| CBOR arrays + Zstd 3, 8 MiB | 3.85 | 0.585 | 0.714 | 35.17 |
| Delta, uncompressed | 3.64 | 0.125 | 0.136 | 28.84 |
| Delta + Zstd 1, 8 MiB | 3.34 | 0.165 | 0.167 | 26.64 |

For 1,024 coasting objects, LZ4 used 6.69 Mbit/s, CBOR maps with Zstd 3 used
1.90 Mbit/s, and deltas with Zstd 1 used 1.56 Mbit/s. For 128 coasting objects,
LZ4 used 0.442 Mbit/s and CBOR maps with Zstd 1 used 0.239 Mbit/s.

Window size alone did not solve the large-scene case: at 8,192 battle objects,
Zstd 1 used about 52.29 Mbit/s with either window. Zstd 3 improved from 53.06
Mbit/s with a 1 MiB window to 38.09 Mbit/s with an 8 MiB window. The compressor's
search strategy also matters. Interleaving the asset burst each second changed
Zstd 3 main-stream bandwidth only slightly (4.105 to 4.107 Mbit/s); asset bytes
add about 2.10 Mbit/s and are excluded from those figures. This workload is only
one asset size and cadence.

Streaming Zstandard is a useful replacement for LZ4 when retaining complete
snapshots. The experimental delta representation saves more CPU and bandwidth,
including against compact CBOR arrays. It also adds baseline state, field-layout
rules, and reset semantics. A production design can keep complete observations
at the UI boundary while reconstructing deltas before the jitter buffer. Coalescing
unsent observations must happen before constructing a delta against the last
transmitted baseline; skipped presentation frames must still be decoded.

## Postcard and encoder memory

The [streaming window study](WINDOWS.md) adds postcard, a wider Zstandard window
sweep, native encoder allocation measurements, and parallel throughput tests.
Its results and source are separate from the original CBOR/delta comparison.
