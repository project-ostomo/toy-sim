# In-memory star catalogue

The Cargo workspace contains the Bevy app at its root, `crates/toy-sim-space`
(shared i128 micrometre coordinates), and `crates/toy-sim-stars` (portable binary
I/O and immutable in-memory queries). Neither library depends on Bevy. There is
no SQLite runtime, database, persisted tree, or connection mutex.

The bundled catalogue contains 1,000,000 real Gaia DR3 sources selected for bright
views near Earth. Its binary file is 76,000,040 bytes. Default limiting magnitude
is 6; max_stars limits the returned brightest matches, not the catalogue size.

## Conversion and configuration

```sh
python3 tools/import_gaia.py assets/catalogues/gaia-dr3-earth-million.stars assets/catalogues/gaia-dr3-earth-million.csv
cargo run -p toy-sim-stars --release --example query -- assets/catalogues/gaia-dr3-earth-million.stars
cargo test --workspace
```

The converter reads CSV or CSV.gz, validates measurements, skips unusable or
nonstellar rows, deduplicates source IDs (first row wins), and writes to a temporary
file before replacing the output. Re-running regenerates the same binary; no
resumability bookkeeping is needed. A companion `.stars.json` records provenance,
calibration, origin and conversion counts. Raw CSV and ADQL remain available for
inspection. `--limit` limits input rows; `--origin-um X Y Z` supplies an integer
micrometre offset; `--min-parallax-snr` defaults to 10.

```toml
[gaia]
path = "catalogues/gaia-dr3-earth-million.stars"
max_stars = 150000
exclude_source_ids = []
```

Paths are relative to assets, or absolute. The app loads and builds the index on
one background task, then shares an immutable `Arc<StarCatalogue>`. Queries run
on background tasks without a database or mutex. The existing 0.05 magnitude
headroom, nearest-distance movement budget and keep-old-results refresh policy
remain. Query diagnostics report candidate stars, bucket count and resident stars.

## Binary format, version 1

All numbers use explicit little-endian encoding, without Rust struct padding.
The header is 40 bytes:

| Offset | Encoding | Meaning |
|---|---|---|
| 0 | 8 bytes | `TOYSTAR\0` magic |
| 8 | u32 | Version: 1 |
| 12 | u32 | Record size: 76 |
| 16 | u64 | Record count |
| 24 | u32 | Coordinate unit: 1 = signed integer micrometres |
| 28 | u32 | Frame: 1 = ICRS Cartesian, fixed J2016.0 |
| 32 | u64 | Identity namespace: 1 = Gaia DR3 |

Each 76-byte record is: source ID (u64), X/Y/Z (three i128 micrometre coordinates),
intrinsic luminosity (f64 lumens), and linear RGB multipliers (three f32 values).
The loader rejects unknown versions, units/frames, incorrect lengths, duplicate
identities, non-finite/nonpositive luminosities and invalid colours. Coordinates
are restricted to [-2^126, 2^126) micrometres to leave subtraction headroom.

In memory `StarId { namespace, value }` separates persistent identity from storage
index and display name. `StarCatalogue::from_stars` accepts records from other
sources, including future procedural regions; mixed namespaces can coexist in an
index. Each binary file currently stores one namespace. No procedural generation
or region streaming is implemented yet.

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

## Progressive sky rendering

The renderer uses Bevy's built-in skybox with CPU-baked RGBA16F cubemaps. A single
background bake progresses through 512, 1024, 2048 and 4096 pixels per face from
one immutable position/catalogue snapshot. Each level starts as soon as the previous
texture is GPU-ready, with no added delay. Invalidation signals the CPU bake to
stop, discards pending stale results, and restarts at 512 for the latest position.
Cancellation is checked before allocation, between mip levels and every 256 stars.
Rotation does not invalidate the cache.
Each star deposits its flux into a
minimal bilinear footprint, divided by the receiving texel's solid angle;
edge taps wrap onto adjacent cube faces. Analytically splatted mip levels preserve
flux when the texture is minified, without scanning the full image. Colours and
radiance remain linear HDR.
Exposure, brightness and bloom are applied live, not baked. Resolved authored
stars remain physical spheres.

Rotation and floating-origin rebases reuse the entire cube. Translation refreshes
it when its conservative half-texel parallax budget is exceeded. Catalogue
revision, magnitude limit, explicit relocation and changes in resolved-star
membership also invalidate it. The worker acknowledges cancellation before a new
bake starts, bounding CPU work and memory without queuing camera poses. GPU
transfers already submitted cannot be interrupted, but stale textures are never
installed as the displayed sky.

The old sky stays displayed until the render world has prepared the replacement
texture. A lower-resolution result only replaces a sharper sky if the latter is
stale. Only the displayed level and an in-flight replacement need be retained;
CPU pixel data moves to the render world without a retained main-world copy.
A 4096 cube is 768 MiB at its base level, approximately 1 GiB including mip levels,
so replacement can temporarily require two large textures
plus staging storage. Uploads currently use Bevy's standard image preparation;
CPU work is asynchronous, but a large upload can still stall rendering. This
readiness handshake prevents missing textures, not bandwidth costs. The Universe
GUI reports displayed resolution, current bake/upload level and CPU bake time.

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
