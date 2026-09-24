# Per-tick Morton BVH rebuilding

The server default now selects `GalacticIndex::bvh()`, rebuilding at each tick's
publication boundary. The current `LuminosityBvh::build` delegates to
`build_morton`, using Morton prefixes rather than the earlier spatial median
partitioning. No partitioning code was changed for this measurement.

Release build, 16 players, sparse scene, three serial repetitions with 70 warmup
ticks and 300 measured ticks each. Each run has 46,908 spatial records and 3,017
collision bodies, with no collision groups or contact pairs. Simulation and
client publication are timed; rendering, network transport, compression, and
setup are excluded. This session ran no builds or tests during timing. Host
activity was not isolated.

| Run | Mean frame (ms) | Frame p50 (ms) | Frame p95 (ms) | Mean spatial replacement (ms) |
|---|---:|---:|---:|---:|
| 1 | 71.8001 | 70.427 | 84.752 | 14.5442 |
| 2 | 73.4145 | 70.691 | 88.471 | 14.2228 |
| 3 | 73.5032 | 71.709 | 87.474 | 14.9823 |

Mean frame time across all 900 measured frames is **72.9059 ms**. Percentiles
are reported per repetition rather than averaged into a pooled percentile.

| Mean milliseconds | Earlier median rebuild | Latest fat dynamic | Current Morton rebuild |
|---|---:|---:|---:|
| Simulation | 57.6337 | 72.4183 | 59.6967 |
| Publication | 12.8995 | 13.8188 | 13.2092 |
| Frame | 70.5332 | 86.2372 | 72.9059 |
| Spatial replacement | 13.9736 | 25.2333 | 14.5831 |
| Collision discovery | 8.0240 | 9.5906 | 8.7880 |

Compared with the latest fat-AABB run, frame time is 15.5% lower and spatial
replacement time is 42.2% lower. Compared with the earlier median rebuild run,
frame time is 3.4% higher and replacement time is 4.4% higher. These historical
comparisons support returning to rebuilding on this workload, but do not
establish a Morton-specific speedup or regression. Source changes, firmware
changes, scene behavior, and host timing variation prevent isolating the
partitioning algorithm's effect. Spatial replacement includes record-map
construction and other work in addition to tree construction.

The earlier median scene had one collision group; the current scene and latest
fat-AABB rerun both have none. The latest fat-AABB run also had a slower third
repetition; all repetitions remain included in the reported means.

Verification of the default-backend change: 63 spatial unit tests passed;
targeted server checks passed 56 tests with the previously recorded failure in
`due_shots_launch_together_and_ccd_reports_their_impacts` (zero impacts rather
than two). The three-repetition production benchmark passed.

Reproduction with the current release server test executable:

```sh
env -u OSG_BENCH_SPATIAL_CAPTURE -u OSG_BVH_PROFILE -u OSG_SPATIAL_PROFILE \
  OSG_BENCH_BVH=static OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=3 \
  OSG_BENCH_FRAMES=300 OSG_BENCH_SCENE=sparse \
  target/release/deps/osg_server-505a05606d0c1953 \
  sim::benchmark::production_tick_and_publication \
  --ignored --exact --nocapture --test-threads=1
```

[Raw results](morton-rebuild-frame-2026-09-23/16-players.txt),
[source and executable hashes](morton-rebuild-frame-2026-09-23/source-sha256.txt),
[earlier median rebuild comparison](dynamic-bvh-2026-09-23.md), and
[latest fat-AABB rerun](fat-dynamic-bvh-frame-rerun-2026-09-23.md).
