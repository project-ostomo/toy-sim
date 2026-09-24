# Spatial collection breakdown

16-player sparse release benchmark, BVH and publication cache, 70 warmup ticks
and 300 measured ticks. Added phase timings and collection counters only.

| Phase, summed per tick | Mean |
|---|---:|
| ECS collection and metadata construction | 8.428 ms |
| Update authoritative index records | 13.744 ms |
| Explicit BVH rebuilds | 10.989 ms |
| Build present-entity set and remove stale entries | 1.117 ms |
| Clear previous metadata | 0.165 ms |
| Projectile collection | 0.0001 ms |
| Full collection including clearing | 34.449 ms |

Rounding and small unscoped gaps account for the difference between the phase
sum and the parent total. Luminosity bounds take 1.078 ms inside record updates.
Record updates include per-object locking, radius/luminosity selection, map
lookups/insertion, change comparison, and invalidating/dropping a previous tree.
Those individual operations have not been timed separately.

There are two complete collections per tick: before collision processing and
in FixedLast. Each processes 46,908 objects, including 43,891 celestials and
3,017 other bodies. Totals are 93,816 object visits and 87,782 celestial visits
per tick. The million static catalogue entries are outside this active index.

Explicit rebuild time here excludes any deferred tree construction charged to
collision discovery. This table therefore must not be treated as all BVH build
time across the frame.

Raw measurements: [spatial-collection-16-players.txt](spatial-collection-16-players.txt).
