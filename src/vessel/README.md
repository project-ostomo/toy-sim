## Controls

Vessels are controlled through a rather intricate control pipeline:

- The **low-level vessel controls** directly interface with the parts on the vessel. e.g. if "full roll to the right" is indicated, every single part that could help rolling to the right is fully activated, regardless of nonlinearity, etc. 
- The **rotational rate fly-by-wire** targets a certain rotational rate, and interfaces with the low-level controls. This module also exposes the maximum rotational rate at a given time
- The **directional fly-by-wire** target a particular direction, and interfaces with the rotational rate fly-by-wire.
- **Autopilots** typically interface with the directional fly-by-wire.

### Representation

Each vessel root carries lightweight control state components:

- `VesselControlState` holds the instantaneous throttle/steering commands.
- `ControlTargets` stores high-level goals (direction quaternions, angular rates).
- `ControlTelemetry` caches the attitude and rate data that controllers consume.

Individual control computers live on their own module entities (e.g. `DirectionalPidController`, `RotationalPidController`) and update those components during the `ControlSystemSet::Modules` phase, mirroring how other ship modules (thrusters, torquers, etc.) integrate with the vessel.
