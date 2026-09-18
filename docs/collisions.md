# Collisions

Ships in space and projectiles are integrated by a time-ordered continuous collision solver ([crates/toy-sim-server/src/sim/physics/collision](../crates/toy-sim-server/src/sim/physics/collision)). Within each 10 Hz tick, the solver predicts the first contact between each nearby pair, processes events in time order, and resolves every contact as an energy-absorbing impact. The absorbed energy is deposited as heat in hulls or shields. The solver also schedules weapon launches ([weapons.md](weapons.md)) and destruction from overheating.

Geometry queries, broad-phase trees and contact manifolds come from Parry (`parry3d-f64`). Trajectory sampling, heat accounting and event scheduling are implemented in this module.

## Participating bodies

The `CollisionBody` marker selects ships in space and weapon projectiles. `apply_forces` in [physics.rs](../crates/toy-sim-server/src/sim/physics.rs) skips these entities; the collision step integrates them.

Docked ships are station inventory. Docking removes their motion, collision and spatial components, and their mass contributes to the host. They do not add hull members to the host's collision body. Gate mouths use a non-solid aperture.

Geometry:

- **Ship hull:** a Parry compound built by rasterizing the compiled assembly onto a 1 m grid and merging adjacent occupied cells into boxes. Cylinders, habitat hubs/rings and hangar interiors have explicit occupancy profiles. Moving habitat spokes reserve their swept region. Geometry is cached per compiled design. Sub-metre barrels clear the conservative voxel envelope by at most 3 m; real part geometry still rejects obstructed fire.
- **Ship shield:** a ball of `shield_radius(r) = r + max(0.1·r, 0.5 m)`, where `r` is the design radius.
- **Projectile:** a ball of the projectile radius for both hull and shield.

Ship geometry is cached per compiled design.

## The collision step

`ecs::step` runs in `FixedPostUpdate` in `SimulationSystems::Integrate`, after `apply_forces`.

1. **Build bodies.** For each root collision body, apply this tick's accumulated force and torque as a kick. Velocity gains `force/mass·dt` (the force includes gravity, thrust and drag), and world angular momentum gains `torque·dt`. Attach ship members with hull hit points and thermal state, and projectile members.
2. **Shield activation** at tick start (`activate`). A field requires an installed generator, enabled and powered hardware, and deployed coolant. It becomes active when its sphere is clear of other bodies. Projectiles launched by the member itself are ignored in the clearance test. Disabled, unpowered, depleted and blocked fields report their corresponding states. Re-enabling a clear field can take effect at the next tick boundary.
3. **Weapons.** Copy each armed ship's weapon state and inventory into the solver workspace.
4. **Simulate** the tick (below).
5. **Write back.** For each body, write position, rotation, velocity, mass properties and end-of-tick angular velocity, and clear the accumulators. Rewrite the `AccelerometerState` as `force/mass − gravity + impulse Δv/dt` in body axes, plus the angular terms. Write hull, thermal and weapon state back to `ShipHardware`, and lifetime and hit points back to `Projectile`. Write poses for docked members.
6. **Effects and destruction.** Send motion segments, shots and impacts to `combat_effects`. Replace destroyed ships with explosions and despawn destroyed projectiles. Despawn dock parents whose members were all destroyed.
7. **Statistics.** Update `CollisionStats`, shown in the Diagnostics window: collision step time, body count, index/query/solve times, candidate pairs, geometry queries, impulses, contact reviews and total impact heat.

## Motion within a tick

Between events, each body translates at constant velocity. Its rotation follows torque-free rigid-body motion with constant world angular momentum, using the split integrator in [rotation.rs](../crates/toy-sim-server/src/sim/physics/rotation.rs). Because forces were applied as a kick at the tick start, the combined scheme is symplectic Euler, consistent with `apply_forces`.

## Broad phase

The solver works in a common translating frame: the first body's velocity plus the mass-weighted mean velocity offset. Ships that share an orbital velocity therefore have short swept volumes. Each live body becomes a proxy: its start position, its displacement over the rest of the tick in that frame, and its radius plus 2 mm.

`RegionIndex` ([spatial_tree.rs](../crates/toy-sim-server/src/sim/spatial_tree.rs)) stores proxies in 100 km integer-addressed regions, each with its own Parry BVH. A swept proxy occupies only the cells along its capsule. The index persists between ticks and is refreshed in place. `pairs()` returns candidate pairs, and after each event `neighbors()` finds the pairs to predict again.

## Narrow phase: predicting the next contact

For each candidate pair, `prediction`:

1. Finds the time window during which the bounding spheres (radii summed, plus 4 mm) overlap along the relative straight-line motion. It uses a closest-approach formula that stays numerically stable. No window means no event.
2. Sets a contact tolerance `ε = clamp(0.0001 × smallest part dimension, 10 µm, 1 mm)`.
3. **Translation-only fast path.** If neither body rotates, or all of a body's live members are centred balls, it runs Parry's `cast_shapes` for every member pair and keeps the earliest approaching hit. It keeps the cast's own witness points and normal. If the pair already penetrates, or Parry reports failure, it falls back to the general path.
4. **General path: conservative advancement.** Starting at the window's entry, it computes the minimum distance over member pairs. It then advances by `0.9·(distance − ε) / speed bound`. The speed bound is the relative linear speed plus each body's radius times a rotation-rate bound, derived for the five-stage rotation sampler. When the distance falls within `2ε`, it switches to a contact review.
5. **Contact review.** For each primitive pair (compound children are pruned by BVH), it sweeps forward and builds convex contact manifolds when primitives come within `2ε`. A manifold point becomes a contact event if the points are approaching or penetrating deeper than `ε`. Otherwise the pair is scheduled for another review after `min(0.01 s, 0.25·feature / speed)`. A face already at rest therefore cannot hide a new contact elsewhere on the same compound.

A numerical assertion stops the program if advancement can no longer make progress in time.

## Event processing

Events go into a binary heap ordered by time. Contacts, reviews and thermal events come before launches at the same instant. Each body has a generation counter, and events from an older generation are discarded. After any event, the changed bodies are re-indexed, and every pair involving them is predicted again from the event time.

| Event | Handling |
| --- | --- |
| Contact | Compute the persistent contact manifolds for the member pair (cached across ticks per entity and geometry). Resolve the triggering point and every manifold point within `2.1ε`, stopping if a member is destroyed or a shield state changes. |
| Review | Predict the pair again from the review time. |
| Fire | Launch a projectile if the weapon's shot counter still matches ([weapons.md](weapons.md)). The new body joins the timeline immediately. |
| Thermal | The member's hull reaches zero from heat (the time is found by bisection), or a projectile reaches its expiry time. The member is destroyed. |

After the queue empties, weapons advance to the end of the tick and every body drifts and advances its thermal state to the tick end. Coolant that escapes through ablation reduces member and assembly mass and inertia. The mass adjustment preserves the surviving body’s velocity and spin.

## Contact resolution

Each member picks its shape against the other body: the shield sphere when its shield is active, unless the other body is a projectile this member launched; otherwise the hull.

### Ordinary impacts

For contact normal `n`, lever arms `ra` and `rb`, and world inverse inertia tensors, the effective inverse mass is:

```
k = 1/ma + 1/mb + (ra×n)·Ia⁻¹(ra×n) + (rb×n)·Ib⁻¹(rb×n)
```

When the bodies are closing at speed `c`:

- The maximum absorbable energy is `q = c² / (2k)`. It corresponds to removing all normal closing velocity, so there is no restitution and no friction.
- If either member is shielded, `q` is limited to twice that shield's remaining interception energy. The impulse is then reduced so the absorbed energy equals `q`, and the bodies keep part of their closing speed.
- The impulse changes linear and angular momentum and is recorded for the accelerometer.
- Half of `q` goes to each member: into its shield when shielded, otherwise into its hull as heat and damage at 1 hit point per 100 kJ.

After resolution, if the shapes still overlap, the bodies are pushed apart along the current contact normal by the penetration depth plus 10 µm. The push is split in inverse proportion to mass.

### Slugs against shields

A projectile meeting another ship's active shield exchanges an impulse along its full relative contact velocity. The energy absorbed is bounded by the available reserve and deployed material. If enough material remains, the projectile is stopped and removed. If the reserve and screen are exhausted first, the projectile keeps the residual velocity and continues into the hull collision path. A trace of newly deployed coolant therefore provides only a finite amount of protection.

All energy dissipated by a shield interception enters the shield coolant. Partial interceptions preserve the projectile’s structural state so its remaining kinetic energy can reach the hull. Interception vaporizes coolant and carries its stored heat away; the solver limits the impulse to the energy the shield can absorb.
### Slugs against hulls

A slug hitting a hull is an ordinary impact. A projectile's hit points equal its mass in kilograms, so a 10 g bearing is destroyed by 1 kJ of impact heat. The ship takes damage from its half of the energy.

### Shield depletion

The shield's circulating material normally stays at full mass because the reserve tank replaces ablation losses. Thermal evolution consumes reserve while preserving coverage and emitting area. Once replacement fails, the deployed inventory falls and protection declines. Exhausting that inventory changes the member's collision shape to its hull; generation counters invalidate earlier predictions and the solver predicts contact again.

See [ships.md](ships.md#heat-hull-and-shields) for the coolant and heat equations.
## Destruction

When a member's hull reaches zero (from impact or heat), a `Destruction` record captures its pose, velocity (including rotation about the body's centre of mass), mass and thermal state. If other members survive in a docked body, the body's centre of mass, inertia, radius and momentum are recomputed without the destroyed member. The survivors keep their velocity field.

In the ECS, `combat_effects::destroy`:

- Despawns destroyed projectiles.
- Ship breakup produces a brief billboard flash and a glowing puff lasting at most two seconds. Fragments remain visible for up to ten seconds: one fragment per part, with momentum-neutral radial velocities from a share of the stored heat and electrical energy, plus small chips. Flashes and puffs share a quad, texture atlas, and material, with bounded screen size and at most 256 sprites per view. If the ship had camera focus, focus moves to the explosion and the camera leaves any orbital framing.
- Despawns the ship.

When the controlled ship is gone, a "Ship destroyed" window offers "Reset encounter".

## Presentation data

The solver records motion segments for bodies whose motion changed within the tick, and for all projectiles. `combat_effects` samples these segments at the presentation time. Fast slugs and hulls are therefore drawn at their true intermediate positions, and tracer ribbons follow the exact launch-to-impact path. Impact events whose energy is not already part of a ship breakup spawn small flashes, with debris chips when no shield was involved.

## Tests and benchmarks

```sh
cargo test -p toy-sim collision
```

[tests.rs](../crates/toy-sim-server/src/sim/physics/collision/solver_tests.rs) covers:

- stable sphere entry and exit times
- head-on momentum conservation with heat counted once
- a fast slug hitting a thin box while missing the empty space inside a bounding sphere
- invariance under large common positions and velocities
- contacts caused by rotation alone
- re-testing against the hull after a shield collapses
- destroyed slugs not hitting a second ship
- region boundaries and diagonal sweeps
- resting penetration correction without heat
- nearest-neighbour queries
- resting parts not masking new contacts
- docked member destruction
- region storage reuse
- the rotation speed bound
- rotational energy in off-centre impacts
- spinning spheres keeping their cast normal
- shields absorbing slow glancing slugs
- projectile expiry at 2 s

[ecs.rs](../crates/toy-sim-server/src/sim/physics/collision/ecs.rs) tests field activation, launched slug materialization, repeated impacts, slug impulse transfer and shield clearance.

Two benchmarks are ignored by default. Run them in release mode:

```sh
# 10,000 and 100,000 bodies in sparse, dense, battles, mixed and slugs scenarios
cargo test -p toy-sim --release physics::collision::tests::scale_benchmark -- --ignored --exact --nocapture

# Nearest-32 sensor queries with occlusion over 10,000 and 100,000 objects
cargo test -p toy-sim --release physics::collision::tests::sensor_scale_benchmark -- --ignored --exact --nocapture
```

`scale_benchmark` prints the thread count, cold, median and p95 milliseconds, the index, query and solve times, and pair, query and impact counts. On Linux it also prints peak RSS. See [ship-step-profile.md](ship-step-profile.md) for other profiling commands.

## Limitations

- Contacts are perfectly inelastic along the normal, with no friction or restitution.
- Forces are applied as one kick per tick, so gravity and thrust do not curve paths within a tick.
- Shields are spheres, and hulls are unions of part boxes. Part models do not affect collisions.
- Projectiles are not in the sensor index.
