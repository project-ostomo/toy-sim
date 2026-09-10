## Controls

The camera always orbits the focused ship. Hold the left mouse button and drag to
orbit; use the scroll wheel to zoom. The floating **Camera target** window selects
any ship, planet, moon, or star and reframes it at a suitable distance. Zoom stops
outside the target's bounds. Camera selection does not change the controlled ship;
manual input continues to go to the ship marked `ControlledVessel`.

- W/S: pitch torque.
- A/D: yaw torque.
- Q/E: roll torque.
- Left Shift/Left Ctrl: increase/decrease throttle (clamped to 0–100%).

`VesselControlState` sends throttle directly to engine modules and steering directly
to torquer modules. Releasing steering keys stops commanded torque; angular momentum
remains. There is no attitude hold, rate controller, or automatic stabilization.

## Aerodynamic drag

At spawn, the positioned and rotated part dimensions determine a ship-aligned
bounding box. Its half-extents multiplied by sqrt(3) define a conservative enclosing
ellipsoid. This is not a minimum-volume fit and may overestimate a sparse ship's area.

For semi-axes (a, b, c) and a unit airspeed direction n in ship coordinates, its
projected area is `A = pi * length((b*c*n.x, a*c*n.y, a*b*n.z))`.
The model applies `F = -0.5 * density * Cd * A * speed² * velocity_direction`,
using a constant `Cd = 2.2`. Drag acts at the centre of mass: there is no lift,
aerodynamic torque, rotational damping, or per-part wing calculation.

Airspeed is relative to the moving atmosphere, including planetary rotation and
orbital motion. Each body's optional atmosphere config supplies density, scale
height, temperature, gas properties, and the outer cutoff. Bodies without that
section are airless. The density and optical profiles taper continuously to zero
at the configured atmosphere height. This remains a rough drag model without
thermospheric variability, rarefied gas transitions, or heating.

The starting orbit and camera position are configured in `assets/stars/helion.star.toml`.
The same file contains every body and its optional atmosphere. The demo
starts 30,000 km above Neris, with five airless moons and vacuum-capable thrust.
