# Incremental dynamic BVH — September 23, 2026

This report records the unpadded implementation. See the
[fat-bounds follow-up](fat-dynamic-bvh-2026-09-23.md) for the subsequent change.

Implemented `DynamicLuminosityBvh<T>` and enabled it for the server's active
`GalacticIndex`. Static catalogues retain the immutable tree; the hash backend
remains available. Geometry remains sphere-derived AABBs, without fat bounds.
Rapier collision groups and CCD retain their existing semantics.

**This implementation is slower overall on the measured workloads.** Three
16-player repetitions average 86.67 ms per frame, versus 70.53 ms with immutable
rebuilding: a 22.9% increase. The implementation is enabled as requested without
a performance adoption threshold. These results do not establish a speedup.

## Implementation

- Reusable node and payload pools, generation-checked leaf handles, insertion,
  removal, payload/geometry/luminosity updates, and independent clones.
- Exact branch unions and luminosity maxima, including decreases. Queries never
  rebuild or mutate the tree.
- Initial median bulk construction; subsequent updates retain the tree.
- Coherent movement refits exact bounds in place. A new leaf center outside its
  previous parent box, or enlargement exceeding twice that box's area, triggers
  reinsertion. This is a topology heuristic, not retained padding.
- Surface-area-guided insertion, height balancing, and local child/grandchild
  exchanges that reduce internal surface area without changing subtree height.
- Batch updates validate inputs first, update payloads, and refit dirty ancestors
  once before reinsertion. Batch scratch storage is reused.
- `GalacticIndex` retains record maps and handles across publication. Staged
  mutations remain separate from published leaf records. Complete replacement
  reconciles inserted, changed, and missing IDs at the existing tick boundary.

The initial remove/reinsert-on-every-move implementation severely degraded
production traversal. Exact refitting and batch maintenance address that concrete
failure without adding fat AABBs. No automatic whole-tree rebuild is present.

## Production frame comparison

AMD Ryzen 5 3600XT, 6 cores / 12 threads, rustc 1.97.1, release profile. All
configurations used the same final executable, selecting only the active-scene
backend with `OSG_BENCH_BVH`. Runs were serial, with no other builds or benchmarks
started by this session. Other host applications were not isolated.

Each repetition has 70 warmup ticks and 300 measured ticks. Sparse scenes have
three repetitions per configuration; dense and rays have one. The table averages
run means. Frame cost includes simulation and client publication, excluding
rendering, network transport, and compression. No optical subscription is enabled.

| Mean milliseconds | Static, 1 player | Dynamic, 1 player | Static, 16 players | Dynamic, 16 players |
|---|---:|---:|---:|---:|
| Simulation | 57.4205 | 73.6669 | 57.6337 | 74.2403 |
| Publication | 6.3680 | 6.4171 | 12.8995 | 12.4248 |
| Combined | **63.7885** | **80.0840** | **70.5332** | **86.6651** |
| Spatial replacement/synchronization | 14.1175 | 29.4374 | 13.9736 | 29.5776 |
| Collision discovery | 8.0306 | 9.0553 | 8.0240 | 9.1109 |

One-player frame time increases 25.5%. The sixteen-player synchronization scope
is about 2.12 times the rebuild cost; collision discovery is 13.5% slower. These
are enclosing operation timings, not isolated tree-internal CPU measurements.
Simulation scopes can overlap and nest; do not sum all table rows.

| 16-player fixture | Static frame p50 / p95 (ms) | Dynamic frame p50 / p95 (ms) |
|---|---|---|
| Sparse, run 1 | See raw report | 86.256 / 91.989 |
| Sparse, run 2 | See raw report | 86.175 / 91.638 |
| Sparse, run 3 | 69.250 / 77.302 | 86.268 / 94.111 |
| Catalogue rays | 69.255 / 79.992 | 86.227 / 92.149 |

Dense contact frames average 77.6702 ms with static rebuilding and 89.1049 ms
with dynamic maintenance. Both retain 468 contact pairs per tick. The dense
static run has a higher rebuild mean (19.0642 ms) than sparse static runs; this
single run should not be used to infer a stable percentage gain or regression.

The rays fixture includes four additional catalogue queries in its frame
percentiles, outside the publication timer. The catalogue itself is unchanged.

In the 16-player dynamic scene, 43,954 of 46,908 records change bounds every
tick and only 2,954 remain unchanged. There are no steady-state insertions,
removals, or luminosity changes in this sparse fixture. One repetition averages
399 reinsertions and 750 rotations per tick, with tree height 18. Most records
are moving celestials; this workload offers few unchanged-record skips.

## Identical-input replay

Captured 100 active-scene snapshots, excluding warmup, and applied them to both
backends. The first snapshot's construction is excluded. Each frame replays
collision-discovery radius inputs for its recorded bodies plus 64 deterministic
body-to-body segments. These are representative query inputs, not a recording
of every actual server query. Backend execution order alternates. File loading
and result sorting/comparison are outside the timers; result allocation and
query-budget accounting are inside. All returned ID sets agree exactly.

Three replay repetitions average:

| Per snapshot | Immutable rebuild | Dynamic maintenance |
|---|---:|---:|
| Update | 11.4693 ms | 26.7577 ms |
| Queries | 9.3272 ms | 8.5810 ms |
| Update + queries | 20.7965 ms | 35.3387 ms |
| Node visits | 108,651.4 | 105,392.4 |
| Leaf tests | 3,147.0 | 3,147.0 |
| Tree-vector allocation | 19,513,520 bytes | 17,364,039 bytes |

Queries improve 8.0%, but combined time increases 69.9%. Replay starts with a
bulk-built full scene, while production grows from its smaller setup scene;
their tree layouts and query timings differ. The storage figures exclude
record/handle maps, wrapper batch scratch vectors, and payload-owned allocations.
They are not process RSS or peak allocation measurements.

The standalone synthetic replay contains 4,096 objects over 240 frames, with
coherent movement, periodic teleports, changing luminosities, and 128 replacements
per tick. Its three-repetition means are:

| Per frame | Immutable rebuild | Dynamic maintenance |
|---|---:|---:|
| Update | 1.2544 ms | 1.7500 ms |
| Queries | 2.8343 ms | 5.1995 ms |
| Combined | 4.0888 ms | 6.9494 ms |
| Node visits | 28,510.5 | 53,572.3 |

All results agree, but dynamic query time grows from roughly 2.5–2.6 ms in the
first 20 measured frames to 5.4–6.7 ms in the last 20. Height remains bounded at
14. This demonstrates a remaining spatial-quality limitation: height balance
and local area rotations do not guarantee rebuild-quality pruning over sustained
movement/churn. Hash-map removal order also varies topology between repetitions.
Pool capacity is retained after churn, so dynamic tree-vector storage is about
3.03 MB versus 1.70 MB for the rebuilt tree in this case.

## Validation

- 53 `osg-spatial-bvh` unit tests and 6 `osg-spatial` unit tests pass.
- Random mutation and batch tests check exact bounds, maxima, balance, slot reuse,
  stale handles, extreme coordinates, clones, and agreement with exhaustive scans
  and freshly rebuilt trees.
- Backend checks cover publication isolation, removals, visibility, nearest,
  translated segment queries, and hash-backend agreement.
- 56 targeted server tests pass: 20 collision/beam/CCD, 13 spatial, 5 sensor,
  6 optical, and 12 slip tests. Older direct-insertion test fixtures now explicitly
  publish their changes before querying, matching the production contract.
- `due_shots_launch_together_and_ccd_reports_their_impacts` still fails: expected
  two impacts, observed zero. It fails identically in the executable saved before
  this change; its raw baseline failure is included. It was not masked or ignored.
- Documentation tests and formatting/diff checks pass. All frame benchmark
  configurations and both replay workloads complete successfully.

## Reproduction

```sh
# Repeat separately with static/dynamic and 1/16 players.
OSG_BENCH_BVH=dynamic OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=3 \
  cargo test --release --offline -p osg-server --lib \
  sim::benchmark::production_tick_and_publication -- --ignored --nocapture

# Additional fixtures: OSG_BENCH_SCENE=dense or rays, OSG_BENCH_REPEATS=1.

# Separate capture run; its I/O affects caches, so do not use its frame timings.
OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=1 OSG_BENCH_FRAMES=100 \
  OSG_BENCH_SPATIAL_CAPTURE=/tmp/active-scene.bin \
  cargo test --release --offline -p osg-server --lib \
  sim::benchmark::production_tick_and_publication -- --ignored --nocapture
cargo run --release --offline -p osg-spatial --example dynamic_replay -- \
  --capture /tmp/active-scene.bin
cargo run --release --offline -p osg-spatial --example dynamic_replay
```

Raw measurements, validation logs, and source/executable hashes:
[dynamic-bvh-2026-09-23](dynamic-bvh-2026-09-23/).
The session's capture remains at `/tmp/osg-dynamic-bvh/active-scene.bin`; it is
about 358 MiB and is not included in the repository.
