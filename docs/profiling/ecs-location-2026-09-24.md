# ECS locations and shared orrery cache

## Implementation

Activated celestial entities now carry Hill radius and ancestry. A persistent,
dedicated BVH reads their current ECS transforms once per tick. Parallel queries
populate `SpatialLocation` components; docked objects inherit their host's
context. The location pass has no universe dependency and does no orbital
evaluation. System roots that are barycenters have nonphysical region entities.
Startup and restoration rebuild these derived components before publication.

Orreries use a process-wide lazy Moka cache with capacity 4,096, keyed by universe
fingerprint and system identity. Concurrent misses share generation. Active
systems retain immutable `Arc` references independently of cache eviction.

## Normal client measurements

Both runs used the user's commands:

```sh
cargo build -p osg-server
cargo build -p osg-debug -p osg-client
cargo run -p osg-debug
```

The client ran visibly on the desktop with the Intel Arc B570 Vulkan backend,
using fresh default debug state and ordinary UI controls. Server timing and
client diagnostic logging were enabled. No other builds or benchmarks ran during
the comparison. The previous debug save is preserved under
`target/live-debug-repro/baseline-world.sqlite`.

| Steady idle metric | Before | After |
| --- | ---: | ---: |
| Samples | 201 | 201 |
| Median simulation tick | 155.930 ms | 74.628 ms |
| 95th percentile | 189.885 ms | 83.933 ms |
| Maximum | 475.304 ms | 93.404 ms |
| Ticks exceeding 100 ms | 201 | 0 |

Both samples use ticks 900–1100 with the game filling its desktop workspace.
Desktop recording had stopped before the baseline interval. Median tick time
decreased by 52.1%. An earlier 501-tick idle sample also stayed below 100 ms,
with snapshot publication taking a median 7.235 ms, measured separately.
The client reported zero underruns during these idle samples.

The route to HIP 8433 was planned and engaged through the visible map. Ticks
2100–3199 cover engagement, charging and transit: median 73.798 ms, p95 81.779 ms,
maximum 95.874 ms, and no ticks above 100 ms across 1,100 samples. The client had
one brief underrun during route preview at tick 1747, then maintained a small
buffer without further underruns through the measured interval. This verifies
entry into transit; it does not cover the complete journey or arrival.

These are observations from the interactive debug build on this machine, with
desktop activity and changing window layout; they are not an isolated benchmark.

## Correctness checks

- All 27 targeted server tests passed across the initial run and the rerun of
  corrected fixtures. Coverage includes exhaustive membership comparison,
  movement/removal, nested docking, barycenter isolation, physical-index
  isolation, telemetry/services, and restored docking/transit contexts.
- Five universe tests and the full-catalogue lazy-loading test passed, including
  concurrent initialization, universe isolation, and eviction/regeneration.
- The server all-targets compile check and normal server/debug/client builds
  passed.

Server test logs, both runtime logs, and the visible route/transit screenshots
are in `target/live-debug-repro/`. Rendering overexposure remains visible in the
transit screenshot and requires its own investigation.
