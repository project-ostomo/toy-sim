# Weapons

The catalogue includes projectile guns and lasers. Projectile guns launch physical slugs. A slug is a small sphere with mass that flies through the same continuous collision solver as ships. Firmware controls weapons by staging a short-lived aim-and-trigger setting each tick. The hardware slews turrets, checks firing conditions, and schedules launches inside the tick. Electric weapons draw shot energy from the shared ship battery. Conventional guns use cartridges and draw no shot electricity or separate counterpropellant.

Source:

- Definitions, specs and servo mechanics: [crates/osg-ships/src/weapons.rs](../crates/osg-ships/src/weapons.rs)
- Hardware commands and readings: [server hardware systems](../crates/osg-server/src/sim/hardware.rs)
- Launch events inside the collision timeline: [crates/osg-server/src/sim/physics/collision/weapons.rs](../crates/osg-server/src/sim/physics/collision/weapons.rs)
- Standard firmware engagement policy: [crates/osg-example-controller/src/weapons.rs](../crates/osg-example-controller/src/weapons.rs)

## Defining a weapon part

A weapon part uses `kind = "weapon"` and a `[parts.weapon]` table. Unknown keys are rejected.

| Field | Validation | Meaning |
| --- | --- | --- |
| `mechanism` | Tagged `gun` or `laser` table | Mechanism-specific parameters listed below |
| `cycle_interval_s` | > 0 | Minimum time between shots |
| `efficiency` | in (0, 1] | Fraction of supplied energy converted to projectile kinetic energy or emitted light |
| `dispersion_half_angle_rad` | in [0, 0.1) | Half angle of random pointing error for each shot |
| `muzzle_offset_m` | finite, nonzero; guns require length above projectile radius | Muzzle position from the pivot, in barrel axes |
| `slew_rate_rad_s` | ≥ 0 | Yaw and pitch rate. Zero makes a fixed gun. |

Gun mechanisms contain `kind = "gun"`, `drive` (`electric` by default or `chemical`), `ammunition` (a catalogue resource), `projectile_radius_m`, and `muzzle_speed_m_s`. Radius and speed must be positive. One resource unit is one projectile, with the resource's unit mass.

Laser mechanisms contain `kind = "laser"`, `pulse_energy_j`, `range_m`, `beam_waist_m`, and `divergence_half_angle_rad`. All are positive; divergence must be below 0.1 radians. Pointing dispersion and beam divergence are independent.

For example:

```toml
[parts.weapon]
mechanism = { kind = "laser", pulse_energy_j = 200000.0, range_m = 100000.0, beam_waist_m = 0.1, divergence_half_angle_rad = 0.00002 }
cycle_interval_s = 0.1
efficiency = 0.4
dispersion_half_angle_rad = 0.00002
muzzle_offset_m = [0.0, 0.0, -1.1]
slew_rate_rad_s = 3.14159
```

`device_spec` returns `GunSpec` (160 bytes, device kind 9) or `LaserSpec` (152 bytes, device kind 11). Both contain the common `WeaponSpec` mount record (120 bytes). Derived records add:

- The ammunition resource ID and projectile mass for guns.
- A pivot at the device origin.
- Travel limits. A turret (slew rate above zero) gets yaw −π to π and pitch −80° to 80°. A fixed gun gets zero travel on both axes.

Electric shot energy is `½·m·v² / efficiency`. An electric shot also consumes 10% of the projectile mass in counterpropellant, rounded stochastically to integer units. Chemical weapons consume one cartridge instead; their cartridge energy is `½·m·v² / efficiency`, and the difference from kinetic energy becomes heat. If the ammunition resource is itself propellant, one additional kilogram of propellant must remain.

### Bundled weapons

| Part | Projectile or beam | Firing rate | Energy input |
| --- | --- | --- | --- |
| `railgun_compact` | 1 g at 20 km/s | 20 shots/s | 400 kJ per shot |
| `autocannon_compact` | 0.1 kg at 1100 m/s | 40 shots/s | Chemical cartridge; no shot electricity |
| `laser_2m` | 200 kJ optical pulses | 10 pulses/s | 500 kJ per pulse |
| `laser_4m` | 4 MJ optical pulses | 10 pulses/s | 10 MJ per pulse |

Patrol ships and defense installations carry lasers.

## The weapon setting

Firmware writes `SET_WEAPON` with a `WeaponSetting` (72 bytes):

| Field | Validation | Meaning |
| --- | --- | --- |
| `aim_direction` | Unit vector (±1e-6), world axes | Desired bore direction at the tick start |
| `aim_angular_velocity_rad_s` | Finite, magnitude at most 100 | The desired direction rotates at this rate during the tick |
| `maximum_pointing_error_rad` | In [0, π] | Largest bore error at which a shot may fire |
| `valid_until_s` | Finite, no later than one physics step after the current time (+1e-6 s) | Expired commands are ignored; shots after the deadline are refused |
| `trigger` | 0 or 1 | Whether to fire |

Settings outside these rules return `ERR_ARGUMENT` and are not staged. A valid
setting at or before the current time returns success without changing the
weapon command, so a suspended callback can safely finish after its aim expires.
The host never extends a stale lease. Because `valid_until_s` expires within one
tick, firmware must restate its intent every tick in order to keep firing.

## What happens during a tick

The hardware step stages each operational weapon's command. During collision integration, barrels track the requested direction in steps of at most 5 ms. Each mount respects its slew rate and travel limits; full rotation turrets wrap across ±π yaw.

The next launch is scheduled after the firing interval, inside the current tick and before the command expires. Launches share the ordered event queue with impacts and destruction. At the launch instant the gun checks availability, battery energy, ammunition, propellant, pointing and barrel clearance. Simultaneous guns consume the same battery in deterministic event order, so the first shot can leave insufficient energy for another.

Barrel clearance sweeps a projectile-sized ball from pivot to muzzle against the other ship parts and docked members. The muzzle must also lie outside its own part. Failed attempts consume no ammunition or shot energy.

A successful electric shot subtracts `½ m v² / efficiency` from the battery and consumes one round and its counter-exhaust propellant. A chemical shot consumes its cartridge. Both restart the firing interval. Projectile kinetic energy is `½ m v²`; the remainder enters the ship's waste heat system. The shield cooling circuit accepts waste heat while available, and the internal heat buffer stores it otherwise. See [ships.md](ships.md#heat-hull-and-shields).

Direction is sampled uniformly within the dispersion cone using the ship entity, part ID and shot count as a deterministic seed. Launch velocity is `ship velocity + ω × muzzle offset + direction × muzzle speed`. Projectile mass and propellant leave the ship. Angular momentum scales with the reduced mass, preserving ship spin and velocity under the prototype recoilless launch model.

Each launch creates a physical spherical projectile with a 2 s lifetime and hit points equal to its mass in kilograms. It passes through its owner's shield and collides with other ships through the ordinary collision pipeline. Shields and hulls use the same partially elastic contact response and split dissipated energy equally. A 1 g round has 0.001 hit points and is destroyed after receiving 100 J of impact heat; surviving low-energy or grazing rounds can deflect. Contact itself does not delete a projectile. See [collisions.md](collisions.md#shared-impact-response).

## Interlocks and readings

`device_read` returns `WeaponReading`: status, inhibit flags, ammunition units, shots fired, available battery energy, energy per shot, yaw, pitch and next firing time. Exact layouts are generated from [abi.rs](../crates/osg-ship-api/src/abi.rs).

| Flag | Value | Set when |
| --- | --- | --- |
| `WEAPON_UNAVAILABLE` | 1 | The part is not operational |
| `WEAPON_AMMO` | 4 | Less than one unit of ammunition |
| `WEAPON_COOLDOWN` | 16 | Cycle interval not yet elapsed |
| `WEAPON_BLOCKED` | 128 | The last launch attempt found an obstructed barrel |
| `WEAPON_TRAVEL` | 256 | The last attempt needed pitch outside the mount's limits |
| `WEAPON_POINTING` | 512 | The last attempt exceeded the allowed pointing error |
| `WEAPON_EXPIRED` | 1024 | The last attempt had no trigger or an expired setting |
| `WEAPON_ENERGY` | 2048 | The shared battery cannot supply a shot, or the launch energy or mass is invalid |
| `WEAPON_PROPELLANT` | 4096 | Not enough propellant for the shot |

The reading reports current resource availability and relevant conditions from the most recent launch attempt.

## Standard firmware engagement

The bundled firmware separates target marking from the firing latch (see [ship-abi.md](ship-abi.md#requests)):

- `REQUEST_MARK_TARGET { contact, maximum_flight_time_s }` replaces the marked target and stops firing. The contact must be a visible ship, and the maximum flight time must be in [0.01, 60] s.
- `REQUEST_UNMARK_TARGET` clears the mark and stops firing.
- `REQUEST_START_FIRING` enables fire against the marked target. It is rejected when no target is marked.
- `REQUEST_STOP_FIRING` disables fire while retaining the target and its aiming solutions.

The standard controller has one marked weapons target. Overview selection and navigation targets are independent. Align, Approach and Keep range never start firing; navigation orders cannot override Stop firing.

While a target is marked, for each control-enabled weapon every tick, the firmware:

- Computes the muzzle position from the device mount, the current yaw and pitch, and the spec offsets.
- Solves the earliest constant-velocity intercept for a projectile at muzzle speed. It corrects the target's relative velocity for the muzzle's rotational motion, and accepts only solutions within the maximum flight time.
- For lasers, aims directly at the target's current position within the laser's range and reports zero flight time. Its aim marker is at that position.
- Solves again for the target one physics step later, and uses the change as the aim angular velocity.
- Sets the pointing tolerance to `clamp(0.5·target radius / range − dispersion, 0, 0.005)` rad.
- Pulls the trigger only when firing is enabled and a solution exists and the tolerance is above zero. The weapon must be available and have enough battery energy; guns also require ammunition and propellant. The hardware then refuses shots that exceed the pointing tolerance.
- Publishes an aim marker (IDs 16 to 23, when the client requests markers) at the predicted intercept point in the ship frame.

The weapons instrument has a 2 s lease. Its reason text is "Target marked", "Tracking", "Firing" or "Reacquiring target". The firing latch reports the requested policy; interlocks can still inhibit individual weapons. When the target has been unseen for more than 2 s, the firmware clears the mark and firing latch with the reason "Target lost".

## In the simulator

Select a ship in Overview or the HUD, then use Mark target in Selected Item. Start firing and Stop firing operate on the authoritative marked target, even when a different row is selected. Unmark target clears that target. The panel displays the marked ship and firing policy separately from the current selection.

The hostile patrol is marked and ordered to fire as part of scenario setup. Player target marking does not itself fire or provoke an automatic retaliatory command. When a controlled ship enables firing against an uncontrolled ship, the demo retaliation policy marks the player and enables fire in return.

Presentation: barrel meshes interpolate yaw and pitch between ticks. Slugs draw as camera-facing tracer ribbons spanning one display frame, adjusted for simulation playback speed and compensated for camera motion. Impacts produce short flashes and debris sprites. Shield hits add a brief temperature flash that fades exponentially.

## Tests

```sh
cargo test -p osg-ships weapons
cargo test -p osg-server collision::weapons
cargo test -p osg-server collision::ecs
cargo test -p osg-ship-wasm weapon
cargo test -p osg-example-controller weapons
```

These cover:

- turret yaw wrapping
- recoilless launch energy and mass accounting
- battery energy, ammunition, propellant and obstruction checks
- launches and hits sharing one timeline (two railgun shots 0.05 s apart)
- a destroyed ship not firing
- shots crossing the owner's shield while hitting another ship's shield
- ECS materialization of launched slugs
- setting validation, and fault rollback of staged fire
- firmware engagement staying within budget
- intercept solutions
- repeatable pulse dispersion, laser aiming, finite beam waist and distance falloff
- distinct gun/laser catalogue records and pulse energy accounting

## Laser equipment

Each laser pulse draws `pulse_energy_j / efficiency` from the battery and deposits emitter losses as ship heat. It samples pointing error with the same deterministic cone sampler used by guns, then resolves a hitscan ray against the first intersected shield, hull, or projectile. Successful pulses advance the cadence and pulse count. Lasers require no ammunition or counterpropellant.

Beam width is `sqrt(beam_waist_m² + (distance × divergence_half_angle_rad)²)`, interpreted as the Gaussian 1/e² intensity radius. The intercepted fraction is `1 − exp(−2 × target_radius² / width²)`. The target radius is the shield radius or the hull's bounding radius: this is a centered circular silhouette approximation after the collision ray establishes a hit. It gives finite near-field intensity and inverse-area far-field falloff, with deposited energy bounded by pulse energy. Range and nearest-hit occlusion still apply. Visible beam traces and impact events accompany each pulse.

The small water-NTR patrol blueprint uses `autocannon_compact`: 0.1 kg rounds at 1100 m/s and 40 shots/s, with 35% efficiency. It can fire with an empty battery and no counterpropellant. Avionics still need power to issue aim and trigger commands.
