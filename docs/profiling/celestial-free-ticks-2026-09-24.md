# Tick timings after removing celestial location queries

Measured on 2026-09-24 with the normal visible `cargo run -p osg-debug`
client, after building `osg-server`, `osg-debug`, and `osg-client`. Both runs
loaded the same saved idle world in sequence and were closed cleanly. This is
an idle measurement, not a slip or route-planning measurement.

Population: 46,893 spatial objects, including 43,891 celestials, 3,002 other
objects, and two ship computers running firmware. The first 50 ticks of each
run were excluded as warmup.

## Overall results

| Run | Samples | Mean tick | Median tick | p95 tick | Maximum tick |
| --- | ---: | ---: | ---: | ---: | ---: |
| Scope logging disabled, ticks 3134–3537 | 404 | 60.630 ms | 59.792 ms | 69.169 ms | 82.310 ms |
| Selected scopes enabled, ticks 3588–4087 | 500 | 60.838 ms | 60.036 ms | 68.004 ms | 78.438 ms |

The previous measurement before celestial queries were removed was 74.963 ms
median and 84.277 ms p95. The new median is about 20% lower. These are successive
measurements of the idle world, not a randomized comparison.

Selected scope logging changed the observed mean by 0.208 ms (0.34%); this
comparison does not show a material instrumentation effect. It does not isolate
logging overhead from normal run-to-run variation.

## Phase durations

The following are wall-clock durations from the 500-tick profiled run. Nested
rows are included in their parents. Other ECS systems and scheduling overhead
are not all timed, and concurrently scheduled systems can overlap; this table
is not an additive accounting of every millisecond.

| Phase | Mean | p95 |
| --- | ---: | ---: |
| Physical spatial collection and index construction | 19.921 ms | 25.718 ms |
| ↳ Collect ECS records | 5.618 ms | 6.792 ms |
| ↳ Prepare spatial records | 1.807 ms | 2.192 ms |
| ↳ Replace physical BVH | 12.330 ms | 18.203 ms |
| Location refresh | 12.645 ms | 14.876 ms |
| ↳ Rebuild Hill BVH | 11.320 ms | 13.549 ms |
| Collision processing, excluding spatial collection | 11.092 ms | 12.761 ms |
| ↳ Collision integration | 8.435 ms | 9.706 ms |
| Celestial motion | 3.829 ms | 4.810 ms |
| Gravity | 3.203 ms | 3.876 ms |
| System activation checks | 2.563 ms | 3.560 ms |
| Ship firmware system | 0.732 ms | 0.951 ms |
| Hardware utilities | 0.724 ms | 0.913 ms |
| Reactor generation | 0.577 ms | 0.756 ms |
| Cooling | 0.340 ms | 0.476 ms |
| Reactor processing | 0.056 ms | 0.082 ms |
| Sensor publication | 0.098 ms | 0.180 ms |

Location refresh minus Hill rebuilding averages 1.325 ms. This includes
membership queries, component provisioning, docked-location inheritance, and
system dispatch; it is not a direct measurement of queries alone.

The two BVH replacement scopes total 23.650 ms per tick, about 39% of mean tick
time. Including ECS collection and record preparation makes spatial collection
plus Hill rebuilding about 31.2 ms. This is the largest remaining measured cost.

Display and snapshot publication happens outside the simulation tick. It adds
7.446 ms on average (p95 8.600 ms) in the profiled run, giving approximately
68.284 ms for simulation plus publication. The unprofiled run averages
7.495 ms for publication. Network handling and persistence are not included in
that sum.

## Instrumentation and artifacts

Added `OSG_SPATIAL_PROFILE_SCOPES`, a comma-separated list of scope names, to
restrict the existing opt-in profiler. This avoids the thousands of per-object
log messages that inflated the previous detailed profiling run. No simulation
optimization was made in this measurement pass; the traversal change remains
deferred.

Raw logs, JSON summaries, and the analysis script are in
`target/live-debug-repro/celestial-free/`. The selected scope names are recorded
in the JSON summary. Profiling was enabled with `OSG_SPATIAL_PROFILE=1` and
`RUST_LOG=info,osg_server::timing=debug,osg_server::profile=debug`; the control run
enabled only `osg_server::timing=debug`.

Verification: normal server/debug/client builds passed, and formatting checks
passed for the instrumentation change.
