# Spatial service

`SpatialService<T>` owns three BVHs: static, instantaneous dynamic, and swept
dynamic. The static tree and its records are shared with `Arc`;
`rebuild_dynamic(entries)` builds both dynamic trees in parallel and replaces them
in place. The game calls this once per simulation
tick at 10 Hz. The service does not schedule ticks or store timestamps.

Leaves retain only identities supplied by `SpatialObject::spatial_id`. The game
uses an entity ID plus its proxy role, allowing observation and collision proxies
for the same entity. Static sources use catalogue IDs. Both dynamic trees share
one lookup table of object records outside the BVH nodes.

The instantaneous tree indexes `bounds` at the start of the tick. The swept tree
indexes `swept_bounds`, which must enclose `bounds` and all predicted motion during
the tick. Motion and collision queries use the swept tree; all other queries use
the instantaneous tree. Bounds and luminosity are stored in the nodes without
duplicate copies in their payloads.

The public methods provide sphere, AABB, visibility, nearest-object, segment,
motion, first-hit, and collision-pair queries. Collision candidates include
dynamic–dynamic and dynamic–static pairs once each, with no self-pairs or
static–static pairs. Exact collision times and physical response belong to the
caller. Segment queries expand boxes conservatively; sphere sweeps can produce
extra candidates near box corners.

`query_dynamic`, `nearest_dynamic`, and `dynamic_collision_candidates` exclude
the static catalogue without visiting its nodes. Ship-only queries use these
methods through the same service. `SpatialRecord`
provides common source, body, and collision roles with stable kind-scoped IDs.

Positions use signed 128-bit integer micrometres. Distances use metres, luminosity
uses watts, and visibility thresholds use W/m². Visibility is conservative:
callers check exact brightness and occlusion. Zero threshold includes dark
objects; bounds overlapping the observer region remain candidates. Invalid
inputs panic as described in the API documentation.

## Bounded traversal

`query(SpatialQuery)` starts a cursor for sphere, AABB, visibility, segment, or
motion candidates. `advance(QueryBudget)` limits both work and result count.
Each visited node and each tested object costs one work unit, including rejected
candidates. `QueryBatch.stats` describes that advance; `QueryCursor::stats()`
reports cumulative work. Exact predicates and serialization need caller-side
accounting. Nearest, first-hit, and pair queries currently run to completion.

Always check `complete`: an empty batch may mean its budget was exhausted.
Zero work or zero results pauses the traversal. Results arrive in traversal
order; sort by stable identity when deterministic ordering is needed.

A cursor borrows the current service and must finish before its next rebuild.
The service retains no previous trees. An asynchronous caller can explicitly
clone the service (copying dynamic trees), or build an index of the records it
needs. The static tree remains shared and unchanged.

## Verification

```sh
cargo test -p osg-spatial-bvh --lib
cargo test -p osg-spatial-bvh --doc
cargo bench -p osg-spatial-bvh --bench gaia -- --test
```

## Profiling

`OSG_SPATIAL_PROFILE=1` logs dynamic-entry preparation, parallel build wall time,
replacement time, and collision-pair traversal counts. `OSG_BVH_PROFILE=1` logs
individual build phases for trees with more than 1,000 entries. Build phase
durations are exclusive, not cumulative; the two parallel tree times overlap.

`OSG_SPATIAL_PROFILE_DETAIL=1` additionally times every input record and metadata
insertion. `OSG_SPATIAL_PROFILE_QUERIES=1` logs every nearest traversal. These
detailed modes add substantial measurement overhead, especially when thousands
of queries run each tick. Use coarse timings and a profiling-disabled control
for throughput comparisons.

The server's `osg_server::profile=debug` log target reports matching scopes when
`OSG_SPATIAL_PROFILE` is enabled. See
[the profiling report](../../docs/spatial-profiling-2026-09-22.md) for measurements
and reproduction commands.

Service tests compare mixed static/dynamic queries with full scans, check
in-place rebuilds, explicit clones, work budgets, boundary contacts, and galactic coordinates.
See [benches/README.md](benches/README.md) for benchmark inputs and limitations.

`LuminosityForest` remains available but is not used by the service.
