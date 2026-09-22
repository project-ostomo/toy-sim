# Collision profiling, 2026-09-22

The collision system accounts for about half of a normal simulation tick in
these release measurements. Most of that time builds the shared spatial indexes.
The measured scene has no candidate collision pairs or impacts.

## Method

- AMD Ryzen 9 5900XT, 16 cores / 32 threads.
- Repository base `7be7bae`, with the profiling scopes and build correction in
  this change; no simulation optimization was applied.
- Normal release build, with the repository's `panic = "abort"` setting.
- Fresh worlds created by the existing network benchmark, including background
  traffic, infrastructure, and celestial bodies. These are not captures of an
  existing user's saved world or a combat workload.
- One player / one session and 16 players / four sessions, measured sequentially.
- 70 warmup ticks and 60 measured frames per session. Tables aggregate completed
  server ticks 74–133, excluding initialization and shutdown.
- Opt-in wall-clock scopes and existing spatial traversal counters. A separate
  control disables the profiling flags. A separate six-second `perf` capture
  samples only the server process after tick 80, at 199 Hz with DWARF call stacks.
- No compilation or other benchmark ran concurrently with these measurements.

## Overall timings

| Workload | Mean tick | p95 tick | Mean collision system | Collision share | Publication outside tick |
|---|---:|---:|---:|---:|---:|
| 1 player, 1 session, profiling | 55.94 ms | 61.62 ms | 29.01 ms | 51.9% | 7.08 ms |
| 16 players, 4 sessions, profiling | 59.79 ms | 64.92 ms | 30.56 ms | 51.1% | 13.19 ms |
| 16 players, 4 sessions, control | 57.99 ms | 61.38 ms | 29.31 ms* | 50.5%* | 13.49 ms |

All three runs sustained 10 received updates per second with zero skipped ticks.
The profiled 16-player tick is 3.1% slower than the control. A single comparison
cannot separate instrumentation overhead from run variation.

*The control's existing `CollisionStats` timer ends before local cleanup. The
profiled whole-system scope also includes cleanup, about 0.30 ms in this run.
The comparable profiled `CollisionStats` mean is 30.26 ms. This distinction also
explains why the scope is slightly larger than the collision field in tick logs.

## Inside the collision system

Means for the 16-player workload; these rows partition the 30.56 ms whole scope.

| Stage | ms/tick | Share of collision system |
|---|---:|---:|
| Rebuild shared spatial indexes, including collision entries | 22.21 | 72.7% |
| Broadphase: find candidate pairs and map/sort their IDs | 4.69 | 15.3% |
| Prepare simulation bodies and cached geometry references | 1.06 | 3.5% |
| Write results to ECS and ingest combat results | 1.03 | 3.4% |
| Remaining work, cleanup, and scope/logging overhead | 1.59 | 5.2% |

The remaining work includes 0.55 ms preparing thermal/weapon events, 0.23 ms of
parallel drift and cooling, 0.08 ms preparing weapons, 0.07 ms finalizing weapons,
0.06 ms activating shields, and 0.05 ms preparing rotation trajectories. Initial
contact prediction takes about 0.001 ms and resolving the empty event queue
takes less than 0.001 ms. These are idle-scene costs, not measurements of narrow
phase or impact solving under load.

Inside the **22.21 ms rebuild**:

| Stage | ms/tick |
|---|---:|
| Collect world records, including clearing the previous index | 4.45 |
| Prepare entries, bounds, luminosity, metadata map, and input vectors | 10.33 |
| Construct the instantaneous and swept BVHs concurrently | 6.87 |
| Collision-entry creation and surrounding rebuild work | 0.56 |

Each tree spends approximately 2.88 ms partitioning its input, 2.73 ms assembling
nodes, and 0.70 ms preparing partition keys and storage. The two tree builds
overlap, so their times must not be added to the 6.87 ms wall time.

The existing tick log's `index_ms` combines rebuilding the shared service and
broadphase pair discovery. Its `solve_ms` includes weapon finalization, drift,
and cooling as well as collision response. Those labels are broader than the
physical operations their names might suggest.

## Why idle collision work costs this much

Each main dynamic tree contains **49,925 records**:

- 43,891 celestial observation bodies, including 3,048 stars;
- 3,017 other observation bodies;
- 3,017 collision proxies.

The broadphase traverses the swept tree against itself and filters for collision
records at the leaves. It visits about **170,580 node pairs per tick**, reaches
**3,017 overlapping leaf pairs**, rejects every pair by record kind, and returns
**zero candidates**. Detailed contact queries and impacts are also zero in all
60 measured ticks.

The one-player world still contains 3,002 physical bodies and 49,895 indexed
records. The background population explains why removing 15 player ships hardly
changes the collision cost.

The earlier [spatial report](spatial-profiling-2026-09-22.md) measured an expensive
copy of the collision service. Current code borrows that service; the copy is
already gone. These measurements also use release optimization, whereas that
report used the development profile, so differences are not an isolated measure
of any one change.

## Independent CPU sample

The separate `perf` capture recorded 1,614 samples with no lost samples. Exclusive
sample shares across the entire server process include:

| Symbol or group | CPU sample share |
|---|---:|
| BVH partition routine | 7.62% |
| Its quicksort partition implementation | 4.89% |
| Its insertion-sort implementation | 1.55% |
| BVH node assembly | 3.28% |
| Main spatial input iterator | 3.41% |
| Swept sphere bounds | 1.18% |
| Spatial record map insertion | 0.81% |

These are CPU shares across workers and all server stages, including smaller
aperture index builds. They are not percentages of collision wall time. Inlining
and partially unresolved library symbols limit attribution. The sample supports
the measured construction costs; it does not establish allocation or hashing as
the dominant cause of entry-preparation time.

## Where to optimize next

1. Investigate excluding observation-only records from the swept collision tree.
   It currently has 46,908 such records, which this pair query rejects. Audit
   every motion-query consumer before changing tree membership. This should
   reduce construction and traversal work, but this report does not measure the
   resulting savings.
2. Reduce repeated entry preparation and rebuilding. Bounds, record maps, and
   buffers are prepared every tick for the entire scene. Measure reuse and
   updates separately while preserving the frozen scene used during the solver.
3. Add a deliberate contact/combat workload before optimizing narrow phase or
   impact resolution. The current benchmark does not exercise those paths.

## Reproduce

```sh
cargo build --release -p osg-server --bin osg-server --example benchmark --offline

OSG_SPATIAL_PROFILE=1 OSG_BVH_PROFILE=1 \
RUST_LOG=osg_server::profile=debug,osg_server::timing=debug,osg_server::sim::spatial=debug \
target/release/examples/benchmark \
  --server "$PWD/target/release/osg-server" \
  --ships 16 --sessions 4 --frames 60 --warmup-ticks 70
```

Use `--ships 1 --sessions 1` for the smaller workload. For the control, omit both
`OSG_*` variables and use `RUST_LOG=osg_server::timing=debug`.

Raw logs and JSON aggregates are in
`/tmp/osg-collision-static-{1-profile,16-profile,16-control}.{log,json}`.
The parser is `/tmp/analyze_osg_collision.py`. CPU samples are in
`/tmp/osg-collision.perf.data`, with the capture script at
`/tmp/capture_osg_collision.py` and a flat symbol report at
`/tmp/osg-collision-perf-flat.txt`.

## Build correction

Both `osg-server` and `osg-ui` already guarded their `bevy_dylib` imports with
`debug_assertions`, but Cargo still compiled the unconditional native dependency
in release. That failed with the release abort panic strategy. The dependency is
now optional behind `dynamic_linking`; normal release builds omit it. Development
build instructions are in the [README](../README.md#setup).

Initial diagnostic runs used a command-line unwind override. They are excluded
from the tables above; all reported wall times use the corrected normal release
build. The final server's `ldd` output has no Bevy or Rust standard-library dylib.

Validation: the normal release server and benchmark build succeeds; all three
final benchmark runs complete; all 38 existing collision tests pass with
`cargo test --release -p osg-server --lib collision --offline`. Dependency-tree
checks confirm that `bevy_dylib` is omitted by default and enabled by the explicit
feature. Formatting checks pass for changed Rust files, and `git diff --check`
passes. The full workspace formatting check also reports pre-existing differences
in unrelated files.
