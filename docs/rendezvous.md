# Navigation to a target

The standard firmware can fly a ship toward another ship on its own. The host exposes this through two requests, `SelectTarget` and `EngageNavigation`, and a navigation instrument. This guide describes that contract, the hardware it needs, the states it reports, how the simulator uses it, and the tests that check it.

The current guidance is a **braking rendezvous**. It flies to an aim point and slows down so that it arrives with near-zero relative velocity. The aim point is the target, or a point short of the target when a stand-off is requested. When the ship is within 2 m of the aim point and within 0.5 m/s of the target's velocity, guidance ends and returns to `Ready`. It does not keep station afterwards. [pursuit-trajectory-design.md](pursuit-trajectory-design.md) is an older design proposal written for the previous full-thrust pursuit law.

The same guidance flies sublight legs of travel orders in the authoritative world. The travel planner feeds it a synthetic contact instead of a sensor contact ([Travel legs](#travel-legs)).

Source:

- Requests and validation: `Pilot::request` in [lib.rs](../crates/toy-sim-example-controller/src/lib.rs)
- Guidance state machine and control law: [navigation.rs](../crates/toy-sim-example-controller/src/navigation.rs)
- Forecast: [prediction.rs](../crates/toy-sim-example-controller/src/prediction.rs)
- Travel planner: [world.rs](../crates/toy-sim-example-controller/src/world.rs)
- Instrument publication: [firmware.rs](../crates/toy-sim-example-controller/src/firmware.rs)
- Closed-loop tests: [physics/rendezvous_tests.rs](../crates/toy-sim-server/src/sim/physics/rendezvous_tests.rs)

## Request sequence

1. **Select a target.** `REQUEST_SELECT_TARGET { contact }`. The contact must appear in the current sensor scan with kind `CONTACT_SHIP`, and guidance must not be active. On success, all previous guidance state resets and the target's relative position and velocity are recorded.
2. **Engage.** `REQUEST_ENGAGE_NAVIGATION { throttle_limit, stand_off_m }`. It checks:
   - Hardware discovery has completed.
   - The ship has forward thrust above zero and bidirectional torque authority on every axis, counting only control-enabled engines, RCS blocks and torquers.
   - A control-enabled sensor and accelerometer are operational and powered.
   - The accelerometer delivered a usable sample (at most 0.15 s old) this callback.
   - Guidance is not already active, and the selected target is visible.
   - `throttle_limit` is in (0, 1] and `stand_off_m` is in [0, 1,000,000].

   On success, guidance enters `Pursuing`, the attitude reference is set to the current orientation, manual throttle and steering are cleared, and any aim command is dropped. The stand-off becomes a fixed offset: the aim point is `stand_off_m` short of the target, along the line of sight measured at the moment of engagement. The offset stays fixed in world axes afterwards. It does not follow later changes in the line of sight.

Failures reply `REPLY_REJECTED` with a message. The reason field of the navigation instrument repeats the last one.

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
| `Ready` | `NAV_IDLE` (0) | No guidance, including after a successful arrival |
| `Pursuing` | `NAV_ACTIVE` (1) | Guidance commands attitude and throttle |
| `Paused` | `NAV_SUSPENDED` (2) | Guidance stopped. Throttle is zero, and the attitude reference holds. |

`NAV_UNAVAILABLE` (3) is part of the ABI, but the standard firmware does not publish it.

## Guidance law

Guidance uses these quantities each callback:

| Symbol | Meaning |
| --- | --- |
| `e` | Displacement from the ship to the aim point |
| `u` | The ship's velocity relative to the target |
| `a` | Usable acceleration: thrust × measured engine effectiveness × throttle ceiling ÷ mass |
| `τ` | Response time: `max(2 × turn allowance + 2, 2)` seconds, where the turn allowance comes from the ship's inertia and torque authority |

1. **Arrival speed.** The allowed closing speed at distance `d` is
   `arrival_speed(d) = min(√((aτ)² + a·d) − aτ, d / (4τ))`.
   It shrinks to zero at the aim point, and it leaves room for the time needed to turn and respond.
2. **Economical speed.** Let `λ = 3600 / current_mass_kg` seconds/kg, `q` be propellant flow at the throttle ceiling, and `v₀` be nonnegative closing speed. The cruise target is `max(v₀, √((d + v₀²/(2a)) / (1/a + 2λq/a)))`. Clamp it to the arrival-speed envelope. Keeping the `v₀` floor avoids braking merely to discard momentum that has already been paid for.
3. **Commanded acceleration.** `command = clamp((ê × speed − u) / τ − disturbance, a)`.
   This is velocity feedback toward the desired closing velocity, corrected for the estimated relative disturbance (differences in gravity and target acceleration). It brakes when `u` is faster than the allowed speed.
4. **Throttle.** The throttle is the throttle ceiling × `|command| / a`. It is further scaled by the engine's alignment with the commanded direction when their cosine exceeds 0.995. Otherwise the throttle is zero, so the ship coasts while it turns.
5. **Stopping distance.** `u² / (2a) + |u| × τ` is computed internally.
6. **Arrival.** When `|e| ≤ 2 m` and `|u| ≤ 0.5 m/s`, guidance zeroes throttle and returns to `Ready`.

For a collinear transfer from rest, the constant-acceleration objective is `d/v + v/a + λ × 2qv/a`. Its stationary point gives the cruise-speed formula above, bounded by the no-coast speed `√(ad)`. This trades longer coasts for less fuel. The close-range `d/(4τ)` bound gives a critically damped velocity response when thrust can track the command. Actual control still accounts for finite turns and disturbances. It does not solve a global orbital boundary-value problem.

The throttle ceiling is the smaller of the engaged limit and the power limit. It is lowered further, where needed, so that an off-centre engine's moment uses at most 80% of torque capacity.

The forecast rolls the same law forward with the same response time and alignment gating. Its ETA uses the same terminal condition as guidance: the first sample whose distance to the aim point (the stand-off point, when a stand-off is set, rather than the target itself) is at most 2 m and whose relative velocity is at most 0.5 m/s, so the forecast arrival also matches the target's velocity. The rollout continues for 10 s after it.

### Travel legs

In the authoritative world, the travel planner expands a destination into queued orders through `world_query`. For a sublight leg, it builds a contact with ID `u64::MAX` from the resolved destination's relative position and velocity. It appends this contact to the scan results, selects it, and engages with throttle limit 1 and stand-off 0. The contact is rebuilt every callback, so the aim point follows the destination. When the order reports arrival within 2 m and 0.5 m/s, the planner sends `CompleteOrder`. It aborts guidance when there is no travel contact but the synthetic target is still selected. During a docking approach, the planner also holds the bay's attitude. See [server-client.md](server-client.md#travel-orders-and-firmware-planning).

The debug launcher uses the same server world services and planner as a remote client.

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
| `present` | `NAV_ARRIVAL` when the forecast has an arrival time within its 2 s validity; `NAV_FUEL` when a forecast exists |
| `throttle_limit` | The engaged limit |
| `throttle` | The current commanded throttle fraction |
| `arrival_time_s` | Absolute time of predicted arrival (within 2 m and 0.5 m/s) |
| `predicted_fuel_kg` | Propellant used over the forecast |
| `stand_off_m`, `approach_speed_limit_m_s`, `braking_distance_m` | Always 0 (not present). The firmware computes an allowed speed and a stopping distance internally but does not publish them. |
| `reason` | Status or error text |

It also publishes a contacts instrument that names the selected target, and a `MARKER_TARGET` marker in `FRAME_CONTACT` (ID 3, label "Target") when the client requests markers. The forecast's arrival marker still carries the label "Closest approach", although it now marks the predicted arrival.

## In the simulator

The client currently shows the scene HUD and one "Hello world" window. The orbit overlay still consumes published navigation instruments. Guidance, target selection and weapon commands remain available through the session protocol, while their former control windows have been removed during the UI rebuild. Selecting a HUD contact changes the local selection without issuing a guidance command.

The startup scene includes an orbital traffic ship. Travel orders and target pursuit use the same session commands available to remote clients. Demo retaliation resolves targets through the ship's fused contact handles.

## Checking designs

The editor's Systems mode warns about missing forward thrust along the control orientation, and about axes without bidirectional steering ("automatic rendezvous unavailable"). See [ship-editor.md](ship-editor.md#systems-mode).

## Tests

Tests written for the braking law:

- `rendezvous_brakes_and_matches_terminal_velocity` in [navigation.rs](../crates/toy-sim-example-controller/src/navigation.rs). It checks that the command brakes when closing fast, and that an ideal double integrator driven by the law arrives within 2 m and 0.5 m/s.
- `travel_order_runs_in_stock_wasm_and_brakes_at_destination` in [toy-sim-server/src/sim/travel/router_tests.rs](../crates/toy-sim-server/src/sim/travel/router_tests.rs). It runs the bundled firmware in the headless world and requires a galactic travel order 100 m away to complete within 2 m and 0.5 m/s.

The closed-loop tests in `apps/toy-sim` were first written for the previous pursuit-pass law and now check braked arrival through the shared finish condition below. Some test names (`pursuit_pass_…`, `missed_pass_turns_back_…`, `forecast_reaches_first_pass_…`) and setup checks such as "first pass must miss" keep the old wording; they describe the initial geometry, not the terminal behaviour.

The closed-loop tests in [rendezvous_tests.rs](../crates/toy-sim-server/src/sim/physics/rendezvous_tests.rs) run the real `Pilot` against the server hardware ECS systems at 10 Hz. They use the starter design, an exact rotational integrator, an ideal accelerometer, and optional point-mass gravity (μ = 3.986004418e14). A flight finishes when the distance to the aim point is at most 2 m and the relative speed is at most 0.5 m/s. Guidance must stay active until then, and the flight fails if that does not happen within 100,000 ticks.

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
- forecast accuracy against flown positions, forecast clearing on abort, an overshoot forecast whose arrival ends at no more than 0.5 m/s and continues 10 s past the ETA, and forecast retention between refreshes
- no throttle for a target that is already at the requested 100 m stand-off
- no throttle while the engine is still turning toward the commanded direction, then braked arrival

The accelerometer model is also checked: no signal in free fall, mount rotation handled, gravity subtracted while other forces are kept.

```sh
cargo test -p toy-sim rendezvous_tests
cargo test -p toy-sim-example-controller navigation
cargo test -p toy-sim-server --lib sim::travel::router_tests
cargo test -p toy-sim-ship-wasm armed_starter_discovers_rcs_and_accepts_distant_pursuit_after_boot
```

## Limitations

- Guidance brakes to arrive, but it does not keep station. After arrival it returns to `Ready`, and a moving or accelerating target drifts away until guidance is engaged again.
- The stand-off offset is fixed along the engagement line of sight. It is not maintained as a range.
- The navigation instrument does not publish stand-off, approach speed limit or braking distance.
- Guidance needs a visible target ship and a live accelerometer sample. Celestial bodies cannot be targets through `SelectTarget`. Travel orders reach destinations through the planner's synthetic contact.
- Sensor contacts are exact. The disturbance estimate compensates only for smooth differences in gravity and target acceleration.
