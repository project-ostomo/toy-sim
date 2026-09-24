# Account-view cache retention fix

Removed the 256-account clear-all capacity policy. Cached views now remain
available for connected accounts. At the application update boundary, prune
views whose accounts have no attached sessions. Multiple sessions for the same
account share a view; disconnecting one preserves it until the last session
leaves. Direct session despawning is handled by the same pruning pass.

ECS changes affecting identities, ownership, access policies, vessel names, and
the ownership directory still invalidate views. Lookups check invalidation but
do not prune: session publication temporarily removes its Session component
from the world, so pruning there would evict the account currently publishing.

All 29 selected session and ownership tests pass. New regressions cover repeated
hits across 300 connected accounts, the temporary Session removal used by frame
publication, multiple sessions per account, disconnection, direct despawning,
and reconnection. Existing permission, name, and entity-removal tests still pass.

## Short 2,000-player comparison

AMD Ryzen 5 3600XT, release build, sparse scene, immutable BVH rebuilt per tick.
Both samples use one warmup tick followed by five measured ticks. No other
builds or tests were run by this session during timing. Host activity was not
isolated. This is a short early-run comparison, not a steady-state tail study.

| Mean milliseconds per tick | Before | After |
|---|---:|---:|
| Simulation | 98.2898 | 98.8886 |
| Publication | 12,219.9825 | 4,082.6081 |
| Combined | **12,318.2723** | **4,181.4967** |
| Account-cache access and construction | 8,244.5288 | 324.0523 |
| Observable-ship selection, including cache access | 8,055.6240 | 165.1564 |
| Sensor segment queries | 2,673.7950 | 2,740.8707 |

Frame time falls approximately 66.1%; the account-cache scope falls 96.1%.
Scopes overlap and must not all be added. Sensor segment queries now dominate
the remaining frame cost. The cache still performs freshness checks on lookups
and globally invalidates views on relevant ECS changes.

The first verification attempt encountered a separate firmware mismatch: the
host now expects ABI 51 while the bundled firmware was ABI 50. The bundled
`example-controller.wasm` was rebuilt from current source before successful
tests and timing. The earlier timing used ABI 50, so this historical comparison
does not isolate the cache change from all other workspace changes.

[Raw benchmark](account-cache-fix-2026-09-23/2000-players.txt),
[test results](account-cache-fix-2026-09-23/tests.txt),
[hashes](account-cache-fix-2026-09-23/source-sha256.txt), and
[baseline](player-scaling-2026-09-23.md).
