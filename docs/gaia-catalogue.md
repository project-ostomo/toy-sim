# In-memory star catalogue

The Cargo workspace contains the Bevy app at its root, `crates/osg-space`
(shared i128 micrometre coordinates), and `crates/osg-stars` (portable binary
I/O and immutable in-memory queries). Neither library depends on Bevy. There is
no SQLite runtime, database, persisted tree, or connection mutex.

The bundled catalogue contains 1,000,000 real Gaia DR3 sources selected for bright
views near Earth. Its binary file is 84,000,040 bytes. Default limiting magnitude
is 6; the app caps the returned brightest matches at 150,000. The crate embeds the
binary with `include_bytes!`, so the executable needs no catalogue file at runtime.
`StarCatalogue::embedded()` decodes those bytes and builds the in-memory index;
`load(path)` and `from_bytes(bytes)` remain available for tools and other inputs.

## Conversion and configuration

```sh
python3 tools/import_gaia.py crates/osg-stars/data/gaia-dr3-earth-million.stars crates/osg-stars/data/gaia-dr3-earth-million.csv
cargo run -p osg-star-query --release
cargo test --workspace
```

The converter reads CSV or CSV.gz, validates measurements, skips unusable or
nonstellar rows, deduplicates source IDs (first row wins), and writes to a temporary
file before replacing the output. Re-running regenerates the same binary; no
resumability bookkeeping is needed. A companion `.stars.json` records provenance,
calibration, origin and conversion counts. The raw CSV is ignored by Git and can
be downloaded again with `tools/download_gaia_earth.py`; the ADQL and
[source notes](../crates/osg-stars/data/README.md) live alongside the embedded catalogue. `--limit` limits input rows; `--origin-um X Y Z` supplies an integer
micrometre offset; `--min-parallax-snr` defaults to 10.

The universe manifest contains only authored system paths. Rendering defaults live
in `crates/osg-client/src/ui/scene/sky.rs` (magnitude 6, brightness 1) and `crates/osg-client/src/ui/scene/sky.rs` (150,000-star cap).
The Universe GUI still adjusts magnitude and brightness live. Replacing the bundled
catalogue requires rebuilding the executable. The app decodes and indexes it on
one background task, then shares an immutable `Arc<StarCatalogue>`. Queries run
on background tasks without a database or mutex. The existing 0.05 magnitude
headroom, nearest-distance movement budget and keep-old-results refresh policy
remain. Query diagnostics report candidate stars, bucket count and resident stars.

## Binary format, version 2

All numbers use explicit little-endian encoding, without Rust struct padding.
The header is 40 bytes:

| Offset | Encoding | Meaning |
|---|---|---|
| 0 | 8 bytes | `OSGSTAR\0` magic |
| 8 | u32 | Version: 2 |
| 12 | u32 | Record size: 84 |
| 16 | u64 | Record count |
| 24 | u32 | Coordinate unit: 1 = signed integer micrometres |
| 28 | u32 | Frame: 1 = ICRS Cartesian, fixed J2016.0 |
| 32 | u64 | Identity namespace: 1 = Gaia DR3 |

Each 84-byte record is: source ID (u64), X/Y/Z (three i128 micrometre coordinates),
intrinsic luminosity (f64 lumens), linear RGB multipliers (three f32 values), and
generation temperature (f64 kelvin).
The loader rejects unknown versions, units/frames, incorrect lengths, duplicate
identities, non-finite/nonpositive luminosities or temperatures, and invalid colours. Coordinates
are restricted to [-2^126, 2^126) micrometres to leave subtraction headroom.

In memory `StarId { namespace, value }` separates persistent identity from storage
index and display name. `StarCatalogue::from_stars` accepts records from other
sources, including future procedural regions; mixed namespaces can coexist in an
index. Each binary file stores one namespace.

The importer prefers a supplied positive `teff_gspphot`, followed by a spectral
class estimate, then a coarse BP−RP interpolation. It uses 5772 K when none is
available. The bundled CSV supplies BP−RP and has no temperature or spectral-class
column, so its temperatures are approximate inputs for procedural generation.
Enriched astronomical records override those temperatures in the shared universe.

`osg-universe` reconciles the catalogue with 2,990 enriched nearby primary stars,
48 companions, and ten authored systems. Exact numeric source matches alias 1,216
primaries and 24 companions to their Gaia DR3 observations. It adds 1,774 missing
primaries under the EDR3 namespace and includes 24 missing companions inside their
owning systems. Matched companions cease to be independent system roots. This
produces 1,001,760 system roots, including the authored definitions. No proximity
merge is performed.

The shared universe indexes compact system summaries and generates planetary
definitions on demand. Both client and server use the same definitions and stable
system/body keys. A 128-entry definition cache limits retained idle definitions;
active consumers can retain their own references. Stellar inputs and conservative
generator bounds suffice to build the spatial index without generating planets.
Runtime inhabitation is derived from actual broadcasting installations.

## Luminosity-bucket KD-trees

Stars are divided by floor(log2(intrinsic luminosity)), giving factor-of-two
luminosity bands. Each band owns a `kdtree` 0.8.1 tree with leaf capacity 32.
Its largest actual luminosity sets the visibility search radius:

```
radius = sqrt(bucket_max_luminosity / brightness_threshold)
```

KD coordinates are f64 metres relative to a catalogue anchor. Queries receive
conservative numerical padding, then candidates are filtered using i128 position
subtraction before conversion to f64. The final test is actual luminosity/distance².
This preserves nearby differences and boundary matches despite approximate tree
coordinates. The maximum selection uses a bounded heap and returns compact indices
into the catalogue, brightest first; candidate/match counts are reported separately.

The crate also supports radius queries, nearest-star lookup and persistent-ID
lookup. Nearest lookup refines the approximate KD result through a padded radius
query. A zero-distance star is omitted from point-source brightness queries, but
is returned by nearest/radius queries. A zero brightness threshold selects all
non-coincident stars. Queries compare against brute force in the crate's tests,
including translated galactic origins, exclusions, caps and rounded KD coordinates.

A release benchmark on the development Ryzen 9 5900XT loaded/indexed one million
stars into 28 buckets in about 0.74 seconds. At magnitude 6 near Earth, queries
including nearest lookup returned 6,373 matches from 10,281 candidates in about
1–2 ms. At magnitude 9 they returned 173,830 matches in about 58–60 ms. These are
measurements, not strict complexity guarantees. The contiguous star records occupy
about 91.6 MiB in Rust; ID lookup and KD-trees consume additional memory. The
standalone loader/query process peaked at approximately 214 MiB resident memory.

## Sky rendering

The renderer uses Bevy's built-in skybox with CPU-baked RGBA32F cubemaps, always
2048 pixels per face (`RESOLUTION` in `crates/osg-client/src/ui/scene/sky/bake.rs`). Radiance is stored
without the former half-float brightness clamp; the renderer requires the GPU's
`FLOAT32_FILTERABLE` feature for linear cubemap filtering. There are no coarse
passes or progressive refinements. A snapshot of the selected stars and camera
position is prepared on the main thread, then one task on Bevy's
`AsyncComputeTaskPool` builds the texture. Each frame checks the task without
waiting for it. Invalidation signals cancellation, discards stale results and
starts a new bake at the latest position once the cancelled worker has stopped.
Cancellation is checked before allocation, between mip levels and every 256
stars. Rotation does not invalidate the cache.

Each star's angular radius is `asin(min(radius / distance, 1))`. Authored stars
use their configured physical radii. The compact Gaia records lack radii, so the
renderer uses `696000 km * sqrt(luminosity / solar_luminosity)`: an explicitly
approximate assumption of solar luminous surface brightness, a rendering proxy
rather than a measured radius. No binary format change is required.

The cubemap contains point sources only. Stars whose angular radius reaches
`HANDOVER` — half a texel at the finest mip level, `1 / RESOLUTION` radians, in
`bake.rs` — are diverted out of the bake entirely and drawn as emissive sphere
meshes at their galactic coordinates
(`crates/osg-client/src/ui/scene/sky/geometry.rs`). Disk rasterization is
gone: every baked star is below half a texel at the finest mip and therefore
below half a texel at every coarser one, so only bilinear point footprints are
splatted, normalized by solid angle, with edge-crossing taps reprojected onto
adjacent faces. Each mip is still baked analytically at its own level with the
same flux, so stars retain their light when minified. Limb darkening and stellar
surface detail are not modeled.

The meshes are one shared unit sphere scaled by the star radius. Emissive
radiance is `luminosity / (4 pi^2 r^2)` times the sky brightness setting, chosen
so the sphere's apparent irradiance `pi * radiance * (r/d)^2` equals the baked
point flux `luminosity / (4 pi d^2)` exactly at handover. Spheres get true
per-frame parallax — authored bodies track their live poses each frame — and
participate in bloom through the existing HDR pipeline, so they are
exposure-dependent. They never cast shadows and use a black base colour. The
handover applies uniformly to authored celestial bodies and Gaia catalogue stars,
since gate travel can put a player near any catalogue star, but spheres are only
drawn inside the camera far plane: resolvable stars beyond `0.9e15` metres stay
baked as flux-conserving point sources.

Spheres appear and disappear as snapshots divert or drop their stars. A celestial
that becomes unsubscribed is hidden until the next bake replaces the snapshot,
and spheres are despawned together with their view.

Exposure, brightness and bloom are applied live, not baked. Simulation bodies and
their lighting remain active as before.

Rotation and floating-origin rebases reuse the entire cube. Translation refreshes
it when displacement from the baked position exceeds `nearest_star_distance / 8192`
(`0.25 / 2048` times the distance, in `Snapshot::valid`). The nearest distance
includes faint authored and Gaia stars as well as those currently visible, but
diverted stars never enter the snapshot, so inside a system it is set by distant
background stars instead of collapsing to the in-system star's distance. That
greatly reduces rebake frequency while flying in-system. A new catalogue
selection, magnitude limit change, or explicit relocation also invalidates the
cube. Camera rotation, field of view, exposure and sky brightness do not require
a rebake. The worker acknowledges cancellation before a new bake starts, bounding
CPU work and memory without queuing camera poses. GPU transfers already submitted
cannot be interrupted, but stale textures are never installed as the displayed sky.

The old sky stays displayed until the render world has prepared the replacement
texture. Only the displayed texture and an in-flight replacement need be retained;
CPU pixel data moves to the render world without a retained main-world copy.
A 2048 cube is 384 MiB at its base level, approximately 512 MiB including mip levels,
so replacement can temporarily require two textures plus staging storage. Mip
levels are ordinary texture filtering data, all generated in the same bake; they
are not separately displayed refinement passes. Uploads use Bevy's standard image
preparation. CPU baking is asynchronous, but a large upload can still stall
rendering. The readiness handshake prevents missing textures, not bandwidth costs.
The Universe GUI reports bake/upload state, star count and CPU bake time.

## Measurement policy

A positive supplied distance_pc or r_med_geo takes precedence. Otherwise the
converter uses inverse positive parallax only when parallax/parallax_error >= 10.
Gaia G magnitude approximates visual magnitude; colour uses temperature, spectral
class or BP-RP, with white as fallback. RGB is normalized to unit luminance.
There is no extinction correction, proper motion, or detailed spectral calibration.

The Earth-selected catalogue remains incomplete elsewhere and at Gaia's bright
end. It is a point-light population, not a million active planetary systems.
Authored systems still supply physical celestial bodies and lighting. Stellar
system generation remains separate from sky visibility and is future work.
