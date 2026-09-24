# Current frame and BVH baseline — September 23, 2026

Measured the current working tree on an AMD Ryzen 5 3600XT (6 cores / 12
threads), using rustc 1.97.1 and the release profile. This machine differs from
the Ryzen 9 machine used for earlier reports; do not interpret differences from
those reports as code regressions or improvements. HEAD was
`81a0d5a4827596e6135d537cba1dcde2c856b34d`, with pre-existing uncommitted changes.
No implementation changes were made for this measurement.

The existing `production_tick_and_publication` benchmark runs the production
simulation, navigation publication, displays, session snapshots, serialization,
and event pruning. Each run has 70 warmup ticks and 300 measured ticks. Sparse
scenes have three serial repetitions per player count. Build diagnostics, dense
contacts, and synthetic catalogue rays each have one separate repetition.
No other benchmark or build was run concurrently by this session.

This is a server simulation/client-publication frame, excluding client rendering,
network I/O, compression, and optical subscriptions. Diagnostic scopes and
counters are enabled. Startup ticks are excluded. The host was not isolated
from other applications. Hardware CPU sampling was unavailable because
`perf_event_paranoid=3`; these are elapsed-time measurements, not cycle profiles.

## Frame measurements

Means below average the three run means, each based on 300 ticks.

| Milliseconds per tick | 1 player | 16 players |
|---|---:|---:|
| Simulation | 54.830 | 55.013 |
| Publication | 6.426 | 12.322 |
| Simulation + publication | **61.256** | **67.335** |
| Spatial collection including clearing | 19.399 | 19.070 |
| ECS spatial collection | 5.242 | 5.218 |
| Prepare spatial records | 2.180 | 2.091 |
| Replace records and build BVH | 11.864 | 11.652 |
| Collision discovery | 8.037 | 8.059 |
| Collision solver and isolated-body handling | 0.382 | 0.388 |
| Sensor radius queries, simulation + publication | 0.023 | 0.153 |
| Sensor segment queries, simulation + publication | 0.014 | 1.742 |
| Celestial motion (`move_orrery`) | 11.818 | 11.831 |

Scopes are nested and simulation systems can overlap. Do not add all rows to
obtain frame time. In particular, ECS collection, record preparation, and
replacement are inside spatial collection. Query scopes include wrapper work,
allocation, record lookup, and exact geometry filtering as well as BVH traversal.
Collision discovery additionally includes body lookup, candidate filtering, and
group formation; its 8.059 ms is not a pure BVH query measurement.

| Configuration | Run means, simulation + publication (ms) | Frame p50 by run (ms) | Frame p95 by run (ms) |
|---|---|---|---|
| 1 player | 60.674, 61.131, 61.963 | 60.020, 60.380, 61.182 | 66.425, 68.134, 69.101 |
| 16 players | 65.761, 68.276, 67.969 | 64.583, 67.680, 67.152 | 73.641, 76.228, 74.743 |

These percentiles belong to individual repetitions, not a pooled distribution.

## BVH construction

The active scene contains 46,908 records for 16 players, including 43,891
celestials. Counters show exactly one collection per tick. The static catalogue
has 1,001,760 entries and is built during setup.

The existing `OSG_BVH_PROFILE=1` diagnostics produced exactly 370 active-scene
builds. Dropping the first 70 gives the same 300-tick measurement window as the
frame benchmark. This separate instrumented run reports:

| Builder phase | Mean ms/build | Attribution |
|---|---:|---|
| Collect payloads | 5.412 | Includes the caller's iterator: record-map insertion and AABB creation, plus payload allocation/collection |
| Prepare partition keys | 0.739 | Validation, integer centers, key storage, node capacity and leaf-index allocation |
| Partition | 2.814 | Recursive widest-axis selection and median partition; Rayon branches at 8,192 entries |
| Assemble nodes | 3.034 | Serial recursive node construction and aggregate bounds |
| Free temporary payloads/keys | 0.005 | Explicit temporary-vector destruction |
| Builder total | **12.003** | Elapsed time, including parallel partition work |
| Enclosing `spatial.replace` | **12.402** | Also includes replacement/destruction around the builder and diagnostic output |

The payload phase is timed inside `LuminosityBvh::build`, but much of its work
is supplied by `GalacticIndex::replace`. It must not be described as time spent
only constructing tree nodes. The partition and assembly phases together take
5.847 ms. This diagnostic run is separate from the 11.652 ms baseline replacement
mean and is not a subtraction-based estimate of baseline internal phases.

Active-tree node capacity occupies 12,008,320 bytes (about 11.45 MiB), excluding
record maps, temporary arrays, other spatial metadata, and allocator overhead.
The setup catalogue build reported 305.852 ms and 256,450,432 node bytes
(about 244.57 MiB). That is one cold setup sample, not a steady-state build mean
or process memory measurement.

## Query coverage and attribution

Production uses `osg-spatial::GalacticIndex` with `LuminosityBvh` for the active
scene and catalogue. Its queries use `LuminosityBvh::try_visit`; radius and
segment visitors do additional exact geometry checks using the record map.
`try_visit` allocates a traversal stack and orders children using distance to
the query origin. These are code-inspection observations, not isolated timings.

The 16-player sparse fixture records, per tick:

- 3,017 collision bodies, one two-body group, and zero contact pairs.
- 17 sensor scans, 289 range results, and 272 selected contacts/rays.
- 14,688 segment node probes: exactly 54 nodes per ray on average.
- 1,664 segment leaf candidates and 1,664 returned intersections, before caller
  exclusions such as the observer and target.

Sensor segment time is approximately 6.41 microseconds/ray including the wrapper
and exact filtering. The counters cover sensor rays, not all catalogue,
collision, lighting, route, or optical queries. Collision discovery issues a
radius query for each live body; its traversal counts are not exposed by this
fixture. Catalogue and route queries may occur within larger system scopes.
The present instrumentation cannot assign every such caller's time to the crate.

Additional existing fixtures passed:

| 16-player fixture | Frame p50 / p95 (ms) | Discovery mean (ms) | Solver mean (ms) |
|---|---:|---:|---:|
| Dense: 64 reset projectiles, 468 contact pairs/tick | 69.792 / 78.009 | 9.033 | 1.145 |
| Rays: four catalogue capture segments of length 1e17 m/tick | 68.259 / 76.649 | 8.091 | 0.385 |

The rays fixture includes its extra queries in total-frame percentiles, after
the publication timer ends. It has no dedicated ray timer or catalogue query
counters. Differences from sparse timings do not isolate their cost. Dense
contacts exercise a synthetic stationary cluster, not sustained combat.

## Standalone BVH measurements

Ran the existing Gaia Criterion suite filtered to `luminosity_bvh`, with its
default 30 samples, one-second warmup, and three-second measurement target.
All 12 visibility workloads passed their independent exhaustive-scan checks.
The table gives Criterion's central time estimates; confidence intervals and
outlier counts are retained in the raw log.

| Construction size | Time |
|---|---:|
| 10,000 | 1.653 ms |
| 100,000 | 17.624 ms |
| 1,000,000 | 262.080 ms |

| Observer | Magnitude 0 | Magnitude 2 | Magnitude 4 | Magnitude 6 |
|---|---:|---:|---:|---:|
| Earth | 5.060 µs | 33.035 µs | 402.050 µs | 5.817 ms |
| Star 104729 | 11.977 µs | 45.553 µs | 269.420 µs | 2.698 ms |
| Star 523645 | 2.667 µs | 13.908 µs | 129.150 µs | 1.467 ms |

Construction excludes source-file loading and final tree destruction, but
includes conversion of input records. This suite uses static Gaia point bounds
and the `visible_from` iterator. The production wrapper uses `try_visit`, with
different geometry and filtering. These results do not isolate production
radius or segment traversal. `SpatialService` and `LuminosityForest` are not
called by the measured frame path and were not included in this run.

## Optimization priorities

1. Investigate replacement payload materialization alongside tree construction:
   record-map rebuilding and AABB/payload creation account for 5.412 ms in the
   diagnostic build. Partitioning and serial node assembly account for another
   5.847 ms. Measure changes against the full replacement/frame workload.
2. Instrument collision radius queries separately from group formation before
   attributing the entire 8.059 ms discovery scope to BVH traversal. Candidate
   counts, node visits, and exact-filter costs would distinguish tree pruning
   from wrapper overhead.
3. Sensor rays are a smaller current target at 1.742 ms/tick. Range queries take
   only 0.153 ms. Optical subscriptions and active route workloads need their
   own fixtures before making claims about those paths.

Celestial motion costs about 11.831 ms per tick outside these measured spatial
scopes, so even a large BVH improvement will leave a substantial frame cost.

## Reproduction and raw data

```sh
OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=3 OSG_BENCH_FRAMES=300 \
  cargo test --release --offline -p osg-server --lib \
  sim::benchmark::production_tick_and_publication -- --ignored --nocapture
```

Use `OSG_BENCH_PLAYERS=1` for the smaller baseline. Set `OSG_BVH_PROFILE=1`
and `OSG_BENCH_REPEATS=1` for construction diagnostics. Separately use
`OSG_BENCH_SCENE=dense` or `OSG_BENCH_SCENE=rays`, with 16 players and one repeat,
for the additional fixtures. Run all workloads serially.

```sh
cargo bench --offline -p osg-spatial-bvh --bench gaia -- luminosity_bvh
```

Raw logs and source hashes are in [current-frame-2026-09-23](current-frame-2026-09-23/).
The hashes identify the BVH builder, integration wrapper, and benchmark source
used for these measurements; they do not describe every dependency or source file.
