# Fat bounds in the dynamic BVH — September 23, 2026

`DynamicLuminosityBvh` now retains fat leaf bounds across small movements.
The server's dynamic `GalacticIndex` uses this policy automatically.

## Maintenance and query behavior

Each leaf stores its current exact AABB separately from its retained tree box.
Contained movements publish geometry and payload without refitting ancestors
unless luminosity changes. Luminosity maxima propagate increases and decreases.
Branch traversal uses conservative fat bounds; leaf callbacks receive exact
bounds. Query result sets therefore retain their existing geometric filtering.

Per-axis padding is one percent of the current extent, rounded down with a
minimum of one coordinate unit, plus four times the largest observed endpoint
displacement on that axis. Displacement is capped at 64 times the largest current
box extent, with a minimum extent of one unit. This permits fast local motion but
bounds the padding from a teleport even when a caller skips stationary records.
Integer arithmetic saturates at the i128 coordinate limits.

An update replaces padding when the exact box escapes, or when the old fat box
does not fit an envelope with four times the newly calculated padding. Thus
shrinking objects and slowing motion can shed excessive padding on subsequent
updates. Unchanged records skipped by `GalacticIndex` retain their bounded padding.
Existing refit/reinsertion and balancing policies handle the replacement box.
Batch maintenance skips redundant node writes and surface-area rotation searches
on ancestors whose boxes remain unchanged. There is no automatic full rebuild.

## Captured replay

The same 100 snapshots and query inputs used in the
[unpadded baseline](dynamic-bvh-2026-09-23.md) were replayed three times.
Initial construction is excluded, leaving 99 measured updates per repetition.
Radius and segment ID sets match the immutable backend on every frame.

| Mean per frame | Unpadded dynamic | Fat dynamic |
|---|---:|---:|
| Changed retained boxes | 43,954.0 | 3,079.6 |
| Reinsertions | 536.4 | 108.6 |
| Rotations | 846.3 | 238.1 |
| Query node visits | 105,392.4 | 105,413.5 |
| Query leaf tests | 3,147.0 | 3,147.0 |
| Maximum height | 17 | 17 |
| Tree-vector capacity, bytes | 17,364,039 | 21,867,207 |

Retained-box changes fall 93.0%, reinsertions 79.8%, and rotations 71.9%.
Exact leaf storage adds 4,503,168 bytes for 46,908 records. Storage figures exclude
wrapper maps, wrapper scratch vectors, and allocations owned by payloads.

The synthetic replay also verifies result equality across motion, teleports,
luminosity changes, insertions, and removals. Its query costs still deteriorate
over time: fat bounds do not fix the existing topology-quality drift.

## Timings and limits

These measurements do **not establish a reliable wall-clock speedup**. The
unpadded executable was saved before this change; the workspace also received
independent bulk-builder changes during this work. The host was not isolated,
and control timings varied substantially. No builds, tests, or other benchmarks
were run concurrently by this session during the final timing sequence.

Three captured replay repetitions average:

| Mean milliseconds | Saved unpadded executable | Final fat executable |
|---|---:|---:|
| Dynamic update | 27.3573 | 32.1292 |
| Dynamic queries | 8.7746 | 10.1962 |
| Dynamic combined | 36.1319 | 42.3254 |
| Static control combined | 21.2853 | 26.3511 |

Both backends became slower across these runs. Do not interpret the dynamic
timing difference as an isolated effect of fat bounds, or normalize it into a
claimed speedup. The maintenance counters above are the reproducible benefit.
The final synthetic replay averages 8.2662 ms for dynamic update plus queries,
versus 4.3835 ms for immutable rebuilding plus queries.

Production tests used 16 players, sparse scenes, 70 warmup ticks and 300 measured
ticks, three repetitions per backend. Both configurations use the same final
release executable. All dynamic repetitions precede all static repetitions.

| Mean milliseconds | Fat dynamic | Static rebuild |
|---|---:|---:|
| Simulation | 75.7170 | 57.0556 |
| Publication | 13.4425 | 11.9920 |
| Combined | 89.1595 | 69.0476 |
| Spatial replacement | 26.7317 | 14.5364 |
| Collision discovery | 9.8275 | 7.8884 |

Fat dynamic frame medians were 96.729, 83.440, and 81.094 ms; corresponding
95th percentiles were 132.155, 96.269, and 88.100 ms. Static frame medians were
66.385, 65.844, and 69.926 ms. The substantial variation prevents a clean
comparison with the earlier unpadded dynamic frame mean of 86.6651 ms. Dynamic
maintenance remains slower than rebuilding on this measured workload; no
greater-than-30-percent frame speedup is demonstrated.

## Verification

Spatial crate tests cover randomized differential queries and structural
invariants, plus contained movement through both single and batch updates,
luminosity increases/decreases inside retained bounds, teleport padding caps,
shrinkage, stale handles, and coordinate limits. All 63 spatial unit tests and
the crate doctest pass. Server collision, spatial, sensors, optical, and slip
checks yield 56 passes and one existing failure:
`due_shots_launch_together_and_ccd_reports_their_impacts` reports zero impacts
instead of two. The same failure was reproduced in the saved pre-implementation
binary in the unpadded report; this change does not resolve it.

Raw reports are in [fat-dynamic-bvh-2026-09-23](fat-dynamic-bvh-2026-09-23/).
The captured binary input remains `/tmp/osg-dynamic-bvh/active-scene.bin`.
Use the replay and production benchmark commands in the unpadded report to
reproduce; `bounds_updates` now counts retained-box changes, not moving records.
