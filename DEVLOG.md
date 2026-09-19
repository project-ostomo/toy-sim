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

The spatial/optical piece was committed as `666a051`. Its interactive client
closed normally with exit status zero.

## 2026-09-19 — Piece 4: inhabited map and procedural systems

Work has started on approximately 3,000 systems across a roughly 250-light-year
region. The old core network is trunk-oriented, the sovereign LFS members have
more crosslinks, and neutral states remain distinct. Gate control remains a
physical defense problem. The ten authored systems retain their names and
useful corridor connections, with positions corrected to fit their politics.

The generation review already found two factual problems in the earlier draft:
its catalogue coordinates were Galactic Cartesian while the existing sky uses
ICRS, and generated orbital angles were in degrees while the solver consumes
radians. These are being corrected before import. The existing eccentric-orbit
velocity formula also needs a consistency fix. Catalogued multiple stars will
use a virtual barycenter and hierarchical Kepler orbits, rather than assigning
multiple stars to the same fixed origin.

The client map is being changed to cache topology/layout and cull to the visible
viewport. The firmware's small full-graph limits cannot handle thousands of
gates; its planner needs bounded sparse queries while keeping route decisions
in the ship computer. Protocol 20 adds a topology revision and public system
sovereignty/population metadata. No NPC behavior or industrial systems are being
implemented during this piece.

### Map and generation checks

The generated network contains 3,000 systems and 6,104 reciprocal gate pairs.
An all-pairs breadth-first check found a maximum shortest route of 32 gate hops,
so the existing 256-order queue remains sufficient. The allocation is 369 USE
systems, 2,111 systems in six sovereign LFS members, and 520 independent systems,
including 62 in permanently neutral Nova Partenia. Population figures are
procedural setting parameters. The imported astronomical positions use ICRS and
the importer reproduces its recorded data checksum.

The universe suite initially passed 19 tests, building and validating 44,163
bodies, including 48 virtual barycenters, in about 177 ms in the optimized
development profile. Further review added planetary Roche clearance around
compact stars and shared ages for generated binary components; the final suite
will cover those changes too. Binary illumination and atmosphere selection also
need adjustments to assumptions inherited from the single-star renderer.

All 97 client tests pass at the map checkpoint. Headless interactions searched
for the last of 3,000 systems and submitted its route with the chosen fuel
priority. Cached map drawing took 0.165 ms median and 0.355 ms at the 95th
percentile; zoom reduced the emitted geometry from 6,027 to 429 shapes. These
measurements cover egui drawing rather than total GPU frame time.

Integration exposed a quadratic gate-pair search in the collision collector.
That collector now resolves paired identities directly and considers active
systems for physical entry mouths. Remote destinations remain globally known.
The full map continues to travel in state snapshots; actual serialization and
2 MiB-window compression costs are being measured before making any further
transport decision.

### Measured snapshot and firmware problems

The full-map snapshot benchmark changed the transport decision. With 12,208
gate mouths, a frame is 2,843,528 bytes. Zstd level 3 with the specified 2 MiB
window still emits about 1,034,761 bytes per steady frame, roughly 10.35 MB/s per
client at 10 Hz. Compression alone takes 11.18 ms per frame. This exceeds the
lookbehind window and defeats the intended removal of repeated map data.

The stable public navigation catalogue is therefore becoming a content-hashed
asset on the existing asset stream. Main snapshots carry its hash and current
beacons relevant to focused systems and queued orders. Catalogue reference
positions support map layout and gate-hop previews; the flight computer queries
current server facts for physical routing. The normal state stream keeps sending
complete relevant samples. Protocol 20's new navigation asset format validates
version, size, unique identities, system references, and reciprocal gates. Its
13 protocol tests pass.

This also revealed that the server's asset collection had been frozen at startup.
It now uses a shared store so newly published catalogues and appearances can be
served. A transfer takes an immutable reference to its bytes and releases the
store lock before doing network I/O.

The actual stock WASM tests caught problems that native planner tests missed:
growing a large vector and initializing an entire graph in one callback could
exhaust the instruction allowance. Bounded allocation/initialization and cached
idle-time catalogue loading are being checked against the real VM. Wide binary
systems also require care when placing gate rings: a distant circumbinary orbit
can sit outside the old activation bound. Stable circumstellar hosts are used
where suitable, and infrastructure is included in activation coverage.

### Final generation and routing results

The final universe suite passes all 20 tests. Release measurements: map
construction 26.96 ms, generation and validation 84.93 ms, and solver/index
construction 32.77 ms. There are 44,346 bodies: 44,298 physical objects and
48 virtual barycenters. The 13,641 atmospheres retain finite renderable radii;
their modeled heights range from about 2 km to 4,167 km. Climate and stellar
properties are explicit procedural estimates.

After the static map asset change, the measured main frame is 4,135 bytes and
steady compression is about 318 bytes per frame; compression takes 0.03 ms.
The first compressed frame is 2,309 bytes. The catalogue itself downloads once
as an asset. Shared mutable asset serving passes all five existing tests,
including 128 concurrent transfers with a stalled request and a 17 MiB asset
inserted after serving started.

The actual 3,000-system Helion-to-Terminus route now finishes in 5.8 seconds when
the idle computer has prefetched its catalogue, down from 29 seconds. Fully cold
planning takes 32.7 seconds and reports loading, graph construction, and search
progress. Repeated routes reuse topology without paging it again. The final
firmware tests pass: 31 native controller tests, three actual WASM routing tests,
and 16 sandbox/ABI tests. Peak combined instruction and syscall use is 960,262
gas per tick; peak linear memory is 6.19 MiB within the 8 MiB hardware limit.

The route search keeps transfers from a staging point to every local gate.
Pruning uses an optimistic time/fuel bound, including an asymmetric regression
where the best next gate is not the staging point's own gate. This avoids
removing useful routes just to make the default journey fast.

Server optimization now avoids checking every remote orbit as though its
configuration changed each tick. Only rigid orbital groups with disjoint
conservative envelopes are certified; uncertified or modified mouths retain
ordinary ongoing exclusion checks. The remaining custom aperture tree has been
replaced by the shared hash, with bounded visits and a conservative far-mass
curvature allowance. Public orbital gate facts are cached, with exact current
poses evaluated when queried. Full server checks and the quiet performance
measurement are next.

The first combined client suite passed 105 tests and found one outdated binary
fixture that violated the new two-component barycenter validation. That fixture
now uses the real generator. The services cache test similarly needed a fully
provisioned scene; its fixture has been corrected. These are included in the
final incremental build together with the latest firmware.

### Server verification and live map checks

The complete server library suite passes: 238 tests, with five explicitly ignored
benchmarks. The client suite passes 106 tests; model and protocol pass 12 and 13.
The quiet full-map profile measures 9.70 ms mean and 11.04 ms maximum per tick,
0.14 ms for optical rebuilding, and 4.81 ms for publication and encoding. These
figures cover the three starting ships and all 12,208 gate mouths. They do not
resolve the earlier large moving-fleet scaling limitations.

Live keyboard and mouse testing found Terminus in the full catalogue, selected
it, and produced a four-order route: the Sol gate, an exclusion-zone transfer,
a slip to Terminus, and a final transfer. The HUD displays all four orders and
their estimated arrival times. The map displays the slip segment and an estimated
33.7 t propulsion-fuel budget against 53.4 t aboard. Panning and scrolling work.
Screenshots are in `/tmp/toy-sequential-playtests/map-*.png`.

The live run also exposed a map-height feedback bug: allocating the canvas from
remaining height before drawing a larger-than-reserved footer made the window
grow every frame. A stable layout and repeated-frame regression are being added.
A separate review found indefinitely cached orbital positions in the stock
computer; selected route legs are being refreshed before publication. A changed
slip eligibility regression exposed excessive search time, so the planner will
return its best validated feasible route after a bounded search budget. Its
time/fuel estimates remain approximations, rather than claims of global optimality.

The user's enforcement rule applies throughout subsequent work: law and
territorial policy are enforced by fallible in-world actors. Credentials,
equipment, and physical proximity may constrain operations directly. Gate transit
will have no administrative permission check.

### Failures caught by continuing the actual journey

The four network integration tests pass, including the authenticated map asset
and process restart. All 107 client tests pass after the stable footer layout,
including 120 frames of map search appearing and disappearing.

The live route reached Sol, but further observation caught an automatic return
through the aperture and a later firmware fault. A new server regression
reproduced the return only 3.4 simulated seconds after the first crossing. The
selected exclusion-zone staging point was on the other side of the mouth, so
guidance aimed back through it. Work continues on physical departure guidance;
gate immunity or transit permissions are not being introduced.

The arrival whiteout has an independent rendering cause. The ship emerges just
outside the mouth, but its trailing orbit camera can still lie inside the
220 m visual sphere. The old shader drew its opaque glowing interior across
the screen. The gate shader now omits that interior surface, and the unchanged
slip tunnel still renders from inside. The fragment passes Naga validation;
live verification will follow the flight fix.

The updated real-VM route checks pass five cases, including a staging point
that moves 20 km and a slip point that becomes unavailable. A limited search
result is marked explicitly in the AP display. In the changed-eligibility case,
one rejected candidate is refreshed and retried, then a valid alternative is
returned. This does not establish global optimality of the mixed-route graph.

### Navigation correction before committing the map

The second fault was measured, rather than inferred: navigation telemetry
contained throttle `1.0000000000000002`. The actual actuator path already clamps
its controls, but the display publication did not. The firmware correction clamps
the normalized alignment used for telemetry. Departure guidance now chooses the
actual outward arrival side and resets reused target estimators across host order
changes. The native controller tests pass; the full server continuation check is
next.

Longer journeys also reveal a stale-destination problem: a slip endpoint frozen
at initial planning can lag a moving gate by tens of megametres after the
preceding local transfer and charge. The route's slip destination is being
changed to the same typed coordinate/beacon/body-relative destination used by
other navigation orders. The computer can resolve public orbital ephemerides at
a future time, refresh the lead while charging, and submit a concrete endpoint.
Once physical transit starts, its endpoints stay fixed. Public moving beacons
without orbital ephemerides retain an explicitly approximate linear forecast.

A read-only review prepared the next surface-generation piece. Existing drafts
have reusable terrain, crater, and mip-generation code, but their normal-map
direction, polar sampling, aliasing, cancellation, and memory accounting need
correction. Surface implementation has not started while these map/travel
failures remain under verification.

### Future arrival and remote route data

The real VM now preserves typed slip destinations, predicts their public orbital
position at arrival, and refreshes that lead while charging. The ABI26 firmware
passes 37 native controller tests, six real WASM routing tests, and 16 sandbox
tests. The physical gate regression now continues outward for 400 ticks after
arrival without returning through the mouth or faulting.

Remote celestial waypoints carry public ephemeris asset references scoped to the
actual focused view. They reuse the normal solver and asset cache without adding
remote systems to the camera subscription. The integrated server regression and
two registry checks pass; client lifecycle checks are included in the next build.

A charge-completion regression exposed an old integer-accounting mismatch:
floating-point required energy could exceed its integer joule value by a tiny
fraction, leaving a practically endless stochastic final payment. Slip charge
requirements now round to whole joules when computed. The final moving-mouth
arrival test and a new live journey remain before the map milestone is committed.

### Live Sol arrival verified

The final client/model/protocol suites pass 112, 12, and 15 tests respectively.
The server run passed 244 cases and exposed one incorrect gate-name lookup in
the new moving-arrival test. That fixture is corrected: all 17 services tests
and 16 travel tests now pass, including arrival beside a real orbiting mouth
after a 13-second charging power interruption. A rejected exit forecast no
longer debits energy without recording work. Four network integration tests pass.

A fresh live world successfully searches Terminus in the 3,000-system map,
queues a gate/transfer/slip/final-transfer route, and crosses from Helion to Sol.
The map stays at its intended height, the Sol arrival has no gate-interior
whiteout, and outward guidance continues. The long local transfer is running
under accelerated debug time to check the full slip journey. Screenshots are
`/tmp/toy-sequential-playtests/map-final-*.png`.

This run exposed clipped long off-screen waypoint labels. Their measured text
bounds are being constrained to the view without moving the directional pointer.
Startup window resizing still produces Vulkan presentation-layout warnings;
this run has not produced a device loss. Display performance will need separate
measurement once accelerated simulation and concurrent builds have stopped.

### Accelerated-time input backpressure

The accelerated journey uncovered a real connection failure: a debug batch of
100 simulation ticks could keep the main loop busy long enough for the ordinary
client's empty input frames to fill its 16-slot channel. The network task treated
that temporary fullness as excessive input and closed the connection. The journey
is paused and saved during the Sol transfer while the correction is rebuilt.

Idle clients now send no empty input frames. The server awaits room in its
existing bounded input channel, allowing transport backpressure while the
independent outgoing task continues publishing. A real picomux regression fills
the queue with 64 inputs, verifies outgoing progress, and drains every input in
order. The client regression verifies 100 idle flushes and preservation of real
commands under local backpressure. Both pass. No acknowledgment protocol or new
unbounded input queue has been introduced.

### Map milestone: full journey completed

The same saved patrol completed the entire normal flight-computer route:
Helion to Sol through a physical gate, outward transfer beyond the exclusion
zone, 101 ly of slip travel to Terminus, and the final sublight rendezvous. The
HUD reports `Route complete`. About 24.8 t of the initial 54 t propulsion fuel
remains. No computer fault or connection drop occurred after the backpressure
fix. The live restart preserved the route, location, and inventories. The ship
was returned to normal simulation rate before shutdown.

The final screenshots include `map-final-slip-transit.png`,
`map-final-terminus-arrival.png`, and `map-final-route-completed.png`. The slip
visual and ETA worked, remote scenery cleared during transit, and the destination
system loaded on arrival. Normal-rate display measured roughly 60–70 FPS in this
particular 1600×1000 logical window on a scaled display; this is not a general
120 FPS claim. Existing resize-related Vulkan validation warnings remain a
separate rendering issue.

All targeted checks are passing. The map milestone includes the 3,000-system
catalogue, 6,104 gate pairs, deterministic stellar and planetary parameters for
catalogue systems, public map assets, bounded stock-computer route search,
future arrival guidance, and route-only ephemerides. Eight retained named system
files are still old star-only placeholders; the upcoming procedural surface
piece will populate those explicitly and add physical appearance metadata for
authored planets. Large-fleet spatial performance remains on the integration list.
