# Shared spatial index

`SpatialHash` stores integer galactic positions, conservative object radii, and
current optical luminosities. Geometry, sensor queries, stellar lookup, and
optical replication use the same implementation. Callers own the mapping from
compact `u32` handles to their entities or durable identifiers.

Occupied cells form a sparse hierarchy addressed by integer coordinates and a
power-of-two size. Empty spatial scales are compressed. A second set of cells
partitions luminous entries by powers of two in luminosity. Each brightness
bucket has a conservative visibility radius, so a query can reject distant dim
objects without examining every object in the galaxy. The entry records are
shared between the geometry and luminosity indexes.

Positions remain signed 128-bit micrometres throughout cell addressing. Distance
calculations subtract integer coordinates before converting to metres. Unsigned
absolute differences preserve the entire separation between opposite ends of
the signed coordinate range. Cell rejection includes a floating-point tolerance;
segment queries also account for projection error over very long sweeps.

`insert` updates an existing handle. Movement within the current leaf keeps its
membership and only refits affected radius bounds. Crossing a cell or luminosity
bucket updates the affected paths. `set_luminosity` handles engine activity,
lighting changes, or other physical changes supplied by the caller. Occlusion
between a particular observer and a source remains the caller's responsibility.

## Queries

- `within_radius` tests centre distance.
- `intersecting_sphere` includes the stored object radius.
- `segment_candidates` finds stored spheres touched by a swept query sphere.
- `visible` applies `luminosity / distance² >= threshold`. The caller chooses
  consistent luminosity and threshold units, including any geometric factors.
- `visible_from_sphere` returns sources visible from at least one location in an
  observer neighbourhood. This permits shared candidate discovery for nearby
  sensors, followed by their individual visibility checks.
- `nearest_filtered` ranks accepted handles by centre distance, the supplied tie
  key, and handle. `brightest` ranks brightness with distance clamped to one metre
  to give a finite value at the source position.

Completed range, visibility, and segment results have deterministic handle
ordering. Nearest results have deterministic distance ordering. Statistics count
visited cells and tested entries.

## Metered traversal

`range_cursor` and `advance_range` allow callers to charge for range queries as
work happens. Each advance limits both work and result count. One visited cell or
tested entry costs one work unit, including a leaf with many coincident entries.
Tag matching and result serialization can incur additional caller-defined gas.

A cursor belongs to one immutable index version. Mutations and passing it to a
different index invalidate it explicitly. Retain the corresponding `Arc`
snapshot while paging; create the cursor after any copy on write. Each batch is
sorted, while callers requiring a globally sorted aggregate sort after traversal.

## Verification and measurement

Run `cargo test -p toy-sim-spatial --lib` for comparisons with brute force,
coordinate extremes, boundary contacts, dynamic refits, cursor budgets, and
ordering checks. Run `cargo run --release -p toy-sim-spatial --example benchmark`
for seeded sparse and clustered populations of 100,000 objects. The benchmark
reports query work, elapsed time, and process resident-memory changes. Allocator
reuse means the second population's memory delta is not its complete footprint.
