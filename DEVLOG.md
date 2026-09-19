# Toy Sim MVP development log

This is the running record of the autonomous MVP sprint. Entries distinguish
implementation, verification, and remaining work. A feature is complete only
after relevant automated checks, an interactive playtest, and a commit.

## 2026-09-18 — Scope and decisions

The requested end state is a playable, persistent single-shard space MMO
prototype with political ownership, inhabited space, procedural planets,
ship computers, missiles, industry, local chat, and lore-driven NPC activity.
The user subsequently requested sequential delivery: finish and playtest one
piece before beginning the next. Subagents may collaborate within that piece.

Agreed constraints:

- Real OpenRouter use has a total $100 testing budget, including across restarts.
  No paid model calls have been made by this sequential implementation yet.
- SQLite will hold transactional world snapshots. Structured metadata and blobs
  are preferred over a large ORM. Snapshot pauses or scheduled downtime are
  acceptable. WASM bytes and explicit durable program state must survive;
  native execution stacks may restart after loading.
- The game calendar is real UTC plus exactly 400 Gregorian years. The setting is
  2426; older dates in the worldbuilding material do not override this.
- Ownership follows Sovereignty > Organization > Player. The LFS consists of
  sovereign members, rather than being one sovereign faction. Advertised IFF
  remains distinct from actual ownership and control.
- Optical visibility uses actual changing brightness, including illumination,
  planetary shadow, engines, and shields. Sensor intelligence and IFF are
  distinct from whether a mesh can be seen optically.
- Cargo transfers require co-located inventories and permission. Industry can be
  managed remotely. Initial mines can create raw materials at a fixed rate;
  detailed planetary extraction is deferred.
- Gas exhaustion must suspend a computer and continue it on a later tick.
  Invalid program operations still fault and reboot. Missile computers survive
  the destruction of their launching ship.
- Live keyboard and mouse playtesting is explicitly authorized for this sprint.
- At least 100 NPC organizations must exist at launch. Each needs detailed lore
  consistent with the shared history, sovereign politics, economic roles, and
  relationships. Their assets, objectives, behavior, and dialogue must reflect
  that lore. This is a required feature, not optional flavor text.
- Gates have no transit permissions. Control of a gate must be enforced by
  physical ships, turrets, missiles, or drones. Long-range missile/drone defense
  is a core requirement for installations. Docking and cargo permissions remain.
- Laws and sovereignty are enforced by fallible in-game actors. The server must
  not make territorial entry, criminal conduct, or other legal prohibitions
  impossible simply because the law says so. Patrols, inspection, interception,
  combat, and consequences provide enforcement. Hard checks represent physics
  or actual technical access controls, such as computer credentials and locked
  storage; capture and theft can change possession of that access.

The implementation order is ownership; persistence/calendar; spatial indexing
and optical visibility; inhabited map and procedural stars/planets; planetary
surfaces; resumable WASM and account gas; missiles; industry/logistics; chat,
LLM integration and NPC populations; then whole-game journeys and balancing.

## 2026-09-18 — Baseline and workspace isolation

Committed the existing work as `6847e53`:
“Complete navigation, ship HUD, resource accounting and slip motion updates.”
This preserves the starting point before the MVP changes.

An initial parallel implementation was interrupted by the instruction to work
sequentially. Its unverified scaffolding was preserved in Git stash commit
`3df0e421275d1ec71105425161a5c333944a2c4f`. Some earlier workers remained active
but were no longer reachable from the current agent control tree. To keep their
unfinished edits out of the sequential build, created branch `mvp-sequential`
in `/tmp/toy-sim-mvp-sequential` from the baseline. The original checkout and
stash have been preserved. Later features can recover individual useful changes
after review; the entire scaffold is not being treated as a delivered feature.

The repository documentation policy normally delegates English prose to Claude
Code. Claude reported its monthly spending limit in
`/tmp/toy-sim-docs-result.log`, enabling the policy's explicit Codex fallback.

## 2026-09-19 01:29 UTC — Piece 1: ownership and standings

Implemented so far:

- Sovereignties, organizations, player membership, officers, asset ownership,
  access grants, and friendly/neutral/hostile standing inheritance.
- Initial political directory with USE, League member states, independent
  states, Unifleet Station Services, Unifleet Defense, St Raphael Trade
  Confraternity, Helion Flight Cooperative, and Terminus Privateers.
- A Society client window for inspecting affiliations, overriding personal
  standings, creating organizations, managing membership/officers, and editing
  asset permissions. The toolbar uses a shield icon below Map.
- Overview and HUD colors derived from advertised IFF. Capturing or transferring
  a ship does not silently rewrite its advertised identity.
- Explicit View, Control, Configure, TransferCargo, Industry, and ManageAccess
  permissions. Organization creation makes its founder an officer. A last
  officer cannot accidentally abandon an organization without a replacement.
- Protocol version 17 includes society state and commands. Other players'
  private standing overrides are filtered out of each client's snapshot.

Verification and fixes before playtesting:

- Seven initial model/server ownership tests passed.
- The first workspace library test run reached the client with 80 passing tests
  and two new headless UI failures. The failures were unused egui texture deltas
  in the test harness; both targeted tests pass after consuming those deltas.
- Review found ship command dispatch still required flight control before
  checking a command's specific permission. Configuration and cargo operations
  now use their own permission, while navigation/combat use Control. Revision
  checks remain common. Added a regression for Configure-only and Control-only
  delegates, stale revisions, View-only observation, and access revocation.
- Added a real two-client network test covering organization creation, private
  standings, ownership transfer while preserving IFF, delegated control,
  configuration denial, access-management denial, and revocation.
- That test initially failed to compile because network frames are `Arc<Frame>`;
  corrected the helper's return type. The full rerun is pending.
- Shared Cargo builds were delayed by the earlier workers. A private
  copy-on-write build cache is being prepared to isolate test binaries and
  eliminate that contention.

Current status: implementation under verification. Interactive playtesting and
the ownership commit are still pending. Later MVP pieces have not started in
the sequential worktree.

### Verification update

The isolated build confirms the broad WASM import failures came from stale
shared-cache artifacts. After cleaning workspace packages in the private cache,
the server library run has 192 passing tests and three failing display fixture
tests (plus three ignored tests). Those fixtures have now been updated to carry
real ownership and to test capture revocation. The client library's 82 tests pass.

The ownership review also found older private docking and automatic station
service checks based on a ship's historical controller account. Those checks
could preserve access after ownership changed. Station access is being migrated
to current asset ownership and explicit Dock/cargo permissions. The user clarified
that gates must have no transit authorization check at all: they are physical
passages, to be defended physically. The tentative Transit permission is being
removed together with existing gate access restrictions before this feature is
committed.

### Automated checkpoint

The clean workspace library run now passes: **380 passed, 0 failed, 3 ignored**
across 15 suites. This includes private docking/resupply revocation, public gate
passage, and display access changes. The real-network ownership test initially
tried to issue a throttle command before the computer finished booting. It now
subscribes to the delegated ship's instruments and waits for the authoritative
Running status before testing control. Network verification and interactive
playtesting are still in progress.

### Interactive ownership playtest

Ran the native client against its local server and used actual keyboard/mouse
input. Captures are in `/tmp/toy-sequential-playtests/`.

- The initial patrol is red/hostile; Neris Anchorage is blue/friendly.
- Opened Society, searched for Terminus, and selected Terminus Privateers.
  Changing the personal override to Friendly changed the Overview patrol row to
  blue. Restoring inheritance and creating a new organization updated the
  effective relationship through the new hierarchy.
- Created Playtest Cooperative by typing its name and clicking Create. The
  directory and player breadcrumb updated, and the founder is shown as an officer.
- Enabled public View on the patrol ship, applied it, then reloaded the policy
  from the server. The checkbox remained checked after authoritative reload.
- Attempting to leave as the only officer produces the explicit response
  “appoint another officer before leaving” and retains membership.
- The transfer recipient list exposed irrelevant destinations and was awkward
  to navigate over the bottom HUD in this test. A focused fix is restricting
  ownership-transfer choices to eligible principals; permission grants retain
  the broader list. A headless popup interaction check is being added to
  distinguish application behavior from the virtual input device's wheel path.

The native renderer produced two swapchain validation messages during initial
window resizing. It continued rendering throughout the session without device
loss or disconnection. These existing renderer diagnostics are recorded for the
later performance/stability pass; ownership changes do not alter render passes.
The default bottom console also scrolls its final reserve row at this window/UI
scale, which merits checking in the final HUD pass.

### Ownership completed

The real-network suite passes all three tests. The final five Society UI tests
also pass, including native egui popup scrolling, eligible transfer recipients,
and authoritative access-policy refresh. The popup scrolling problem observed
with virtual desktop input was not reproduced in the headless interaction test.

A second interactive session created Playtest Cooperative, enabled public View,
and transferred the patrol to the organization. The owner breadcrumb changed to
the organization, the server accepted the transfer, and the public View checkbox
cleared with the transferred asset's reset policy. The founder retained control
through organization authority. Both playtest processes were closed cleanly.

Evidence: `/tmp/toy-sequential-playtests/ownership-transfer-complete.png`,
`/tmp/sequential-ownership-unit5.log`, `/tmp/sequential-ownership-network7.log`,
and `/tmp/sequential-society-popup.log`. The feature is ready to commit.

Next piece: SQLite snapshots, restart recovery, and the real UTC plus 400 years
calendar. Later feature work remains deferred until that piece is verified.

## Persistence and calendar

Ownership and standings committed as `c29da36`. Work is confined to persistence
and the calendar before any later feature integration.

- SQLite stores versioned world blobs with a manifest and checksums, committing
  all records in one transaction. The default interval is 15 minutes, with an
  initial checkpoint and a final checkpoint on graceful shutdown. Three complete
  generations are retained. Corrupt or unsupported newest snapshots fail startup
  explicitly; rollback is an operator decision.
- Capture happens at the simulation boundary; SQLite disk work runs on one
  background worker. Capture and write durations are logged separately.
- Saved identities, ownership/access policy, ship state, resources, motion,
  docking, travel, and WASM programs are being covered by restart tests. Guest
  programs receive a bounded explicit persistent-data API. Native VM execution
  stacks restart after loading a world.
- Debug launches will retain their account and world by default under
  `$XDG_STATE_HOME/toy-sim/debug` or `~/.local/state/toy-sim/debug`. Use
  `--state-dir PATH` for a separate world, or `--ephemeral` for a disposable run.
- The calendar is real UTC plus exactly 146097 days (400 Gregorian years). It
  advances while simulation time is paused or accelerated. The client uses the
  server's calendar samples independently of the interpolation jitter buffer.
  Simulation T+ remains available in the calendar tooltip.

Six targeted calendar tests pass: leap days, century boundaries, weekday/date
formatting, signed millisecond protocol round-trip, pause/speed independence,
and delayed snapshot handling. Full restart verification remains in progress.

### Recovery review

The review found and corrected several concrete recovery errors before live
playtesting: restore depended on the original `--ship` source still existing; a
missing debug identity could silently generate new credentials for an existing
world; identity replacement needed a parent-directory sync; and saved hardware
needed the separate reactor/NTR thermal reservoirs. The launcher now refuses an
incomplete saved identity and tightens secret-file permissions. Startup locks and
loads the checkpoint before choosing a bootstrap design.

Normal server recovery retains the host navigation queue and its current stage.
The existing invalid-access fault path still resets navigation. VM stacks restart
on recovery, while programs can restore their explicitly saved bytes.

Ten storage/world tests pass, covering transaction failure, checksums, format
errors, retention, exclusive locking, asynchronous write failure, and ship/docking/
transit/program state. The calendar also now converges after small clock offsets
and adopts larger authoritative corrections; all three focused correction tests
pass. Native process restart verification is running next.

### Persistence completed

Verification finished successfully:

- All four real-network tests pass, including a process restart with exact paused
  tick/time, pose, inventory, battery/heat, IFF, group secret, authority revision,
  ownership, standing overrides and access grants restored. The calendar advances
  across downtime. The restarted computer reaches Running and simulation resumes.
- Three world tests cover docking and wreck containment, thermal/program state,
  rejected corrupt references/program hashes, and continued slip transit with
  queued navigation orders. Nine storage/lifecycle tests cover atomic commits,
  corruption, version checks, retention, locks and failure propagation.
- All 16 WASM sandbox tests pass under ABI 24, including durable writes surviving
  reboot/checkpoint and failed or out-of-bounds callbacks not committing writes.
  Bundled firmware, C/Rust fixtures and generated bindings were rebuilt.
- Both debug identity tests pass. Date/protocol tests and the three final client
  clock correction tests pass. Server and native debug client builds succeed.

The first restart test attempted to reprogram the old faction after joining a
new organization; the server correctly rejected that credential claim. The test
now advertises the authorized new organization. Its final computer check also
needed to wait through the normal five-second cold boot instead of asserting at
two seconds. No production behavior was weakened to satisfy either test.

Interactive verification used actual keyboard/mouse input. Created Checkpoint
Cooperative, requested a live SIGUSR1 checkpoint, closed the client normally,
and moved the original ship source out of the way. Reopened using the same
`--state-dir` and even the same now-missing `--ship` argument. Pilot identity and
organization were unchanged, the calendar showed 2426, and the scene resumed.
Clicking the thrust gauge produced 24% throttle and about 599 kN; the ship moved
and consumed charges. Set throttle back to zero and closed normally again.
Both GUI launches exited with status zero.

Captures: `persistence-organization-created.png`, `persistence-reopened.png`,
`persistence-resumed-control.png` under `/tmp/toy-sequential-playtests/`. The saved
playtest world remains in that directory for inspection. Manual save created
generation 2, normal close generation 3, and later launches retained exactly the
latest three generations. The missing-source CLI check also passed.

For the initial small scenario, capture was approximately 0.9–1.1 ms for 329 KB;
SQLite writes were approximately 4–10 ms on their worker. First physics updates
still incur about 220 ms of cold cache work. Native rendering remained around
47–55 FPS at the tested large scaled window, with the previously recorded initial
resize validation messages. These remain for the rendering/performance pass.

Next: unify spatial queries and implement optical replication and distant ship
glints, then verify that piece before expanding the inhabited map.

## 2026-09-19 — Piece 3: spatial queries and optical rendering

Persistence and calendar work was committed as `49e7749`. The spatial piece is
being integrated and verified before map expansion begins.

The common `toy-sim-spatial` crate now serves stellar visibility, sensor and
optical queries, intelligence query cursors, travel exclusion geometry, and
collision broad phase. It uses integer galactic coordinates, compressed occupied
cells and luminosity buckets. Parry still handles detailed shape collision and
CCD. Tests include extreme signed coordinates, conservative sweeps, randomized
brute-force comparisons, stable ordering and bounded cursor work.

Protocol 19 adds a required optical observation section. Radio tracks remain
tactical information; a focused physical vantage authorizes optical geometry.
Observations carry opaque session identities, optional authenticated associations,
pose, radius, apparent-direction luminosity and engine/shield/turret visuals.
The server recalculates illumination, planetary shadow, reflection phase, engine
emission and shield glow. Publication has per-view count and serialized-byte
budgets, preserving the focused ship first. The client uses separate optical ECS
entities, interpolates brightness, and transitions small projected meshes to one
batched sprite mesh per view.

Early verification found and corrected several real issues:

- The initial moving-fleet hash wrapper regressed against the old collision
  index. Measurements with moving poses exposed this; the old benchmark reused
  identical poses. In-place leaf updates and integer frame anchoring are being
  measured before accepting the replacement.
- Main-engine emission needed the scalar actual thrust field; the vector field
  represents RCS thrust.
- A restored paused world needs its optical index rebuilt before its first
  snapshot, without advancing the simulation or discarding saved tracks.
- Completing an intelligence query could destroy an old retained index inside
  the script syscall. Releasing these snapshots now happens in existing tick
  maintenance. Query creation only takes an Arc and prepares bounded cursors.

The million-star catalogue test loads all entries in 2.178 seconds and averages
2.096 ms across 500 visibility queries; whole-process peak memory is about
575 MiB, including catalogue data and identity/index storage. A magnitude-six
query returned 6,373 stars after checking 17,856 candidates. These are local
optimized-development measurements, not capacity guarantees.

At this checkpoint, the initial server suite had 226 passing tests and one new
fixture error (missing mandatory firmware), now corrected. The initial client
suite had 92 passing tests and one new photometry test exercising the intentional
brightness clamp rather than the unsaturated inverse-square range. Final tests,
interactive rendering verification and the commit are still pending.

### Spatial/optical verification completed

Final checks pass: 229 server library tests, 92 client library tests, 14 spatial
core tests, six intelligence tests, six star tests, 12 model tests, 11 protocol
tests and all four real-network tests. The network test downloads a focused
ship appearance through its optical observation and confirms that a radio-only
view can receive shared tracks without receiving optical geometry or appearance
hashes. Formatting and whitespace checks pass.

Interactive checks used the actual client: selected the hostile patrol, used
Look at to inspect its mesh and active RCS, zoomed out until both ships became
small glowing points, dragged the orbit camera and toggled orbit overlays, then
returned to the own-ship close view. Mark and Fire were accepted, battery charge
fell, and Hold fire stopped the latch. No shader compilation error, panic or
connection failure occurred during this run. Captures include
`optical-hostile-close.png`, `optical-glint-far.png`,
`optical-glint-orbited.png`, and `optical-combat-restored-mesh.png` in
`/tmp/toy-sequential-playtests/`. The first attempt to drive wheel input used a
legacy virtual device; a device exposing high-resolution wheel events worked.

The large collision benchmark preserves exact outcomes, including all 1,000
slug impacts. It also exposes an unresolved scale cost: the moving 100k-body
broad phase takes about 122–204 ms versus 38–99 ms for the previous BVH, using
eight Rayon threads in the development profile. The standard reset-pose
benchmark takes 90–266 ms total depending on scenario. Fewer false candidates
and lower peak memory do not cancel this CPU regression. The small scene is
fast, but this implementation does not yet demonstrate 100k-body 10 Hz capacity.
This remains an explicit performance task for the final MVP load/playability
pass; the feature must not be described as production-scale performance.

A dense lighting benchmark with 20,040 objects takes about 89.5 ms to rebuild
illumination and 145 ms for 500 observer queries in aggregate. Rebuilding every
active object and serial per-session publication are further scale costs to
address. Live rendering in the tested scaled window remains roughly 41–50 FPS;
CPU-heavy concurrent test runs were excluded from that observation.

Next piece: generate the inhabited political map and deterministic stellar and
planetary parameters, then verify navigation and saved-world integration before
working on planetary surfaces.
