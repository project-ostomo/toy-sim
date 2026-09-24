# Spatial indexes

`LuminosityBvh<T>` is the immutable tree used for catalogues and bulk builds.
`DynamicLuminosityBvh<T>` supports incremental insertions, removals, and updates.
The server's active-scene `GalacticIndex` rebuilds the immutable tree every tick.
The dynamic tree remains available for experiments. `SpatialService` is a
separate API described below; the server does not currently use it.

## Bulk construction

`LuminosityBvh::build(entries)` uses balanced median splits along the widest
center axis, also available explicitly as `build_median(entries)`. Use
`build_morton(entries)` for Morton LBVH construction. Both algorithms have the same
`(tree, leaf_indices_in_input_order)` return value.
Both support non-Clone payloads and retain exact bounds and luminosity maxima.

The Morton builder normalizes centers within the input's center bounds, encodes
21 bits per axis, and performs eight stable byte radix passes. Internal nodes
split at the highest differing Morton bit; equal codes split into equal counts
in input order. Sorting and assembly are sequential. Both builders share node
assembly and produce depth-first preorder storage with one object per leaf.

Quantization affects grouping, not query correctness. Very large scene spans
can collapse nearby objects onto the same code and reduce query pruning.
Construction benchmarks do not establish traversal performance. See
[construction benchmarks](benches/README.md) for reproduction.

## Dynamic BVH

`DynamicLuminosityBvh::build(entries)` explicitly uses the median builder to
create a balanced initial tree for its height-balancing updates, and returns
handles in input order. `new()` starts empty. `insert(BvhEntry<T>)` returns a
`LeafHandle`; `update(handle, entry)` replaces geometry, luminosity, and payload;
`remove(handle)` returns the removed payload. Removed handles are rejected even
after storage reuse. Handles are scoped to their originating tree; a clone is
independent and initially accepts the same handles.

Nodes and payload slots are pooled separately. Leaves retain fat AABBs padded
on each axis by one percent of its extent (at least one coordinate unit) plus
four times the last update's endpoint displacement, capped to 64 times the box's
largest extent (at least one coordinate unit) before multiplying. This caps teleport
padding even if a caller stops updating a stationary object. Contained motion updates
only the exact leaf geometry and payload. On each update, padding shrinks if the
retained box exceeds an envelope with four times the newly calculated padding.
Arithmetic saturates at the coordinate limits. When padding changes, a leaf
is reinserted if its new center leaves its parent's
previous box or enlarging that box would more than double its surface area.
Otherwise its ancestors refit in place.
Insertion uses surface-area costs and height-balancing rotations. Additional
child/grandchild exchanges reduce internal surface area while preserving height
and balance. Branch bounds remain exact unions of their padded children. Luminosity
changes propagate both increases and decreases; payload-only updates do not
change topology. There are no automatic full rebuilds.

`update_batch(&mut updates)` validates a vector of `(LeafHandle, BvhEntry<T>)`
before mutation, then drains it while retaining capacity. Dirty ancestors refit
once per batch before any required reinsertions. This avoids repeatedly refitting
shared ancestors when most of a scene moves. Ancestors whose boxes remain
unchanged skip surface-area rotation searches. Repeated handles apply in order.

`try_visit` uses the same near-first, pruning, and early-error contract as the
immutable tree. Branch callbacks receive conservative padded bounds; leaf
callbacks receive current exact bounds, preserving geometric filtering.
Queries borrow the tree and never mutate it. The tree itself
publishes updates immediately; `GalacticIndex` controls publication when using
this backend. `take_stats`, `height`, and `allocated_bytes` expose mutation work
and tree storage for profiling. Allocated bytes exclude payload-owned allocations.

## Spatial service

`SpatialService<T>` owns three BVHs: static, instantaneous dynamic, and swept
dynamic. The static tree and its records are shared with `Arc`;
`rebuild_dynamic(entries)` builds both dynamic trees in parallel and replaces them
in place. Its caller chooses when to rebuild. The service does not schedule
ticks or store timestamps.

Leaves retain only identities supplied by `SpatialObject::spatial_id`. A caller
can use an entity ID plus its proxy role, allowing observation and collision proxies
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
