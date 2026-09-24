# Publication invalidation — 2026-09-24

Moved account-cache change detection to `session::prepare_publication`, once
before each batch of session frames. Cache hits now perform a map lookup and Arc
clone. Removal streams are also drained at `Last` so changes survive multiple
application updates without publication. Preparation runs even while paused and
prunes accounts only while session components are attached.

Callers must prepare again after changing ownership, permissions, identities, or
vessel metadata. Frame generation only mutates publication/session bookkeeping;
input and simulation changes precede the publication boundary.

## Measurements

Ryzen 5 3600XT, release build, static BVH, sparse scene, 2,000 players scattered
across distinct systems with seed `0x5343415454455231`. One repetition, one warmup
tick and five measured ticks for each run; no concurrent builds. Totals include
snapshot serialization and diagnostic instrumentation, exclude network transport
and rendering. Preparation is included in publication time.

The baseline is commit `c3dd74a350225e165fc47b6cf3d9251e0788828d`. This version
already removes society data from per-tick frames, so the earlier 1,670 ms result
is not an appropriate baseline for this fix.

| Mean ms/tick | Before | After |
| --- | ---: | ---: |
| Simulation + publication | 708.8820 | 412.6159 |
| Simulation | 196.3138 | 178.9725 |
| Publication | 512.5682 | 233.6434 |
| Account-cache lookups | 268.1961 | 0.6871 |
| Publication preparation | — | 0.7356 |
| Session navigation | 151.9804 | 148.8341 |
| Serialization | 16.8214 | 16.2523 |

Publication decreased 54.4%; combined mean decreased 41.8%. Cache lookup plus
preparation decreased from 268.2 ms to 1.42 ms. Simulation variation is not an
expected benefit of this publication change. Five early ticks are a short sample.
Scopes nest and must not all be summed.

After the change, every measured tick had exactly one invalidation scan, 2,000
cache hits and zero cache misses. Both runs ended with 78,629 spatial records,
4,997 occupied systems and 5,001 collision bodies. No sensor contacts were selected.

Raw output: [before](publication-invalidation-2026-09-24/before.txt),
[after](publication-invalidation-2026-09-24/after.txt).
After-run binary/source hashes: [sha256](publication-invalidation-2026-09-24/after-sha256.txt).

## Verification

- 19 session tests passed; after extending mutation coverage, all four cache tests
  and eight ownership tests passed again.
- Coverage includes cache retention over 300 accounts, one scan per batch,
  paused permission/name/ownership/control/directory changes, session detachment,
  multiple connections, and removals across four updates without publication.
- Both five-tick benchmarks passed; targeted rustfmt and diff checks passed.
- The star catalogue initially contained an LFS pointer. Hydrated it from the
  previous checkout after verifying its SHA-256 matched the tracked LFS OID
  `2b01514e3ec161582229fac7b3f82624a5b740fff0ff6f9f1856b73fe8a60a94`.

```sh
cargo test --offline --release -p osg-server --lib --no-run
env -u OSG_BENCH_SPATIAL_CAPTURE -u OSG_BVH_PROFILE -u OSG_SPATIAL_PROFILE \
  OSG_BENCH_BVH=static OSG_BENCH_PLAYERS=2000 OSG_BENCH_REPEATS=1 \
  OSG_BENCH_FRAMES=5 OSG_BENCH_WARMUP=1 OSG_BENCH_SCENE=sparse \
  target/release/deps/osg_server-f1823a54d9a109c8 \
  sim::benchmark::production_tick_and_publication \
  --ignored --exact --nocapture --test-threads=1
```
