# Orbital navigation

The Navigation instrument draws trajectories directly over the flight view. Cyan is
the controlled ship's instantaneous coast orbit, amber is the flight computer's
published maneuver, and magenta is the selected target's estimated track. Direction
arrows follow time. Occluded segments are subdued and dashed. The overlay starts
enabled and shows a coast orbit even without a selected target or maneuver.

**O** toggles the overlay. Keyboard shortcuts respect egui keyboard focus. The current client shows the HUD and one "Hello world" window; the orbit controls, camera framing buttons and preview slider have been removed during the UI rebuild. Mouse dragging and scrolling still orbit and zoom the camera. Clicking a celestial label focuses that body, and Escape restores the controlled ship as the camera focus.

Apsis labels are omitted for nearly circular orbits. Relative ascending and descending nodes require distinct orbital planes around the same primary. Closest-approach and published path markers remain part of the HUD.

## Prediction and observation semantics

This is an instantaneous two-body estimate, not a change to the physics integrator.
The strongest applicable gravitational source defines each fitted orbit. The native
universal-variable solver supports bound, parabolic and escape states, with an
inertial straight-line fallback outside gravity. Coast curves end at their primary
surface. The published plan is also clipped at the observer primary's surface.
External forces, moving targets' future commands, other-body collisions and future
changes of primary are not forecast. Closest approach is a bounded sampled search
with local refinement, using both positions at the same timestamp.

The coast horizon is one ship period, or one hour for an unbound coast, clamped between one minute and one year. The target is propagated across the same horizon.

All curves share a translating, nonrotating frame centered on the ship's current
primary. A common primary's future translation cancels exactly; targets around a
different primary use future celestial ephemerides. The renderer follows the body's
interpolated presentation position, subtracts integer world anchors before floating
point conversion, and clips in double precision before emitting egui screen points.

Target coast prediction receives only admitted sensor data. The host retains two
observations per contact, interpolates between them and extrapolates measured
velocity for at most two seconds. Hulls, target boxes, coast estimates and markers
use one presentation clock; no renderer queries live target entity transforms.

The current ABI ([version 21](ship-abi.md)) represents trajectories and space labels as leased paths and markers, as introduced in ABI 11. Snapshot
frames retain the precise source observation origin. Forecast vertices carry absolute
times, so a calculation can span callbacks without shifting old geometry onto a newer
ship position. Ship, body and admitted-contact frames support current annotations;
fixed path markers identify events on a particular publication revision.

The amber path is firmware-owned geometry. The stock controller publishes its full
arrival forecast and a separate target forecast with its chosen linear-motion
assumption. The renderer consumes path IDs from NavigationState and otherwise renders
arbitrary paths and markers without interpreting pursuit phases. It never rebuilds
a ship forecast from a target coast. The native coast estimate remains available when
firmware publishes no target path.

Metadata and vertices commit atomically. Omission retains them until absolute expiry;
explicit removal or reboot invalidates them. The stock pilot uses two-second leases
and refreshes complete forecasts independently from guidance. Replacing a path
invalidates markers attached to its previous revision until firmware republishes them.

Native navigation headings remain idle/active/suspended/unavailable. Range, closing
speed and relative speed are derived from the same contact resolver as target boxes.
Optional constraints, arrival time and predicted fuel come from firmware. The client owns overlay colors and layout.

## Cost and validation

Only the controlled vessel's visible overlay predicts trajectories. The event and
encounter model refreshes with presentation time so estimated target and ship
geometry agree during interpolation; unchanged presentation times reuse it. Published paths use celestial ephemerides to translate their vertices into the display frame.

Own and same-primary target coasts render as exact rational quadratic conic arcs.
The client clips those arcs against the near plane and viewport in homogeneous
double precision, including curved entries with both endpoints outside. It fits
the visible portions with ordinary screen-space cubic Beziers, using a conservative
0.25-physical-pixel error bound. Egui's Bezier flattener then uses another
0.25-physical-pixel tolerance and its stroke tessellator draws the result. Geometry
adapts to camera zoom without changing simulation or firmware. Per coast, work is
bounded to 8,192 fitting visits and 1,024 visible cubics; pathological intervals that
exceed the budget are omitted instead of drawn as inaccurate chords. The display
has at most 32 annotations.

The own coast is fitted to the rendered ship's interpolated position and velocity,
relative to the primary's interpolated state. This avoids attaching a latest-tick
orbit to an earlier presentation pose. Published WASM trajectories retain their
epochs, frame semantics and piecewise-linear corners. Dash and arrow phase carries
across adjacent curve pieces instead of restarting at tessellation boundaries.

The guest's trajectory interest bit controls publication. Camera movement and resizing make no guest calls.

On the local Ryzen 9 5900XT, optimized development tests measured approximately:

| Work | Mean | p95 |
| --- | ---: | ---: |
| Native prediction refresh, including a 512-segment plan | 2.73 ms | 2.81 ms |
| Orbit view + egui tessellation, 1600×1000, no GPU | 0.51 ms | 0.62 ms |
| Ship view at 100 m + egui tessellation, same resolution, no GPU | 0.055 ms | 0.070 ms |
| 501-ship pursuit simulation, interest enabled | 7.05 ms/tick | — |
| Same pursuit simulation, interest disabled | 7.16 ms/tick | — |

These historical measurements used the Bezier renderer and ABI 7 with an explicit linear-frame
publication. The preceding polyline renderer measured about 0.37 ms/frame in orbit
view; the new renderer spends additional work on accurate geometry and clipping.
Fleet rows are the earlier ABI 6 baseline; they are not a new ABI comparison.
Automatic startup pursuit has since been removed, so the current fleet fixture
uses coasting traffic and does not reproduce that historical pursuit workload.
Fleet measurements use 16 worker threads and 200 measured ticks after warmup, with
frozen positions to isolate ship work. They exclude the native overlay and GPU;
the small enabled/disabled difference is run-to-run noise. The first ship is idle
in that fixture; its interest bit does not request an active maneuver. The separate
client benchmark includes a target and published plan. Costs should not be added to
the simulation's per-ship timing counter.

Run current validation and benchmarks with:

```sh
cargo test --workspace --offline
cargo test -p toy-sim --offline profile_orbit_overlay -- --ignored --nocapture
SHIP_PROFILE_MODE=idle SHIP_PROFILE_THREADS=16 cargo test -p toy-sim --offline profile_default_fleet -- --ignored --nocapture
SHIP_PROFILE_MODE=no_instruments SHIP_PROFILE_THREADS=16 cargo test -p toy-sim --offline profile_default_fleet -- --ignored --nocapture
```

Regression coverage includes conic energy/momentum and reversibility, energetic
escape convergence, surface entry, synchronous closest approach, scan/publication
epochs, stale and skipped observations, explicit clear/expiry/reboot, guidance-status and target independence, cross-primary
reconstruction, huge anchors, clipping, occlusion, camera restoration and unchanged
flight outputs across instrument subscriptions. Bezier-specific tests compare dense
exact-curve samples to egui's flattened strokes at 1 m through 100,000 km camera
distance, multiple inclinations and 1x/2x/4x pixel scales, including eccentric,
parabolic and hyperbolic paths, clipping and occlusion transitions. Presentation
velocity tests cover ships, celestial ephemeris velocities and teleport resets.
The Bezier renderer was also checked in the running client on an isolated X11
display for close and orbit views.
