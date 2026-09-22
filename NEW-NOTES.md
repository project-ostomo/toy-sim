# Some design notes

## Rendering

## Autopilot

The current autopilot system really sucks. It's overall at the wrong level of abstraction, with concepts like waypoints floating in space that force the ship computer to be unable to dynamically adjust goals, etc, in an intelligent way.

Instead, the autopilot should be made higher-level, and integrated more tightly with the trip planner. More specifically, the autopilot should *only* support high-level commands:
- Slip to star system X
- Dock at station Y

*How* to achieve each of these is something that the per-ship autopilot needs to either figure out, or report a failure on. If failure is reported, the failure string is placed in the autopilot state and the AP turns off.

For instance, if we currently have a command to slip to star system X, but the path is obstructed by a planet, then the ship computer might decide to plot a minimal course to get the planet out of the way. The ship might decide even just to wait a while if the planet will naturally get of the way.

For instance, if we have a command to dock at station Y, but the station is orbiting a totally different planet, we may need to compute a slipdrive course that crashes onto that planet's exclusion zone, that 1. has acceptable risk 2. places us at as close to the optimal place to intercept the station as possible.

The current status may all be displayed on an advanced MFD.

### Layers

| Layer | Owner | Lifetime | Examples |
|---|---|---|---|
| Itinerary | Trip planner (server router) → server state | Until finished, failed or replaced | `[SlipToSystem(A), SlipToSystem(B), DockAt(Y)]` |
| Directive | Head of the itinerary | Until done or failed | `SlipToSystem(X)`, `DockAt(Y)` |
| Plan | Ship firmware, private | Recomputed continuously | departure windows, aim points, sidesteps, burns |

The server is a referee and a provider of world services. It stores intent (itinerary) and the firmware's published status, and it validates physical actions (slip commits, bay reservations, docking). It no longer holds a plan. The current `orders` / `order` / `revision` / `CompleteOrder` machinery and frozen `Destination` offsets go away.

### Directives

Directives are deliberately local in scope:

- **`SlipToSystem(X)`** — one interstellar hop, ending in a natural capture somewhere in X. The firmware chooses the capture body, the aim point on its exclusion sphere, and the departure time. It carries the risk allowance the trip planner assigned to the hop.
- **`DockAt(Y)`** — Y must be in the ship's overview, i.e. in the current system and known to the ship. If it isn't, the directive fails immediately. Getting from the capture point to the bay (transfer, rendezvous, bay reservation, docking) is up to the firmware.

Because `DockAt` only works locally, we still need a trip planner. It is the existing server A* router, reduced to its strategic job: choose the sequence of systems and divide the risk and fuel budgets among the hops. It produces an itinerary of directives. It knows nothing in-system: no capture sphere geometry, departure clearance or local transfers. That is the firmware's job at run time, using live geometry.

The directive completes when the firmware reports it complete (captured in X; docked at Y). The server pops the itinerary and hands over the next directive.

### Autopilot state

```
AutopilotState {
  enabled: bool,
  itinerary: Vec<Directive>,          // head = active directive
  status: FirmwareStatus,             // published by firmware, display-only
  failure: Option<String>,            // Some → enabled = false
}
```

`FirmwareStatus` is whatever the firmware wants to show: a phase (`Planning`, `Waiting { until, why }`, `Charging`, `Transit`, `Maneuvering`, `Docking`…), an ETA, and a summary of its current plan. The server doesn't interpret it.

**Failure** is the only way the autopilot gives up: the firmware sets a failure string and the autopilot turns off. The itinerary stays, so the player can read why it stopped, fix the cause and re-engage. **Waiting is not failure.** There is no cap on waiting. If the only acceptable plan is to wait three hours for a planet to move out of the way, the ship waits and says so.

Manual input (stick, throttle, and the old approach / keep range / align assists, which are now manual flight assists) turns the autopilot off.

### Firmware owns all the reasoning

The autopilot is part of the user-programmable WASM firmware. The directive/status/failure contract is the public ABI; the standard firmware is one implementation of it, and players can write their own.

Planning can be as heavy as it needs to be. WASM slices auto-suspend and resume, so a planner can just run a long search across many ticks without being written as a hand-sliced state machine. Meanwhile the control loop keeps the ship safe: holding attitude, finishing the current burn, or coasting.

The firmware computes everything, including risk. The trip planner hands each `SlipToSystem` a risk allowance. The firmware decides how to spend it (centre aim vs limb aim, waiting for a beacon, etc.) and reports what it spent. The server doesn't audit it. As today, a buggy controller can throw the ship away.

Suggested structure inside the standard firmware, running at three rates:

- **Directive planner** (slow, event-driven, allowed to take many slices): reruns on a new directive, arrival, a missed capture, beacon loss, a large change in mass or fuel, or the previous plan becoming infeasible.
- **Tactical tracker** (every few seconds): checks the current plan against live geometry, re-predicts windows and aim points, and asks for a replan when the plan drifts too far. Plans should switch only when the new one is better by some margin, to avoid thrashing.
- **Control** (every tick): the existing guidance law and slip-charge aim updates.

### Worked examples

**Slip line obstructed by a planet.** Choose between:

1. **Wait**: sample ephemerides forward for the earliest time the departure line is clear.
2. **Sidestep**: a burn perpendicular to the line, just big enough that the ship's capsule clears the body.
3. **Re-aim**: a different capture body in X whose line isn't blocked.

Minimise `time_to_departure + λ·Δv`, remembering that charging overlaps with both waiting and manoeuvring ("ordinary motion continues during charging"). The real cost is `max(charge time, clearing time)`, not the sum.

World services needed: `SlipEligibility` / `LocalSpace` answer "is it clear at t". The firmware needs to call them over a range of t cheaply, or we add a "first clear time along this line within horizon H" query.

**Docking at Y, which orbits a different planet P.** The main freedom is where on P's exclusion sphere we arrive:

- Aiming at P's centre gives the highest P(capture).
- Aiming toward the limb nearest where Y will be at arrival shortens the transfer afterwards, but lowers P(capture).

So we trade capture risk against transfer time/Δv, within the hop's allowance. Other levers:

- **Departure time**: the ship arrives with the galactic velocity it left with. Choosing a departure moment when that velocity best matches P's velocity at arrival reduces the matching burn.
- **Capture body**: P's sphere is small for low-mass planets (Earth ≈ 0.0012 AU ≈ 180,000 km). Aiming at one of P's moons, or at the star and accepting a longer transfer, may beat aiming at P itself.
- **Timing the station's phase**: arrival time sets where Y is in its orbit. Waiting for a better phase is legitimate (no waiting cap).

The trip planner only needs to know that "a `DockAt` in system X follows the hop into X". It can leave the choice of capture body to the firmware, since the firmware sees the next directive in the itinerary.

### Displays

A dedicated autopilot MFD page (a firmware screen) shows: the itinerary with the active directive, current phase and why (especially for `Waiting`), plan summary (capture body, aim offset, window, ETA, planned risk and Δv), risk spent vs allowance, and the last failure string. The flight instance publishes the plan summary so the display instance can draw it. The HUD's waypoint markers become a projection of the firmware's current plan rather than server queue entries.

### Migration

- Remove `Order::{Guidance, Sublight, TravelTo, WaitUntil, Slip, Undock}` from the queue; `TravelToSystem` / `Dock` become `Directive::{SlipToSystem, DockAt}`. Undocking is implicit when a directive needs the ship in space.
- Approach, keep range and align become manual flight assists; using one disengages the autopilot.
- The router returns a directive itinerary with per-hop risk/fuel allowances instead of a fully expanded order list.
- Travel `ProgramAction`s become: `Complete { directive_revision }`, `Fail { reason }`, `PublishStatus { .. }`, plus the physical actions (`Slip`, `ReserveBay`, `Dock`, `Undock`) validated on physics alone, not against a queued order.
- Persistence stores intent only; after a load, the firmware replans from scratch.

### Still open

- Should a ship that captures in the wrong system (a missed capture that was saved by an unplanned capture) ask the trip planner for a new itinerary automatically, or fail?
- Can the player edit the itinerary in the middle of a hop, or only replace it?
