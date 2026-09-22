# Spatial profiling — 2026-09-22

The current three-tree service is limited by record preparation, repeated work,
and query volume as well as BVH construction. This investigation adds opt-in
timers and traversal counters; it does not implement the proposed optimizations.

## Workload and measurement

- Development profile: workspace optimization level 1, dependencies level 3.
- 16 player ships, four connected sessions, 70 warmup ticks, 60 measured frames
  per session. The world also contains its normal infrastructure and celestial
  population.
- Statistics below use 60 completed steady-state ticks, 74–133. Startup and the
  final checkpoint are excluded. Publication measurements use the corresponding
  steady-state window before benchmark completion.
- Rebuilds use parallel instantaneous/swept construction. Static trees do not
  rebuild during these ticks.
- Host `perf_event_paranoid=3` blocks `perf` sampling. These are instrumented
  wall-clock measurements and traversal counts, not sampled CPU stacks or
  allocator traces.
- Source clocks include scheduling and allocation time. Nested scopes must not
  be added together; concurrent tree times overlap.

Final same-binary comparison:

| Profiling | Average tick | p95 tick | Received updates/sec |
|---|---:|---:|---:|
| Disabled | 107.060 ms | 119.201 ms | 7.60 |
| Coarse scopes and counters | 107.082 ms | 117.526 ms | 7.57 |

Earlier coarse passes averaged 105.076 and 112.584 ms, showing meaningful
run-to-run variation. The final control pair shows no discernible coarse
instrumentation penalty within that variation. Detailed per-entry timing raised
a separate run to 115.720 ms; logging every sensor query raised another to
152.028 ms. Those intrusive runs are diagnostic, not throughput baselines.

## Steady-state costs

Means from the final coarse pass:

| Operation | ms/tick | Relationship |
|---|---:|---|
| Simulation tick | 107.08 | Overall update |
| Collision step | 56.51 | Includes spatial rebuild, copy, broadphase and solver |
| Complete spatial rebuild | 40.12 | Inside collision step |
| World-record collection | 6.92 | Inside rebuild; includes clearing old geometry |
| Prepare dynamic BVH entries and metadata | 21.35 | Inside rebuild |
| Build both dynamic trees, parallel wall time | 10.66 | Inside rebuild |
| Copy service for collision solver | 5.65 | Inside collision step, outside rebuild |
| Collision broadphase traversal | 5.31 | Inside collision step |
| Shield activation | 0.13 | Inside collision step |
| Publish sensor observations | 15.91 | Separate from collision step |
| Sensor detection | 13.13 | Inside sensor publication |
| Move celestial bodies | 12.67 | Existing orbital simulation work |
| Activate systems | 2.67 | Existing system activation work |
| Publish world-service indexes | 7.26 | Includes aperture indexes |
| Build both aperture services | 3.15 | Inside world-service publication |
| Display and network snapshot publication | 24.98 | Outside simulation tick |
| Optical observations for sessions | 1.36 | Inside display/network publication |
| Display work | 6.91 | Inside display/network publication |

Remaining rebuild time covers surrounding resource/system work. Remaining sensor
publication work is about 2.8 ms. The publication total includes more than optical
queries and displays, so those rows do not fully explain its 25 ms.

With approximately 25 ms of publication work, the simulation needs roughly a
75 ms tick to sustain an end-to-end 10 Hz loop on this workload.

## What is being indexed

Both main dynamic trees contain **49,925 leaves**:

- 43,891 celestial observation bodies, including 3,048 stars;
- 3,017 non-celestial observation bodies;
- 3,017 collision proxies for those non-celestial bodies.

The static catalogue remains shared and unchanged. Instantiated celestial bodies
still enter the dynamic service; the presence of a static catalogue does not
remove those records from dynamic indexing. Some stars have orbital motion, so
moving all instantiated stars to the static tree would require checking their
motion and query semantics first.

Each large dynamic tree allocates **12,780,672 bytes** of nodes, about 12.19 MiB.
The collision caller's `.service().clone()` copies both node vectors every tick:
approximately **24.38 MiB copied**, plus allocation, in 5.65 ms on the final pass
(5.87–7.41 ms on earlier coarse passes). This call used to clone an `Arc`; the
ownership refactor unintentionally turned it into a full dynamic-tree copy.
See `crates/osg-server/src/sim/physics/collision/ecs.rs`.

## Entry preparation and construction

Preparation takes 18–24 ms across coarse runs, before parallel BVH construction
starts. It pulls the lazy server iterator, computes instantaneous and swept
bounds, evaluates luminosity bounds, validates bounds, inserts records into the
ID lookup map, and fills two build-input vectors.

A separate intrusive per-entry pass measured approximately **9.7 ms pulling
input records** and **3.7 ms inserting records into the metadata map**. These
measurements contain timer overhead. The remaining preparation time cannot be
assigned entirely to allocation: it also includes validation, vector writes,
and instrumentation. No allocator stack sampling was available.

The large-tree build phases are approximately:

| Phase | ms per tree |
|---|---:|
| Collect payload storage | 0.6 |
| Construct partition keys and allocate output storage | 0.9 |
| Partition keys | 5.2 |
| Assemble nodes | 3.6–3.8 |
| Release temporary buffers | below 0.02 |

These phases are exclusive. The two trees run concurrently, giving about 11 ms
of combined wall time rather than the sum of both tree durations.

## Query workload

Collision broadphase visits approximately **170,580 node pairs** per tick. It
reaches **3,017 overlapping leaf pairs**, all rejected by the collision-role
predicate, and returns **zero collision candidates** in this benchmark. The
scene therefore measures idle broadphase overhead, not a collision-heavy combat
load. Physics narrow-phase and solve costs are small here.

All current game consumers of `SpatialQuery::Motion` filter for collision
proxies: changed trajectories/projectiles and beams. The pair query also filters
for collision proxies. The swept tree currently carries 46,908 additional
observation records that these consumers reject. This is a concrete opportunity
to reduce its membership while preserving a single spatial service.

Sensor publication runs **3,017 nearest queries per tick**. The detailed query
pass counted about **105,861 node visits**, **6,578 leaf tests**, and **272 accepted
distance callbacks** in total. Median per-query counts are 33 nodes, two leaves,
and zero accepted records. Coarse aggregate sensor timing confirms **272
candidates and 62 visible contacts** per tick. Individual traversals are short;
the number of scans matters. Any reduction in scan frequency or demand must
preserve autonomous station/ship behavior.

The two asynchronous aperture services hold **3,017** and **3,001** bodies. Each
builds both dynamic trees even though its queries use only instantaneous bounds.
That is four additional small BVH builds per tick, costing about 3.2 ms together.

## Recommended order of work

1. Remove the accidental collision-service copy by borrowing the current service
   through the collision step. Preserve the frozen `t1` query state. This is the
   clearest avoidable cost.
2. Give the swept tree only the records its motion/collision consumers require.
   Audit those consumers and verify CCD, beams, and newly created projectiles.
   Keep all indexes within the spatial service.
3. Reduce entry-preparation work and allocation. Measure buffer reuse, repeated
   bounds calculations, and lookup-map construction independently; do not assume
   all preparation time is hashing or allocation.
4. Avoid building unused swept trees for asynchronous aperture checks.
5. Investigate the demand and frequency of the 3,017 sensor scans. Query timing
   does not justify replacing the nearest traversal algorithm first.

These are proposals, not changes made by this profiling task. Their individual
savings may overlap and should not be added into a promised final frame rate.

## Reproduction

```sh
cargo build -p osg-server --bin osg-server --example benchmark --offline

OSG_SPATIAL_PROFILE=1 OSG_BVH_PROFILE=1 \
RUST_LOG=osg_server::profile=debug,osg_server::timing=debug,osg_server::sim::spatial=debug \
LD_LIBRARY_PATH="$PWD/target/debug/deps:$(rustc --print target-libdir)" \
target/debug/examples/benchmark \
  --server "$PWD/target/debug/osg-server" \
  --ships 16 --sessions 4 --frames 60 --warmup-ticks 70
```

Omit the `OSG_*` environment variables for a control run. Add
`OSG_SPATIAL_PROFILE_DETAIL=1` for per-entry timings or
`OSG_SPATIAL_PROFILE_QUERIES=1` for per-nearest-query counters. Query logging is
deliberately separate because its volume significantly changes runtime.

Raw logs and JSON summaries from this session are under
`/tmp/osg-spatial-baseline/profile-*.{log,json}`. The temporary analysis script is
`/tmp/osg-spatial-baseline/analyze_profile.py`; it aggregates the final 60 completed
ticks before the benchmark CSV summary. The per-query discovery pass predates
the separate query flag and logged queries with the coarse flag alone.

Validation: 47 BVH tests and one doctest passed; the server binary compiled and
completed all profiling/control benchmark runs. No optimization was applied,
and the existing docking route-search timeout was not addressed here.
