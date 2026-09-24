# osg-spatial

The game-facing spatial API. Choose `GalacticIndex::bvh()` for catalogues and
scenes rebuilt in batches, `GalacticIndex::dynamic_bvh()` for incremental updates,
or `GalacticIndex::spatial_hash()` at construction. All return the same type.
The private backend enum delegates to the retained `osg-spatial-bvh` and
`osg-spatial-hash` engines.

Records use exact galactic positions, radii in metres, and intrinsic luminosity.
Queries cover radius overlap, nearest objects, luminosity candidates, and finite
ray/swept-sphere intersections. Filtered rays select geometry roles whose radii
must fit inside their stored record bounds. Results have no specified order
except `nearest_many`, which sorts by distance.

Hash mutations update in place, with a 512 km minimum cell size. BVH mutations
are staged; `rebuild()` explicitly publishes them, while `replace()` publishes a
whole batch. The immutable backend reconstructs its tree at publication. The
dynamic backend retains records and handles, updates changed leaves, and removes
absent IDs. It batches exact ancestor refits and reinserts leaves that move out
of their spatial neighborhood; luminosity changes also refit ancestors.
Fat AABBs amortize movement within retained bounds; leaf filtering uses the
current exact sphere-derived box. Queries always use the last published tree and never
rebuild it. Production uses the immutable BVH backend and rebuilds it once before
collision discovery each tick; new objects wait until the next publication.
Immutable star catalogues have their own index.

A query budget limits traversal work and returns an error on exhaustion rather
than an incomplete result. Probe counts represent nodes for BVH queries and
spatial probes for hash queries; these counters are not equivalent units across
backends.

## Replaying dynamic updates

Run the deterministic movement/churn comparison:

```sh
cargo run --release -p osg-spatial --example dynamic_replay
```

Capture 100 production active-scene snapshots (about 360 MiB), then replay both
BVH implementations against the same records and query inputs:

```sh
OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=1 OSG_BENCH_FRAMES=100 \
  OSG_BENCH_SPATIAL_CAPTURE=/tmp/active-scene.bin \
  cargo test --release -p osg-server --lib \
  sim::benchmark::production_tick_and_publication -- --ignored --nocapture
cargo run --release -p osg-spatial --example dynamic_replay -- \
  --capture /tmp/active-scene.bin
```

Capture writes outside the frame timer but affects caches; use a separate run
without capture for frame timings. Replay excludes file loading and the initial
build. Each subsequent snapshot is applied to both indexes, alternating execution
order. Every radius/segment result is checked for exact set equality outside query
timing. Captured queries reproduce collision-discovery search radii and add 64
deterministic body-to-body segments; they are not a full recording of server
queries. Reports include node/leaf visits, mutation counts, and tree-vector
storage (excluding maps, scratch collections, and payload-owned allocations).

`OSG_BENCH_BVH=static` or `dynamic` selects the active-scene backend in the server
benchmark for repeatable comparisons. It does not change catalogue indexing.

Player ships are scattered across distinct catalogue systems using a fixed seed,
with positions outside each primary and local circular-orbit velocities. System
activation follows the normal simulation path. Earlier line-of-ships benchmark
reports use a different spatial distribution and are not direct performance
comparisons. `OSG_BENCH_WARMUP` defaults to 70 ticks; `OSG_BENCH_FRAMES` controls
the measured tick count and defaults to 300.
