# Production tick breakdown — September 23, 2026

Measured the current spatial hash / Rapier implementation in the production
application. Release build on an AMD Ryzen 9 5900XT, 16 cores / 32 threads.
Each configuration ran serially, with 70 warmup ticks and 300 measured ticks.
These are single-run measurements, not a comparison against the old BVH build.

The sparse fixture retains the normal 3,000 navigation installations and all 1,001,760
static catalogue entries. Player ships are spaced apart. The smallest hash cell
is 512 km. One session per player subscribes to instruments and one display at
10 Hz. Publication includes navigation, displays, session snapshots, postcard
serialization, and event pruning. It excludes network I/O, compression, and
optical view subscriptions. There is no sustained combat load in these runs.

| Mean time per tick | 1 player | 16 players |
|---|---:|---:|
| Simulation, wall clock | 70.16 ms | 105.79 ms |
| Spatial collection and synchronization | 33.21 ms | 33.00 ms |
| Sensor detection and observation publication | 14.33 ms | 49.52 ms |
| Collision pipeline, excluding initial spatial collection | 9.90 ms | 10.22 ms |
| Celestial motion | 3.30 ms | 3.30 ms |
| Celestial activation | 1.64 ms | 1.74 ms |
| Gravity | 2.21 ms | 2.21 ms |
| Aerodynamic environment and forces | 2.16 ms | 2.19 ms |
| Hardware, power, devices, cooling | 2.41 ms | 2.42 ms |
| Controller execution | 0.79 ms | 1.00 ms |
| Service index publication | 2.43 ms | 2.51 ms |
| Chat endpoint refresh | 1.50 ms | 1.48 ms |
| Industry | 0.18 ms | 0.19 ms |
| Travel advancement and order planning | 0.03 ms | 0.03 ms |
| Time outside measured simulation scopes | 1.41 ms | 1.43 ms |
| Publication after simulation | 7.70 ms | 34.22 ms |
| **Simulation + publication** | **77.86 ms** | **140.01 ms** |

System timers measure elapsed time with normal parallel scheduling. Rows within
the simulation overlap and must not be added together; the simulation total is
measured directly. Unscoped time uses the union of all scope intervals, so nested
timers are not counted twice. Full hardware timings are in the raw reports.

Simulation p50/p95: 69.57/75.70 ms for one player; 105.19/112.06 ms for sixteen.
The sixteen-player combined p50/p95 is 139.22/147.37 ms. The one-player raw
combined percentile includes diagnostic sample aggregation; use the independently
measured simulation and publication times above. The benchmark now captures the
combined time before that aggregation.

## Spatial work

The one-player spatial collection scope contains 24.50 ms in `finish_geometry`
and 8.55 ms in the surrounding collection loop, plus 0.17 ms clearing metadata.
Luminosity bounds take 1.12 ms, nested within `finish_geometry`. That function
synchronizes every active record and reconciles removed entities.

The general hash remains allocated and retains its million static records.
However, the ECS integration still collects all active records at the beginning
of the collision schedule and again at the end of the tick. The final hash has
1,048,653 records for one player and 1,048,668 for sixteen: about 46,900 active
records, including celestials. Thus the collection workload is substantially
larger than the 3,002 / 3,017 collision bodies.

## Collisions

| Mean time | 1 player | 16 players |
|---|---:|---:|
| Group discovery | 6.27 ms | 6.48 ms |
| Collision index synchronization | 1.53 ms | 1.50 ms |
| Group solving, isolated drift, and motion result handling | 0.285 ms | 0.280 ms |
| Remaining preparation, forces, damage, writeback, scheduling | 1.81 ms | 1.96 ms |

The one-player case has one cached group with two bodies and 3,000 isolated
bodies. The sixteen-player case has no collision groups and 3,017 isolated
bodies. Neither records contact pairs. The solver timer includes isolated
motion and bookkeeping, so it is not a measurement of pure Rapier time.

## Sensors and publication

Detection consumes 13.90/49.06 ms of the 14.33/49.52 ms sensor publication
system. Observation construction is a small remainder. Inspection of
`SpatialIndex::segment_candidates` also found that every occlusion segment
scans all active objects to recompute their maximum radius. The timing above
does not isolate that scan from the spatial queries themselves.

| Mean publication time | 1 player | 16 players |
|---|---:|---:|
| Navigation publication | 4.96 ms | 5.06 ms |
| Displays | 0.12 ms | 0.39 ms |
| Session snapshots, serialization, event pruning | 2.62 ms | 28.77 ms |

The measured priorities are reducing the repeated full spatial collection,
profiling the sensor query/occlusion path, and reducing per-session snapshot
work. Physics solving is a small part of these sparse ticks.

## Reproduction

```sh
OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=1 \
  cargo test --release -p osg-server --lib \
  sim::benchmark::production_tick_and_publication -- --ignored --nocapture
```

Use `OSG_BENCH_PLAYERS=1` for the other configuration. `OSG_BENCH_FRAMES`
defaults to 300. Run benchmarks serially without other tests or builds competing
for CPU. Profiling does not impose extra ordering between simulation systems.

Raw reports: [one player](tick-breakdown-1-player.txt),
[sixteen players](tick-breakdown-16-players.txt).

## Follow-up: cached physical radius bound

`SpatialIndex` now accumulates the maximum physical object radius during
insertion, resetting it when the active object collection is cleared. Segment
queries read that bound directly, eliminating the full object scan per ray.

A repeat of the sixteen-player configuration (70 warmup, 300 measured ticks)
measured sensor detection at 42.10 ms, down from 49.06 ms, about 14%.
Simulation averaged 103.02 ms versus 105.79 ms; publication averaged 34.71 ms
versus 34.22 ms. Other scopes varied between runs, so the isolated sensor
improvement did not translate one-for-one into overall wall time. This remains
a single-run comparison.

Raw report: [cached radius, sixteen players](tick-cached-radius-16-players.txt).

## Sensor query measurements

After caching the radius, added nested timers for contact selection, range
lookup, sorting, and segment queries. Added query-budget probe counters and
benchmark-only scan/ray/candidate counters. Ran three serial sixteen-player
repeats, each with 70 warmup and 300 measured ticks, in release mode.

| Mean milliseconds per tick | Repeat 1 | Repeat 2 | Repeat 3 |
|---|---:|---:|---:|
| Sensor detection total | 43.04 | 43.00 | 42.10 |
| Contact selection total | 14.49 | 14.33 | 13.94 |
| Range lookup, within selection | 14.08 | 13.94 | 13.54 |
| Sorting/truncation, within selection | 0.084 | 0.081 | 0.080 |
| Segment queries, within occlusion | 27.90 | 28.00 | 27.50 |

All three repeats produced the same per-tick counts:

- 3,017 sensor scans.
- 3,229 range results, including observers themselves.
- 212 selected non-self contacts and 212 occlusion rays.
- 6,996 ray spatial probes, counting nearest-query radius iterations and local
  refinement queries: 33 probes per ray.
- 90,818 ray candidate visits, counted before distance/eligibility rejection.
  These include repeated visits, not 90,818 distinct objects.
- 1,334 sphere intersections returned by segment queries, before the caller
  excludes self, target, and objects that do not occlude.

Average detection time across repeats is 42.71 ms. Range lookup accounts for
13.86 ms, sorting 0.082 ms, and segment queries 27.80 ms. Segment queries cost
about 131 microseconds per ray. Each current spatial probe iterates 64 brightness
buckets, yielding 447,744 bucket iterations per tick for the rays alone.

The measurement rules out sorting as a meaningful bottleneck in this scene.
It establishes that range and occlusion queries dominate, despite small contact
lists. It does not separately attribute query time to bucket lookups, broad
radius bounds, repeated candidate visits, allocations, or missing early exit;
those require further measurements or controlled implementation comparisons.
Diagnostic timers and counters are enabled for these runs.

Raw report: [three sensor profiling repeats](sensor-query-breakdown-16-players.txt).

## CPU sampling of occlusion queries

Sampled the same release executable with `perf record -F 99 -m 4M -e cycles:u
-g --call-graph dwarf,8192 --delay=45000`, sixteen players, one repeat. The delay
excludes provisioning and most/all warmup. This capture reported zero lost
samples. An earlier 499 Hz capture lost samples and was discarded for numerical
analysis.

Exported stacks using `perf script -F period,ip,sym`. Selected samples with
`segment_candidates` in the call chain, and grouped their leaf symbols weighted
by the sampled cycle period. There were 407 matching samples, so percentages
are approximate and refer to sampled CPU cycles, not independent wall timers.

| Leaf operation within ray call stacks | Weighted share |
|---|---:|
| Spatial cell hash-table lookup closure | 72.3% |
| Surrounding spatial cell iterator machinery | 13.6% |
| Nearest query traversal | 11.4% |
| Sphere intersection | 0.6% |
| Other | 2.1% |

The lookup closure is `SpatialHash::nearest`'s `filter_map` calling
`self.map.get(&key)`. Its disassembly contains coordinate hashing and hash-table
control-byte probing/key comparisons. The earlier candidate count does not
count lookups of empty cells: it counts records yielded after those lookups.
Each occupied brightness bucket enumerates a cube of neighboring cell keys
for every probe. Empty brightness buckets already skip that enumeration.

Thus the measured ray bottleneck is repeated spatial cell lookup/traversal.
The sampling does not distinguish memory stalls from hashing/arithmetic cost;
cache-miss attribution would require hardware counter measurements.

## Demand-driven observations

Sensor input publication now prepares query metadata without running scans.
Computer scan/contact requests invoke the sensor service synchronously. Session
frames request observations only for authorized ships whose contact lists they
include. Results are cached by observer and scene revision. Publishing a new
scene, possessing sensor hardware, and validating a cached contact handle do not
initiate scans. Repeated reads share the cached result until the scene changes.

The service shares the persistent spatial hash through a read/write lock; it
does not clone the million catalogue records. Sensor metadata is copied at the
completed-tick boundary. Computer-requested results are copied into ECS for
other consumers, and lifetime/IFF changes invalidate or update cached contacts.
There is no sensor subscription that can survive a client disconnect and keep
initiating queries by itself.

Release measurement, sixteen players, 70 warmup plus 300 measured ticks:

| Measurement | Result |
|---|---:|
| Scans per tick | **17**, previously 3,017 |
| Range queries, simulation + publication | **0.141 ms**, previously 13.86 ms |
| Sensor input publication | 2.15 ms |
| Requested detection during simulation | 2.30 ms |
| Requested detection during session publication | 33.74 ms |
| Simulation | 58.49 ms |
| Publication | 66.84 ms |
| Combined | 125.33 ms |
| Occlusion rays per tick | 272 |

The timing split moved with the work: most requested scans now happen while
building client frames. The combined detection cost remains about 36 ms.
The benchmark places player ships using ECS iteration order, which changed
with component layout; this run has 272 contact rays versus the earlier 212.
Consequently these timings are not a controlled comparison of identical ray
geometry. The scan count demonstrates removal of the eager installation scans.

The existing observation integration test now explicitly requests observations
and verifies immediate computer scan results, lazy publication, cache reuse,
private handles, IFF changes, and removal/reacquisition. It passed, as did the
native scan range/handle test. The release benchmark also completed successfully.

Raw report: [demand-driven sensors, sixteen players](sensor-demand-16-players.txt).
