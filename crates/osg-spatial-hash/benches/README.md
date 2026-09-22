# Gaia benchmarks

Run the Criterion suite from the workspace root:

```sh
git lfs pull
cargo bench -p osg-spatial-hash --bench gaia
```

For a quick initial measurement, append `-- --quick`. To select a group, append
`-- luminosity_map`, `-- neighbor_map/insert`, or `-- neighbor_map/query`.
Criterion stores its results under `target/criterion`.

The catalogue is decoded once per run and shared by all cases. Each selected
map is populated once and reused for queries and random insertion measurements.
The maps run sequentially, so only one million-star index is resident at a time.

The benchmark uses the workspace's 1,000,000 Gaia DR3 stars directly from
`crates/osg-stars/data/gaia-dr3-earth-million.stars`. It is tracked with Git LFS.
The accompanying ADQL and JSON preserve the selection and import metadata.
The sample selects the brightest sources with G magnitude below 12 and parallax
signal-to-noise above 10; it is not a uniform spatial sample.

The binary is OSGSTAR version 2: a 40-byte header followed by 1,000,000 84-byte
records. The benchmark reads the source ID and three signed i128 micrometre
coordinates, converting each coordinate to integer kilometres by truncation
toward zero. This loses less than one kilometre per axis and fits all positions
in `i64`. It also reads the f64 luminosity from each record. IDs remain the
original 64-bit Gaia source IDs. Loading and conversion are outside the timed
operations.

Fixture SHA-256:
`2b01514e3ec161582229fac7b3f82624a5b740fff0ff6f9f1856b73fe8a60a94`

`neighbor_map/insert/random_into_gaia_1m` and
`luminosity_map/insert/random_into_gaia_1m` measure new points inserted into the
existing million-star map. The shared pool has 4,096 deterministic random points
(PCG seed 20260921), each sampled from a catalogue star with independent uniform
offsets of up to one parsec per axis and that star's brightness. Their IDs are
reserved outside the Gaia ID range.

The timer covers batches of up to 256 insertions. Each batch is removed outside
the timer, keeping population between 1,000,000 and 1,000,256. IDs are absent at
insertion, so these measure new entries rather than position updates. Criterion
reports time per point. Random generation, initial loading, and cleanup are
excluded; allocator and hash-table capacity can be reused across batches.

`neighbor_map/query` uses a prebuilt million-star index. Each iteration consumes
one entire query without collecting results or allocating a result vector.
Queries run from Earth and from a deterministic cycle of 64 catalogue positions,
at radii of 1,000 km, 500 AU, and 1, 10, 100, and 1,000 parsecs. Each case prints
the mean returned-star count before timing. Both `NeighborMap::nearest` and
`nearest_finer` filter by Euclidean distance using f64 squared distances.
Queries centred on a star include that star. Benchmark names end in `_precise`
or `_finer_precise` to distinguish these from historical whole-cell queries.

`luminosity_map/query/earth_magnitude_{0,2,4,6}` consumes every borrowed star visible
from Earth at each apparent-magnitude limit. Names end in `_precise` or
`_finer_precise`; both always filter by each brightness bucket's search radius
before applying the exact visibility test. Before timing, it checks both variants' complete
result set against a full scan of all one million records. There is no result
cap or result-vector allocation in the measured query.

Brightness uses the same calibration as `toy-sim`: solar luminosity `3.6e28`
lumens, solar absolute magnitude `4.83`, and a parsec of
`3.085677581491367e13` km. For magnitude 6 the threshold is:

```text
(3.6e28 / 2^64) / (10 * parsec_km)^2 * 10^((4.83 - 6) / 2.5)
```

Both stored luminosities and the threshold are divided by `2^64`. This preserves
visibility while distributing the catalogue across brightness buckets 19–46;
raw lumens would put every source in bucket 63. The common `4π` factor is omitted
from both sides, matching `LuminosityMap` and the source catalogue's query code.

The suite uses several gigabytes of RAM. Filtering to a benchmark group loads
only the map required for that group, once.

## Cell removal and boxed-set A/B

Boxed sets are now unconditional: cells default to `ArrayVec<T, 1>` and promote
to `Box<AHashSet<T>>`. The `boxed-cell-sets` toggle and unboxed variant have been
removed. Occupied-entry removal remains an optional experiment. Normal benchmark
commands now measure the adopted boxed layout.

The results and commands below preserve the historical A/B experiment; its
boxing feature flag is no longer available. Two independent features tested:

- `occupied-cell-removal`: remove through an occupied map entry, avoiding a
  second lookup when removing an emptied cell;
- `boxed-cell-sets`: store the large-case AHashSet in a Box, shrinking the cell
  enum from 64 to 24 bytes on this 64-bit build. Promotion allocates the box in
  addition to the set storage; querying a large cell adds a pointer indirection.

Both features preserve inline capacity 1, set promotion, and the no-demotion
policy. The normal build was the unboxed baseline. Historical invocations tested
the baseline, each feature alone, and their combination with the same workloads:

```sh
for variant in baseline entry boxed both; do
  case "$variant" in
    baseline) features="" ;;
    entry) features=occupied-cell-removal ;;
    boxed) features=boxed-cell-sets ;;
    both) features=occupied-cell-removal,boxed-cell-sets ;;
  esac
  cargo bench -p osg-spatial-hash --features "$features" --bench movement -- \
    --noplot --save-baseline cell_$variant
  GAIA_REPORT_MEMORY=1 cargo bench -p osg-spatial-hash --features "$features" --bench gaia -- \
    'neighbor_map/(query/earth/100_pc|insert)' --noplot --save-baseline cell_$variant
  GAIA_REPORT_MEMORY=1 cargo bench -p osg-spatial-hash --features "$features" --bench gaia -- \
    luminosity_map --noplot --save-baseline cell_$variant
done
```

Each map family runs in a fresh process for RSS measurements. Each selected map
is populated once and reused. All six movement cases, NeighborMap's standard and
finer 100-pc Earth queries, all eight visibility queries, and both random
insertion workloads run for every variant. Allocation counters are disabled for
timing. Movement final states and Gaia query results are verified outside timers.

### Entry/box results, 2026-09-21

AMD Ryzen 9 5900XT, Rust 1.98.1, optimized bench builds, 30 samples,
1-second warmup and 3-second target measurement. All four variants passed the
unit/integration tests, movement final-state checks, and selected full-catalogue
query comparisons. Builds completed before timing; runs were sequential.

Movement times, **milliseconds per frame of 32,768 existing-object updates**:

| Map | Motion | Baseline | Entry only | Box only | Both |
| --- | --- | ---: | ---: | ---: | ---: |
| NeighborMap | unchanged | 0.559 | 0.585 | 0.643 | 0.564 |
| NeighborMap | slow | 6.196 | 6.321 | 4.619 | 4.418 |
| NeighborMap | fast | 18.654 | 19.481 | 14.017 | 12.946 |
| LuminosityMap | unchanged | 0.618 | 0.609 | 0.693 | 0.608 |
| LuminosityMap | slow | 6.319 | 6.333 | 4.786 | 4.544 |
| LuminosityMap | fast | 18.465 | 19.575 | 13.844 | 13.289 |

Fresh-process RSS growth, **GiB per million-star index**:

| Map | Baseline | Entry only | Box only | Both |
| --- | ---: | ---: | ---: | ---: |
| NeighborMap | 4.526 | 4.528 | 2.589 | 2.589 |
| LuminosityMap | 3.335 | 3.339 | 1.947 | 1.947 |

Boxing reduces memory about **43% for NeighborMap and 42% for LuminosityMap**.
The entry change does not change the storage layout.

Queries, **microseconds per complete query**:

| Query | Baseline | Entry only | Box only | Both |
| --- | ---: | ---: | ---: | ---: |
| NeighborMap Earth 100 pc, standard | 1358.413 | 1362.185 | 1506.165 | 1483.597 |
| NeighborMap Earth 100 pc, finer | 589.179 | 591.457 | 695.112 | 681.442 |
| Visibility magnitude 0, standard | 4.249 | 4.289 | 4.339 | 4.239 |
| Visibility magnitude 0, finer | 40.458 | 40.846 | 40.191 | 40.638 |
| Visibility magnitude 2, standard | 10.566 | 10.263 | 10.602 | 10.509 |
| Visibility magnitude 2, finer | 44.434 | 44.346 | 43.594 | 44.652 |
| Visibility magnitude 4, standard | 124.573 | 120.659 | 122.505 | 124.517 |
| Visibility magnitude 4, finer | 105.001 | 106.185 | 113.732 | 113.709 |
| Visibility magnitude 6, standard | 699.868 | 693.801 | 706.689 | 713.968 |
| Visibility magnitude 6, finer | 381.667 | 392.862 | 408.651 | 409.701 |

Random insertion into the existing Gaia map, **microseconds per point**;
removal remains outside the timer:

| Map | Baseline | Entry only | Box only | Both |
| --- | ---: | ---: | ---: | ---: |
| NeighborMap | 2.077 | 2.227 | 2.000 | 1.890 |
| LuminosityMap | 2.461 | 2.832 | 2.412 | 2.572 |

The entry-only change did not improve movement in the initial run. Boxing alone
cut movement time roughly 24–27%; combining both cut it roughly 28–31%.
Query performance is a tradeoff: the boxed variants were about **9–18% slower**
on the 100-pc NeighborMap queries and about **7% slower** on finer magnitude-6
visibility. New-point insertion did not show a consistent gain across both maps.
These results favor boxing for movement and memory, while preserving the current
layout remained reasonable for query-heavy workloads. Boxing has since been
adopted; entry removal stays opt-in.

Small differences between independently seeded builds should be treated
cautiously. An entry-only query difference cannot be attributed to its removal
algorithm, which is not exercised by the timed query.

A fresh NeighborMap movement repeat ran in reverse order: both, boxed, entry,
baseline. Times below are **ms/frame**, with 95% confidence intervals:

| Variant | Slow | Fast |
| --- | ---: | ---: |
| Baseline | 5.430 (5.318–5.529) | 17.527 (17.288–17.837) |
| Entry only | 5.730 (5.624–5.834) | 18.362 (18.235–18.505) |
| Box only | 4.313 (4.208–4.422) | 12.574 (12.287–13.000) |
| Both | 4.381 (4.295–4.467) | 12.779 (12.603–12.945) |

Boxing's movement benefit repeated: roughly **21% less time for slow motion and
28% less for fast motion** in this repeat. Entry removal again provided no win
on its own. The initial incremental advantage from combining it with boxing
did **not** repeat; boxed and combined confidence intervals overlap. The evidence
supports boxing for movement and memory, without a demonstrated additional
benefit from entry removal. Query regressions remain part of that decision.

## ArrayVec capacity experiment

All builds now use hybrid cells, defaulting to `ArrayVec<T, 1>`. The promotion policy
and set iterator are unchanged: exceeding inline capacity promotes to AHashSet,
and a promoted cell stays a set until removed. No vector can spill to the heap.
The optional features `hybrid-cells-1`, `hybrid-cells-2`, `hybrid-cells-4`,
`hybrid-cells-8`, `hybrid-cells-16`, and `hybrid-cells-32` select benchmark
capacities. If multiple capacity features are enabled, the largest wins;
the normal build uses one. Hybrid storage is unconditional, including with
`--no-default-features`; the set-only path and `hybrid-cells` toggle were removed.

Run the same selected workloads for each capacity:

```sh
for capacity in 1 2 4 8 16 32; do
  cargo bench -p osg-spatial-hash --features hybrid-cells-$capacity --bench movement -- \
    'movement/neighbor_map/(slow|fast)' --noplot --save-baseline arrayvec$capacity
  GAIA_REPORT_MEMORY=1 cargo bench -p osg-spatial-hash --features hybrid-cells-$capacity --bench gaia -- \
    'neighbor_map/query/earth/100_pc' --noplot --save-baseline arrayvec$capacity
  GAIA_REPORT_MEMORY=1 cargo bench -p osg-spatial-hash --features hybrid-cells-$capacity --bench gaia -- \
    'luminosity_map/(query/earth_magnitude_[46]|insert)' --noplot --save-baseline arrayvec$capacity
done
```

`GAIA_REPORT_MEMORY` prints RSS growth after populating a map, before timing.
Select only one map family per process for comparable memory measurements;
otherwise allocator memory retained from a previous map can distort the delta.
Each selected map is built once and reused. Full-scan query checks and movement
position/population checks run outside timing. Promotion and removal tests run
at every capacity, including the one-element boundary.

The size experiment uses singleton-cell movement to expose entry-layout costs
and Gaia's varied cell populations to exercise the promotion thresholds. Larger
inline arrays reserve space in every cell-table entry, including large-set cells.

### ArrayVec results, 2026-09-21

Fresh SmallVec-4 reference and ArrayVec capacities 1, 2, 4, 8, 16, and 32,
run sequentially on AMD Ryzen 9 5900XT with Rust 1.98.1 and ArrayVec 0.7.8.
Criterion used 30 samples, a 1-second warmup, and a 3-second target measurement.
The SmallVec reference used preserved executables from before the dependency
switch. Each Gaia map ran in a fresh process; the reference's RSS was measured
in separate memory-only processes. Allocation counting was disabled for timing.

Movement times are milliseconds per frame of 32,768 updates. Memory is RSS
increase after building the million-star Gaia index, excluding the catalogue.

| Storage | Cell bytes | Slow ms/frame | Fast ms/frame | NeighborMap GiB | LuminosityMap GiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| SmallVec 4 | 64 | 9.55 | 30.51 | 4.52 | 3.32 |
| ArrayVec 1 | 64 | 6.41 | 19.19 | 4.53 | 3.33 |
| ArrayVec 2 | 64 | 7.93 | 23.95 | 4.52 | 3.32 |
| ArrayVec 4 | 64 | 7.83 | 23.84 | 4.52 | 3.32 |
| ArrayVec 8 | 80 | 8.71 | 26.81 | 5.29 | 3.88 |
| ArrayVec 16 | 144 | 9.36 | 25.86 | 8.36 | 6.15 |
| ArrayVec 32 | 272 | 9.37 | 26.00 | 14.56 | 10.70 |

Gaia visibility and random insertion, **microseconds per operation**:

| Storage | Mag 4 standard | Mag 4 finer | Mag 6 standard | Mag 6 finer | LuminosityMap insertion |
| --- | ---: | ---: | ---: | ---: | ---: |
| SmallVec 4 | 121.911 | 97.641 | 709.522 | 373.189 | 3.883 |
| ArrayVec 1 | 125.567 | 105.180 | 706.904 | 383.173 | 2.902 |
| ArrayVec 2 | 121.134 | 101.845 | 711.460 | 378.995 | 3.595 |
| ArrayVec 4 | 121.904 | 99.059 | 711.052 | 375.156 | 3.040 |
| ArrayVec 8 | 126.412 | 95.121 | 712.486 | 368.803 | 3.386 |
| ArrayVec 16 | 121.079 | 101.019 | 704.776 | 356.660 | 3.640 |
| ArrayVec 32 | 121.910 | 99.726 | 706.041 | 333.207 | 4.094 |

NeighborMap Earth queries at 100 pc, **microseconds per query**:

| Storage | Standard | Finer |
| --- | ---: | ---: |
| SmallVec 4 | 1440.810 | 588.593 |
| ArrayVec 1 | 1392.780 | 590.669 |
| ArrayVec 2 | 1383.511 | 595.230 |
| ArrayVec 4 | 1771.350 | 597.057 |
| ArrayVec 8 | 1423.007 | 591.458 |
| ArrayVec 16 | 1681.346 | 599.421 |
| ArrayVec 32 | 1469.246 | 692.032 |

ArrayVec-1 was the strongest movement choice in this experiment, with minimal
memory difference from capacity 4. ArrayVec-4 also improved movement over
SmallVec-4 while preserving the promotion threshold. Capacities 8–32 reserve
more space in every entry; capacity 32 more than triples both maps' memory for
about an 11% gain in the finer magnitude-6 query. Standard visibility queries
changed little across capacities.

A fresh movement repeat ran capacity 4 before capacity 1:

| Capacity | Slow ms/frame | Fast ms/frame |
| --- | ---: | ---: |
| 4 | 7.96 | 24.39 |
| 1 | 6.76 | 19.86 |

A repeat of the 100-pc queries measured capacity 4 at **1,415.9 / 615.1 µs**
(standard/finer), and capacity 1 at **1,423.4 / 597.3 µs**. The initial 1,771.4 µs
standard-query result for capacity 4 did not repeat, so it should not be treated
as a reliable regression. The movement advantage of capacity 1 did repeat.

All capacity tests, selected Gaia full-scan comparisons, and movement final-state
checks passed. Capacity one has since become the default; the larger-capacity
benchmark features remain available. These measurements cover this scene
and catalogue, with independently seeded hash tables; small timing differences
are not a general ranking of capacities.

## Moving-object benchmark

`movement` exercises repeated position updates to the same 32,768 IDs in both
NeighborMap and LuminosityMap. It builds a synthetic scene once per selected
case. Each timed iteration is a complete frame updating every object in stable
ID order. Criterion reports time per frame and throughput in object updates;
divide frame time by 32,768 for time per update.

Objects have anchors on a 32³ grid with spacing 4,096 units. Each follows a
bounded, piecewise linear path with reflecting turns, amplitude 1,024 units per
axis, and period 4,096. Deterministic random initial phases vary their positions
and directions. Position calculation uses a precomputed offset table; there are
no timed RNG calls or trajectory allocations. The motion cases advance by:

- `unchanged`: zero, to measure the existing-position fast path;
- `slow`: one phase per frame, moving at most one unit per axis;
- `fast`: 64 phases per frame, moving at most 64 units per axis.

Each object's path stays inside its original width-4,096 cell. Changed finer
cells contain only that object, deliberately exercising singleton cell creation
and destruction. Coarser cells remain unchanged, so the normal early exit in
NeighborIndex applies. Every object's ID remains present throughout the run;
there are no explicit remove calls, timed initial insertions, or untimed resets.
The timed `insert` calls include both removal from old cells and insertion into
new cells. LuminosityMap holds brightness at 1.0 so bucket migration is excluded.
All final positions and the total population are checked outside the timers.

```sh
cargo bench -p osg-spatial-hash --bench movement -- --noplot --save-baseline movement_arrayvec1
# Separate instrumented builds: allocation counts only, no timing results.
cargo bench -p osg-spatial-hash --features bench-allocations --bench movement
```

Allocation mode warms up 32 frames, then counts allocator calls over 128 frames
(4,194,304 existing-object updates) per case. It includes allocations and frees
inside position updates, including any cell-table maintenance. Requested bytes
are allocation traffic, not retained memory. Timing builds use the normal system
allocator without the counting wrapper. Both modes keep the map and motion state
alive across frames.

This is a controlled simulation of objects moving through otherwise empty fine
cells. It does not measure collisions, clustered occupancy, brightness changes,
or objects spawning and despawning. Initial map construction and final
correctness checks are excluded from timing and allocation counts.

### Movement results, 2026-09-21

These measurements used the earlier SmallVec-4 hybrid; current builds use ArrayVec.

AMD Ryzen 9 5900XT, Rust 1.98.1, 30 samples, 1-second warmup and a
3-second target measurement. Both complete timing suites ran sequentially;
allocation instrumentation was disabled. Every final-position and population
check passed.

| Map | Motion | Set ns/update | Hybrid ns/update | Set ms/frame | Hybrid ms/frame | Speedup |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| NeighborMap | unchanged | 15.0 | 19.7 | 0.49 | 0.64 | 0.76× |
| NeighborMap | slow | 447.2 | 272.1 | 14.65 | 8.92 | 1.64× |
| NeighborMap | fast | 1154.1 | 859.1 | 37.82 | 28.15 | 1.34× |
| LuminosityMap | unchanged | 19.6 | 19.0 | 0.64 | 0.62 | 1.03× |
| LuminosityMap | slow | 442.6 | 267.4 | 14.50 | 8.76 | 1.66× |
| LuminosityMap | fast | 1116.8 | 828.9 | 36.60 | 27.16 | 1.35× |

Slow movement improved **1.64–1.66×** and fast movement **1.34–1.35×**.
This confirms a benefit from avoiding repeated singleton-cell allocation,
although updating the outer cell tables still has a substantial cost.

The unchanged control showed run-to-run variation despite exercising the same
fast path: a separate repeat measured NeighborMap at **16.0 → 17.2 ns/update**
and LuminosityMap at **19.3 → 19.6 ns/update**. It allocated nothing in either
layout. Small timing differences, especially in this control, should not be
interpreted as stable effects of cell storage.

Allocation counts over **4,194,304 updates**, identical for the two map types:

| Motion | Set allocations / frees | Hybrid allocations / frees | Set requested bytes/update | Hybrid requested bytes/update |
| --- | ---: | ---: | ---: | ---: |
| unchanged | 0 / 0 | 0 / 0 | 0 | 0 |
| slow | 7,689,902 / 7,689,902 | 1 / 1 | 98.12 | 2.78 |
| fast | 19,934,625 / 19,934,625 | 1 / 1 | 249.93 | 2.78 |

The set layout allocates and frees approximately **1.83 times per slow update**
or **4.75 times per fast update**. The hybrid eliminates allocation of the
singleton cell sets. Its remaining allocation is 11,665,424 bytes for an outer
cell table growing during the measured window; it also frees the old table.
There were no `realloc` calls in either layout. Thus the moving hybrid workload
is almost allocation-free, with occasional outer-table growth still visible.

## SmallVec cell A/B

This section preserves the SmallVec experiment preceding the ArrayVec switch.
The commands below are historical invocations. The `hybrid-cells` toggle and
set-only build were removed when ArrayVec-1 became unconditional.

The optional `hybrid-cells` feature replaced each `AHashSet` cell with an enum:
`Small(SmallVec<[T; 4]>)` or `Large(AHashSet<T>)`. Spatial indexes store `usize`
handles, so these are four inline slot indices. The fifth distinct item promotes
the cell to a set. Small cells use bounded linear duplicate detection and
`swap_remove`; promoted cells stay as sets until empty, when SpatialHash removes
the cell. The SmallVec never spills to a heap allocation. Iterator dispatch uses
an enum and allocates no storage.

Four entries are an initial choice to limit the size of every cell-table entry.
This experiment does not establish the optimal inline capacity or measure
automatic demotion. Both builds use the adopted slab records and the same query
and insertion code; only cell storage differed. The default build then retained sets.

```sh
cargo bench -p osg-spatial-hash --bench gaia -- --noplot --save-baseline set_cells
cargo bench -p osg-spatial-hash --features hybrid-cells --bench gaia -- --noplot --save-baseline hybrid_cells
# Run each memory measurement in a fresh process, after timing completes:
GAIA_MEMORY=neighbor cargo bench -p osg-spatial-hash --bench gaia
GAIA_MEMORY=neighbor cargo bench -p osg-spatial-hash --features hybrid-cells --bench gaia
GAIA_MEMORY=luminosity cargo bench -p osg-spatial-hash --bench gaia
GAIA_MEMORY=luminosity cargo bench -p osg-spatial-hash --features hybrid-cells --bench gaia
```

Criterion saves these runs under `set_cells` and `hybrid_cells` within each
benchmark's directory in `target/criterion`. Each process loads the catalogue
once and builds each selected map once. The usual full-scan correctness checks
run before timing, and insertion removes each random batch outside the timer.

### Results, 2026-09-21

AMD Ryzen 9 5900XT, Rust 1.98.1, SmallVec 1.16.1, optimized bench profile,
30 samples, 1-second warmup, and 3-second target measurement. The two complete
suites ran sequentially in separate processes. All 32 query cases per build
matched full catalogue scans; both random insertion cases completed. Baseline
unit tests and the feature's promotion/removal test passed.

On this build, both `AHashSet<usize>` and the hybrid enum occupy **64 bytes**.
The four inline entries fit without increasing the size of a cell-table value.
The hybrid avoids separate allocations for cells that never exceed four items.

Visibility times in **microseconds per query**:

| Magnitude | Stars | Set standard | Hybrid standard | Set finer | Hybrid finer |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 0 | 4.334 | 4.176 | 41.419 | 40.106 |
| 2 | 1 | 10.419 | 9.984 | 45.057 | 43.307 |
| 4 | 519 | 128.577 | 122.434 | 116.251 | 98.391 |
| 6 | 6,373 | 714.231 | 705.421 | 400.018 | 374.600 |

NeighborMap times in **microseconds per query**. “Stars” rotates through 64
catalogue positions:

| Centre | Radius | Set standard | Hybrid standard | Set finer | Hybrid finer |
| --- | --- | ---: | ---: | ---: | ---: |
| Earth | 1000 km | 0.084 | 0.087 | 3.102 | 3.196 |
| Earth | 500 au | 0.087 | 0.086 | 0.382 | 0.382 |
| Earth | 1 pc | 0.134 | 0.121 | 0.380 | 0.382 |
| Earth | 10 pc | 33.290 | 36.326 | 6.719 | 6.738 |
| Earth | 100 pc | 1,299.313 | 1,371.571 | 561.042 | 592.344 |
| Earth | 1000 pc | 23,835.392 | 25,092.591 | 19,096.001 | 20,707.986 |
| Stars | 1000 km | 0.249 | 0.240 | 6.071 | 6.248 |
| Stars | 500 au | 0.081 | 0.077 | 0.271 | 0.270 |
| Stars | 1 pc | 0.110 | 0.105 | 0.878 | 0.825 |
| Stars | 10 pc | 1.409 | 1.474 | 1.068 | 0.911 |
| Stars | 100 pc | 587.848 | 611.249 | 281.148 | 283.345 |
| Stars | 1000 pc | 20,446.093 | 22,253.505 | 16,157.398 | 17,740.929 |

| Random insertion | Set µs/point (95% CI) | Hybrid µs/point (95% CI) |
| --- | ---: | ---: |
| NeighborMap | 5.867 (5.728–5.983) | 3.991 (3.804–4.187) |
| LuminosityMap | 6.457 (6.390–6.519) | 3.876 (3.806–3.942) |

Insertion latency fell **32%** for NeighborMap and **40%** for LuminosityMap.
Visibility improved about 1–15%, depending on magnitude and resolution.
NeighborMap query results were mixed: several small queries improved, but the
larger queries were generally **4–10% slower**. The hybrid trades some large-query
performance for faster insertion and lower memory use. It was opt-in at that time.

These are single paired runs with independently seeded hash tables. Small
percentage differences should not be treated as reliable wins without repeats.

Fresh-process RSS increase after building the million-star index:

| Index | Set KiB | Hybrid KiB | Reduction |
| --- | ---: | ---: | ---: |
| NeighborMap | 6,268,464 | 4,737,368 | 24.4% |
| LuminosityMap | 5,037,608 | 3,484,196 | 30.8% |

That is approximately **5.98 → 4.52 GiB** for NeighborMap and
**4.80 → 3.32 GiB** for LuminosityMap. RSS includes allocator overhead and
retained allocations during loading. These readings precede random insertion
churn; promoted cells do not demote, so long-lived workloads may differ.

## Slab record layout

Both production maps now use slab records. Spatial cells contain `usize` slot
indices; an item-to-slot hash map is used only for mutation. LuminosityMap shares
one slab across all 64 brightness buckets and reuses squared distance for its
visibility check. The experimental implementations and comparison wrappers have
been removed. Existing benchmark names now measure the adopted slab layout.

```sh
cargo bench -p osg-spatial-hash --bench gaia -- luminosity_map
cargo bench -p osg-spatial-hash --bench gaia -- neighbor_map
# Fresh process for each memory measurement (Linux):
GAIA_MEMORY=neighbor cargo bench -p osg-spatial-hash --bench gaia
GAIA_MEMORY=luminosity cargo bench -p osg-spatial-hash --bench gaia
```

Memory mode prints process RSS before and after building the million-star map.
The delta includes allocator overhead and retained build allocations. Dataset
loading precedes the first reading. Run memory measurements separately from
query timing to avoid resource contention.

The following tables preserve the A/B results from before adoption. “Baseline”
means the old records stored in hash maps; “slab” means the adopted design.

### Results, 2026-09-21

AMD Ryzen 9 5900XT, Rust 1.98.1, optimized bench profile, 30 samples per case,
1-second warmup and 3-second target measurement. Both layouts ran sequentially
in one invocation of `cargo bench -p osg-spatial-hash --offline --bench gaia -- --noplot`.
All 48 NeighborMap query cases and all 16 visibility cases passed their full
scan checks. The two additional insertion cases per layout used the existing
million-star index and excluded cleanup from timing.

Times below are Criterion point estimates in **microseconds per query**:

| Magnitude | Visible stars | Baseline standard | Slab standard | Baseline finer | Slab finer |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 0 | 4.936 | 4.353 | 39.819 | 41.121 |
| 2 | 1 | 18.993 | 10.114 | 46.461 | 44.503 |
| 4 | 519 | 286.264 | 131.953 | 182.722 | 113.351 |
| 6 | 6,373 | 1,529.621 | 739.875 | 780.048 | 413.718 |

For magnitude 6, slab standard is **2.07× faster**, and slab finer is
**1.89× faster**. The slab 95% confidence intervals are 737.14–743.61 µs
and 411.71–415.62 µs respectively. This is approximately **116 ns** or
**65 ns per visible star**, including candidate rejection and cell lookups.
Empty queries see little benefit; finer resolution at magnitude 0 regressed
about 3% in this run. Cell probes still dominate that workload.

NeighborMap times, also **microseconds per query**. "Stars" rotates through
the same 64 catalogue positions for both layouts:

| Centre | Radius | Baseline standard | Slab standard | Baseline finer | Slab finer |
| --- | --- | ---: | ---: | ---: | ---: |
| Earth | 1,000 km | 0.092 | 0.090 | 3.146 | 3.144 |
| Earth | 500 AU | 0.089 | 0.089 | 0.391 | 0.398 |
| Earth | 1 pc | 0.182 | 0.139 | 0.398 | 0.443 |
| Earth | 10 pc | 105.575 | 42.293 | 14.458 | 7.378 |
| Earth | 100 pc | 3,257.776 | 1,506.149 | 1,754.155 | 731.890 |
| Earth | 1,000 pc | 54,724.941 | 26,150.318 | 46,625.970 | 21,255.613 |
| Stars | 1,000 km | 0.269 | 0.261 | 6.211 | 6.239 |
| Stars | 500 AU | 0.096 | 0.089 | 0.296 | 0.302 |
| Stars | 1 pc | 0.127 | 0.119 | 0.874 | 0.897 |
| Stars | 10 pc | 4.226 | 1.978 | 1.952 | 1.319 |
| Stars | 100 pc | 1,539.872 | 651.205 | 635.633 | 341.125 |
| Stars | 1,000 pc | 47,778.242 | 23,213.118 | 40,645.427 | 18,577.282 |

Larger queries improve about 2×; smaller ones depend on whether there are enough
candidates to benefit from direct record access. The Earth 1-pc finer case
regressed about 11%, despite being empty; these small cases measure cell probing
and iterator overhead as well as records. The layout does not make cell visits
or record access sequential.

| Random insertion | Baseline µs/point (95% CI) | Slab µs/point (95% CI) |
| --- | ---: | ---: |
| NeighborMap | 7.184 (7.065–7.282) | 6.875 (6.765–6.976) |
| LuminosityMap | 6.897 (6.798–6.995) | 6.476 (6.352–6.583) |

Insertion improvements are modest: approximately 4% and 6%. These measurements
cover new entries with removal between batches; they do not measure moving
existing entries or compaction after sustained deletion. The slab layout has since been adopted for both production maps.

Fresh-process RSS increase after loading the index:

| Index | Baseline KiB | Slab KiB | Difference |
| --- | ---: | ---: | ---: |
| NeighborMap | 6,261,636 | 6,267,932 | +6.1 MiB (+0.10%) |
| LuminosityMap | 5,088,316 | 5,037,608 | −49.5 MiB (−1.00%) |

Memory use is essentially unchanged: about 5.97 GiB for NeighborMap and
4.85 versus 4.80 GiB for LuminosityMap. The spatial cell tables and their sets
dominate the allocation footprint; a slab primarily improves record lookup here.
These are single-run comparisons with independently seeded hash maps, so small
timing and RSS differences should not be treated as robust improvements.

## Historical measurements

The tables below preserve the experiments that led to making distance filtering
mandatory. The paths without distance filtering have since been removed.

Full suite on an AMD Ryzen 9 5900XT with Rust 1.98.1 and the default optimized
bench profile, using 30 samples per case:

| Operation | Time | 95% confidence interval |
| --- | ---: | ---: |
| Earth visibility, magnitude <= 6 | 2.16 ms/query | 1.99–2.37 ms |
| New random point in NeighborMap | 6.87 µs/point | 6.78–6.95 µs |
| New random point in LuminosityMap | 6.95 µs/point | 6.80–7.09 µs |

The visibility query returns **6,373 stars**, matching the full scan exactly.
That is about **339 ns per visible star**, including candidate rejection and
all bucket lookups.

| Radius | Earth time/query | Earth candidates | Sampled-star time/query | Mean sampled-star candidates |
| --- | ---: | ---: | ---: | ---: |
| 1,000 km | 86.2 ns | 0 | 251 ns | 1.0 |
| 500 AU | 86.6 ns | 0 | 85.7 ns | 1.0 |
| 1 pc | 155 ns | 6 | 118 ns | 1.2 |
| 10 pc | 47.6 µs | 6,517 | 1.94 µs | 243.1 |
| 100 pc | 933 µs | 134,463 | 545 µs | 44,061.3 |
| 1,000 pc | 20.7 ms | 982,743 | 18.8 ms | 890,075.2 |

These are Criterion point estimates from this machine, including consumption
of every candidate. Cell lookups are bounded, but large queries spend most of
their work returning candidates. Timing need not increase monotonically with
radius: changing resolutions also changes cell alignment and the number of
cell lookups. The catalogue's Earth-based selection affects these populations.

## Finer-resolution A/B test

Run both variants against the same loaded map:

```sh
cargo bench -p osg-spatial-hash --bench gaia -- luminosity_map/query
```

`earth_magnitude_6` calls `nearest_visible`; `earth_magnitude_6_finer` calls
`nearest_visible_finer`, which queries one finer existing spatial level.
Both results were checked against the complete catalogue scan and returned
the same 6,373 IDs. In a single run on the machine above:

| Metric | Standard resolution | One level finer |
| --- | ---: | ---: |
| Query time | 2.603 ms | 0.838 ms |
| 95% confidence interval | 2.572–2.635 ms | 0.834–0.843 ms |
| Cell lookups across occupied brightness buckets | 224 | 5,992 |
| Nonempty cells visited | 158 | 2,428 |
| Candidate stars examined | 81,869 | 23,866 |

Cell and candidate counts were calculated from the catalogue and each variant's
cell bounds outside the timed run. The finer variant was **3.1× faster**, reducing
latency by **67.8%**. It traded 5,768 additional cell lookups for 58,003 fewer
candidate evaluations, each of which otherwise retrieves position and brightness
and applies the distance test. This result applies to repeated magnitude-6
queries from Earth; it does not establish a winner for every radius or viewpoint.

### Queries returning fewer stars

The same comparison at brighter limits (lower numerical magnitudes), with the
same map reused for all six measurements:

| Magnitude limit | Visible stars | Standard resolution | One level finer |
| --- | ---: | ---: | ---: |
| 0 | 0 | 4.67 µs | 38.9 µs |
| 2 | 1 | 35.3 µs | 48.4 µs |
| 4 | 519 | 466 µs | 217 µs |

Both variants matched the full scan at every limit. These counts describe this
Gaia sample, not a complete catalogue of the brightest stars. Extra cell lookups
outweigh candidate savings at limits 0 and 2; finer cells win at limit 4. The
results support choosing resolution based on workload rather than always using
the finer level.

```sh
cargo bench -p osg-spatial-hash --bench gaia -- 'luminosity_map/query/earth_magnitude_[024]'
```

## NeighborMap resolution A/B test

Both variants share the same million-star map and requested radius. The finer
variant returns fewer extra whole-cell candidates, so result counts can differ.
Each timing includes consuming all returned candidates.

| Centre | Radius | Standard time | Finer time | Standard candidates | Finer candidates |
| --- | --- | ---: | ---: | ---: | ---: |
| Earth | 1,000 km | 56.7 ns | 2.01 µs | 0 | 0 |
| Earth | 500 AU | 53.2 ns | 255 ns | 0 | 0 |
| Earth | 1 pc | 137 ns | 242 ns | 6 | 0 |
| Earth | 10 pc | 61.4 µs | 9.19 µs | 6,517 | 1,270 |
| Earth | 100 pc | 1.38 ms | 756 µs | 134,463 | 78,229 |
| Earth | 1,000 pc | 30.2 ms | 26.9 ms | 982,743 | 871,060 |
| Sampled stars | 1,000 km | 170 ns | 4.20 µs | 1.0 | 1.0 |
| Sampled stars | 500 AU | 67.8 ns | 199 ns | 1.0 | 1.0 |
| Sampled stars | 1 pc | 87.3 ns | 598 ns | 1.2 | 1.0 |
| Sampled stars | 10 pc | 2.49 µs | 1.29 µs | 243.1 | 45.9 |
| Sampled stars | 100 pc | 805 µs | 367 µs | 44,061.3 | 18,573.9 |
| Sampled stars | 1,000 pc | 27.5 ms | 22.9 ms | 890,075.2 | 753,521.9 |

These measurements are from one paired run; sampled-star counts are averages
over 64 centres. Standard resolution wins for the small queries. Finer resolution
helps where it substantially reduces candidate counts, with a smaller benefit
when both variants return most of the catalogue.

```sh
cargo bench -p osg-spatial-hash --bench gaia -- neighbor_map/query
```

## Distance filtering before brightness lookup

The `_precise` variants have NeighborMap reject candidates beyond each brightness
bucket's search radius using f64 squared Euclidean distance. The final
per-source visibility check still happens in LuminosityMap. This avoids brightness
lookups for rejected candidates, while still reading every candidate's position.
Survivors also undergo the existing brightness-dependent distance test.

All four variants reused one loaded map and matched a full scan at limits 0, 2,
4, and 6. Paired query timings:

| Magnitude | Standard | Standard + distance filter | Finer | Finer + distance filter |
| --- | ---: | ---: | ---: | ---: |
| 0 | 4.58 µs | 5.00 µs | 38.7 µs | 39.6 µs |
| 2 | 37.6 µs | 18.7 µs | 49.9 µs | 48.4 µs |
| 4 | 483 µs | 293 µs | 226 µs | 197 µs |
| 6 | 2.71 ms | 1.57 ms | 0.828 ms | 0.852 ms |

The filter helps standard-resolution queries at limits 2, 4, and 6. With finer
cells, the additional benefit is mixed: it helps at limit 4 but gives no gain
at limit 6 in this run. Fewer brightness lookups do not eliminate the work of
visiting cells, retrieving positions, and computing distances.

## Luminosity and spatial scale experiment (2026-09-21)

All variants use boxed cell sets with inline capacity 1. Scale means the ratio
between adjacent buckets or cell widths, not the shift increment. The current
default remains luminosity ×2 / spatial ×4. Optional Cargo features
`luminosity-scale-4` and `spatial-scale-2` select the other combinations.

Luminosity ×2 has 64 buckets, with the top bucket starting at 2^63;
luminosity ×4 has 32 buckets, with the top bucket starting at 2^62. The bottom
bucket includes smaller values and the top bucket includes all larger values.
Spatial ×4 has 33 levels (shifts 0, 2, ..., 62, 63); spatial ×2 has 64 levels.
The finer query moves one level down: width /4 or /2, respectively. Its maximum
cell count is therefore 729 or 125; standard queries visit at most 27 cells.

Criterion used 30 samples, 1 second warmup, and a 3 second measurement target.
Each variant ran in a separate process, sequentially, in the column order below.
The catalogue was loaded once and the map built once for all selected Gaia
cases in that process. Insertion adds random points to the existing map and
removes them outside timing. Memory is RSS growth during initial map construction.
All eight tests passed for each variant; both query modes matched a full scan
(0, 1, 519, and 6,373 stars at magnitude limits 0, 2, 4, and 6). Movement
correctness checks also passed.

First pass, query time in microseconds (lower is better):

| Query | L2/S4 current | L4/S2 | L2/S2 | L4/S4 |
| --- | ---: | ---: | ---: | ---: |
| Magnitude 0, standard | 4.29 | 2.14 | 3.94 | 3.25 |
| Magnitude 0, finer | 40.35 | 7.59 | 13.77 | 15.69 |
| Magnitude 2, standard | 10.54 | 4.17 | 6.42 | 8.04 |
| Magnitude 2, finer | 44.15 | 11.20 | 18.19 | 31.11 |
| Magnitude 4, standard | 123.62 | 86.45 | 37.93 | 248.04 |
| Magnitude 4, finer | 112.06 | 111.27 | 77.11 | 112.82 |
| Magnitude 6, standard | 697.67 | 745.79 | 359.33 | 2063.62 |
| Magnitude 6, finer | 407.95 | 652.51 | 335.84 | 583.12 |

| Cost | L2/S4 current | L4/S2 | L2/S2 | L4/S4 |
| --- | ---: | ---: | ---: | ---: |
| Insert, µs/object | 2.84 | 6.23 | 6.59 | 2.97 |
| Luminosity index RSS, GiB | 1.948 | 3.782 | 3.739 | 1.958 |
| luminosity_map, unchanged, ms/frame | 0.646 | 0.611 | 0.617 | 0.620 |
| luminosity_map, slow, ms/frame | 4.824 | 9.787 | 10.529 | 5.568 |
| luminosity_map, fast, ms/frame | 13.768 | 33.667 | 35.989 | 15.866 |
| neighbor_map, unchanged, ms/frame | 0.567 | 0.589 | 0.507 | 0.587 |
| neighbor_map, slow, ms/frame | 4.845 | 9.448 | 10.289 | 4.682 |
| neighbor_map, fast, ms/frame | 14.165 | 34.602 | 35.528 | 13.592 |

Each movement frame updates all 32,768 existing objects. Brightness stays at
1.0, so this workload exercises spatial updates, not brightness bucket migration.
NeighborMap does not use luminosity buckets; differences between its runs with
the same spatial scale should not be attributed to the luminosity feature.

Magnitude 4 and 6 queries and insertion were repeated in reverse variant order,
using fresh processes and maps. Repeat query times in microseconds:

| Query | L2/S4 current | L4/S2 | L2/S2 | L4/S4 |
| --- | ---: | ---: | ---: | ---: |
| Magnitude 4, standard | 123.16 | 85.80 | 38.26 | 248.31 |
| Magnitude 4, finer | 115.33 | 114.78 | 78.71 | 111.69 |
| Magnitude 6, standard | 700.24 | 749.08 | 359.10 | 1947.59 |
| Magnitude 6, finer | 408.49 | 657.40 | 337.22 | 576.00 |
| Insert, µs/object | 3.17 | 6.47 | 6.57 | 2.44 |

Both ×2 gives the best larger-query times here, at approximately twice the
memory and substantially higher insertion and movement costs. Luminosity ×4 /
spatial ×2 wins the smallest standard queries in the first pass, but is slower
than both ×2 at magnitude 4 and 6. Both ×4 loses heavily on larger standard
queries. The current default remains a useful balance; these results cover Earth
queries in this catalogue and should not be generalized to all distributions.

Reproduce each variant by applying its feature flags to both commands:

```sh
cargo bench -p osg-spatial-hash --offline --bench gaia -- luminosity_map --save-baseline scale_l2s4
cargo bench -p osg-spatial-hash --offline --bench movement -- movement/ --save-baseline scale_l2s4
# L4/S2: --features luminosity-scale-4,spatial-scale-2
# L2/S2: --features spatial-scale-2
# L4/S4: --features luminosity-scale-4
# Put feature flags before --; use a distinct baseline name for each variant.
# Prefix the Gaia command with GAIA_REPORT_MEMORY=1 to record index RSS growth.
```

Raw logs and copied benchmark binaries: `/tmp/scale-ab/`. Criterion baselines:
`scale_l2s4`, `scale_l4s2`, `scale_l2s2`, `scale_l4s4`, and their `_repeat` variants.
