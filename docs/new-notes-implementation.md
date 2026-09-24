# NEW-NOTES implementation

## Ownership and architecture

- Active celestial entities carry Hill radius and ancestry components. A dedicated spatial BVH uses their ECS positions; parallel membership queries populate object location components. Immutable orreries come from a lazy process-wide Moka cache. Gravity continues to use n-body integration.
- Slip physics owns distance-earned arrival velocity and cumulative integer fuel charges. A requested velocity change is capped at 10 km/s per actual light-year; linear fuel cost scales with departure mass and delta-v.
- Gun and laser configurations and hardware specifications are distinct. Lasers emit dispersed hitscan pulses with beam spreading and bounded deposited energy.
- The strategic server router produces system/directive itineraries and budgets. Firmware owns departure windows, capture geometry, local maneuvers and waiting.
- One flight WASM instance performs planning and control using ordinary suspension. Long planning can delay control. The display instance consumes published flight status.
- The server retains intent, failure and budget progress through save/load. Firmware rebuilds private plans; physical transit resumes its saved progress.
- Rendering owns exposure metering, slip emission and star presentation. Headless captures provide evidence for shadow and exposure fixes.

## Verification completed

Verification completed on 2026-09-24:

| Scope | Result | Evidence |
| --- | --- | --- |
| Model, protocol, ship catalogue and controller unit tests | 85 passed | `/tmp/notes-core-tests.log` |
| Targeted server physics, routing, commands, services, persistence and vessel tests | 83 passed in the grouped run; the remaining real docking test passed after the firmware clock fix | `/tmp/notes-server-tests.log`, `/tmp/notes-docking-test.log` |
| Client scene, console, map and itinerary tests | 100 passed | `/tmp/notes-client-tests.log` |
| Final bundled firmware: routing, route requests, sandbox, targeting and suspension | 36 passed | `/tmp/notes-host-final-tests.log` |
| Authenticated main stream, RPC authority and asset transfers | Passed | `/tmp/notes-network-test.log` |
| `cargo check --workspace --all-targets` | Passed | `/tmp/notes-workspace-check.log` |

The targeted checks cover indexed Hill membership, nested and overlapping spheres,
docking/transit context, slip fuel calibration and velocity limits, early exits,
fuel exhaustion, laser geometry and energy accounting, and persistence of intent
and spending. Firmware tests exercise capture selection, obstruction and repeated
waiting. The server docking test runs real firmware with hull intersection checks.
These checks exercise individual planning and physics cases; they do not establish
every possible journey between stations orbiting different planets.

Integration checks found and corrected projectile insertion into the collision
index, stale firmware searches, and confusion between the hardware callback clock
and the simulation clock. The public ABI is now `GAME_VERSION = 56`; bundled flight,
display and test firmware were rebuilt together. Existing save files are retained
when the prototype save format changes.

## Rendering evidence and remaining uncertainty

The headless software renderer captured 450 frames with the initial expedition
patrol, including startup, charging, camera rebasing and transit. A second run
captured 450 frames with the previous layer scheduling. Charging changed measured
exposure by less than 0.00001 EV. Startup metering, layer publication ordering and
excessive HDR ring emission were corrected; slip glow is excluded from metering,
and transit uses sparse rays with visible background stars.

The reported intermittent black flash was not reproduced in either run. Its exact
cause remains unconfirmed. The capture results do not certify that symptom fixed.

See the [rendering report](../target/render-regressions/README.md), including
validation diagnostics and reproduction instructions. Representative screenshots:

- [Startup](../target/render-regressions/045-cold-start-view0.png)
- [Idle](../target/render-regressions/150-warm-start-view0.png)
- [Full charge](../target/render-regressions/210-charge-view0.png)
- [Transit](../target/render-regressions/375-transit-view0.png)
- [Later transit](../target/render-regressions/450-transit-view0.png)
