# Navigation to a target

The standard firmware can fly a ship toward another ship on its own. The host exposes this through two requests, `SelectTarget` and `EngageNavigation`, and a navigation instrument. This guide describes that contract, the hardware it needs, the states it reports, how the simulator uses it, and the closed-loop tests that check it.

The current guidance flies a **pursuit pass**. It keeps full available thrust directed toward an intercept and flies through the target's position. It does not brake to match velocity, and it does not hold a stand-off distance. The guidance law and its forecast are covered in [pursuit-trajectory-design.md](pursuit-trajectory-design.md).

Source:

- Requests and validation: `Pilot::request` in [lib.rs](../crates/toy-sim-example-controller/src/lib.rs)
- Guidance state machine: [navigation.rs](../crates/toy-sim-example-controller/src/navigation.rs)
- Instrument publication: [firmware.rs](../crates/toy-sim-example-controller/src/firmware.rs)
- Navigation window: [gui/instruments.rs](../apps/toy-sim/src/gui/instruments.rs)
- Closed-loop tests: [physics/rendezvous_tests.rs](../apps/toy-sim/src/physics/rendezvous_tests.rs)

## Request sequence

1. **Select a target.** `REQUEST_SELECT_TARGET { contact }`. The contact must appear in the current sensor scan with kind `CONTACT_SHIP`, and guidance must not be active. On success, all previous guidance state resets and the target's relative position and velocity are recorded.
2. **Engage.** `REQUEST_ENGAGE_NAVIGATION { throttle_limit, stand_off_m }`. It checks:
   - Hardware discovery has completed.
   - The ship has forward thrust above zero and bidirectional torque authority on every axis, counting only control-enabled engines, RCS blocks and torquers.
   - A control-enabled sensor and accelerometer are operational and powered.
   - The accelerometer delivered a usable sample (at most 0.15 s old) this callback.
   - Guidance is not already active, and the selected target is visible.
   - `throttle_limit` is in (0, 1] and `stand_off_m` is in [0, 1,000,000].

   The stand-off value is validated and then ignored; the guidance sets its internal stand-off to zero. On success, guidance enters `Pursuing`, the attitude reference is set to the current orientation, manual throttle and steering are cleared, and any aim command is dropped.

Failures reply `REPLY_REJECTED` with a message. The Hardware diagnostics window lists these messages, and the reason field of the navigation instrument repeats the last one.

| Message | Cause |
| --- | --- |
| Hardware discovery is incomplete | The firmware is still reading the device directory |
| Target ship is not sensor-visible | Not in the latest scan, or not a ship |
| Abort before changing target | Selecting while pursuing |
| Invalid target telemetry | Non-finite contact data |
| Need usable thrust, steering authority, contact sensor and inertial sensing | Hardware check failed |
| Pursuit already engaged | Engaging twice |
| Select a sensor-visible target ship | No visible selected target |
| Invalid throttle limit | Limit or stand-off out of range |

## Phases

| Phase | Instrument `status` | Meaning |
| --- | --- | --- |
| `Ready` | `NAV_IDLE` (0) | No guidance |
| `Pursuing` | `NAV_ACTIVE` (1) | Guidance commands attitude and throttle |
| `Paused` | `NAV_SUSPENDED` (2) | Guidance stopped. Throttle is zero, and the attitude reference holds. |

`NAV_UNAVAILABLE` (3) is part of the ABI, but the standard firmware does not publish it.

A pursuit pauses when:

- the target has not been seen for more than 2 s ("Target lost - re-engage after reacquisition")
- the target is out of sight and no accelerometer sample is available ("Inertial sensing unavailable")
- required hardware or the accelerometer sample becomes unavailable ("Required hardware or accelerometer unavailable")
- flight telemetry is invalid, attitude is invalid, the thrust limit leaves no usable acceleration, or the guidance command is not finite

A paused pursuit does not resume when the target reappears. Send `EngageNavigation` again. Selecting a new target is allowed while paused.

While the target is out of sight for up to 2 s, guidance keeps flying on an extrapolated relative state. Its reason field reads "Target lost - pursuing last observed motion".

### Leaving guidance

- `REQUEST_STOP_GUIDANCE`: cancels pursuit or pause and releases attitude hold (manual flight).
- `REQUEST_HOLD_ATTITUDE`, `REQUEST_AIM_DIRECTION`, `REQUEST_AIM_CONTACT`: cancel guidance and apply their own attitude mode.
- `REQUEST_MANUAL`: cancels active or paused guidance when the throttle differs from the previous manual sample or steering is non-zero. Repeated identical samples with zero steering are ignored while guidance owns the ship.

Cancelling sets the phase to `Ready`, clears the forecast, zeroes throttle, and holds the current attitude (until manual input releases it).

## Navigation instrument

Each callback, the firmware publishes a `NavigationState` with a 2 s lease:

| Field | Value |
| --- | --- |
| `status` | From the phase |
| `target_contact` | Selected target ID, or 0 |
| `own_path`, `target_path` | 1 and 2 when a forecast exists, otherwise 0 |
| `present` | `NAV_ARRIVAL` when the forecast has a closest-approach time within its 2 s validity; `NAV_FUEL` when a forecast exists |
| `throttle_limit` | The engaged limit |
| `throttle` | The current commanded throttle fraction |
| `arrival_time_s` | Absolute time of the predicted pass |
| `predicted_fuel_kg` | Propellant used over the forecast |
| `stand_off_m`, `approach_speed_limit_m_s`, `braking_distance_m` | Always 0 (not present) |
| `reason` | Status or error text |

It also publishes a contacts instrument that names the selected target, and a `MARKER_TARGET` marker in `FRAME_CONTACT` (ID 3, label "Target") when the client requests markers.

## In the simulator

**Contacts window.** Clicking a ship contact selects it and sends `SelectTarget`. The "Aim" button sends `AimContact`.

**Navigation window.**

- The orbit overlay controls ([orbital-navigation.md](orbital-navigation.md)).
- The guidance status heading, target name and reason.
- Range, closing speed and relative speed, taken from the host's track estimate at the presentation time.
- Commanded throttle.
- A "Throttle limit" slider (0.01 to 1, default 1).
- "Engage", which sends the slider value and a stand-off of 100 m. A stand-off entry field appears only when the instrument reports `NAV_STAND_OFF`, which the standard firmware never sets.
- "Abort", which sends `StopGuidance`.
- ETA and estimated propellant when `NAV_ARRIVAL` is present.

**Flight computer window.** "Hold attitude" and "Manual / abort".

At startup, the traffic ship is commanded to select the player and engage with throttle limit 1 and stand-off 1000 m ([vessel README](../apps/toy-sim/src/vessel/README.md#spawning)). Because its computer boots for 5 s first, the requests wait in the host queue until discovery completes.

Relocating the player with "Relocate ship" in the Universe window sends `StopGuidance` and a zero manual sample.

## Checking designs

The editor's Systems mode warns about missing forward thrust along the control orientation, and about axes without bidirectional steering ("automatic rendezvous unavailable"). See [ship-editor.md](ship-editor.md#systems-mode).

## Tests

The closed-loop tests in [rendezvous_tests.rs](../apps/toy-sim/src/physics/rendezvous_tests.rs) run the real `Pilot` against the real hardware interpreter at 10 Hz. They use the starter design, an exact rotational integrator, an ideal accelerometer, and optional point-mass gravity (μ = 3.986004418e14). A test finishes when the relative position and velocity change from closing to separating. At that moment the separation must be below the larger of 10% of the starting range and 1 km, with relative speed above 1 m/s.

Scenarios:

- free space, including lateral and receding initial velocity
- a neighbour in low orbit, and the startup scenario's high inclined orbit (without entering the atmosphere)
- starting with the engine pointed away
- a weak torquer with a 0.5 throttle limit (every throttle command stays at or below 0.5)
- a target manoeuvring at up to 15 m/s²
- unavoidable overshoot, and a target moving too fast to intercept
- contact loss within and beyond the 2 s grace period, then re-engagement and manual takeover
- reduced thrust and short ranges (50 m to 20 km)
- rotated and off-axis engines, and several engines with a rotated extra torquer
- a tumbling, thrusting target in the startup orbit
- a missed first pass followed by another intercept
- forecast accuracy against flown positions, forecast clearing on abort, a jousting forecast continuing 10 s past the pass, and forecast retention between refreshes
- throttle staying high for a target only 100 m away

The accelerometer model is also checked: no signal in free fall, mount rotation handled, gravity subtracted while other forces are kept.

```sh
cargo test -p toy-sim rendezvous_tests
cargo test -p toy-sim-example-controller navigation
cargo test -p toy-sim-ship-wasm armed_starter_discovers_rcs_and_accepts_distant_pursuit_after_boot
```

## Limitations

- There is no velocity matching, braking or station keeping. After a pass, guidance keeps pursuing and turns back for another pass.
- The stand-off value is accepted and ignored.
- Guidance needs a visible target ship and a live accelerometer sample. Celestial bodies cannot be targets.
- Sensor contacts are exact. The disturbance estimate compensates only for smooth differences in gravity and target acceleration.
