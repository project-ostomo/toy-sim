# Navigation and publication — 2026-09-24

Baseline: remote correctness commit `1843a74` plus the local publication-boundary
account-cache fix. Ryzen 5 3600XT, release test executable; each run uses one
repetition, one warmup tick and five measured ticks. The 2,000 player ships are
scattered across distinct systems using seed `0x5343415454455231`. Static BVH,
sparse scene, one session per player, no view subscriptions, screens and instruments
subscribed. No builds or other benchmark runs overlapped the timed runs.

## Changes

- Build a sorted system-to-beacon index once during navigation publication. Use
  it for nearby beacons and slip-route targets, retaining route priority, ID order,
  deduplication, and the existing output limit.
- Cache exact system membership by integer position for the publication batch.
  Use activation's region candidates and retained definitions, with exact influence
  checks and the universe query for missing regions. Preserve overlapping systems.
  Resolve ship positions through the shared dock/transit pose function.
- Share navigation snapshots by nearby-system and route-target sets within a
  batch. Rebuild on every publication, including paused publication and pose-only
  changes; topology revision alone does not determine freshness.
- Share account-independent device readings by entity between detailed and visual
  publication. RPC queries get fresh reading storage. Retain per-recipient access
  checks and instrument choices. Reuse the focused-ship set and visit only the
  unsent suffix of event history, preserving chronological delivery.

Publication order is `publish_navigation` (starts the navigation batch), display
updates, `session::prepare_publication` (account freshness and fresh reading storage),
then session frames. No protocol format, update-rate, or threading changes.

## Results

| Mean ms/tick | Updated baseline | Beacon index | Membership sharing | Final |
| --- | ---: | ---: | ---: | ---: |
| Combined | 466.8444 | 319.1587 | 248.5226 | 273.0736 |
| Simulation | 200.1748 | 188.0162 | 169.0801 | 187.0679 |
| Publication | 266.6696 | 131.1425 | 79.4425 | 86.0057 |
| Session navigation | 173.1261 | 51.3430 | 4.2470 | 4.5541 |

Final publication is 67.7% lower than baseline; combined mean is 41.5% lower.
Session navigation is 97.4% lower. Simulation differences are run variation, not
an expected benefit of these publication changes. Five early ticks are a short
sample. The final stage is slower than the membership-only sample; these results
do not establish an incremental speedup from device-reading sharing in a scene
with one observer per ship.

Final median combined tick: 275.609 ms; maximum: 292.171 ms. The 90 ms combined
goal has not been reached. Simulation alone averaged 187.1 ms. Major remaining
scopes include spatial replacement (54.0 ms), location refresh (37.2 ms), and
celestial movement (21.5 ms).

Final publication details: global navigation preparation 11.20 ms, exact membership
resolution 4.41 ms, beacon-index construction 0.60 ms, session navigation 4.55 ms,
ship presentation 12.97 ms, serialization 16.22 ms. Scopes overlap; do not sum
parent and child timings. Preparation is included in publication totals.

The 2,000-player final run resolved 5,001 distinct positions per tick (including
background beacons), had one membership-cache hit, and built 2,000 navigation
selections and 2,000 device-reading snapshots. There were no reading-cache hits;
the large membership improvement primarily comes from the activation region cache
and retained definitions. Final population remained 78,629 spatial records,
4,997 occupied systems and 5,001 collision bodies, with no selected sensor contacts.

## Multiple observers

A separate run used 32 scattered players, four sessions per player and two focused
views per session: 128 sessions and 256 views. Same one-warmup/five-measured protocol.

Per tick: 32 navigation selection builds with 96 hits, 32 device-reading builds
with 224 hits, and 100 membership hits. Duplicate view and ship positions are
deduplicated before queries, so those eliminations are not counted as cache hits.
Publication averaged 12.92 ms, of which session navigation was 0.144 ms. This
fixture verifies reuse; it has no corresponding pre-change timing baseline.

## Verification and reproduction

Five navigation tests, 19 session tests, eight ownership tests, three identity
tests, and the shared-reading/instrument/freshness test passed. Coverage includes
scan-oracle selection ordering and limits; overlapping systems and exact boundaries
using populated activation caches; movement and teleportation; docked/transit poses;
missing hosts; beacon disablement, dormancy and deletion; snapshot reuse; per-batch
freshness; and chronological event delivery. Formatting and diff checks passed.

```sh
cargo test --offline --release -p osg-server --lib --no-run
env -u OSG_BENCH_SPATIAL_CAPTURE -u OSG_BVH_PROFILE -u OSG_SPATIAL_PROFILE \
  OSG_BENCH_BVH=static OSG_BENCH_PLAYERS=2000 OSG_BENCH_REPEATS=1 \
  OSG_BENCH_FRAMES=5 OSG_BENCH_WARMUP=1 OSG_BENCH_SCENE=sparse \
  OSG_BENCH_SESSIONS_PER_PLAYER=1 OSG_BENCH_VIEWS_PER_SESSION=0 \
  target/release/deps/osg_server-f1823a54d9a109c8 \
  sim::benchmark::production_tick_and_publication \
  --ignored --exact --nocapture --test-threads=1
```

For the sharing fixture, set players to 32, sessions per player to 4, and views
per session to 2. The new options default to 1 and 0 respectively.

Raw results: [baseline](navigation-publication-2026-09-24/before.txt),
[beacon index](navigation-publication-2026-09-24/beacon-index.txt),
[membership](navigation-publication-2026-09-24/membership-sharing.txt),
[final](navigation-publication-2026-09-24/after.txt),
[multiple observers](navigation-publication-2026-09-24/multiple-observers.txt).
[Final binary and source hashes](navigation-publication-2026-09-24/after-sha256.txt)
include documentation-only source edits after the final build.
