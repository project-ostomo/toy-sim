# Morton LBVH construction — September 23, 2026

Added `LuminosityBvh::build_morton(entries)` alongside the existing median
`build(entries)`. Callers can swap function names; both return a tree and leaf
indices in input order. Production callers still use their existing builders.

The new builder computes 63-bit Morton codes (21 bits per axis), performs eight
stable byte radix passes, then recursively splits sorted keys on their highest
differing bit. Equal codes retain input order and split at the median. It shares
node assembly with the original builder. Morton construction is sequential;
the original uses Rayon for median partitioning at 8,192 or more entries.

## Measurements

AMD Ryzen 5 3600XT, 6 cores / 12 logical CPUs, rustc 1.97.1, Cargo bench profile.
One Criterion invocation, 20 samples per case, one-second warmup, three-second
measurement target (automatically extended for expensive cases). Host activity
was not isolated. The benchmark initially waited for another Cargo build's
directory lock; compilation and that wait are outside the measured intervals.

Times below are Criterion's reported central estimates. Reduction is
`1 - Morton / median`; negative means Morton took longer.

| Scene | Objects | Median builder (ms) | Morton builder (ms) | Time reduction |
| --- | ---: | ---: | ---: | ---: |
| Uniform | 10,000 | 2.5574 | 1.0230 | 60.0% |
| Uniform | 46,908 | 7.3526 | 7.4227 | -1.0% |
| Uniform | 100,000 | 21.669 | 20.484 | 5.5% |
| Uniform | 1,000,000 | 324.02 | 276.58 | 14.6% |
| Clustered | 10,000 | 1.9050 | 1.0805 | 43.3% |
| Clustered | 46,908 | 8.5667 | 7.1024 | 17.1% |
| Clustered | 100,000 | 18.890 | 16.314 | 13.6% |
| Clustered | 1,000,000 | 441.82 | 291.43 | 34.0% |

The uniform 10,000-object median measurement was noisy: its reported interval
was 1.9967–3.1489 ms. Treat the exact percentage for that case cautiously.
At 46,908 uniform objects the intervals overlap, so this run does not establish
a construction improvement there. Raw intervals and outliers for every case
are in [the benchmark log](morton-construction-2026-09-23.txt).

Inputs are deterministic synthetic boxes with `usize` payloads. Uniform scenes
span roughly one billion micrometres per axis. Clustered scenes contain 32
widely separated groups with small local offsets. Their scene-wide Morton
quantization collapses many local positions onto equal codes; this lowers
partitioning work but can hurt traversal. No query-performance conclusion is
supported by these construction measurements. The 46,908-object size matches
an earlier production population count, but its geometry is synthetic.

Both builders receive identical inputs. Generation and Rayon startup are
excluded. Collection, validation, temporary allocation, ordering, assembly,
and internal scratch destruction are included. Finished-tree destruction is
excluded, with one result retained at a time to bound memory use.

## Reproduction and validation

```sh
cargo bench --offline -p osg-spatial-bvh --bench construction
cargo test --offline -p osg-spatial-bvh
cargo fmt -p osg-spatial-bvh --check
```

All 57 unit tests and one documentation test passed. Added tests independently
audit aggregate bounds, luminosities, reachability, preorder and input-to-leaf
mapping, compare visibility with exhaustive scans, and exercise empty and
singleton trees, duplicate codes, full i128 bounds, and galactic translations.
Formatting passed.
