# One spatial build per tick

The active-scene BVH is built once after collision-body preparation and before
collision discovery. Preparation supplies conservative hull/shield radii to the
collector. The FixedLast collection and mid-tick collision index synchronization
have been removed.

BVH insertions/removals stage changes without invalidating the published tree.
Only explicit rebuild/replace calls publish a new tree. Query methods never build
trees, and published leaf geometry remains valid even if staged records change.
Newly spawned entities enter spatial queries at the next tick's build. Queries
may return stale entity IDs; ECS readers skip missing entities/components.

Release benchmark: 16 players, sparse fixture, 70 warmup ticks and 300 measured
ticks, one repetition. The counters confirm 1.000 spatial collections per tick.

| Mean per tick | Previous | Once per tick |
|---|---:|---:|
| Simulation | 56.145 ms | 37.058 ms |
| Client publication | 10.368 ms | 10.262 ms |
| Combined frame | 66.513 ms | 47.320 ms |
| Spatial collection + clear | 29.772 ms | 15.355 ms |
| Collision discovery | 10.434 ms | 6.080 ms |

Spatial collection consists of 4.065 ms ECS collection, 1.716 ms record
preparation, 9.483 ms batch replacement/BVH construction, and 0.088 ms clearing.
The discovery timing now excludes tree construction.

Frame p50 was 47.001 ms and p95 50.990 ms. Same limitations as the previous
fixture runs apply: no network I/O/compression or sustained combat.

Raw measurements: [once-per-tick-16-players.txt](once-per-tick-16-players.txt).
