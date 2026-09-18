# Pursuit trajectory presentation proposal

This is a historical design proposal written for an earlier rendezvous controller.
Stock firmware now flies a braking rendezvous: arrival-speed velocity feedback that
ends within 2 m and 0.5 m/s of the aim point. The same law flies sublight travel
legs. The law, its forecast and its instrument fields are described in
[rendezvous.md](rendezvous.md#guidance-law).

The implemented spatial interface is [ABI 14](ship-abi.md). ABI 12 added
`world_query`, `world_command` and the separate `ship_display` entry point. None
of the trajectory extensions proposed below were added: no timed
position/velocity knots, burn events or published target reference paths. The
stock forecast is still published as a timed path, and the navigation instrument
still leaves its stand-off, approach speed and braking distance fields at zero.
The tumbling traffic scenario is implemented separately.

## What the player needs to see

A minimum-time, bounded-acceleration transfer can accelerate and decelerate along
the same spatial line. Reversing thrust changes acceleration, not instantaneous
velocity, so a flip must not introduce a cusp or a kink into the flight path.
With lateral velocity, finite turning time, gravity, or a maneuvering target, the
burns need not be symmetric or switch at half the distance. The stock planner is
a bounded two-burn search with turning/coasting intervals, not an exact global
minimum-time solver. The ideal double-integrator bang-bang case is a useful
reference, not a template for the client to impose on every maneuver.

The proposed native instrument would combine:

- A spatial ship forecast and the target forecast used by that same solution.
- A time ruler with firmware-published burn and reorientation boundaries, plus
  time-spaced markers along the path. Distance spacing alone hides acceleration.
- A synchronized ghost ship and target while scrubbing, with separation and
  relative velocity at the same instant.
- Separate terminal position error and relative speed. Small closest approach
  with large residual speed describes a flyby, not successful rendezvous.

Use one planned-path colour with restrained event markers and a burn strip on the
timeline. Burn direction can be shown at the selected time. Avoid encoding an
entire control program through a rainbow of inferred phase colours. Keep the
unpowered conic as secondary context. An optional target-relative encounter view
can make closing motion easier to read, but must explicitly identify its frame
and retain the same timestamps and world-space solution.

## Publish kinematics, not the planner's private structure

The existing stock predictor publishes 33 uniformly timed positions from the
selected maneuver. Short turns and acceleration changes can fall between those
samples. Native smoothing cannot recover the missing motion or locate burn
boundaries reliably. The existing scalar speed is not a velocity vector and must
not be treated as a cubic tangent.

The preferred future contract is a bounded list of timed position/velocity knots,
with a knot at every change of modeled acceleration. Define velocities relative
to the declared frame, including the rule for adding the frame's world velocity.
Hermite interpolation can represent constant-acceleration segments exactly and
provide a bounded approximation for more general motion. It can then use the
client's screen-space Bezier rendering pipeline. Interpolation semantics must be
part of the versioned contract, rather than guessed by each renderer.

Add optional timed events with firmware-provided labels and generic physical
quantities such as planned thrust direction or commanded thrust fraction. Events
may describe burns, coasts or slews, but the client must never assume five phases,
a midpoint flip, or the stock pilot's state enum. Other firmware may publish any
valid segment sequence, or no events at all. Built-in clients choose presentation;
firmware supplies data, not arbitrary additions to the native UI.

Keep the existing publication epoch, explicit frame, expiry, bounded output/gas
costs and atomic replacement. These additions require a deliberate versioned ABI
change. A smaller ABI-compatible improvement would first sample all selected-plan
segment boundaries inside the firmware, then distribute the remaining point
budget by approximation error. It would improve geometry without inventing native
phase labels, though it would not supply velocity vectors or event annotations.

## Make the target assumption visible

The amber stock forecast assumes the observed target velocity persists, with a
measured relative disturbance correction. The magenta native estimate independently
uses a two-body coast. Their intersection need not be the rendezvous the firmware
solved for. A future solution publication should therefore optionally include its
own target reference path and terminal objective. Display that alongside the ship
forecast when assessing planned rendezvous. Keep the independently observed coast
as clearly identified context, not a replacement target model inside the solution.

A tumbling ship under thrust makes the distinction particularly important. Its
acceleration direction changes continually, and the pursuer only receives sensor
observations. The client must not inspect the target entity's hidden throttle,
orientation or angular velocity to make the forecast look more accurate.

Show forecast age and distinguish a provisional continuation from a feasible
intercept. A prediction spread is useful only if firmware publishes a defined
assumption or uncertainty model; do not fabricate a confidence percentage from
age alone. Replace replanned geometry atomically. Interpolating between unrelated
old and new solutions would display a maneuver that was never planned.

Reference: [MIT's bounded-input double-integrator discussion](https://underactuated.csail.mit.edu/dp.html)
explains the ideal minimum-time bang-bang case. It does not establish optimality
for our constrained, moving-target spacecraft planner.
