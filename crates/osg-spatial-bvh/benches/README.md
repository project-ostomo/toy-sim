# BVH and spatial service benchmarks

The Gaia Criterion suite compares `LuminosityBvh<u64>` with
`SpatialService<u64>`, using the same million-star catalogue and flux thresholds.
The service uses its static tree and pages visibility queries with a budget of
4,096 work units and 256 results per advance. The bare BVH uses its iterator.

Run from the workspace root:

```sh
# Run each workload once, including comparisons against an independent full scan.
cargo bench -p osg-spatial-bvh --bench gaia -- --test

# Collect timings, or select a subset.
cargo bench -p osg-spatial-bvh --bench gaia
cargo bench -p osg-spatial-bvh --bench gaia -- query/earth --quick
```

The suite reads `crates/osg-stars/data/gaia-dr3-earth-million.stars`.
Set `GAIA_STARS` to use another copy of that million-record Gaia file.
The loader checks the file format, record count, coordinate frame, and units.
No dependency on the old spatial crate or the luminosity forest is required.

| Workload | Measurement |
| --- | --- |
| `{index}/build/{10000,100000,1000000}` | Construction, excluding input loading and destruction |
| `{index}/query/{observer}_magnitude_{0,2,4,6}` | Consume all visibility candidates |

The index names are `luminosity_bvh` and `spatial_service`. Observers are Earth
and catalogue entries 104729 and 523645, including zero-distance observations.
Every query is checked against a full scan before timing. Construction uses
catalogue prefixes, which are ordered by apparent magnitude.

Positions retain the catalogue's integer micrometres. Luminosities are converted
from lumens to approximate optical watts using 220 lm/W, matching the server's
current conversion. Both implementations use flux in W/m² with the `4π` factor.
The benchmark conversion does not change game photometry.

These workloads measure static construction and visibility. They do not measure
dynamic rebuilds, collision detection, or full simulation ticks. The historical
comparison reports in the crate root used different APIs and normalization;
their timings are not directly comparable with this suite.

For Linux RSS measurements, run each case in a fresh process:

```sh
GAIA_MEMORY=bvh cargo bench -p osg-spatial-bvh --bench gaia
GAIA_MEMORY=service cargo bench -p osg-spatial-bvh --bench gaia
```

RSS is sampled with input records already loaded. Allocator retention affects
the delta; it is neither exact live storage nor peak memory. Criterion reports
are written under the selected Cargo target directory's `criterion/` folder.
