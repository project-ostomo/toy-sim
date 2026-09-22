# Collisions

Ships in space and projectiles are integrated by a time-ordered continuous collision solver ([crates/osg-server/src/sim/physics/collision](../crates/osg-server/src/sim/physics/collision)). Within each 10 Hz tick, the solver predicts the first contact between each nearby pair, processes events in time order, and resolves contacts with partial restitution and impact heat. The absorbed energy is deposited as heat in hulls or shields. The solver also schedules weapon launches ([weapons.md](weapons.md)) and destruction from overheating.

The shared `osg-spatial-bvh` service supplies broad-phase candidates. Parry (`parry3d-f64`) supplies geometry queries, contact manifolds and acceleration structures inside compound shapes. Trajectory sampling, heat accounting and event scheduling are implemented in this module.

## Participating bodies

The `CollisionBody` marker selects ships in space and weapon projectiles. `apply_forces` in [physics.rs](../crates/osg-server/src/sim/physics.rs) skips these entities; the collision step integrates them.

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
5. **Write back.** For each body, write position, rotation, velocity, mass properties and end-of-tick angular velocity, and clear the accumulators. Rewrite the `AccelerometerState` as `force/mass − gravity + impulse Δv/dt` in body axes, plus the angular terms. Write hull, thermal, inventory and installed weapon components back to their ECS entities, and lifetime and hit points back to `Projectile`.
6. **Effects and destruction.** Record motion segments, shots and impacts through `combat::ingest` for authorized client presentation. Despawn destroyed projectiles. Move destroyed ships into dormant destroyed state, retaining their stable identity.
7. **Statistics.** Update `CollisionStats` and server tracing: collision step time, body count, index/query/solve times, candidate pairs, geometry queries, impulses, contact reviews, rotational-envelope fallbacks and total impact heat.

## Motion within a tick

Between events, each body translates at constant velocity. Its rotation follows torque-free rigid-body motion with constant world angular momentum, using the split integrator in [rotation.rs](../crates/osg-server/src/sim/physics/rotation.rs). Because forces were applied as a kick at the tick start, the combined scheme is symplectic Euler, consistent with `apply_forces`.

Stationary and slowly rotating bodies use the direct drift sampler. Faster
rotation is sampled from cached short-angle segments of the same integrator.
Each segment preserves world angular momentum, and a conservative derivative
bound covers the continuous path across segment boundaries. This prevents a
whole-tick polynomial bound from causing millions of tiny distance-query steps
after a body begins tumbling. Impacts and mass changes invalidate the trajectory.

The cache has at most 4,096 segments. When finer segmentation would be needed,
collision uses a conservative spherical envelope around each rotating member,
including its offset from the body's centre. This can detect contact earlier
than the detailed hull at extreme spin; it does not drop collision candidates.
The solver counts these fallbacks in its diagnostics.

## Broad phase

Each live body contributes a collision record to the shared dynamic BVH: its integer tick-start position, velocity, and conservative radius plus 2 mm. Swept AABBs include the entire predicted translation and rotational extent.

The [spatial service](../crates/osg-spatial-bvh/src/service.rs) finds overlapping swept bounds. The narrow phase tests relative motion before continuous shape queries. Catalogue sources and observation records are excluded from collision pairs by record kind.

The instantaneous and swept dynamic trees rebuild once per simulation tick. Collision pairs and moving-target queries use the swept tree. After events, changed trajectories and newly fired projectiles query the fixed tick-start scene. Generation counters invalidate stale contact predictions. Objects created during a tick become indexed targets on the next tick; no mid-tick tree mutation occurs.

Shield clearance and beams query the service's instantaneous tree. Parry's BVHs within compound hulls still prune primitive pairs during detailed shape queries.

## Narrow phase: predicting the next contact

For each candidate pair, `prediction`:

1. Finds the time window during which the bounding spheres (radii summed, plus 4 mm) overlap along the relative straight-line motion. It uses a closest-approach formula that stays numerically stable. No window means no event.
2. Sets a contact tolerance `ε = clamp(0.001 × smallest part dimension, 10 µm, 1 mm)`.
3. **Translation-only fast path.** If neither body rotates, or all of a body's live members are centred balls, it runs Parry's `cast_shapes` for every member pair and keeps the earliest approaching hit. It keeps the cast's own witness points and normal. If the pair already penetrates, or Parry reports failure, it falls back to the general path.
4. **General path: conservative advancement.** Starting at the window's entry, it computes the minimum distance over member pairs. It then advances by `0.9·(distance − ε) / speed bound`. The speed bound is the relative linear speed plus each body's radius times its conservative trajectory rotation-rate bound. Centred spheres and rotational envelopes contribute no rotational geometry motion. When the distance falls within `2ε`, it switches to a contact review.
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

### Shared impact response

For contact normal `n`, lever arms `ra` and `rb`, and world inverse inertia tensors, the effective inverse mass is:

```
k = 1/ma + 1/mb + (ra×n)·Ia⁻¹(ra×n) + (rb×n)·Ib⁻¹(rb×n)
```

When the bodies are closing at speed `c`:

- The restitution coefficient is `e = 0.3` for normal closing speeds of at least 0.1 m/s, and zero below that threshold. Tangential velocity is unaffected: contacts have no friction.
- The rebound impulse is `J = (1 + e)c/k`, and the dissipated energy is `q = (1 − e²)c²/(2k)`. A 10 m/s normal approach therefore produces 3 m/s normal separation when neither shield limits the response.
- If either member is shielded, `q` is limited to twice that shield's remaining interception energy. When this cap prevents the full response, the solver uses the smaller impulse that dissipates exactly the available energy. The bodies retain some closing velocity instead of receiving a rebound that the shield cannot support.
- The impulse changes linear and angular momentum and is recorded for the accelerometer.
- Half of `q` goes to each member: into its shield when shielded, otherwise into its hull as heat and damage at 1 hit point per 100 kJ.

After resolution, the solver separates the current shapes to a skin of `10ε`, sharing the correction in inverse proportion to mass. The skin is outside the `2ε` detection shell. This prevents repeated near-identical contacts from consuming a tick while bodies remain within numerical contact tolerance; it changes positions without adding velocity or heat.

### Projectiles

Projectiles use the same contact response, energy split and hull damage as ships. There is no separate shield interception impulse or automatic deletion on contact. A projectile's hit points equal its mass in kilograms: a 1 g round is destroyed by 100 J deposited into its hull. Ordinary fast impacts usually exceed that threshold. A slow or grazing impact can leave a projectile alive and deflected; a depleted shield can leave enough residual motion for a later hull impact.

The launching ship's shield remains transparent to its own projectiles. Its hull still participates in collision detection.

### Shield depletion

The shield's circulating material normally stays at full mass because the reserve tank replaces ablation losses. Thermal evolution consumes reserve while preserving coverage and emitting area. Once replacement fails, the deployed inventory falls and protection declines. Exhausting that inventory changes the member's collision shape to its hull; generation counters invalidate earlier predictions and the solver predicts contact again.

See [ships.md](ships.md#heat-hull-and-shields) for the coolant and heat equations.
## Destruction

When a member's hull reaches zero (from impact or heat), a `Destruction` record captures its pose, velocity (including rotation about the body's centre of mass), mass and thermal state. If other members survive in a docked body, the body's centre of mass, inertia, radius and momentum are recomputed without the destroyed member. The survivors keep their velocity field.

The server records destruction through `combat::ingest` before changing the
entity. Ordinary projectiles are despawned. Ships enter `Presence::Destroyed`,
lose active physics and sensor participation, and retain their stable identity.
Stored ships become inventory inside the wreck.

Authorized clients receive the destruction event and render the breakup using
shared billboard effects and debris. The focused hull mesh is removed while its
destruction effect remains visible.

## Presentation data

The solver records motion segments for bodies whose motion changed within the tick, and for all projectiles. The server publishes authorized combat events, and the client samples their motion segments at presentation time. Fast slugs and hulls are therefore drawn at their true intermediate positions, and tracer ribbons follow the exact launch-to-impact path. Impact events whose energy is not already part of a ship breakup spawn small flashes, with debris chips when no shield was involved.

## Tests and benchmarks

```sh
cargo test -p osg-server collision
```

[tests.rs](../crates/osg-server/src/sim/physics/collision/solver_tests.rs) covers:

- stable sphere entry and exit times
- head-on momentum conservation with heat counted once
- a fast slug hitting a thin box while missing the empty space inside a bounding sphere
- invariance under large common positions and velocities
- contacts caused by rotation alone
- re-testing against the hull after a shield collapses
- destroyed slugs not hitting a second ship
- galactic coordinates and diagonal sweeps
- resting penetration correction without heat
- nearest-neighbour queries
- resting parts not masking new contacts
- docked member destruction
- successive snapshots, removal and reordered identities
- the rotation speed bound
- rotational energy in off-centre impacts
- spinning spheres keeping their cast normal
- shared impact damage destroying fast slugs while slow glancing slugs survive
- projectile expiry at 2 s
- persistent three-body contact and large common orbital motion
- bounded rotation caches and conservative extreme-spin envelopes

[ecs.rs](../crates/osg-server/src/sim/physics/collision/ecs.rs) tests field activation, launched slug materialization, repeated impacts, slug impulse transfer and shield clearance.

The shared service has `cargo bench -p osg-spatial-bvh --bench gaia`;
it measures index construction and visibility queries
independently of collision detection. These CPU results do not measure client GPU performance.
