# Inhabited systems and celestial generation

The inhabited map combines ten authored systems with 2,990 catalogue primaries.
The political map assigns their galactic anchors. Authored bodies retain their
names and orbital parameters; procedural systems materialize from their stable
catalogue identities. The generation version belongs to the map. Persistence
stores the dynamic world and a fingerprint of immutable definitions. Loading a
save requires the same bundled universe; a changed generator or authored
configuration is rejected rather than silently moving existing ships and assets.
Full orrery configurations are not stored in SQLite.

The CPU pipeline follows the hierarchy described in the supplied SpaceEngine
notes: catalogue completion, stellar components, planetary architecture, moons,
and environments. It uses separate ChaCha20 streams seeded by BLAKE3 for each
object and generation stage. Generating another system, changing request order,
or traversing companion records in a different order does not consume another
object's random draws. It generates physical parameters and surface seeds here;
terrain rendering is a subsequent feature.

## Stellar components

Catalogue luminosity and effective temperature determine radius through the
Stefan–Boltzmann relation. A piecewise mass–luminosity relation supplies an
approximate main-sequence mass. Compact, hot sources use a white-dwarf mass–radius
relation instead; cool extended sources receive a giant classification. Very
low masses receive a brown-dwarf classification. These are procedural inferences
from heterogeneous photometric estimates, not a precision stellar catalogue.
Names and catalogue identifiers do not imply measured masses or measured planets.

Each binary has a virtual barycenter and two components orbiting it with matching
period, eccentricity, phase, and opposite periapsis directions. Their orbital
radii follow the inverse mass ratio. Wider companions create another level in
this hierarchy. Inferred separations are increased when needed to keep an outer
pair clear of the entire inner pair. Virtual bodies contribute no collision,
rendering, light, or gravity; their mass supplies the orbital parameter for their
children. Summing physical masses excludes virtual nodes.

Planets around individual stars stay within a conservative fraction of the
closest companion's periapsis separation. A sufficiently compact multiple system
can also have planets around its outer barycenter, beyond the complete stellar
orbital envelope. These are hierarchical Kepler orbits; mutual perturbations are
not integrated.

## Planets and moons

Each host receives a coherent orbital plane and three to ten candidate slots.
The radiation field sets the initial inner region and snow line. Rocky planets,
ice giants, and gas giants draw different mass and radius distributions. Adjacent
planetary radial excursions have a clearance of eight mutual Hill radii. Slots
are expanded to satisfy that clearance and the fluid Roche limit, then rejected
beyond the host's allowed outer region. All stored angles are radians. Periods derive from mass and orbital
radius, and the solver differentiates the same eccentric orbit used for position.

Moons use the planet's equatorial orientation, remain outside the fluid Roche
limit, and stay within one quarter of the planet's Hill radius at periapsis.
Adjacent moons have six mutual Hill radii of clearance. Rotation periods are
synchronized for generated moons and sufficiently close planets. These spacing
rules prevent crossing orbital envelopes and provide conservative separation;
they do not prove stability under a full gravitational integration.

The environment model computes average radiative equilibrium from luminosity,
albedo, distance, and eccentricity. Mass, escape energy, irradiation, and a volatile
retention draw determine whether a rocky body has an atmosphere. Pressure and gas
properties then determine density and hydrostatic scale height. A simple pressure
dependent greenhouse term and giant-planet internal heat set display temperature.
Ocean coverage additionally requires atmospheric pressure and a temperature
between freezing and the approximate pressure dependent boiling point.

`StellarParameters` records the inferred stellar class, temperature, and age.
`PlanetParameters` records material class, equilibrium and surface temperatures,
albedo, ocean fraction, and an independent surface seed. Existing atmosphere
parameters drive both drag and atmospheric rendering. The model supplies varied,
internally consistent environments without claiming detailed climate simulation.

## Verification

Run `cargo test -p toy-sim-universe --lib`. Tests cover complete-map validation,
global names, seed isolation, companion ordering, binary center of mass, eccentric
velocity derivatives, Hill and Roche limits, hydrostatic atmosphere relations,
and preservation of authored systems. Sol's source angles use radians consistently
with the solver.

Run `cargo run --release -p toy-sim-universe --example generation_benchmark` to
measure map construction, generation and validation, and solver/index construction
separately. The example reports physical and virtual body counts and environment
classes.
