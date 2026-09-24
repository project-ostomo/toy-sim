# Spatial index benchmark results — 2026-09-21 16:08:14 EDT

Report saved at 2026-09-21 16:08:14 EDT (20:08:14 UTC).

AMD Ryzen 5 3600XT; Rust 1.97.1; release profile; default crate features.
30 samples, 1 s warmup, 3 s target measurement per case; larger builds extend measurement.
Benchmark processes ran sequentially. Times are Criterion slope estimates, or means when flat sampling was used.
Intervals are Criterion's 95% confidence intervals; they do not account for systematic machine-load effects.

`bvh-suite` runs both implementations from osg-spatial-bvh; `hash-suite` runs osg-spatial's own benchmarks.
Both use the same million-star catalogue and normalized flux. Movement times are per frame of 32,768 updates.
Construction excludes destruction; insert excludes cleanup; removal excludes setup.
The BVH uses point AABBs and no balancing or periodic rebuilds.
Query validation runs once per case; mutation population validation runs after all samples.

Commands used (each crate's working directory):

```sh
# osg-spatial-bvh
cargo bench --offline --locked --bench gaia
cargo bench --offline --locked --bench movement

# osg-spatial
cargo bench --offline --locked --bench gaia -- luminosity_map
cargo bench --offline --locked --bench movement -- luminosity_map

# osg-spatial-bvh: separate processes for memory
GAIA_MEMORY=bvh cargo bench --offline --locked --bench gaia
GAIA_MEMORY=luminosity cargo bench --offline --locked --bench gaia
```

Build artifacts and raw Criterion output were redirected to `/tmp/osg-comparison.yF3hPa` using
`CARGO_TARGET_DIR` and separate `CRITERION_HOME` directories for the two suites.
The raw table in this report preserves all 64 timing estimates and confidence intervals.

## Comparison

For Earth queries, insertion, and movement, hash timings below come from its own original suite.
Construction, removal, memory, and other observers use the shared comparison harness.
The raw results below include both hash runs wherever both are available.

| Operation | BVH | Spatial hash | Faster |
| --- | ---: | ---: | --- |
| Build 10,000 | 3.5 ms | 22.5 ms | BVH 6.4× |
| Build 100,000 | 51.7 ms | 455 ms | BVH 8.8× |
| Build 1,000,000 | 645 ms | 5.27 s | BVH 8.2× |
| Insert into 1M | 4.37 µs | 4.31 µs | Approximately tied (overlapping intervals) |
| Remove recent insertion from 1M | 588 ns | 2.22 µs | BVH 3.8× |
| Unchanged frame, 32,768 updates | 9.02 ms | 700 µs | Hash 13× |
| Slow frame, 32,768 updates | 85.3 ms | 8.9 ms | Hash 9.6× |
| Fast frame, 32,768 updates | 85.2 ms | 25.6 ms | Hash 3.3× |
| RSS growth after 1M build | 282 MiB | 2002 MiB | BVH 7.1× smaller |

RSS growth includes handle storage and may include allocator retention from construction temporaries; it is not exact live or peak memory.

| Observer | Magnitude | BVH | Hash precise | Hash finer |
| --- | ---: | ---: | ---: | ---: |
| earth | 0 | 5 µs | 5.75 µs | 55.9 µs |
| earth | 2 | 35.1 µs | 13.8 µs | 59.5 µs |
| earth | 4 | 427 µs | 148 µs | 152 µs |
| earth | 6 | 5.69 ms | 1.12 ms | 544 µs |
| star_104729 | 0 | 12.7 µs | 9.22 µs | 89.7 µs |
| star_104729 | 2 | 48.8 µs | 13.2 µs | 74.5 µs |
| star_104729 | 4 | 259 µs | 93.1 µs | 106 µs |
| star_104729 | 6 | 3.32 ms | 876 µs | 579 µs |
| star_523645 | 0 | 2.73 µs | 6.98 µs | 81 µs |
| star_523645 | 2 | 14.5 µs | 9.58 µs | 62 µs |
| star_523645 | 4 | 132 µs | 56.2 µs | 76.5 µs |
| star_523645 | 6 | 1.85 ms | 649 µs | 418 µs |

These measurements favor the hash for movement and most queries, and the BVH for construction, removal, and measured memory growth.
The BVH currently reinserts every moved leaf and refits ancestors on unchanged updates. Movement uses adaptive frame counts and no rebuilding.
All full-scan query and mutation-population checks passed. Collision detection and fat AABBs are outside this comparison.

## Raw estimates

| Suite | Benchmark | Time | 95% interval |
| --- | --- | ---: | ---: |
| bvh-suite | luminosity_bvh/build/10000 | 3.5 ms | 3.45 ms–3.56 ms |
| bvh-suite | luminosity_bvh/build/100000 | 51.7 ms | 50 ms–53.9 ms |
| bvh-suite | luminosity_bvh/build/1000000 | 645 ms | 636 ms–655 ms |
| bvh-suite | luminosity_bvh/insert/random_into_gaia_1m | 4.37 µs | 4.27 µs–4.51 µs |
| bvh-suite | luminosity_bvh/query/earth_magnitude_0 | 5 µs | 4.94 µs–5.06 µs |
| bvh-suite | luminosity_bvh/query/earth_magnitude_2 | 35.1 µs | 33.5 µs–36.5 µs |
| bvh-suite | luminosity_bvh/query/earth_magnitude_4 | 427 µs | 422 µs–434 µs |
| bvh-suite | luminosity_bvh/query/earth_magnitude_6 | 5.69 ms | 5.55 ms–5.88 ms |
| bvh-suite | luminosity_bvh/query/star_104729_magnitude_0 | 12.7 µs | 12.5 µs–13 µs |
| bvh-suite | luminosity_bvh/query/star_104729_magnitude_2 | 48.8 µs | 48.2 µs–49.5 µs |
| bvh-suite | luminosity_bvh/query/star_104729_magnitude_4 | 259 µs | 254 µs–264 µs |
| bvh-suite | luminosity_bvh/query/star_104729_magnitude_6 | 3.32 ms | 3.21 ms–3.42 ms |
| bvh-suite | luminosity_bvh/query/star_523645_magnitude_0 | 2.73 µs | 2.7 µs–2.76 µs |
| bvh-suite | luminosity_bvh/query/star_523645_magnitude_2 | 14.5 µs | 13.9 µs–15.1 µs |
| bvh-suite | luminosity_bvh/query/star_523645_magnitude_4 | 132 µs | 128 µs–136 µs |
| bvh-suite | luminosity_bvh/query/star_523645_magnitude_6 | 1.85 ms | 1.75 ms–1.97 ms |
| bvh-suite | luminosity_bvh/remove/random_into_gaia_1m | 588 ns | 555 ns–615 ns |
| bvh-suite | luminosity_map/build/10000 | 22.5 ms | 21.8 ms–23.3 ms |
| bvh-suite | luminosity_map/build/100000 | 455 ms | 438 ms–472 ms |
| bvh-suite | luminosity_map/build/1000000 | 5.27 s | 5.19 s–5.36 s |
| bvh-suite | luminosity_map/insert/random_into_gaia_1m | 4.11 µs | 4.08 µs–4.15 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_0_finer_precise | 57 µs | 55.4 µs–58.4 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_0_precise | 5.78 µs | 5.73 µs–5.84 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_2_finer_precise | 65 µs | 62.4 µs–68.4 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_2_precise | 13.6 µs | 13.3 µs–13.9 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_4_finer_precise | 156 µs | 151 µs–160 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_4_precise | 155 µs | 151 µs–160 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_6_finer_precise | 523 µs | 512 µs–535 µs |
| bvh-suite | luminosity_map/query/earth_magnitude_6_precise | 1.38 ms | 1.24 ms–1.57 ms |
| bvh-suite | luminosity_map/query/star_104729_magnitude_0_finer_precise | 89.7 µs | 87.6 µs–91.4 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_0_precise | 9.22 µs | 8.78 µs–9.62 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_2_finer_precise | 74.5 µs | 72.9 µs–76 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_2_precise | 13.2 µs | 13.1 µs–13.4 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_4_finer_precise | 106 µs | 102 µs–110 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_4_precise | 93.1 µs | 91.9 µs–94.4 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_6_finer_precise | 579 µs | 571 µs–588 µs |
| bvh-suite | luminosity_map/query/star_104729_magnitude_6_precise | 876 µs | 821 µs–942 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_0_finer_precise | 81 µs | 80.1 µs–82.3 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_0_precise | 6.98 µs | 6.89 µs–7.1 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_2_finer_precise | 62 µs | 60.1 µs–63.9 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_2_precise | 9.58 µs | 9.28 µs–9.96 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_4_finer_precise | 76.5 µs | 75.5 µs–77.6 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_4_precise | 56.2 µs | 55.1 µs–57.4 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_6_finer_precise | 418 µs | 408 µs–426 µs |
| bvh-suite | luminosity_map/query/star_523645_magnitude_6_precise | 649 µs | 577 µs–736 µs |
| bvh-suite | luminosity_map/remove/random_into_gaia_1m | 2.22 µs | 2.15 µs–2.3 µs |
| bvh-suite | movement/luminosity_bvh/fast/32768 | 85.2 ms | 83.9 ms–86.6 ms |
| bvh-suite | movement/luminosity_bvh/slow/32768 | 85.3 ms | 84.2 ms–86.5 ms |
| bvh-suite | movement/luminosity_bvh/unchanged/32768 | 9.02 ms | 8.85 ms–9.2 ms |
| bvh-suite | movement/luminosity_map/fast/32768 | 23.3 ms | 22.8 ms–23.9 ms |
| bvh-suite | movement/luminosity_map/slow/32768 | 8.37 ms | 8.09 ms–8.65 ms |
| bvh-suite | movement/luminosity_map/unchanged/32768 | 742 µs | 736 µs–750 µs |
| hash-suite | luminosity_map/insert/random_into_gaia_1m | 4.31 µs | 4.28 µs–4.34 µs |
| hash-suite | luminosity_map/query/earth_magnitude_0_finer_precise | 55.9 µs | 53.9 µs–57.8 µs |
| hash-suite | luminosity_map/query/earth_magnitude_0_precise | 5.75 µs | 5.63 µs–5.85 µs |
| hash-suite | luminosity_map/query/earth_magnitude_2_finer_precise | 59.5 µs | 57.6 µs–61.1 µs |
| hash-suite | luminosity_map/query/earth_magnitude_2_precise | 13.8 µs | 13.7 µs–13.8 µs |
| hash-suite | luminosity_map/query/earth_magnitude_4_finer_precise | 152 µs | 149 µs–156 µs |
| hash-suite | luminosity_map/query/earth_magnitude_4_precise | 148 µs | 146 µs–151 µs |
| hash-suite | luminosity_map/query/earth_magnitude_6_finer_precise | 544 µs | 539 µs–549 µs |
| hash-suite | luminosity_map/query/earth_magnitude_6_precise | 1.12 ms | 1.05 ms–1.17 ms |
| hash-suite | movement/luminosity_map/fast/32768 | 25.6 ms | 25.1 ms–26.1 ms |
| hash-suite | movement/luminosity_map/slow/32768 | 8.9 ms | 8.39 ms–9.45 ms |
| hash-suite | movement/luminosity_map/unchanged/32768 | 700 µs | 689 µs–713 µs |
