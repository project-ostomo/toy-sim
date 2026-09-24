# BVH through the shared spatial API

September 23, 2026. Release build, 16 players, sparse production fixture,
70 warmup ticks and 300 measured ticks, one repetition. Command:

```sh
OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=1 OSG_BENCH_FRAMES=300 cargo test --release -p osg-server sim::benchmark::production_tick_and_publication -- --ignored --nocapture
```

The active scene uses `osg_spatial::GalacticIndex` with the BVH backend and
explicit rebuilds at each spatial collection. It contains 46,908 records after
warmup. The 1,001,760 static catalogue entries remain in the universe's separate
immutable index. Hash and BVH implementations are both retained.

| Scope | Mean per frame |
|---|---:|
| Simulation + publication | 93.41 ms |
| Simulation | 61.77 ms |
| Publication | 31.64 ms |
| Spatial collection + clear | 33.49 ms |
| Collision pipeline | 13.46 ms |
| Collision discovery, including deferred BVH construction | 10.53 ms |
| Client sensor detection | 1.42 ms |
| Client occlusion segment queries | 1.15 ms |
| Controller sensor detection | 0.14 ms |
| Navigation publication | 4.62 ms |
| Displays | 0.38 ms |

Frame p50 was 92.80 ms; p95 was 99.50 ms. Nested scopes overlap their parent
and must not be added to it. Collection includes record synchronization,
lighting bounds, and tree construction; it is not pure BVH build time.
There were 17 scans and 272 rays per tick, examining 1,664 leaves and 14,636
tree nodes in total. The collision fixture had 3,017 bodies, one two-body group,
and no contacts.

The previous demand-driven hash run averaged 125.33 ms combined and 33.35 ms
for client occlusion queries. Treat these as separate fixture runs: ship placement
depends on ECS iteration order, and this change also moves static catalogue
records out of the active index. This is not an isolated backend comparison.
Network I/O, compression, and optical view subscriptions are excluded.

Raw scope measurements: [bvh-16-players.txt](bvh-16-players.txt).
