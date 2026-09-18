# Ships

A ship is a set of box-shaped parts placed on a grid, plus integrated standard avionics and a flight computer program. This guide covers the data model and the native hardware simulation in [toy-sim-ships](../crates/toy-sim-ships). It also describes how the simulator runs ships and what the standard firmware does.

Related guides: [ship-editor.md](ship-editor.md) for building designs, [ship-abi.md](ship-abi.md) for firmware, [weapons.md](weapons.md), [collisions.md](collisions.md), and the [server architecture](server-client.md).

## The catalogue

The catalogue is [crates/toy-sim-ships/data/catalogue.toml](../crates/toy-sim-ships/data/catalogue.toml). `Catalogue::builtin()` parses and validates it at startup, and the binaries embed it at compile time. It has three top-level keys:

- `revision` (currently `3`): blueprints must name the same revision.
- `resources`: slot 0 must be `propellant` and slot 1 must be `fuel`. Resource IDs must be unique and non-empty. `mass_kg` and `volume_m3` must be positive.
- `parts`: part definitions.

### Resources

| ID | Title | Unit mass | Unit volume |
| --- | --- | --- | --- |
| `propellant` | Propellant (kg) | 1 kg | 0.001 m³ |
| `fuel` | Generator fuel (kg) | 1 kg | 0.001 m³ |
| `bearing` | Ball bearings | 0.01 kg | 0.000002 m³ |
| `coil_slug` | Heavy coilgun rounds | 10 kg | 0.0015 m³ |

### Part fields

Every part has `id`, `title`, `dimensions` (grid cells, each 1 to 1000), `mass_kg` (positive), `hull` (positive hit points), `color` (RGB in [0, 1]), an optional `model` (see [assets/models/parts/README.md](../assets/models/parts/README.md)), and `kind` with kind-specific fields. All rates and capacities must be finite and positive.

| `kind` | Fields | Device | Tick priority |
| --- | --- | --- | --- |
| `structure` | none | none | passive |
| `storage` | `capacity_m3` | Storage | passive |
| `battery` | `capacity_j` | Battery | passive |
| `generator` | `power_w`, `fuel_kg_s`, `efficiency` | Generator | 0 |
| `engine` | `thrust_n`, `propellant_kg_s`, `power_w`, optional `[parts.plume]` | Engine | 3 |
| `rcs` | `thrust_n`, `propellant_kg_s`, `power_w` (each per nozzle pair) | RCS | 3 |
| `torquer` | `torque_nm`, `power_w` | Torquer | 3 |
| `shield` | `deployed_mass_kg`, `radiator_area_m2`, `feed_rate_kg_s`, `power_w` | Shield | 4 |
| `coolant_tank` | `capacity_kg` | none | passive |
| `heat_sink` | `capacity_j` | none | passive |
| `weapon` | `[parts.weapon]` table | Weapon | 5 |

### Bundled parts

| ID | Title | Size (cells) | Mass | Hull | Key values |
| --- | --- | --- | --- | --- | --- |
| `structure` | Hull block | 10×10×10 | 100 kg | 1000 | |
| `storage` | Cargo storage | 10×10×10 | 80 kg | 200 | 0.8 m³ |
| `battery` | Battery | 10×10×10 | 200 kg | 200 | 300 MJ |
| `engine` | Electric propellant engine | 10×10×20 | 500 kg | 300 | 200 kN, 1 kg/s, 200 MW |
| `torquer` | Powered torque actuator | 10×10×10 | 100 kg | 200 | 20 kN·m per axis, 1 MW |
| `generator` | Fuel generator | 10×10×10 | 500 kg | 400 | 300 MW electrical, 40% efficiency, 0.0001 kg/s |
| `shield` | Shield generator | 10×10×10 | 200 kg | 200 | 5 kg deployed, 66 m² radiator, 50 kg/s feed, 1 MW |
| `coolant_tank` | Shield coolant tank | 10×10×10 | 50 kg dry | 150 | 100 kg reserve |
| `heat_sink` | Heat sink | 10×10×10 | 150 kg | 200 | 300 MJ additional heat storage |
| `railgun_turret` | Light railgun turret | 10×10×10 | 150 kg | 150 | see [weapons.md](weapons.md) |
| `coilgun_turret` | Heavy coilgun turret | 10×10×20 | 750 kg | 500 | see [weapons.md](weapons.md) |
| `coilgun` | Fixed heavy coilgun | 10×10×30 | 600 kg | 500 | see [weapons.md](weapons.md) |
| `rcs` | Six-direction RCS block | 5×5×5 | 40 kg | 100 | 2 kN, 0.5 kg/s, 20 kW per nozzle pair |

### Engine plumes

`[parts.plume]` controls how the vacuum exhaust looks. It never changes engine performance. Unknown keys are rejected.

| Field | Meaning and validation |
| --- | --- |
| `origin_m` | Nozzle opening relative to the part centre in metres. Exhaust points along part-local +Z. |
| `length_m`, `nozzle_radius_m` | Positive. |
| `expansion_half_angle_rad` | In [0, π/2). Radius at distance z is `nozzle_radius + z·tan(angle)`. |
| `color_linear_rgb` | Linear RGB, each in [0, 1]. |
| `intensity` | Non-negative HDR multiplier, calibrated at Bevy's default exposure. |
| `axial_falloff`, `radial_falloff` | Positive exponents. |
| `noise_strength` | In [0, 1]. Zero disables variation. |
| `noise_scale_m` | Positive. |
| `noise_speed_m_s` | Non-negative apparent texture speed. |

RCS blocks get fixed built-in plume visuals on their six faces, each driven by the delivered thrust on its axis.

## Blueprints

A `ShipBlueprint` ([design.rs](../crates/toy-sim-ships/src/design.rs)) contains:

| Field | Type | Default |
| --- | --- | --- |
| `format_version` | u32 | Must equal `SHIP_FORMAT_VERSION` = 2. A missing value reads as 0 and is rejected. |
| `name` | string | "Untitled ship" in a new design; at most 256 bytes |
| `catalogue_revision` | u32 | 3 |
| `parts` | list of `PlacedPart` | |
| `firmware` | `Standard` or `Custom(bytes)` | `Standard` |
| `avionics` | `Avionics` | sensor enabled, orientation 0, no exclusions |

`PlacedPart` fields:

- `id`: u64, unique within the ship.
- `name`: free text, at most 256 bytes.
- `alias`: optional device alias for firmware. At most 64 bytes of ASCII letters, digits and `_`, unique within the ship. `computer`, `accelerometer` and `radar` are reserved. Structural parts cannot have an alias.
- `groups`: up to 16 unique labels using the same character rules. Structural parts cannot have groups.
- `prototype`: catalogue part ID.
- `position`: `[i32; 3]` grid coordinates of the minimum corner.
- `orientation`: 0 to 23, one of the 24 proper cube rotations. 0 is the identity.

`Avionics` fields (unknown keys rejected):

- `sensor_enabled`: the sensor's default enable setting.
- `control_orientation`: 0 to 23. Rotates the control axes into assembly axes. At 0, forward is −Z and up is +Y.
- `excluded_actuators`: part IDs of engines, torquers, RCS blocks or weapons that automatic control should not use.

### Blueprint files (`.ship`)

`.ship` files are the blueprint serialized as a CBOR map with `ciborium`.

- `firmware` is written as a tagged map: `{kind: "standard"}` or `{kind: "custom", program: <bytes>}`.
- Unknown top-level fields are ignored on load and dropped on the next save.
- A file must be 1 byte to 16 MiB with no trailing data. It may hold at most 4096 parts, and the controller program may be at most 1 MiB.
- `save` writes `<name>.ship.tmp` and then renames it over the target.

A design saved with an older format version fails with "incompatible ship format … (rebuild the design with standard avionics)".

## Compiling a design

`ShipBlueprint::compile(&Catalogue)` checks the blueprint and produces a `CompiledShipDesign`. It fails if:

- the catalogue is invalid, the format or catalogue revision does not match, or an orientation is out of range
- there are no parts or more than 4096, the controller is empty or larger than 1 MiB, or there are more than 4096 actuator exclusions
- a part ID is duplicated, a prototype is unknown, or labels are invalid
- two part boxes overlap with positive volume
- the parts are not all connected by faces (two parts connect when they share a face patch with positive area)
- an exclusion names a missing part or a part that is not an actuator
- the logical device count exceeds 4096

A part occupies `[position, position + |R|·dimensions)` cells, where R is its orientation matrix. One grid cell is 0.1 m (`GRID`).

The compiled design holds:

- **Mass properties.** Dry mass is the sum of part masses plus `AVIONICS_MASS_KG` (71 kg). The centre of mass is weighted over parts. The inertia tensor adds each part box's own inertia to its parallel-axis term. The total is then scaled by `(parts + 71 kg) / parts`, because avionics mass is treated as distributed.
- **Extents.** `radius` is the distance from the centre of mass to the furthest part corner. `semi_axes` (bounds × √3) sizes the drag ellipsoid.
- **Totals.** Hull hit points, internal heat storage, deployed coolant mass, reserve tank capacity, radiator area, replenishment rate, storage volume, battery capacity, and the exposed area (box surfaces minus shared faces).
- **Tick plan.** `active_parts` lists generator, engine, RCS, torquer, shield and weapon parts sorted by (priority, part ID). Passive parts never tick.
- **Weapon tables.** `weapon_parts`, `part_weapons` and derived `weapon_specs`.
- **Device directory.** `device_catalogue` holds one entry per non-structural part in part order, followed by three avionics devices: `computer` (its rotation is the control orientation), `accelerometer`, and `radar` (a sensor with 100,000 km range). Each descriptor records a dense handle, the part ID (0 for avionics), a control-enabled flag (false for excluded actuators), alias, groups, kind, position relative to the dry centre of mass, and rotation.

## Hardware simulation

`ShipState` ([runtime.rs](../crates/toy-sim-ships/src/runtime.rs)) is the mutable hardware state. Inventory quantities are fractional units per resource, and energy is stored in joules.

### Settings

Commands persist until replaced. `apply_commands` validates the whole command list before applying any of it: every handle must exist, and every value must be finite and match the device kind. `reset_commands` restores defaults:

| Device | Setting | Default |
| --- | --- | --- |
| Engine | `Throttle(f)` in [0, 1] | 0 |
| Torquer | `TorqueNm([x, y, z])` in device axes | 0 |
| RCS | `RcsThrust([x, y, z])` newtons in device axes | 0 |
| Generator | `GeneratorDemand(f)` in [0, 1] | 1 |
| Shield | `ShieldEnabled(bool)` | true |
| Sensor | `SensorEnabled(bool)` | `avionics.sensor_enabled` |
| Weapon | `Weapon(WeaponSetting)` | none |

Hardware resets its commands when the hull reaches zero, when avionics lose power or fail, and when the computer is not running.

### One hardware tick (`step`)

`dt` must be in (0, 1] seconds. Each tick:

1. Clears per-tick weapon and shield flags. If the hull is zero, zeroes all output and returns mass properties only.
2. Walks `active_parts` in priority order. Avionics are stepped just before the first non-generator part. They draw 101 W; when the sensor is enabled and 1 kW is available, the sensor range becomes 100,000 km for the tick.
3. Consumes inputs through `spend`, which allocates all of an operation's inputs together. Output scales with the fraction that was actually available.
   - **Generator:** produces up to `power_w·dt·demand`, limited by free battery capacity, and burns fuel in proportion. Waste heat is `generated_energy · (1/efficiency − 1)`.
   - **Engine:** throttle `f` consumes `propellant_kg_s·dt·f` and `power_w·dt·f`. Force `rotation·(−Z)·thrust·f·fraction` acts at the part centre, so an off-centre engine adds torque.
   - **RCS:** each axis demand is clamped to ±`thrust_n`. Propellant and power scale with the sum of the absolute axis fractions. Force acts along device axes at the part centre.
   - **Torquer:** per-axis demand is clamped to ±`torque_nm`. Power scales with the largest axis fraction.
   - **Shield:** while enabled, draws `power_w` continuously. It counts as powered only when the full demand is met.
   - **Weapon:** see [weapons.md](weapons.md).
4. Returns force and torque in ship axes plus current mass and inertia. Inertia is the dry tensor scaled by `current mass / dry mass`.

The computer is running when the hull is above zero and avionics are operational and powered.

### Test loadout

`test_loadout` fills the battery. It adds generator fuel of `min(10·capacity_m3, 10)` kg. For each weapon, it adds 1000 rounds if the projectile is lighter than 1 kg (otherwise 12 rounds), limited by free volume. It then fills the remaining volume with propellant. The simulator applies this to every spawned ship.

### Readings

`snapshot` produces one `DeviceStatus` per device, with operational and powered flags and a kind-specific reading:

- engine: delivered thrust
- torquer: delivered torque magnitude
- generator: power
- RCS: delivered thrust per axis
- shield: state, coolant temperature, reserve mass and capacity, strength, ablation rate, radiated power and drawn power
- sensor: current range
- weapon: see [weapons.md](weapons.md)
- accelerometer: filled in by the simulator at the mount

## Heat, hull and shields

[thermal.rs](../crates/toy-sim-ships/src/thermal.rs) stores internal heat in joules and tracks a separate hot coolant screen. The internal quantity represents a cooling circuit and heat storage budget; it does not assign a temperature to the whole hull.

The internal budget is `dry_mass · 250,000 J/kg` plus the capacities of installed `heat_sink` parts. Stored heat may exceed that budget. Damage then grows as:

```
HP loss per second = maximum_HP · 0.1 · max(0, stored_heat / budget − 1)²
```

Hull impacts also cause immediate structural damage at 1 HP per 100 kJ and add their energy to the internal buffer. Backup cooling removes up to `2000 W · exposed_area_m²` from that buffer. The model treats this as a fixed capacity for auxiliary radiators.

### Shield appearance

The shared renderer draws shields with an analytic spherical shell shader. The edge is antialiased and independent of mesh tessellation. Shell path length determines visible transmission and temperature-dependent emission, giving a brighter rim and a clearer center. Opaque scene depth clips the shell behind the hull. The default shell thickness is 2% of radius, with a visible optical depth of 0.000005 through both central walls at full strength. These visual parameters are independent of the simulation's effective radiator emissivity.

The `thermal_shield` example in `toy-sim-ship-view` provides a standalone visual check. `THERMAL_TEMPERATURE_K`, `THERMAL_RADIUS`, `THERMAL_CAMERA_DISTANCE`, and `THERMAL_STRENGTH` control the scene. It defaults to 3500 K and an exterior camera; a camera distance below the radius tests an interior view.

### Shield coolant

A shield generator specifies its deployed coolant mass, effective emitting area, replenishment throughput and electrical power. Separate `coolant_tank` parts add reserve mass. Tanks and the deployed circuit start full; their coolant contributes to ship mass, and material lost through evaporation or interception leaves the ship.

For deployed mass `m` and stored shield energy `U`, coolant temperature is:

```
T = 300 K + U / (m · 500 J/(kg·K))
```

The field radiates with emissivity 0.8 toward a 3 K background:

```
P = 0.8 · σ · A · (T⁴ − 3⁴)
σ = 5.670374419e−8 W/(m²·K⁴)
```

Effective area is a part parameter, independent of the sphere used for shield collisions. The deployed fraction scales protection and area. The reserve replaces evaporated material and supplies rapid coolant flushing during impacts, keeping the screen at full strength while material remains. Continuous replenishment has the generator’s specified pump throughput. Once the tank empties, the small deployed inventory rapidly loses protection under continued heating or impacts.

The evaporation approximation uses:

```
flux = 0.001 · sqrt(4500/T) · exp(60000 · (1/4500 − 1/T)) kg/(m²·s)
```

Escaping coolant takes its sensible heat plus 20 MJ/kg of vaporization energy. Loss is limited by available material and heat. Fresh coolant arrives at 300 K and dilutes the remaining heat. Impacts can immediately vaporize material once stored energy exceeds the sensible heat at 6000 K. Fresh reserve coolant replaces it first, consuming the energy needed to warm and vaporize that coolant. When reserve runs out, vaporization consumes the deployed screen. Remaining interception energy is bounded by the heat needed to warm and vaporize the available reserve and deployed mass; powerful projectiles can exhaust both and continue toward the hull.

While enabled, powered and clear, the shield circuit accepts internal heat at up to 400 MW per kilogram of deployed coolant. This represents heat delivery from the abstract hot circuit. The circuit uses the shield's electrical power allocation. Every installed shield generator must be enabled, operational and fully powered for the combined field to operate. Disabled or blocked shields leave heat in onboard storage and stop radiating through the field.

Generator inefficiency and shield electrical consumption become waste heat, distributed over the hardware tick. Weapon waste heat and impacts enter as instantaneous events. Thermal integration uses steps of at most 10 ms and an implicit radiation solve. A stock 300 MW electrical generator at 40% efficiency produces 450 MW of waste heat at full output. Its 66 m² radiator settles near 3500 K under that sustained load, with slow reserve consumption.

The six states are absent, off, active, depleted, unpowered and blocked. Activation checks the shield sphere for clearance at each tick boundary; depleted screens can replenish continuously, and activation resumes when coolant and clearance permit. The collision sphere has radius `r + max(0.1·r, 0.5 m)` around hull radius `r`. See [collisions.md](collisions.md).

## Starter designs

- `starter(controller)`: seven main parts stacked along +Z at 1 m spacing, plus a coolant tank and command module ahead of the hull block. In order: structure, storage (`storage`), battery (`battery`), generator (`generator`), torquer (`attitude_control`), shield (`shield`), engine (`main_engine`). The unarmed starter used by `toy-ship-editor --example` and the editor's Starter button.
- `armed_starter()`: the starter named "Armed explorer" with the standard firmware, plus `railgun_turret_8`, `coilgun_turret_9` (group `weapons`) and four RCS blocks `rcs_10` to `rcs_13` (group `rcs`). The simulator uses it for traffic and as the default player ship. [assets/ships/starter.ship](../assets/ships/starter.ship) contains this design.

## Ships in the simulator

The simulator spawns ships from the fixed startup scenario (see the [README](../README.md#what-happens-at-startup)). Once per 10 Hz tick, each ship's computer receives an observation, its commands are applied, and typed hardware ECS systems apply resource allocation and actuation before gravity and integration. The full sequence is in [server-client.md](server-client.md).

GUI windows for the controlled ship:

- **Ship:** altitude, airspeed, density, pressure and manual control bars.
- **Hardware diagnostics:** hull, internal heat budget, shield temperature, coolant reserve and strength, test buttons (Hull +25 MJ, Shield +250 MJ, Reset encounter), faults, boot progress, step timings, per-part status and rejected request messages.
- **Flight computer:** boot state, Hold attitude, Manual / abort, the attitude instrument or HUD, and Reset encounter.
- **Contacts**, **Weapons** and **Navigation:** fixed instruments driven by firmware publications.
- **Ship systems:** mass, hull, shield state, energy and resources.
- **Inventory:** raw resource quantities.

The two "Reset encounter" buttons behave differently. In Hardware diagnostics, it resets the controlled ship's hardware state and reboots its computer. In Flight computer (and in the "Ship destroyed" window), it despawns all ships, projectiles and explosions and respawns the scenario.

## The standard firmware

`Firmware::Standard` runs [data/example-controller.wasm](../crates/toy-sim-ships/data/example-controller.wasm), built from [toy-sim-example-controller](../crates/toy-sim-example-controller). Each callback, the firmware ([firmware.rs](../crates/toy-sim-example-controller/src/firmware.rs)):

1. Reads the tick context. It then discovers resources and devices, up to 16 records per callback, until discovery completes. Requests wait in the host until then.
2. Reads every device, the flight state and propellant mass.
3. Scans up to 256 contacts with the first available sensor.
4. Runs the travel planner through server world services. It pages through public beacons, searches routes across multiple paired gates, checks slip eligibility and guides the ship through each accepted leg. Planning work is bounded across callbacks ([server-client.md](server-client.md#docking-and-travel)).
5. Replies to each request (below).
6. Runs attitude control and allocation, then weapons control.
7. Publishes the attitude, navigation, contacts and weapons instruments with 2 s leases. Also publishes a target marker, plus forecast paths when the client shows interest.
8. Sets the callback interval to 0, so it runs every tick.

The firmware also exports `ship_display`, which draws a "Ship status" text screen when a client subscribes ([mfds.md](mfds.md#over-the-network)). Remote and debug clients use the same display path.

Requests and their effect:

| Request | Effect | Rejected when |
| --- | --- | --- |
| Manual (throttle, steering) | Sets manual throttle (clamped to [0, 1]) and steering (clamped to ±1). A changed throttle or non-zero steering cancels active or paused guidance. | Non-finite values |
| Hold attitude | Holds the current orientation, cancelling guidance. | |
| Stop guidance | Cancels guidance and releases attitude hold. | |
| Aim direction | Points the thrust axis along a direction. | Zero direction |
| Aim contact | Points at a sensor contact. | Contact not visible |
| Select target | Chooses a navigation target. | Guidance active, or target not a visible ship |
| Engage navigation | Starts pursuit ([rendezvous.md](rendezvous.md)). | Missing hardware or IMU sample, invalid limit, no visible target |
| Engage weapons | Starts automatic weapons control ([weapons.md](weapons.md)). | No control-enabled weapon, invalid flight time, target not visible |
| Hold fire | Stops weapons control. | |
| Anything else | | Always ("Unsupported request") |

Discovery binds the first control-enabled sensor, accelerometer and computer. Thrust authority, torque authority and propellant flow are computed from control-enabled engines, RCS blocks and torquers along the computer's forward axis (−Z rotated by the control orientation).

Manual steering commands torque equal to `steering × per-axis torque capacity` in control axes. With a hold or aim reference, a rate-limited attitude controller is used instead (maximum 0.5 rad/s, gyroscopic compensation). The allocator ([allocation.rs](../crates/toy-sim-example-controller/src/allocation.rs)) solves a box-constrained least-squares problem over all control-enabled engines (throttle 0 to limit), RCS axes (−1 to 1) and torquer axes (−1 to 1) for the requested force and torque. Torque error is weighted eight times more heavily, and torque actuators are solved first. It runs up to 24 warm-started coordinate-descent sweeps, stopping early when instruction budget runs low. Its residual is published as the attitude instrument's `control_error`. The Flight computer window warns when that value exceeds 0.05.

## Tests

```sh
cargo test -p toy-sim-ships
cargo test -p toy-sim-example-controller
```

[tests/ships.rs](../crates/toy-sim-ships/tests/ships.rs) covers avionics mass and power, format version rejection, the 24 rotations, CBOR round trips, overlap and connectivity checks, inventory capacity, fractional propellant, lever-arm torque, power loss, the stable tick plan, device metadata, atomic commands, alias uniqueness, sensor power and RCS behaviour.

## Configurable resource tanks

A catalogue part can declare `tank_volume_m3` (zero by default). An installed part's `tanks` array allocates this space through entries containing `resource` (catalogue ID), `volume_m3`, and `initial_fill` (0–1). An empty tank still reserves its full allocated volume. Compilation checks resource IDs, finite positive volumes, bounded fills, at most 32 tanks per part, and the sum against the part's capacity.

Resource definitions specify mass and occupied volume per inventory unit. Density is `mass_kg / volume_m3`; a tank starts with `allocated_volume_m3 * storage.usable_fraction * initial_fill / resource.volume_m3` units. Loaded contents contribute to ship mass and decrease as engines, generators, and weapons consume resources. Storage coefficients add containment mass and reduce usable volume. Stored hydrogen remains available without refrigeration or passive loss.

At runtime, tanks for a resource share one supply. Inventory tracks aggregate quantities and dedicated volume per resource; it does not simulate individual valves or draining order. Ordinary storage provides shared cargo volume in addition to those dedicated capacities. Resources cannot occupy a tank allocated to another resource. Insertion and transfer validate capacity before mutation. The debug loadout fills ordinary cargo independently and preserves the configured starting tank quantities. Client inventory telemetry includes dedicated capacity for each resource plus shared cargo capacity; shared capacity is not independently available to every resource at once.

The existing physics approximation scales dry inertia with total loaded mass. Loaded tank contents do not yet shift the centre of mass individually. Containment fittings contribute to compiled dry mass, centre of mass, and inertia at their installed part positions.

## Fission propulsion and equipment

The catalogue uses fission micropulse drives, nuclear thermal engines, and electric propulsion. See [the equipment catalogue](parts-catalogue.md) for their fuel cycles, reactor and cooling behaviour, utility hardware, and example ships.

`micropulse_engine` consumes complete manufactured `micropulse_charge` units. It requires no separate propellant. Thrust divided by exhaust velocity gives charge flow, with exhaust velocity equal to `9.80665 * specific_impulse_s`. The released energy is apportioned between directed exhaust, recoverable electricity, absorbed heat, and outgoing radiation. Validation prevents these allocations from exceeding the available energy. Charge shortages reduce all outputs together. Electrical recovery fills available battery capacity; unused recovery remains in outgoing energy.

The four engine widths are 1, 2, 4, and 8 metres. Their authored specific impulses are 2,500, 3,500, 4,500, and 5,000 seconds. They share a scaled model, but performance is explicitly authored rather than inferred from mesh scale. The charge energy and manufacturing coefficients are provisional game balance values for fictional equipment.

The script interface exposes micropulse, thermal, and electric propulsion as engine actuators with explicit propellant resource identifiers. Thermal engines additionally consume reactor fuel and retain spent fuel. Computers require installed, operational command hardware and electricity; batteries provide startup power.
