# Roadmap

- [ ] Make current architecture solid
    - [x] Track down all numerical issues causing crashes
    - [x] Test an extremely basic multipart airplane
    - [ ] Figure out the right approach for modules and parts
    - [x] Simplify controls to direct manual thrust and torque
    - [ ] Region-based Rapier integration for collisions and such
    - [ ] A basic framework for serializing save files
- [ ] Very basic part library with graphics
    - [ ] Nuclear reactors
    - [ ] Electric fans (how would animations work?)

See [ship controls and drag model](src/vessel/README.md) for the current controls and physics approximations.

Run with `cargo run`. Native debug builds dynamically link Bevy for faster incremental linking;
Cargo sets up the library search path automatically. `cargo build --release` produces a binary
that does not require Bevy's shared library.

The render origin shifts when the camera reaches 100 km from it. In `PostUpdate`,
`PrecisionSystems` orders presentation interpolation, camera placement, rebasing,
projection, Bevy transform propagation, then `WorldReady`. Schedule gizmos and tools that read current world
transforms in `WorldReady`, using `GlobalTransform` for entities in hierarchies.
Only roots with `PreciseTransform` are projected; ordinary `Transform`-only roots
keep their Bevy coordinates across rebases.

Simulation runs at 10 Hz; rendering interpolates the last two completed poses each
frame, displaying the world one tick (100 ms) behind the virtual clock. Camera
input remains per-frame. Diagnostics shows application frames and completed gameplay
ticks separately; loading frames count as frames, but not gameplay ticks.

`PreciseTransform` is authoritative. Ships and celestial bodies opt into interpolation
through `InterpolatedTransform`; their `PresentationPose` is only for rendering and
camera tracking. Never feed that pose back into physics. Interpolation uses absolute
128-bit integer-micrometre positions and double-precision relative offsets before projection,
so origin changes do not invalidate history. Ordinary local child transforms follow
Bevy's normal hierarchy propagation.

New entities initialize both interpolation endpoints from their spawn pose. For a
teleport, call `InterpolatedTransform::teleport(&mut authoritative, destination)`;
this changes the authoritative pose and requests a history reset before rendering.
Set velocity and other simulation state separately if the teleport requires it.
The reset survives multiple fixed ticks in one frame. Camera target changes do not
reset the target's history.

`GalacticPosition` stores three signed `i128` micrometre coordinates. Relative-vector
conversion first subtracts the integer coordinates, then uses an `i64` conversion
when each displacement fits; larger separations use a cold, out-of-line `i128`
conversion. This retains exact local differences at galactic locations without
putting wide float conversions in ordinary physical interactions. Physics still
uses global positions and `f64` displacement vectors. Floating-point movement
increments are rounded to micrometres without fractional-remainder state.

Human-readable serialized positions are three decimal strings, preserving all
128 bits even in formats or consumers with narrower numeric types. Legacy arrays
of integer coordinates remain readable; binary serializers receive `i128` values.

The **Sensor debug** window controls the orbital explorer's omnidirectional sensor:
range (kilometres, default 100,000), celestial occlusion, HUD visibility, object
categories, labels, and minimum marker size. HUD squares follow interpolated
objects; displayed distances are measured from the sensor ship at the last scan.
All markers use exact perspective bounds of the object's enclosing sphere,
expanded to a square with the configured minimum size. Spheres touching or
crossing the camera's eye plane have no finite bounds and receive no marker.
Selecting a different camera target does not move the sensor. Markers behind the
camera or outside the viewport are not drawn. Dense labels can overlap; they can
be toggled off without hiding the squares. The monitoring renderer still draws
the authoritative scene; detection currently filters HUD contacts, not meshes.

`SpatialBody` opts an object into the index. Target centres are stored in eight
sparse spatial hashes from 1,000 km to 10 billion km cell widths. A radius query
uses an appropriate level and exact centre-distance filtering. Negative cells
use Euclidean division of integer coordinates. Extremely large queries traverse
occupied cells rather than an unbounded grid of empty cells.

Opaque celestial spheres are also centre-indexed in radius classes, each with
its own multiresolution hashes. Each class is queried with `sensor range + class
maximum radius`, then filtered against actual sphere extents. No global celestial
list is scanned for occlusion. Blockers are gathered once per scan and tested
against sensor-to-target segments. Tangency is transparent, surface observers
can look outward, and targets never occlude themselves. This is geometric
centre-point visibility: partial exposure of large targets, terrain, atmospheric
attenuation, signatures, sensitivity and light-travel delay are not modelled yet.

Index rebuild and sensor scans run in ordered `FixedLast` sets after integration.
`SensorContacts` holds the resulting authoritative detections. Only the original
ship initially has a `Sensor`, but the scan supports multiple independent sensors.
The egui HUD runs after transform propagation and camera projection updates.

The **Universe** window browses authored systems and any enabled synthetic
systems. The default uses Helion plus the separate Gaia background catalogue. Select a celestial name to inspect it without moving a ship. **Relocate
ship** moves only the orbital explorer into a circular orbit around that planet;
the traffic fleet stays behind. Returning to a ship in **Camera target** releases
the inspection pin. These are monitoring actions, independent of sensor visibility.

System definitions and fixed galactic anchors remain in memory. Only systems
containing ships (including their next-tick approach) or an explicit inspection
have ticking celestial entities. Influence radii include the full orbital extent
plus `sqrt(G * total_mass / gravity_cutoff)`. Gravity has a hard per-ship cutoff;
interstellar ships coast or thrust without celestial gravity. Activation and
visual thresholds have no hysteresis or grace periods. Inactive planets are not
sensor contacts, although their stars remain available to the background renderer.

A static median-split catalogue tree bounds stellar brightness, stellar radius,
and system influence. It serves whole-sky magnitude queries, resolved-star
selection, nearest unresolved distance, lighting selection, and ship proximity.
Distant stars use a progressively CPU-baked HDR skybox: 512, 1024, 2048, then
4096 pixels per cube face. One background worker splats the cached catalogue into
linear-radiance textures, including flux-conserving mip levels. Camera rotation
reuses the sky; translation refreshes it according to angular error. A completed
level replaces the previous sky only after GPU preparation, and lower-resolution
results do not replace a still-accurate sharper sky. Resolved stars remain spheres.
The Universe GUI reports displayed/baking resolution, star count and bake time,
and controls magnitude and display gain. Exposure and bloom remain live. The final
4096 texture with mip levels occupies approximately 1 GiB; large texture uploads
can still cause stalls. Atmospheric extinction of catalogue stars is not implemented.

Optional `spectral_class = "G"` on a star selects an approximate O/B/A/F/G/K/M colour
preset, shared by the skybox and resolved stars. Without it, `surface_color` supplies
the colour. Luminance normalization preserves the star's configured luminosity.
Synthetic stars now have illustrative mass-based classes. See
[the in-memory Gaia catalogue guide](docs/gaia-catalogue.md) for the flat-file converter,
luminosity-bucket KD-tree queries, configuration and measurement limitations. A real 1,000,000-source
Gaia DR3 bright-star catalogue is enabled; the full release is not bundled. Imported sources are
background lights, not automatically generated simulation systems.

The camera uses a neutral fixed exposure multiplier of 1; automatic metering
adjusts exposure across a -24 to +24 log-luminance range. Ship-axis gizmos are
disabled. Stellar surface display emission is capped below the HDR framebuffer
limit; catalogue luminosities and gravitational/lighting calculations are unchanged.

The camera uses Bevy’s global tone mapper and natural, non-anamorphic bloom.

## Workspace

The root package is the Bevy app. `crates/toy-sim-space` provides the shared
`GalacticPosition` in i128 micrometres; `crates/toy-sim-stars` provides portable
star files and immutable in-memory luminosity-bucket indexes without Bevy.
Use `cargo test --workspace` for all tests. Run the standalone catalogue benchmark
with `cargo run -p toy-sim-stars --release --example query -- assets/catalogues/gaia-dr3-earth-million.stars`.

Authored system coordinates now use `position_um` (decimal integer strings).
The previous `position_mm` field must be renamed and its values multiplied by
1,000 when migrating external configurations. Render transforms remain in metres.
