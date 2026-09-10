# Celestial configuration

`stars/helion.star.toml` contains the complete system. Its `[scenario]` table selects the starting body and vessel, altitude (metres), inertial
radial direction and orbit normal, and orbit-camera distance/yaw/pitch. The ship
starts with circular velocity relative to its parent plus the parent's orbital
velocity. The simulation clock starts at MJD 0; orbital angles are radians.

Each `[[bodies]]` entry describes one star, planet, or moon. Parent references
are resolved regardless of entry order. Mass, radius, orbital elements, rotation,
and surface colour belong to that entry. Distances and times accept units such as
`"6000 km"`, `"1 au"`, and `"24 h"`; unadorned numbers use SI units.

An optional `[bodies.atmosphere]` table immediately following its body entry configures:

- `height`, `scale_height`, `mie_scale_height`: metres above the surface.
- `surface_density`: kg/m³; `temperature`: kelvin.
- `specific_gas_constant`: J/(kg K); `heat_capacity_ratio`: dimensionless.
- `rayleigh_scattering`: RGB coefficients in m⁻¹.
- `mie_scattering`, `mie_absorption`: coefficients in m⁻¹.
- `mie_asymmetry`: between -1 and 1, exclusive.
- `ground_albedo`: linear RGB reflectance, each channel between 0 and 1.

Gas density and Rayleigh scattering share a normalized exponential falloff,
reaching zero at `height`. Pressure follows the ideal gas law. Missing atmosphere
means vacuum: no scattering and no aerodynamic drag.

Rendering uses Bevy 0.19's `Atmosphere` and `ScatteringMedium` assets with the
raymarched camera mode. Atmospheres follow their bodies with the floating origin;
body meshes are scaled separately so atmosphere distances stay in metres.
Bevy renders only the nearest atmosphere per camera. The included scenario has
one atmosphered planet and five airless moons.

The optional `[scenario.traffic]` table creates additional coasting ships of the
scenario vessel type. `count` is the number of additional ships, `seed` selects a
reproducible random distribution, and `min_altitude` / `max_altitude` are metres
above the planet surface. Altitudes must stay above the configured atmosphere.
Planes, phases and altitudes are randomized; each ship starts at circular speed
plus the parent's orbital velocity. The included scenario has 500 traffic ships
and one controlled sensor ship.

## Universe manifest and synthetic fixtures

`universe.toml` explicitly selects authored system files and the starting system.
Only the starting system's `[scenario]` creates ships. The unused `sol.star.toml`
is not loaded: its angular units need correcting before it is used with this solver.
The `[synthetic]` section adds deterministic runtime fixtures: one fixed star and
three non-crossing planets per system, distributed uniformly in volume between
`min_distance_pc` and `max_distance_pc` around the starting system. The shipped
seed is 42; count is now zero because the Gaia catalogue supplies background stars.
Set count above zero to add test systems between 2 and 30 parsecs. No generated TOMLs are needed.

Each authored system may set `position_um = ["x", "y", "z"]` at its top level;
these are exact signed integer micrometres and default to the origin. Every
celestial name must be globally unique. Names, including parent/scenario references,
are identities; there is no additional persistent body-ID namespace. Helion's
planet is `Helion I Neris`, with moons `Helion I a Ione` through `Helion I e Orin`.
This version requires exactly one stationary root star per system. All other
bodies have parents, positive mass/radius, and finite elliptic orbit parameters.

`gravity_cutoff` is acceleration in m/s², defaulted in the shipped manifest to
`1e-8`. The derived boundary is orbital extent plus `sqrt(GM / cutoff)`, with a
hard gravity cutoff and immediate activation/deactivation (no hysteresis).
`[sky]` sets `magnitude_limit` and skybox display `brightness`.
Magnitude selection uses the synthetic visual calibration of absolute magnitude
4.83 for 3.6e28 lumens at 10 pc; it is not an imported Gaia photometric model.
The half-magnitude interval below the limiting magnitude fades smoothly to zero.
Display brightness does not change which stars pass magnitude selection.

Stars may specify `spectral_class = "G"` (O, B, A, F, G, K, M). These are approximate display presets; omission preserves `surface_color`.

Optional `[gaia]` config selects a flat binary `.stars` catalogue, `max_stars` and `exclude_source_ids`. The shipped manifest enables a 1,000,000-source ESA Gaia DR3 bright-star catalogue; see `catalogues/README.md` and `../docs/gaia-catalogue.md`.
