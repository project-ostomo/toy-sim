# Server frame scaling: 1, 32, 128, and 2,000 players

AMD Ryzen 5 3600XT, six cores / twelve threads, release build. The server uses
an immutable BVH rebuilt every tick; the current default builder is median,
not Morton. Sparse fixture: player ships are placed along a line at 2,000 km
spacing. Each player has a session subscribing to instruments and a screen.
Timings include simulation, publication and snapshot serialization; rendering,
network transport, and compression are excluded. Profiling scopes and query
counters are enabled. Runs were serial with no concurrent builds/tests from
this session; other host activity was not isolated.

One repetition is reported per count. The 1-, 32-, and 128-player samples use
70 warmup ticks and 300 measured ticks. The 2,000-player sample uses **one warmup
tick and five measured ticks**, shortened at the user's request; treat it as
a short early-run measurement, not a warmed steady-state distribution. The
original long 2,000-player run was interrupted before producing a summary.
The initial 1-player command completed three repetitions before the requested
reduction; only repetition zero is used below, while the raw file retains all
three. The interrupted 32-player attempt is excluded.

| Players | Simulation mean (ms) | Publication mean (ms) | Frame mean (ms) | Frame median (ms) | Frame p95 (ms) |
|---|---:|---:|---:|---:|---:|
| 1 | 61.1600 | 6.7249 | 67.8849 | 65.880 | 81.729 |
| 32 | 57.3965 | 23.4427 | 80.8392 | 79.336 | 92.433 |
| 128 | 61.9718 | 181.9468 | 243.9186 | 242.695 | 262.229 |
| 2,000 | 98.2898 | 12,219.9825 | 12,318.2723 | 12,273.663 | 12,748.697 |

With only five samples the reported 2,000-player p95 is the maximum sample, not
a robust tail-latency estimate. Frame means sum the measured simulation and
publication means. Other scopes below overlap and must not all be summed.

## Larger-count breakdown

| Mean milliseconds per tick | 32 players | 128 players | 2,000 players |
|---|---:|---:|---:|
| Spatial index replacement | 13.7980 | 14.7713 | 16.9040 |
| Collision discovery | 8.1226 | 8.5861 | 16.7299 |
| Sensor detection, including selection and occlusion | 7.5009 | 135.4996 | 2,827.4310 |
| Sensor range queries, inside detection | 0.2735 | 2.9661 | 108.4823 |
| Sensor segment queries, inside detection | 6.6809 | 130.5578 | 2,673.7950 |
| Account-view cache access and construction | 5.9808 | 19.5500 | 8,244.5288 |
| Observable-ship selection, including cache access | 3.1467 | 9.8860 | 8,055.6240 |
| Session society data, including cache access | 3.4419 | 12.9238 | 440.8648 |
| Session navigation | 1.4288 | 7.8688 | 283.5018 |
| Snapshot serialization | 0.7102 | 3.5642 | 329.3081 |
| Global navigation publication | 5.7342 | 5.9761 | 7.5724 |
| Display updates | 0.7601 | 2.5836 | 9.3233 |

Account-view caching is capped at 256 entries and clears the entire cache at
that threshold. With 2,000 distinct accounts processed sequentially, retained
views cannot cover the working set. Cache misses rebuild ship visibility and
permission results. This explains a concrete source of repeated work in the
dominant account-cache scope; the timing is not a BVH rebuild cost.

## Sensor work

| Per tick | 1 player | 32 players | 128 players | 2,000 players |
|---|---:|---:|---:|---:|
| Observer scans | 2 | 33 | 129 | 2,000 |
| Range results, including self | 2 | 1,025 | 10,377 | 199,448 |
| Selected contacts / occlusion rays | 0 | 992 | 10,248 | 197,448 |
| Segment BVH probes | 0 | 55,544 | 1,111,844 | 24,284,748 |
| Segment candidates examined | 0 | 11,904 | 250,698 | 5,211,498 |

The default starter sensor range is 100,000 km, while adjacent player ships are
2,000 km apart. Each observer searches within range, removes itself, selects up
to 256 nearest candidates, and performs one segment query per selected candidate
when occlusion is enabled. Cache hits for the same observer and scene revision
reuse the observation. The ray count is directed: A observing B and B observing
A are separate queries. The fixture has additional observer activity beyond
the requested player count.

For an ideal line of 128 ships within 50 neighbour spacings, directed pairs total
`2 * sum(128 - d, d=1..50) = 10,250`, close to the measured 10,248. This idealized
count is explanatory rather than an exact reconstruction of simulated positions.
The counters give the exact reconciliation: `10,377 - 129 = 10,248` after removing
self-results. At one player, the two scans return only self-results and no rays
are cast. These zero-ray results are specific to this fixture.

Segment candidates are collected before the caller tests for a blocking object.
Thus finding an early blocker does not terminate BVH traversal early. The
128-player rays average approximately 108.5 BVH probes each and cost 130.6 ms
combined. This is publication-time sensor visibility work, separate from
collision discovery and Rapier CCD.

## Reproduction

The existing harness now accepts `OSG_BENCH_WARMUP` with default 70 and records
the chosen warmup in its result line. Only this harness setting was changed to
enable the short 2,000-player run. Recorded BVH, wrapper, server-index, and
firmware hashes matched between the long and short runs.

```sh
OSG_BENCH_BVH=static OSG_BENCH_SCENE=sparse OSG_BENCH_PLAYERS=128 \
  OSG_BENCH_REPEATS=1 OSG_BENCH_FRAMES=300 OSG_BENCH_WARMUP=70 \
  cargo test --release --offline -p osg-server --lib \
  sim::benchmark::production_tick_and_publication -- --ignored --exact --nocapture --test-threads=1
```

For the short sample use `OSG_BENCH_PLAYERS=2000 OSG_BENCH_FRAMES=5
OSG_BENCH_WARMUP=1`. Clear `OSG_BENCH_SPATIAL_CAPTURE`, `OSG_BVH_PROFILE`, and
`OSG_SPATIAL_PROFILE` when measuring. The actual runs invoked saved release test
executables directly after building, with these settings explicitly supplied.

[Raw outputs and hashes](player-scaling-2026-09-23/).
