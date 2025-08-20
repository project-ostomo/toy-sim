## Controls

Vessels are controlled through a rather intricate control pipeline:

- The **low-level vessel controls** directly interface with the parts on the vessel. e.g. if "full roll to the right" is indicated, every single part that could help rolling to the right is fully activated, regardless of nonlinearity, etc. 
- The **rotational rate fly-by-wire** targets a certain rotational rate, and interfaces with the low-level controls. This module also exposes the maximum rotational rate at a given time
- The **directional fly-by-wire** target a particular direction, and interfaces with the rotational rate fly-by-wire.
- **Autopilots** typically interface with the directional fly-by-wire.

### Representation

Instead of using a constellation of related components, we simply have a unified VesselControls component that fully encapsulates the state of the contrlls of a particular vessel.