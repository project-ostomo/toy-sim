# Borrowed spatial access

The server now owns `GalacticIndex` directly in its `SpatialIndex` resource.
Systems borrow the resource using `Res` and `ResMut`. Collection prepares one
batch, then replaces the BVH scene without per-object locks, change comparisons,
or a separate stale-record removal pass. The shared spatial API still keeps
geometric records for exact sphere queries and incremental collision updates.

Sensors borrow the index and read target components through ECS queries at scan
time. Sensor publication advances the observation-cache revision without copying
scene geometry or all target metadata. Missing entities/components are skipped.
Already computed observation results remain cached for the publication revision.

Computer and display execution pass borrowed scan access to `run_slice`.
The WASM store and suspended future retain no scan source. A private scoped
thread-local bridge exposes the borrow only during the synchronous future poll,
restoring previous access on return/unwind. This isolates the lifetime erasure
needed by Wasmtime's persistent store; resumed execution supplies a fresh borrow.

Release benchmark: 16 players, sparse scene, 70 warmup ticks and 300 measured
ticks, one repetition.

| Mean time per tick | Previous collection run | Borrowed access |
|---|---:|---:|
| Spatial collection + clearing | 34.449 ms | 29.772 ms |
| ECS collection | 8.428 ms | 8.162 ms |
| Prepare batch records | — | 2.897 ms |
| Replace records and construct BVH | — | 18.538 ms |
| Sensor publication | about 2.16 ms | 0.089 ms |

Simulation averaged 56.145 ms and publication 10.368 ms, totaling 66.513 ms.
Frame p50 was 65.972 ms, p95 73.057 ms. The scene still collects twice per tick
and contains 46,908 active spatial records. The earlier record-update/rebuild
timings are not individually comparable to the new batch replacement timing.

Validation: all seven WASM resumption tests, the live sensor/handle lifetime test
(including despawning a target with its spatial entry still present), nearest
selection and CCD regression checks, and the production benchmark.

Raw measurements: [borrowed-spatial-16-players.txt](borrowed-spatial-16-players.txt).
