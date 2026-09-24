# Remaining tick cost, 2026-09-24

Investigation using the normal visible `cargo run -p osg-debug` client and its
server. No implementation changes were made during this investigation.

## Measurements

With detailed spatial profiling disabled, warmed ticks 1360–1500 (141 samples)
had a median wall time of 74.963 ms and p95 of 84.277 ms. A subsequent ten-second
CPU sampling run collected 6,491 samples with no lost samples.

The world contained 46,893 spatial records, including 43,891 celestials and
3,002 other objects. The default world instantiates navigation installations
for 3,000 settlements, keeping their systems active.

CPU self samples included:

| Function or operation | Share |
| --- | ---: |
| `__floatuntidf`, unsigned integer to floating point conversion | 23.97% |
| AABB distance iterator | 13.04% |
| Hill query closure | 5.62% |
| `LuminosityBvh::try_visit<Entity>` | 4.72% |

Resolved stacks connect location updates to Hill queries, BVH traversal, AABB
distance calculations, and integer conversions. Inclusive percentages overlap;
these CPU measurements are not a decomposition of tick wall time.

## Findings and proposed changes

1. `location::update_locations` queries Hill membership for celestials as well
   as ships. Celestials account for about 94% of queried objects and already
   have no need for location membership. Exclude them from geometric location
   discovery and do not provision location components for them.
2. Hill membership uses a radius-zero spatial query. Internal bounding-box
   checks calculate floating point squared distances from integer coordinates.
   Traversal calculates both child distances again to choose visitation order.
   An unordered point-containment traversal using integer AABB comparisons could
   retain the exact sphere check at leaves. Deferred at the user's request.
3. The physical and Hill trees are rebuilt each tick. Revisit this remaining
   cost after measuring the first two changes. Existing dynamic BVH benchmarks
   in this directory were slower than static rebuilds, so switching backends
   alone is not a supported optimization.

Detailed spatial scope logging was also collected, but produces thousands of
log lines each tick and inflates timing. Its scope times should not be used to
estimate savings against the normal run. The measured costs above come from
the run with those detailed scopes disabled.

## Local artifacts

- `/tmp/ecs-location-sampling.log`: normal run with tick timings.
- `/tmp/ecs-location-sampling-perf.data`: ten-second CPU sample.
- `/tmp/ecs-location-detail.log`: detailed instrumentation, timing overhead.
- `/tmp/ecs-location-detail-perf.data`: instrumented CPU sample.

Both visible runs were closed cleanly after profiling. Following the user's
clarification, celestial membership queries and location provisioning were
removed. The traversal optimization is deferred. No performance reduction is
claimed without a new measurement.
