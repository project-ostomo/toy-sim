# Weapons

Weapons are ship parts that launch physical slugs. A slug is a small sphere with mass that flies through the same continuous collision solver as ships. Firmware controls weapons by staging a short-lived aim-and-trigger setting each tick. The hardware slews turrets, checks firing conditions, and schedules launches inside the tick. Each shot draws its energy directly from the shared ship battery.

Source:

- Definitions, specs and servo mechanics: [crates/toy-sim-ships/src/weapons.rs](../crates/toy-sim-ships/src/weapons.rs)
- Hardware commands and readings: `ShipState::step` in [runtime.rs](../crates/toy-sim-ships/src/runtime.rs)
- Launch events inside the collision timeline: [crates/toy-sim-server/src/sim/physics/collision/weapons.rs](../crates/toy-sim-server/src/sim/physics/collision/weapons.rs)
- Standard firmware engagement policy: [crates/toy-sim-example-controller/src/weapons.rs](../crates/toy-sim-example-controller/src/weapons.rs)

## Defining a weapon part

A weapon part uses `kind = "weapon"` and a `[parts.weapon]` table. Unknown keys are rejected.

| Field | Validation | Meaning |
| --- | --- | --- |
| `ammunition` | Must name a catalogue resource | One unit of that resource is one projectile. The projectile mass is the resource's unit mass. |
| `projectile_radius_m` | > 0 | Collision sphere radius |
| `muzzle_speed_m_s` | > 0 | Speed relative to the muzzle |
| `cycle_interval_s` | > 0 | Minimum time between shots |
| `efficiency` | in (0, 1] | Fraction of shot energy that becomes projectile kinetic energy |
| `dispersion_half_angle_rad` | in [0, 0.1) | Half angle of the random launch cone |
| `muzzle_offset_m` | finite, longer than the projectile radius | Muzzle position from the pivot, in barrel axes |
| `slew_rate_rad_s` | ≥ 0 | Yaw and pitch rate. Zero makes a fixed gun. |

The derived `WeaponSpec` (the ABI record returned by `device_spec`) adds fixed values:

- The ammunition resource ID.
- A pivot at the device origin.
- Travel limits. A turret (slew rate above zero) gets yaw −π to π and pitch −80° to 80°. A fixed gun gets zero travel on both axes.

Shot energy is `½·m·v² / efficiency`. Each shot also consumes 10% of the projectile mass in propellant (kg). If the ammunition resource is itself propellant, one additional kilogram of propellant must remain.

### Bundled weapons

| Part | Ammunition | Radius | Shot energy | Cycle | Efficiency | Dispersion | Muzzle offset | Slew |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `railgun_turret` | `bearing` (0.01 kg) | 5 mm | ≈ 416.7 kJ | 0.05 s | 0.3 | 0.1 mrad | (0, 0, −1) m | π rad/s |
| `coilgun_turret` | `coil_slug` (10 kg) | 67 mm | 250 MJ | 1 s | 0.5 | 0.2 mrad | (0, 0, −1.6) m | π/3 rad/s |
| `coilgun` | `coil_slug` (10 kg) | 67 mm | 250 MJ | 1 s | 0.5 | 0.2 mrad | (0, 0, −1.7) m | fixed |

All three fire at 5000 m/s. The armed starter carries one railgun turret and one coilgun turret. The test loadout provides 1000 bearings and 12 coilgun slugs when storage allows ([ships.md](ships.md#test-loadout)).

## The weapon setting

Firmware writes `SET_WEAPON` with a `WeaponSetting` (72 bytes):

| Field | Validation | Meaning |
| --- | --- | --- |
| `aim_direction` | Unit vector (±1e-6), world axes | Desired bore direction at the tick start |
| `aim_angular_velocity_rad_s` | Finite, magnitude at most 100 | The desired direction rotates at this rate during the tick |
| `maximum_pointing_error_rad` | In [0, π] | Largest bore error at which a shot may fire |
| `valid_until_s` | From the current time to one physics step later (+1e-6 s) | Shots after this time are refused |
| `trigger` | 0 or 1 | Whether to fire |

Settings outside these rules return `ERR_ARGUMENT` and are not staged. Because `valid_until_s` expires within one tick, firmware must restate its intent every tick in order to keep firing.

## What happens during a tick

The hardware step stages each operational weapon's command. During collision integration, barrels track the requested direction in steps of at most 5 ms. Each mount respects its slew rate and travel limits; full rotation turrets wrap across ±π yaw.

The next launch is scheduled after the firing interval, inside the current tick and before the command expires. Launches share the ordered event queue with impacts and destruction. At the launch instant the gun checks availability, battery energy, ammunition, propellant, pointing and barrel clearance. Simultaneous guns consume the same battery in deterministic event order, so the first shot can leave insufficient energy for another.

Barrel clearance sweeps a projectile-sized ball from pivot to muzzle against the other ship parts and docked members. The muzzle must also lie outside its own part. Failed attempts consume no ammunition or shot energy.

A successful shot subtracts `½ m v² / efficiency` from the battery, consumes one round and its counter-exhaust propellant, and restarts the firing interval. Projectile kinetic energy is `½ m v²`; the remainder enters the ship's waste heat system. The shield cooling circuit accepts waste heat while available, and the internal heat buffer stores it otherwise. See [ships.md](ships.md#heat-hull-and-shields).

Direction is sampled uniformly within the dispersion cone using the ship entity, part ID and shot count as a deterministic seed. Launch velocity is `ship velocity + ω × muzzle offset + direction × muzzle speed`. Projectile mass and propellant leave the ship. Angular momentum scales with the reduced mass, preserving ship spin and velocity under the prototype recoilless launch model.

Each launch creates a physical spherical projectile with a 2 s lifetime and hit points equal to its mass in kilograms. It passes through its owner's shield and collides with other ships through the ordinary collision pipeline.

## Interlocks and readings

`device_read` returns `WeaponReading`: status, inhibit flags, ammunition units, shots fired, available battery energy, energy per shot, yaw, pitch and next firing time. Exact layouts are generated from [abi.rs](../crates/toy-sim-ship-api/src/abi.rs).

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

The bundled firmware adds two requests (see [ship-abi.md](ship-abi.md#requests)):

- `REQUEST_ENGAGE_WEAPONS { contact, maximum_flight_time_s }`. Rejected unless a control-enabled weapon exists, the flight time is in [0.01, 60] s, and the contact is a visible ship.
- `REQUEST_HOLD_FIRE`. Clears the target.

While engaged, for each control-enabled weapon every tick, the firmware:

- Computes the muzzle position from the device mount, the current yaw and pitch, and the spec offsets.
- Solves the earliest constant-velocity intercept for a projectile at muzzle speed. It corrects the target's relative velocity for the muzzle's rotational motion, and accepts only solutions within the maximum flight time.
- Solves again for the target one physics step later, and uses the change as the aim angular velocity.
- Sets the pointing tolerance to `clamp(0.5·target radius / range − dispersion, 0, 0.005)` rad.
- Pulls the trigger when a solution exists and the tolerance is above zero. Beyond that, the weapon must be available, have ammunition and enough battery energy, and carry no `PROPELLANT` flag. The hardware then refuses shots that exceed the pointing tolerance.
- Publishes an aim marker (IDs 16 to 23, when the client requests markers) at the predicted intercept point in the ship frame.

The weapons instrument has a 2 s lease. Its reason text is "Tracking", "Firing" or "Reacquiring target". When the target has been unseen for more than 2 s, the firmware holds fire with the reason "Target lost".

## In the simulator

The Weapons window ([gui/instruments.rs](../crates/toy-sim-client/src/ui/instruments.rs)) has "Engage selected", which uses the contact selected in the Contacts window with a 2 s maximum flight time, and "Hold fire". For each weapon row it shows the mode, target, reason, rounds, battery energy versus shot energy, pointing error, flight time and active inhibit flags.

When the player engages an uncontrolled ship, that ship is ordered to engage the player in return ([vessel README](server-client.md#server-tick)).

Presentation: barrel meshes interpolate yaw and pitch between ticks. Slugs draw as camera-facing tracer ribbons over a 2 ms exposure while their published trajectory is active. Each view compensates for its own camera motion; focus changes and discontinuities reset that history. Impacts produce short flashes, with debris chips when no shield was involved.

## Tests

```sh
cargo test -p toy-sim-ships weapons
cargo test -p toy-sim collision::weapons
cargo test -p toy-sim collision::ecs
cargo test -p toy-sim-ship-wasm weapon
cargo test -p toy-sim-example-controller intercept
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
