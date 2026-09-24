# Fat-AABB production frame rerun

Release build, 16 players, sparse scene, 46,908 spatial records and 3,017
collision bodies. Three serial repetitions each use 70 warmup ticks and 300
measured ticks. No builds or tests were run concurrently by this session.
Host activity was not isolated. Frame timings cover simulation and client
publication, excluding rendering, network transport, and compression.

| Repetition | Mean frame (ms) | Frame p50 (ms) | Frame p95 (ms) | Spatial replacement mean (ms) |
|---|---:|---:|---:|---:|
| 1 | 82.3609 | 81.951 | 88.653 | 24.4609 |
| 2 | 81.8325 | 81.146 | 89.090 | 23.7092 |
| 3 | 94.5181 | 87.449 | 143.310 | 27.5298 |

The mean across all 900 measured frames is **86.2372 ms**. The third repetition
has materially higher latency and is included in that mean. Percentiles above
are per repetition, not pooled percentiles.

| Mean scope | Milliseconds |
|---|---:|
| Simulation | 72.4183 |
| Client publication | 13.8188 |
| Spatial collection loop, including replacement | 33.0757 |
| Spatial replacement | 25.2333 |
| Collision discovery | 9.5906 |

Scopes are nested: do not add all rows. Mean frame time is simulation plus
publication. Every repetition averages 3,153.940 retained-bound changes and
82.400 reinsertions per tick. There are no contact pairs or collision groups in
this current sparse fixture.

The first attempt failed during provisioning because the bundled controller
exported ABI version 49 while the current host expected version 50. Rebuilt
`crates/osg-ships/data/example-controller.wasm` from the current firmware source
and rebuilt the server before the successful run. No BVH source changes were
made for this rerun. Firmware and other workspace changes mean this is a current
frame measurement, not an isolated comparison with the earlier implementation.

Reproduction after building the current bundled firmware and release server:

```sh
env -u OSG_BENCH_SPATIAL_CAPTURE \
  OSG_BENCH_BVH=dynamic OSG_BENCH_PLAYERS=16 OSG_BENCH_REPEATS=3 \
  OSG_BENCH_FRAMES=300 OSG_BENCH_SCENE=sparse \
  target/release/deps/osg_server-505a05606d0c1953 \
  sim::benchmark::production_tick_and_publication \
  --ignored --exact --nocapture --test-threads=1
```

[Raw output](fat-dynamic-bvh-2026-09-23/frame-rerun-16.txt) and
[source, firmware, and executable hashes](fat-dynamic-bvh-2026-09-23/frame-rerun-sha256.txt).
