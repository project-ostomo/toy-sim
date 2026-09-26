# Society storage refactor

Implemented September 26, 2026. `SocietyState` owns social records, economy and
gas accounts. Its authoritative maps, sets, indexes and histories use persistent
`imbl` collections. Large social records share immutable `Arc` values. Cloning
the state shares storage; subsequent mutations copy affected paths.

Queued actions operate on a draft and publish it on success. Actions that also
change physical ECS components prepare the component replacements before
publication. This removes the economy's separate change overlays and commit
machinery. Derived asset records live in a separate ECS resource.

Flight computer execution debits fair gas grants, runs computers with `par_iter`,
then refunds unused gas and records actual spending. Display computers use the
same debit/refund model with serial execution. Gas reservation objects, shared
mutexes and settlement systems are gone.

Saves serialize canonical values and rebuild indexes when loading. Save-only
domain validation, duplicate-key checks, payload checksums, definition fingerprints
and program hash rechecks were removed. Malformed saves have unspecified
behavior. Database ownership, format version and exclusive file locking remain.
Gas accounts are part of the society payload. Game version is 63; bundled firmware
was rebuilt. See [Persistence](persistence.md).

## Measurements

The baseline was an archive of the working tree immediately before this refactor,
including the earlier uncommitted society changes. These numbers do not compare
against Git HEAD. Both versions were built in release mode on the same machine.
The microbenchmark table reports medians from five runs, with repeated operations
reduced to a median within each run. Timing includes allocation instrumentation.

| Operation | Size | Before | After |
| --- | ---: | ---: | ---: |
| Society clone | 100,000 accounts and players | 658.07 ms | 0.65 µs |
| Transfer | 100,000 accounts | 1.32 µs | 6.11 µs |
| Society serialization | 100,000 accounts and players | 24.82 ms | 28.64 ms |
| Society clone | 1,000,000 ledger entries | 51.02 ms | 0.76 µs |
| Transfer | 1,000,000 ledger entries | 1.12 µs | 2.94 µs |
| Society serialization | 1,000,000 ledger entries | 62.73 ms | 54.86 ms |
| Market fill | 100 maker orders | 484.92 µs | 530.37 µs |
| Gas allocation and settlement | 100,000 requests | 25.22 ms | 15.57 ms |
| Diplomacy mutation | 100,000 history records | 4.06 ms | 0.40 ms |

The 100,000-account clone dropped from 3,652,162 allocations and 342 MB of extra
live allocation to zero allocations. The ledger-heavy clone also allocated
nothing. Transfers copy tree paths: at 100,000 accounts, allocations increased
from 4 to 17 and peak extra allocation from 904 to 9,024 bytes. Filling 100 makers
increased peak extra allocation from 201 KB to 369 KB. Gas processing reduced it
from 18.4 MB to 12.4 MB.

Five full server runs used 16 ships, four sessions, 70 warmup ticks and 20 measured
frames per session. The following are medians across those runs:

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Average tick time | 55.394 ms | 59.598 ms | +7.6% |
| Tick p50 | 53.962 ms | 57.382 ms | +6.3% |
| Tick p95 | 64.417 ms | 72.629 ms | +12.7% |
| Peak resident memory | 2,638,288 KiB | 2,685,232 KiB | +1.8% |

The median tick cost meets the plan's 10% threshold. The short samples give limited
evidence about tail latency, which increased here. Persistent storage makes
snapshots much cheaper and simplifies action boundaries, with measurable costs
for small writes and some serialization workloads. It is not a universal speedup.

Formatted Rust line counts relative to the archived baseline:

| Category | Change |
| --- | ---: |
| Production | −1,351 |
| Tests | −132 |
| Benchmark harness | +225 |
| Total | −1,258 |

These savings include the requested removal of save validation. They cannot all
be attributed to persistent collections. Production counts exclude dedicated test
files and inline test items; benchmark code is counted separately.

All microbenchmark sizes and allocation measurements are in
[the microbenchmark CSV](benchmarks/society-storage-micro.csv). Individual full
server runs are in [the server CSV](benchmarks/society-storage-server.csv).

## Reproduction and validation

Run the allocation benchmark alone because its counters cover the entire process:

```sh
cargo test -p osg-server --release --features society-bench --lib society_storage -- --ignored --nocapture --test-threads=1
```

Run the full server benchmark against a matching server binary:

```sh
cargo build -p osg-server --release --bin osg-server --example benchmark
target/release/examples/benchmark --server target/release/osg-server --ships 16 --sessions 4 --frames 20 --warmup-ticks 70
```

Validation passed: 376 server library tests, five network tests, 183 client tests
with UI enabled, 20 model tests and 19 WASM library tests. Targeted checks also
covered snapshot isolation, indexed records, failure atomicity, gas conservation
on computer traps, gas checkpoint restoration and durable WASM data restoration.
The release server and benchmark built successfully. Wallet screenshots were
checked with headless rendering after removing the gas reservation display.
