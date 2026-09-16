# Streaming Zstandard window measurements

This experiment extends the existing benchmark with complete postcard snapshots,
Zstandard windows from 64 KiB to 16 MiB, retained encoder allocations, and parallel
compression throughput. It does not change the game protocol.

## Findings

A larger window helps when it reaches useful repeated content, but the compression
level must also find that content. In these workloads, 256 KiB was enough for a
1,024-object battle. An 8,192-object battle benefited from level 3 with a 2–4 MiB
window. Windows of 8 and 16 MiB gave almost identical bandwidth at greater memory
cost. Long-distance matching added CPU and memory without improving these results.

These are measurements of deterministic synthetic observations on a Ryzen 9 5900XT,
not captured player traffic. Absolute capacity on a production server remains to
be measured.

| Battle objects | Codec | Main traffic at 10 Hz | Serialization + compression per snapshot | Retained encoder memory |
| --- | --- | ---: | ---: | ---: |
| 1,024 | LZ4 linked | 7.06 Mbit/s | 0.219 ms | 0.52 MiB |
| 1,024 | Zstd 1, 256 KiB | 3.95 Mbit/s | 0.211 ms | 1.18 MiB |
| 1,024 | Zstd 1, 1 MiB | 3.95 Mbit/s | 0.220 ms | 1.93 MiB |
| 1,024 | Zstd 3, 4 MiB | 3.95 Mbit/s | 0.446 ms | 5.62 MiB |
| 1,024 | Zstd 3, 16 MiB | 3.95 Mbit/s | 0.448 ms | 17.62 MiB |
| 8,192 | LZ4 linked | 56.37 Mbit/s | 1.691 ms | 2.27 MiB |
| 8,192 | Zstd 1, 1 MiB | 50.90 Mbit/s | 2.337 ms | 2.81 MiB |
| 8,192 | Zstd 3, 1 MiB | 51.36 Mbit/s | 5.227 ms | 3.49 MiB |
| 8,192 | Zstd 3, 2 MiB | 31.62 Mbit/s | 4.056 ms | 4.49 MiB |
| 8,192 | Zstd 3, 4 MiB | 31.61 Mbit/s | 3.763 ms | 6.49 MiB |
| 8,192 | Zstd 3, 16 MiB | 31.61 Mbit/s | 3.470 ms | 18.49 MiB |

The postcard snapshots average 17.5 KiB for 128 coasting objects, 138.4 KiB for
1,024 battle objects, and 1,104.4 KiB for 8,192 battle objects. They contain the same
numeric precision and fields as the original benchmark: small sequential u64 IDs,
names, design references, f64 positions/velocities/orientations, thermal and weapon
values, a small instrument display and timed events. Production identifiers,
quantization, richer screens, different ordering and view composition can change
the results. These observations contain no state deltas.

The 1,024-object workload with an additional 256 KiB incompressible asset burst
per second used 4.19 Mbit/s for the main stream with Zstd 1 and 256 KiB history,
versus 3.96 Mbit/s with 1 MiB history. Asset bytes add about 2.10 Mbit/s to both.
This supports leaving some history room for multiplexed bulk traffic. The test
places a burst between observations; actual picomux scheduling is not reproduced.

![Bandwidth and CPU versus history window](results/windows.png)

## Server budgets

At ten snapshots per second, a 1,024-object battle using Zstd 1 with 1 MiB history
costs approximately 2.2 core-equivalents and 1.89 GiB of encoder memory per 1,000
clients. Ten thousand such clients imply about 22 core-equivalents, 18.9 GiB of
encoder memory and 39.5 Gbit/s of payload traffic. This arithmetic extrapolates
single-core timing; it is not a full server capacity measurement.

For 1,000 clients each viewing the 8,192-object battle with Zstd 3 and 4 MiB history,
the corresponding estimates are 37.6 core-equivalents, 6.34 GiB of encoder memory,
and 31.6 Gbit/s. A 2 MiB window uses about 4.39 GiB but took
40.6 core-equivalents in the paired timing trials. Increasing the window to 16 MiB raises encoder memory to about
18.1 GiB with essentially no bandwidth benefit.

These figures exclude physics, sensors, observation extraction, authentication,
encryption, mux framing, TCP buffers, receive-side decompressors, input parsing,
queued snapshots, client jitter buffers and process overhead. The serializer is
measured on already constructed observations. An outgoing codec history is private
to a connection; the compressor cannot generally share it with other observers.

Memory includes the native encoder context and reusable output buffers. Those
buffers grow to fit a compressed snapshot, so the large battle needs more than
the 1,024-object workload even at the same compression settings. A sender that
streams directly into a bounded output queue could use less buffering. Smaller
upstream windows can avoid paying the downstream history cost for command traffic.

## Parallel compression

Each worker has its own encoder and a separate allocation containing its snapshots.
The streams use the same synthetic values, but their input buffers do not share
cache lines. Measurements use one hardware thread per physical core. Serialization
is excluded from this experiment.

| Codec, 1,024 battle objects | One core, snapshots/s | 16 cores, snapshots/s | Speedup |
| --- | ---: | ---: | ---: |
| LZ4 linked | 5,758 | 83,202 | 14.45× |
| Zstd 1, 1 MiB | 5,698 | 80,006 | 14.04× |
| Zstd 3, 1 MiB | 2,388 | 31,577 | 13.22× |
| Zstd 3, 8 MiB | 2,447 | 30,913 | 12.63× |

Zstd 1 therefore supports about 8,000 of these 10 Hz streams on 16 cores in this
compression-only benchmark. This excludes serialization and the rest of the server.
It also keeps only one encoder hot on each worker; scheduling thousands of encoder
contexts may have worse cache behavior. This experiment does not establish
128-core scaling.

## Method

- Rust release build; postcard 1.1.3; zstd 0.13.3 with bundled Zstandard 1.5.7;
  lz4 1.28.1 with bundled LZ4 1.10.0. Versions are pinned in Cargo.lock.
- Streaming levels −1, 1 and 3, windows 64 KiB, 256 KiB, 1 MiB, 2 MiB, 4 MiB, 8 MiB and
  16 MiB; additional level-3 runs enable long-distance matching at 8 and 16 MiB.
- Each method uses a single compression thread and a fresh context per trial.
  Every observation and asset packet is flushed without ending the stream.
- Each scenario warms with at least approximately 32 MiB of unique observations,
  then measures 200 further ticks. There are three repetitions per method and
  five scenarios: 360 trials. The 2 MiB setting was added in a later pass.
  Another 18 trials paired 2 and 4 MiB on the large battle, bringing the recorded
  total to 378. Those combinations use medians across six trials. Method order
  rotates between repetitions. Timing varies between passes on this shared
  workstation; bandwidth is stable.
- Compression uses Linux thread CPU time. Per-call wall-clock p95 is also recorded.
  Postcard serialization is timed separately with a wall clock over the generated
  corpus and reused across codec trials. Its result is not a separate repeated
  CPU measurement. Dataset construction, validation, and retaining compressed
  data for verification are outside timed compressor calls.
- Every postcard frame is decoded and compared with its original observation.
  Every compressed trial is decoded byte-for-byte before ending the stream.
  A separate test verifies each small flush is immediately decodable.
- Native context sizes come from `ZSTD_sizeof_CCtx`, plus output-vector capacity.
  A second experiment holds sixteen warmed encoders and measures the change in
  glibc `mallinfo2` allocated heap and mapped bytes. The two approaches agree
  closely. LZ4 has no corresponding exported size API, so its size comes from the
  allocator measurement. RSS, fragmentation, and overall process memory are
  different quantities.
- The parallel experiment runs three repetitions for 1, 4, 8 and 16 physical
  cores, with 2,400 measured frames per worker following warmup. Its 200-frame
  corpus is reused; one complete corpus exceeds every tested parallel window.
- Other processes were running on this workstation. See the machine record for
  CPU and toolchain details. Results should be checked against production traffic
  before selecting final budgets.

## Reproduce

Run from the repository root. `taskset` and allocation measurements require Linux;
`mallinfo2` requires glibc. The plotting script needs matplotlib.

```sh
cargo test --manifest-path tools/net-compression-bench/Cargo.toml --offline
cargo build --manifest-path tools/net-compression-bench/Cargo.toml --bin windows --release --offline

taskset -c 2 tools/net-compression-bench/target/release/windows \
  > tools/net-compression-bench/results/windows-measurements.csv
taskset -c 2 tools/net-compression-bench/target/release/windows memory \
  > tools/net-compression-bench/results/windows-memory.csv
taskset -c 2 tools/net-compression-bench/target/release/windows memory 8192 \
  > tools/net-compression-bench/results/windows-memory-large.csv
tools/net-compression-bench/target/release/windows parallel \
  > tools/net-compression-bench/results/windows-parallel.csv
MPLCONFIGDIR=/tmp/toy-sim-matplotlib python3 tools/net-compression-bench/analyze_windows.py
```

Pass a scenario name, such as `battle8192`, to run one workload. `WINDOW_REPEATS`
changes the default three trials. `WINDOW_CODEC_FILTER=w21,w22` selects specific
codec label fragments for follow-up measurements. Parallel affinity assumes CPUs 0–15 are distinct
physical cores, as verified on this workstation; adapt that mapping elsewhere.

Artifacts:

- [Raw trials](results/windows-measurements.csv)
- [Medians and per-client budget calculations](results/windows-summary.csv)
- [Encoder allocations](results/windows-memory.csv)
- [Large-view encoder allocations](results/windows-memory-large.csv)
- [Parallel measurements](results/windows-parallel.csv)
- [Machine record](results/windows-machine.json)
- [Plot as SVG](results/windows.svg)

`encoder_bytes = 0` in raw LZ4 trial rows means the codec has no exported allocation
counter; consult the allocation CSV for its measured memory.
