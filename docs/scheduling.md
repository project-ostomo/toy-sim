# Scheduling audit

The client uses Bevy 0.19 states and ordinary schedule boundaries for session
initialization. Discovery and asynchronous completion can take another frame;
newly constructed ships initialize their hardware on the next simulation tick.

## Client lifecycle

`ClientPhase` is a `States` enum: `Loading`, `Running`, and `Failed`.
`SessionFailure` stores the error message and whether retry is available.
`Bootstrap` owns incomplete data; `GameSession` remains a fully initialized
resource with a generation-tagged `SessionKey`.

1. `PreUpdate` receives transport events, then consumes the session event queue.
   World replacement requests `Loading`; disconnect requests `Failed`. Either
   overrides a pending activation before Bevy runs `StateTransition`.
2. `OnEnter(Loading)` and `OnEnter(Failed)` remove session resources and replicated
   entities and reset actions, subscriptions, panes, and pending requests.
   Entering `Loading` also clears the error and starts missing bootstrap tasks.
   `NextState::set(Loading)` intentionally runs the hooks even when already loading.
   Cleanup preserves the new world's bootstrap data and buffered frames.
3. During `Loading`, `Update` polls bootstrap results and installs the validated
   session through ordinary commands. The end of the schedule applies them.
4. `FixedUpdate` replicates frames when `GameSession` exists, including during
   loading. Its end applies entity commands. A subsequent bootstrap update checks
   the applied `SessionKey` and requests `Running` for the next state transition.

Gameplay sets in `Update`, `PostUpdate`, `FixedPostUpdate`, the egui pass, and
`Last` use `in_state(Running)`. A disconnect observed in `PreUpdate` therefore
disables gameplay in that same application frame. Bootstrap completion can take
additional frames and waits for a fixed tick to apply its first frame. No ship
selection is required. Ordinary input-queue status does not affect the phase.

## Deferred work

| Work | Producer and consumer boundary |
| --- | --- |
| Appearance and navigation assets | Synchronize in `Last`; consumers see completion on the next frame. |
| Celestial definitions | Select systems in `PostUpdate`, load/synchronize in `Last`, evaluate poses in the next `Update`. |
| View cameras | Create inactive cameras in `Last`; the next `Update` assigns their viewport, pose, and active status. |
| Owned-ship visuals | Copy visual telemetry in `Last`; presentation uses it on the next frame. |
| Query refresh after mutation | Poll mutation completion in `Last`; query systems observe the revision in the next `Update`. |
| Surface textures | Gather demand in `PostUpdate`; poll and launch tasks in `Last`. |
| Sky snapshots | Accept GPU-ready snapshots in `Last`; geometry and sprites consume the same snapshot in the next `PostUpdate`. |
| Exposure meters, debris, tracers, slip emitters | Discover or create in `Last`; update during a subsequent frame. |
| Combat event clock | Record the rendered time in `Last`, after effects have consumed the previous time. |
| Editor weapon visuals | Discover in `Last`; new visuals reach normal transform and visibility processing on the next frame. |

Initial asynchronous discovery may involve several of these boundaries. Existing
poses and camera motion still update together each frame. Sky mesh swaps continue
to wait for GPU readiness so loading a replacement does not blank the sky.

## Server tick

Hardware reset preparation and initialization now run in `FixedPreUpdate`.
Facility initialization belongs to the same initialization set and only depends
on ship designs. Industry actions follow initialization; Bevy inserts the
synchronization needed to make initialized components available to those actions.

Production advances and commits delivery in `FixedUpdate`. Delivery consumes
reserved cargo, records the docked hull and berth relationship, and adjusts stored
mass as one action. The hull starts cold and dormant with its base inventory and
mass present. Its device entities and facility modules initialize in the next
`FixedPreUpdate`. The previous additional initialization pass inside production
completion has been removed. Tests verify that later ticks do not duplicate
delivery or change the paid inventory.

Independent impact application and computer-clock advancement run as a group.
Mass publication, dormant thermal updates, and sensor overrides share a group
after power accounting. Sensor publication to components and computer restart
cleanup are independent consumers of completed firmware execution.

## Retained dependencies

The audit covered runtime scheduling in the client, server, shared ship renderer,
shared UI, and ship editor, plus affected fixtures. Iterator `.chain()` calls are
unrelated to scheduling. Explicit `ApplyDeferred` calls decreased from eight in
production (three client, five industry) and three replication fixtures to zero.
Automatic synchronization remains enabled; removing a marker alone would not
remove a barrier generated by a real dependency.

| Dependency | Reason retained |
| --- | --- |
| Transport reception before session event consumption | Disconnect and replacement take effect before the current frame's state transition. |
| Interpolation, celestial evaluation, camera updates, scene presentation | Positions, camera origins, and relative transforms describe a consistent rendered frame. |
| Camera drag capture before controls; controls before camera animation | Controls consume the current capture decision and produce the pose being rendered. |
| egui context initialization, input capture, shell/console ordering, dispatch | Prevent UI input from leaking into gameplay and dispatch the actions produced by the UI. |
| Transform/camera propagation, visibility, render passes, GPU upload acknowledgement | Renderer prerequisites and exposure measurement require these boundaries. |
| Editor visual assets before preview setup | Preview setup requires the initialized resource. Other setup and presentation work is independent. |
| Industry policy, cancellation, cargo, admission, mass publication | Preserve ordered action validation, resource availability, and accounting. |
| Firmware gas allocation, execution, settlement | A tick must reserve and settle its actual computation budget. |
| Generation, device demand/allocation, utilities, production, cooling, power accounting | Preserve energy and thermal accounting within a tick. |
| Force preparation, collision integration, celestial integration, tick completion, observation/query publication | Preserve consistent simulation samples and publish completed results. |
| Collision solver, aerodynamic environment, display preparation/execution | These pipelines consume intermediate results produced by their preceding steps. |
| Bootstrap world publication | Initial indexes and observations must exist before scenario setup resolves handles. |

No global suppression of ambiguity reporting or automatic deferred synchronization
was introduced. Additional ordering should identify the invariant it protects;
discovery and initialization should normally use the existing schedule boundaries.

## Verification

- `cargo test -p osg-client --features ui --offline`: 185 passed, including native
  state transitions, materialized first-frame entities, pending activation
  interrupted by session events, and headless loading/error/Directory rendering.
- `cargo test -p osg-server --offline --lib`: 366 passed, 2 ignored. The final
  firmware scheduling adjustment also passed all 12 vessel tests.
- `cargo test -p osg-ship-view -p osg-ship-editor --offline`: 8 passed.
- `cargo check --workspace --all-targets --features osg-client/ui --offline`:
  passed, with existing server warnings. Formatting was checked for changed files;
  `git diff --check` passed.
- The software Vulkan `--render-regressions` capture completed successfully.
  Ship, camera-rebase, and transit screenshots were inspected. The run emitted
  Vulkan upload-overlap validation messages also present in older workspace
  captures; it is not a clean Vulkan validation run.

Local capture artifacts are in `/tmp/osg-session-captures` and
`/tmp/osg-scheduling-render`; the scene log is `/tmp/scheduling-render.log`.
