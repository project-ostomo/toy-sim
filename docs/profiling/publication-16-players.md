# Client publication breakdown

16-player sparse production fixture, BVH backend, release build, 70 warmup
ticks and 300 measured ticks, one repetition. Added scopes around session
snapshot construction and serialization; publication behavior is unchanged.

| Work across all 16 sessions per tick | Mean |
|---|---:|
| Complete session frame construction | 26.801 ms |
| Observable ship selection | 14.442 ms |
| Society and asset snapshots | 10.066 ms |
| Contacts, subscriptions, views and events | 1.483 ms |
| Session navigation snapshots | 0.550 ms |
| Detailed ship presentation | 0.164 ms |
| Ship telemetry | 0.026 ms |
| Screen updates | 0.015 ms |
| Postcard serialization | 0.300 ms |
| Event pruning | 0.003 ms |

Frame construction is the parent of the selection/snapshot rows, and includes
sensor detection inside contacts. Full publication including global navigation
and display updates averaged 32.365 ms. Serialization includes freeing the
serialized byte buffer.

`session::observable_ships` scans all vessels for every session, runs
`commands::observe` (View authorization, falling back to Control authorization),
sorts eligible ships, and truncates to 64. The truncation happens after scanning.
In this fixture that repeats over about 3,017 vessels for each of 16 players.
Failed authorization constructs errors even though selection only needs a boolean.

`ownership::snapshot` clones the society directory, scans the entire identity
index, checks ownership/access, builds and sorts visible asset records, and
collects gas accounts. This repeats every publication for each account.
The measurement covers the whole function; it does not isolate directory cloning
from the identity scan.

These two scopes explain 24.508 ms per tick. Candidates for improvement are
revision-based caching of per-account visibility/affiliation data and reusable
ECS queries, with explicit invalidation for ownership, ACL, membership, and
asset-lifecycle changes. Live ship telemetry can still update each tick.

Raw measurements: [publication-16-players.txt](publication-16-players.txt).

## Basic account cache

The same 16-player benchmark with the account cache, 300 measured ticks:

| Scope | Before | Cached |
|---|---:|---:|
| Full publication | 32.365 ms | 10.278 ms |
| Session frame construction | 26.801 ms | 4.786 ms |
| Observable ship selection | 14.442 ms | 1.185 ms |
| Society snapshot | 10.066 ms | 1.398 ms |

Combined simulation and publication averaged 72.651 ms; p50 was 72.003 ms
and p95 79.226 ms. Cache lookup/invalidation checks took 2.336 ms across all
sessions, nested within selection and society timings.

The bounded cache shares results per account and invalidates all accounts for
changes to identities, controls, vessel names, ownership, ACLs, or the society
directory, including component removals. Application-update maintenance consumes
removal notifications even without publication requests. Identity cleanup now
marks its index changed only when it actually removes entries.

Focus ordering, presence, telemetry, and gas balances are read live. This is a
temporary cache around the existing queries; invalidation is deliberately broad
until queryable ownership storage replaces it.

Validation covers cache reuse, access grants/revocation, renaming, despawning,
and the existing focused/controllable ship prioritization case.

Raw measurements: [publication-cache-16-players.txt](publication-cache-16-players.txt).
