# Ship-step measurements

These are historical measurements. Automatic startup pursuit has since been
removed; the current fleet benchmark defaults to coasting traffic in every mode.
The game now spawns two ships; the benchmark explicitly retains the historical
501-ship orbital fixture.

## Standard avionics and native instruments (ABI 6)

Measured 2026-09-12 with the same 501-position frozen fleet fixture, 24 compute
workers and release-built WASM. The first ship subscribes to native instruments;
500 traffic ships received the then-default pursuit order. After 100 warm-up ticks,
all 100,200 measured callbacks completed, with no gas skips or faults.

| Stage | First ship mean µs | Traffic mean µs |
|---|---:|---:|
| Prepare | 1.2 | 2.7 |
| WASM + ABI, excluding native scene query | 65.3 | 65.9 |
| Native sensor query | 137.9 | 136.6 |
| Publish | 12.8 | 11.9 |
| Hardware | 0.9 | 0.6 |
| **Total** | **218.1** | **217.8** |

First-ship p50 was 210.2 µs, p95 316.5 µs; fleet wall time was 5.85 ms/tick.
A preceding run measured 224.9 µs / 5.83 ms. The earlier MFD firmware measured
396 µs on the displayed ship (see historical results below). Native instruments
remove its all-contact label reads and default screen generation. Whole-fleet
wall time remains around 5–6 ms; most ships already had no requested MFDs.
This change primarily removes the displayed ship's extra guest work.

| Workload | First mean µs | Traffic mean µs | Fleet ms/tick |
|---|---:|---:|---:|
| Pursuit + native instrument subscription | 218.1 | 217.8 | 5.850 |
| Idle traffic + native instrument subscription | 202.6 | 196.3 | 5.315 |
| Idle traffic + no client subscription | 208.7 | 196.7 | 5.307 |
| Empty scan source + native instruments | 8.1 | 8.5 | 0.407 |
| Pursuit + native instruments + first-ship custom screen | 217.6 | 198.8 | 5.406 |

The custom-screen workload uses the stock-controller wrapper example, refreshing
its diagnostics screen once per simulation second. Native summaries are always
published; hiding an instrument only suppresses optional trajectory generation.
Idle ships have no planned trajectory. The idle visible/hidden difference is
within run variability. These are headless controller/hardware measurements:
no egui rendering, orbital integration, moving-scene costs, network or RSS
measurements are included. Parallel durations overlap and are not CPU times.

Sensor queries still dominate. The fixed `1000*N` scan price does not bound the
current range-query candidate search by N. MMO-density spatial traversal remains
separate work; these numbers should not be extrapolated to 100,000 ships.

## Historical MFD firmware baseline (ABI 5)

The following measurements predate configured avionics and native instruments.
Their two-page and contact-label workloads no longer describe standard firmware.

Measured 2026-09-12 on the Ryzen 9 5900XT (16 physical cores, 32 logical),
using the normal development profile: workspace opt-level 1, dependencies 3,
and the bundled release-built WASM. Bevy used 24 compute workers.

`Ship step` is the last ship update's wall-clock duration, not a CPU-time
measurement or moving average. It includes the controller's sensor syscalls and
MFD draw-command production. Gravity, orbital integration, index rebuilding,
shared scan-scene construction, the serial boot/refill pass, and display
rasterization/rendering are outside it. Switching orbital mechanics cannot
reduce this particular counter.

The headless fixture uses the default 501 orbital positions and authored system
bodies, the real spatial index, nearest-256 scans with occlusion, hardware, WASM,
and the production parallel ship-update system. It warms for 100 ticks and measures
200 more. Positions remain frozen and IMU samples are refreshed; this isolates
ship work, rather than benchmarking a complete moving/rendered simulation.
The first ship requests its two default MFD pages.

## Observed cost

One representative run with the new traffic pursuit orders:

| Stage | First ship, mean µs | Traffic, mean µs |
|---|---:|---:|
| Prepare own state/input | 1.2 | 2.5 |
| Controller excluding native scene query | 232.4 | 61.5 |
| Native scene query, including contact construction | 138.5 | 129.0 |
| Apply/publish output and replace old data | 22.9 | 9.7 |
| Hardware step and force/mass updates | 1.0 | 0.7 |
| **Total** | **396.0** | **203.5** |

The first ship's median was 363 µs, p95 618 µs, and maximum 1,693 µs.
Fleet wall time was 5.66 ms per update. Per-ship durations overlap in parallel;
their sum is not fleet wall time, nor a measurement of CPU consumption.
252/500 traffic ships acquired the first ship in this frozen scene. The remaining
ships retain their orders while waiting for nearest-256, unoccluded visibility.
A repeat measured 361 µs mean / 511 µs p95 on the first ship and 5.47 ms per
fleet update; all 100,200 measured callbacks ran, with no gas-skipped callbacks.

Controlled comparisons with idle traffic:

| Workload | First ship mean µs | Traffic mean µs | Fleet ms/update |
|---|---:|---:|---:|
| Real scans, two MFDs on first ship | 367.8 | 189.3 | 5.15 |
| Real scans, no MFD requests anywhere | 185.1 | 182.4 | 4.95 |
| Empty scan results, two MFDs on first ship | 36.0 | 6.9 | 0.38 |
| Real scans/MFDs, one compute worker | 149.7 | 78.1 | 39.40 |

Removing MFD requests also disables contact-label reads in the example guest;
the approximately 183 µs difference is their combined cost, not rasterization.
The empty-scene experiment removes both the spatial query and downstream
contact processing. These differences are indicative, not additive exact
attributions. Parallel cache/SMT/contention effects and OS scheduling contribute
to the observed wall times; no allocation-specific attribution was measured.

Changing workspace opt-level from 1 to 3 in the one-worker comparison only
changed the first ship from 149.7 to 147.7 µs and fleet time from 39.40 to 38.46 ms.
This is not predominantly an unoptimized-build problem. Parallel runs vary more.

## Syscalls and likely next work

A separate warm, single-VM microbenchmark measured a metered gas read at
13.8 ns/iteration and a metered 64-byte header read at 30.3 ns/iteration,
including the guest loop and amortized callback cost. The empty loop measured
0.9 ns/iteration. These are cheap calls without scans, strings, drawing, or fleet
contention; they do not predict the cost of a complete sensor syscall.

The language-neutral ABI boundary is therefore not inherently a hundreds-of-
microseconds operation. Inspection identifies the following work to address:

1. `detect_nearest` gathers every in-range candidate before selecting N. Charging
   a fixed multiple of N currently does **not** bound the native search cost by N.
   A nearest-neighbor traversal and a policy bounding search work need attention
   before MMO-scale density.
2. The old example read names for every scanned contact whenever any MFD was requested. ABI 6 removes this from standard firmware.
   Each host label lookup linearly searches the contact vector. Read only labels
   actually needed by the current page/target and avoid repeated linear lookups.
3. Both host and guest allocate/copy contact vectors and strings every callback;
   publishing replaces/frees previous data. Reuse buffers and avoid unnecessary
   owned-name copies, then measure again under parallel load.
4. Separate display refresh cadence from control/scan cadence where appropriate.
   Only observed ships need display work on a server.

Hardware is under 1% of this measured update. Orbital physics needs its own fleet
measurement, but putting ships on rails would not address these measured costs.

## Reproduce

Run the current coasting fleet with the commands below. The historical automatic
pursuit workload is no longer part of this fixture.

```sh
cargo test -p toy-sim profile_default_fleet -- --ignored --nocapture
SHIP_PROFILE_MODE=idle cargo test -p toy-sim profile_default_fleet -- --ignored --nocapture
SHIP_PROFILE_MODE=no_instruments cargo test -p toy-sim profile_default_fleet -- --ignored --nocapture
SHIP_PROFILE_MODE=custom_screen cargo test -p toy-sim profile_default_fleet -- --ignored --nocapture
SHIP_PROFILE_MODE=empty_scan cargo test -p toy-sim profile_default_fleet -- --ignored --nocapture
SHIP_PROFILE_THREADS=1 SHIP_PROFILE_MODE=idle cargo test -p toy-sim profile_default_fleet -- --ignored --nocapture
cargo test -p toy-sim-ship-wasm profile_syscall_overhead -- --ignored --nocapture
```

Run each measurement separately, without concurrent builds or benchmarks.
The in-game Hardware diagnostics window displays the same timing stages for the
actual moving/rendered workload. The native scan time is included in controller
time there; do not add it twice.
