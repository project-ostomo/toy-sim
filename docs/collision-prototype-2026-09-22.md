# Spatial hash and Rapier collision experiment

This is an optional, headless experiment behind the `collision-prototype`
feature. The production schedules still use their existing collision solver.
The purpose is to delegate collision detection and response to Rapier and
replace repeated global BVH reconstruction with persistent index updates.
The current prototype includes the full static catalogue by default and uses
512 km cells at the finest spatial resolution. Index coordinates still retain
kilometre precision; queries filter candidate records by their stored positions.
The experiment compares local Rapier worlds with f64 or f32 arithmetic, fresh
or cached worlds, and 100, 10, or 5 ms steps inside a 100 ms server tick.

## Running it

```sh
cargo run --release -p osg-server --features collision-prototype \
  --example collision_prototype -- --ships 16 --step-ms 10
```

Options:

- `--ships 1` or `--ships 16`: extract a warmed production scenario, including
  its background traffic, loaded celestial bodies, and catalogue summaries.
- `--scripted`: repeatedly collide 32 fast projectiles with thin compound hulls.
- `--no-catalogue`: omit the static catalogue from the prototype index while
  retaining the same active celestial sources and collision bodies.
- `--f32`: use single precision inside each local world.
- `--fresh`: construct a new world every tick. Otherwise retain worlds whose
  sorted membership is unchanged; rebuild after merges or splits.
- `--step-ms 100|10|5`: Rapier step size. Launch and expiry times introduce
  additional boundaries.
- `--ticks 300 --warmup 70`: measured and warmup ticks.
- `--baseline`: time the existing production scenario and its collision stages.

Both precisions share the same engine implementation. Immutable collision
geometry is cached across worlds in both fresh and cached modes. No dynamic
Bevy linking is needed.

The serial measurement driver preserves each run's CSV and stderr log:

```sh
python3 scripts/benchmark-collision-prototype.py --output /tmp/collision-timings
python3 scripts/benchmark-collision-prototype.py --allocations \
  --output /tmp/collision-allocations
```

The second command builds with a counting allocator. Its timings must be kept
separate from ordinary measurements. The full default timing matrix has 114
runs: six production baselines and 108 prototype runs, with three repeats,
70 warmup ticks, and 300 measured ticks. Results are resumable by output path;
use a new directory after changing the implementation. The full matrix has
not been completed; the measurements below focus on indexing and grouping.

## Catalogue and isolated-body measurements

With the full catalogue included and the finest cells changed to **512 km**,
the same three-run protocol now measures:

| Finest cells | Index update | Discovery | Physics execution | Total tick |
|---|---:|---:|---:|---:|
| 1 km | 20.00 ms | 10.69 ms | 0.19 ms | 34.00 ms |
| 512 km | 7.28 ms | 6.95 ms | 0.18 ms | 17.60 ms |

The 512 km runs retain all 1,001,760 catalogue entries, 43,891 active celestial
sources, and 3,002 collision bodies. They still produce one two-body group
and 3,000 isolated bodies. The mean of the three per-run p95 tick times is
19.45 ms. [Raw 512 km measurements](measurements/collision-prototype-512km.csv).

The index omits spatial levels below 512 km. Movement within a cell updates
the stored record position without moving its membership in that cell or
coarser cells. Queries smaller than a cell still apply exact distance filtering
to the stored kilometre coordinates. Scan comparisons cover geometric and
visibility queries, movement, brightness changes, and removal at minimum
shifts 0, 9, and 63; both spatial scale configurations pass. All 16 prototype
physics tests pass with the 512 km setting.

The following earlier measurements use kilometre coordinates and the original
spatial resolutions, starting with 1 km cells. These runs use f64, cached
worlds, 10 ms Rapier steps, 70 warmup
ticks, and 300 measured ticks, repeated three times serially. Catalogue
inclusion order alternates between repeats. Values below are mean milliseconds
per tick across the three runs.

The one-player fixture contains 3,002 collision bodies and 43,891 active
celestial sources. An average of 40,534 index records change per tick. The
optional catalogue adds 1,001,760 static records. Removing it does not change
the active sources, gravity inputs, candidate pairs, or collision bodies.

| Group handling | Catalogue in index | Index update | Discovery | Physics execution | Total tick |
|---|---|---:|---:|---:|---:|
| Singleton groups | Yes | 20.84 | 14.96 | 3.00 | 42.32 |
| Singleton groups | No | 15.56 | 8.70 | 2.96 | 30.72 |
| Separate free flight | Yes | 20.00 | 10.69 | 0.19 | 34.00 |
| Separate free flight | No | 14.61 | 6.33 | 0.19 | 24.27 |

The current implementation reports one two-body collision group and 3,000
isolated bodies in every measured tick. The old group count of 3,001 included
those isolated bodies as singleton groups. The free-flight pass avoids their
individual group vectors, contact sets, job keys, and parallel job dispatch.
Discovery still needs spatial lookups to determine which bodies are isolated.

These measurements show that the static catalogue adds query and update cost
even when its entries are never updated. They do not isolate which cache or
hash-table effects cause that cost. The fixture loader still loads the bundled
universe when `--no-catalogue` is set; the flag removes catalogue records from
the prototype index, not from the fixture loader's retained universe data.

[Raw per-run measurements](measurements/collision-prototype-catalogue.csv)
include all twelve runs. All 16 prototype correctness tests and the spatial
hash tests pass, and the hash benchmark targets compile with the updated API.

## Ownership of the tick

Ordinary ECS systems perform force sampling, persistent index updates, group
discovery, parallel simulation, writeback, removal, and celestial source drift.
They use queries and resources rather than an exclusive `&mut World` system.

The force system applies gravity and externally supplied acceleration and
torque once per server tick. Rapier receives the resulting velocities, explicit
mass and inertia, zero gravity, and cleared force accumulators. A scheduled
projectile inherits its owner's velocity kick; a scheduled arrival gets its
own kick over its remaining lifetime in the tick. Specific acceleration
includes thrust and collision impulses and excludes gravity.

Authoritative positions remain i128 micrometres. Hash coordinates are checked
i64 kilometres, using floor division even for negative coordinates. Index
records are updated only when their coarse position or brightness changes.
The one `LuminosityMap` contains summaries, active celestial sources, and dark
collision bodies. It supports geometric queries over every brightness bucket
and conservative visibility candidates with position-error padding.
Static catalogue records are seeded directly into the map once. They are not
ECS entities. A single synchronization system updates the index at tick start;
movement during the tick is incorporated at the next tick boundary.

Candidate pairs use a 100 km travel bound plus both bounding radii. A 4 km
padding covers coordinate quantization before exact authoritative-position
filtering. Scheduled launches are back-projected to tick start for discovery.
Only bodies with candidate neighbours enter connected components. Isolated
bodies go through one free-flight pass, without allocating singleton groups,
contact sets, or Rapier jobs. Components are retained whole and ordered
deterministically. The
assumption is that relative speeds remain at most 1,000 km/s throughout the
interval; speed diagnostics do not enforce or repair this assumption.

Each component gets a world-aligned local frame, with a common position and
velocity removed. World position is reconstructed using the frame's drift.
Singletons use the existing free-flight rotation integrator. Rapier uses
compound hulls, shield spheres, projectile spheres, CCD, gyroscopic forces,
restitution 0.3, zero friction and damping, and disabled sleeping. Collider
density is zero so the game's supplied mass and inertia remain authoritative.

Rapier bodies use principal inertia axes internally, with an inverse rotation
on the collider. ECS continues to use the model frame. This is necessary for
Rapier 0.34's `angvel_with_gyroscopic_forces`, which applies diagonal inertia
in body axes. Passing a non-diagonal tensor without this frame conversion
failed the free-spin comparison by about 0.42 radians after ten seconds.

Contact-start events estimate dissipated normal kinetic energy using the
effective contact mass, including rotational inertia. Each episode deposits
energy once, split between the participants. Shield depletion changes the
collider for the next step; hull destruction disables the body before the
next step. This is a deliberately small damage model for the experiment.

## Scope and limitations

Production fixtures are snapshots after 70 ticks. The prototype then replays
gravity with linear celestial ephemerides; it does not run live controllers,
sensors, travel, weapons scheduling, docking, or the production thermal model.
The celestial records participate in indexing and gravity, but are not Rapier
colliders in this experiment. Scheduled launches and shield changes are
exercised by dedicated fixtures and tests.

Prototype tick time therefore measures the proposed physics/index pipeline,
not an equivalent whole-server replacement. Production baseline time includes
the rest of the simulation. Construction and solver columns sum worker time;
the physics column is elapsed wall time and includes parallel dispatch.

RSS includes the bundled universe and the allocator's retained storage from
constructing the production fixture, including its original BVH. It cannot
be interpreted as the incremental memory cost of replacing that BVH.

The f32 tests establish a local error sample, not a galaxy-wide precision
guarantee. Transitive components can span large distances; a common frame
does not make every large component safe for f32. Fast rotations and contact
normal/point estimates also need broader validation before migration.

## Migration work after this experiment

1. Resolve index update and query costs, including the cost of moving active
   celestial records through the luminosity buckets. Retain lazy catalogue
   summaries and materialize only active systems.
2. Introduce one authoritative ECS index resource and move production spatial
   consumers to it, including sensors, illumination, activation, and collision
   grouping. Preserve exact brightness checks after conservative queries.
3. Connect actual thrust, drag, gravity, torque, launch, and arrival scheduling
   to the once-per-tick force handoff. Preserve orbital and accelerometer tests
   across changing group membership.
4. Connect contacts to the game's thermal, shield, destruction, and weapon
   rules; define docking and multi-body ownership explicitly.
5. Choose precision and step policy from error budgets, validate dense groups
   and long-running contact episodes, then remove the obsolete production BVH
   and solver interfaces with their callers.
