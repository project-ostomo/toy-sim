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

## Procedural planetary surfaces

Map and travel milestone committed as `fab4a0a`. The next piece separates the
CPU texture generator, client residency/job management, and authored environment
data. Existing drafts are being reused selectively after review. The generator
uses angular band limits, corrected polar sampling and tangent normals, linear
albedo mip filtering, and normalized normal-map mips. The client budget includes
running and canceled-but-unfinished work, shared image ownership, and replacement
overlap; eviction must actually detach textures from materials.

The authored-data subtask already passes 23 universe tests. All eight bundled
star-only placeholders now receive deterministic planets, bringing the map to
44,467 bodies, including 48 virtual barycenters. Their original stellar bodies
and ephemerides remain identical; arbitrary custom empty systems stay empty.
All existing Sol/Helion masses, sizes, orbits, rotations, and identities are
preserved. Earth has an explicit atmosphere and biosphere; Neris has an explicit
ocean/climate description without a declared biosphere. Airless moons remain
airless. Geology and weather use independent random domains. Immutable definition
hashes change, so older development checkpoints are rejected explicitly.

The full universe suite now passes 31 tests. New checks cover longitude/pole
continuity, analytic tangent-normal slopes, linear-light mip filtering, declared
ocean/cloud coverage, explicit biospheres, cancellation, and exact payload sizes.
An independent client review found no blocking lifecycle or scheduling issue.

The six-body CPU preview is `/tmp/sequential-planet-surfaces.png`: Earth, Mars,
Rime, Jupiter, Neptune, and Neris. A quiet release benchmark measured an Earth
surface at about 23 ms for the first 256-wide texture and 1.88 s for the final
2,048-wide texture. Jupiter measured 9.5 ms and 815 ms respectively. The largest
Earth payload is 34.67 MiB; baking adds an 8 MiB height field plus a small reserve.
These bakes run on background workers. Client screenshots and normal controls
are being checked next; this preview alone is not an in-game rendering test.

### Surface milestone verified

Software-rendered screenshots confirmed progressive loading, a close Neris view,
camera rotation across the day/night boundary, and returning to the ship. The
managed cache counter reached about 100 MiB during an upgrade, below its 128 MiB
limit. The first images exposed featureless white cloud patches; a focused
density/envelope correction added translucent wisps and internal variation while
keeping declared coverage calibrated. It adds no new texture or shader pass.

The desktop playtest then selected Neris and the airless moon Rime through the
Overview and returned to the ship with Escape. Terrain, oceans, clouds, craters,
and ice are visible. Screenshots are
`/tmp/toy-sequential-playtests/surfaces-live-*.png`; the earlier software checks
are in `/tmp/toy-sequential-surface-checks/`. The normal renderer showed about
47–73 FPS across these views, so the broader performance objective remains open.
Startup resizing still logs Vulkan swapchain validation warnings, without a
device loss during this run. Cloud shells currently do not cast cloud shadows.

Verification passed: 121 client tests, 247 server tests with five intentionally
ignored benchmarks, four real network tests including checkpoint restart, and
the universe/surface tests. The final cloud change has 11 targeted surface tests,
including density variation and LOD stability. Workspace formatting and diff
checks pass. Final quiet release bakes measured Earth at 23.5 ms for 256 pixels
and 1.97 s for 2,048 pixels; Jupiter at 9.4 ms and 814 ms. The playable client
closed cleanly after the live check.

## Resumable computers and global gas

Planetary surfaces committed as `b33ceac`. The final full universe run passed
34 tests. Work has moved to execution budgeting and owner-wide accounts.

The runtime review found that Wasmtime's periodic async yielding does not make
fuel exhaustion resumable: exhausted total fuel still traps. An old draft used
a huge fuel tank, estimated yield counts, and instruction debt. That cannot
guarantee prepaid account spending, so it is not being adopted. An isolated Wasmi
prototype already demonstrates preserved local state and a deferred host call
executing exactly once. Its portable interpreter is being compared with an
instrumented JIT alternative before choosing the production runtime.

The ledger uses tagged player, organization, and sovereignty principals. Actual
asset ownership selects the payer; advertised IFF cannot redirect a bill. A
checked integer reservation is made before execution and settled against actual
usage. Parallel computers share low balances fairly, including rotating integer
remainders. Checkpoint capture rejects live reservations rather than inventing
refunds for work that may already have happened. Flight work settles at its own
barrier; requested MFD work later spends only the remaining physical capacity
for that ship and tick and settles before publication returns.

The UI contract separates READY, SUSPENDED, and NO GAS from actual faults and
reboot progress. Commands can still queue while an account is empty. The Society
window receives only personal balances and pooled accounts the caller administers.

Review also found an unrelated but concrete violation of the requested gameplay
principle: the last officer could not leave an organization. That restriction is
removed. An organization may become administratively abandoned; it keeps its
assets and gas, and the former officer loses management credentials. Physical
and credential limits remain enforceable; legal obligations are for in-game
institutions and actors to enforce.

Native-call admission is also being audited. Track queries must reserve their
copying overhead as well as query work, and guest output buffers must be validated
before cursor mutation. A sensor-history eviction path repeatedly searched every
incoming contact for every possible eviction; it now builds one ID set and sorts
eviction candidates once. The existing deterministic age/ID eviction rule remains.

### Runtime comparison and admission details

The completed interpreter comparison used the actual bundled computer and the
3,000-system map. Wasmi's portable dispatch took about 120 microseconds for an
idle callback versus 7.7 microseconds in Wasmtime. Cold routing used 550 ms of
aggregate CPU versus 15.6 ms; warm routing used 111 ms versus 3.0 ms. The default
interpreter dispatch also overflowed a normal worker stack, while portable
dispatch passed the resumption tests. These results favor retaining the JIT if
its explicit metering prototype passes correctness and workload checks.

The JIT prototype injects a private prepaid meter through structured WASM
rewriting. It has already preserved recursive locals and bulk memory work across
ten suspensions. Guest modules cannot import its reserved host interface or
address the appended private state through their original indices. Production
integration is still pending the benchmark and remaining review.

A ledger review caught a starvation case before integration: dividing one
million available gas equally among three computers waiting for indivisible
700,000-gas calls would stall all three forever. The allocator is being extended
to admit complete minimum-sized execution steps, with rotating fairness, before
sharing the remaining budget. It must allow one affordable call to proceed.

ABI 27 removes the old saved reserve and separate instruction counter. Its
budget record contains current remaining gas, the granted slice, and the ship's
physical per-tick limit. The bundled computer now sizes optional catalogue
queries from that allowance. In-flight request batches stay stable across a
suspension; later commands queue for the next callback. Read-only observations
and scene access can refresh when execution resumes.

### JIT selected; integration checks

The instrumented JIT prototype passed its execution and index-remapping checks.
Idle and tracking callbacks took about 18.7 and 21.5 microseconds, versus 7.5 and
9.0 microseconds with the old runtime. That comparison still had the old fuel
meter enabled as well; production uses only the explicit prepaid meter. The
production rewrite has 11 focused passing tests, including bulk memory pricing,
recursive locals, indirect calls, original-global isolation, reserved names, and
bounded loop segments. It uses wasm-encoder's structured rewriting rather than
a fork of Wasmtime.

The initial integrated Controller checks pass for local variables across many
small grants, deferred calls with no unfunded effects, current scene access after
resumption, stable request batches, durable writes before a later trap, and
initialization across paid slices. The borrowed snapshot test exposed a missing
lookup hook and is being rerun after that correction. The new rule retains one
borrowed snapshot until the next explicit successful tick_read or callback end;
existing snapshot_keep pins support longer retention. Implicit gas waits alone
do not invalidate the borrowed handle.

ABI version validation is now declarative: ship_api_version must return the
literal version constant. Offline validation does not execute guest startup.
This avoids rejecting a valid persisted computer merely because its initializer
needs more than one execution slice. Live initialization still spends gas and
can suspend normally.

Server review also fixed startup-slot starvation. The per-tick creation cap
applies to funded new VM instances; fee-only work, empty accounts, and existing
suspended initializers do not keep those slots occupied. A regression exercises
64 infinite initializers followed by a healthy computer. Full server, firmware,
and interactive checks remain to be completed before this piece is committed.

### Piece 6 completed — 2026-09-19 06:43 UTC

The resumable JIT, prepaid host calls, shared owner gas ledger, and computer UI
are integrated. All 51 runtime checks passed, including the full 3,000-system
routing fixtures. The client suite passed 122 tests, protocol 16, model 13,
intelligence queries 7, and network integration 4. Server verification covered
268 passing tests with five explicit benchmark tests ignored. One old headless
smoke test initially failed because it created unowned, unfunded computers; it
now provisions an account through the production path and passes. The runnable
debug client and server build successfully.

Live keyboard and mouse playtesting used a fresh durable world. The Computer
gas tab displayed exact integer account balances whose available and spent
amounts summed to the starting credit. Planning Terminus produced the four-stage
Sol gate, local transfer, slip, and arrival route, with ETAs and visible engine
activity. CPU usage settled from catalogue prefetch to about 14–17 percent.
Stopping paused the route; double-click alignment and Shift throttle control
worked, with the HUD showing manual thrust and turning torque. The program did
not fault during these interactions. Network tests also verify checkpoint
restart with the new ledger. Screenshots are in
`/tmp/toy-sequential-playtests/gas-live-*.png`; the archived client log is
`/tmp/sequential-paid-runtime.log`.

The live client still ran around 36–46 FPS in this scene, and initial window
resize produced the existing Vulkan presentation-layout warnings. These remain
open performance/rendering issues for the final integration pass; this milestone
does not claim that the 120 FPS target is met. The account and runtime work is
ready to commit. Missiles sharing their parent's surviving computer are next.

## Piece 7 — missiles and physical defenses

Work began after commit `425bccb`. The interceptor is a small ordinary ship with
a chemical engine, finite battery and propellant, turning actuators, and sensors.
A launcher consumes one packaged round with matching mass and applies ejection
recoil. Existing collision, laser, thermal, and optical systems handle it.
Guidance receives the fused observed track, including uncertainty; it cannot read
an unobserved target from the authoritative world.

The computer exports an optional missile_tick(handle) callback. Flight and
missile callbacks share memory, the same resumable VM, one hardware allowance,
and the actual owner's gas account. A destroyed parent leaves its computer data
available while surviving guided missiles depend on it, with no remaining hull,
engine, or sensor contribution. Exhausted missiles coast physically rather than
being deleted by an arbitrary lifetime rule.

The installation-defense catalogue will include a launcher platform and a patrol
variant. Gates remain physically traversable without a sovereignty permission
check. Future policing policy must act through observers and weapons, and may
fail or misidentify an attacker. Review found that the old demo retaliation code
reads the player's private firing state; the upcoming NPC policy pass must
replace that shortcut with behavior driven by observations.

Integration also exposed a gap in the preceding gas-scheduler tests: flight and
display execution sampled HardwareClock on opposite sides of its increment.
That can accidentally replenish a per-tick allowance during publication. The
missile scheduler work will establish one tick identity across all callbacks and
add a real application-update/publication regression.

### Missile catalogue and callback checks

ABI 28 adds the optional shared missile callback, a 192-byte observation record,
and a 32-byte current-missile control record. Tests cover shared memory,
resumption with the correct callback identity, changing sensor observations,
prepaid host calls, and safe completion after a missile disappears. The runtime
checks passed 57 tests. All four bundled WASM binaries were rebuilt.

The Kite interceptor has 160 kg dry mass and 240 kg of storable gel propellant,
a 25 kN chemical motor with 3 km/s exhaust speed, and a powered 2 Mm seeker.
Its battery gives a design endurance of roughly 900 seconds; actual stored
energy governs operation. A launcher holds 12 rounds and cycles every ten
seconds. The optional Shrike patrol carries 24 rounds, and the Kestrel defense
installation carries 48. Their blueprints are in `assets/ships/`. Ship catalogue
and physical specification verification passed all 44 tests, plus three shared
part-information UI checks.

Stock guidance uses proportional navigation, acquisition thrust, coasting, and
alignment-dependent throttle. The controller has 43 passing tests, including
finite-fuel crossing and long-range intercept cases. The client UI suite passed
123 tests. Missiles have a distinct rocket marker while using ordinary ship
geometry, exhaust, and optical glints. A separate review caught and fixed the
focused destroyed hull continuing to render beside its debris. The actual server
launch, torquer, shootdown, shared scheduling, and restart tests are still in
progress; native guidance simulations alone do not establish those behaviors.

### Additional integration review notes

Missile launch review also checks stored mass in a station carrying docked ships,
off-axis ejection momentum, and guidance retirement with a tiny unusable battery
residue. Parent and last-missile destruction in one collision tick must leave a
checkpointable world immediately. The retained computer's world-service source
is rebuilt from current authorized fused observations, with the remembered hull
position used only as a coordinate origin.

A privacy item for final integration: identity::attach_ship currently serializes
a sanitized blueprint as the visual asset, but still includes tank configuration
and original initial_fill values. Firmware, names, and aliases are already
removed. The visual asset should expose geometry without those internal loadout
details; simply removing tanks would change the compiled centre of mass, so this
needs a geometry representation that preserves the original visual origin.

### Full missile verification — 2026-09-19 07:27 UTC

The complete server library suite passed 281 tests (five benchmark cases ignored),
and the client suite passed 123. Physical regressions cover launch mass and
momentum with stored ships, ordinary same-owner laser and projectile shootdown,
continued guidance after a collision destroys the carrier, docked carrier
undocking, and one shared gas allowance across actual application updates and
MFD publication. The stock firmware intercepted a target initially 10 km away
and moving sideways at 200 m/s, with 144 of 240 kg propellant remaining.

Three network tests passed, but the authenticated snapshot test timed out waiting
for a subscribed MFD frame. This is under investigation before the live missile
playtest or commit; successful library tests alone are insufficient for the
shared scheduling change.

### Shared display scheduling and expiring intent

The network failure was display starvation: stock flight software used roughly
915,000 gas per tick while warming its navigation catalogue. Correctly sharing
the hardware cap left the separate MFD VM paying its boot cost extremely slowly.
The scheduler now shares the actual funded allowance between active flight and
display work, rotating priority per computer when an atomic call cannot fit both.
The deterministic stock-display regression renders at tick 101 while preserving
exact account debits and the one-million-gas physical cap.

That scheduling exposed an additional suspension problem. Stock weapon commands
and instrument publications carry short expiry times. A callback can legitimately
resume after its original command has expired, but the host treated expiry as an
invalid argument, causing the stock program to panic. The runtime fix will let
well-formed expired intent lapse without reviving old firing or stale display
state. Malformed accesses and invalid values must still fail normally. The
network subscription test now passes, but the observed guest fault is being
resolved before live verification.

### Live missile freeze — 2026-09-19 07:55 UTC

The extended 200-tick MFD regression and the authenticated network test now pass
without a guest fault. The runtime's 21 sandbox tests include a real suspended
callback that safely discards expired fire and geometry publications. All four
ABI 28 programs were rebuilt. A separate stock-WASM test proves that marking,
starting, stopping, and unmarking work with no turret handles installed.

The live Shrike patrol accepted Mark and Fire through the Selected Item panel.
Two Kite contacts appeared in Overview with their missile classification. About
5.3 seconds after firing, however, the server stopped advancing at tick 1066.
The client stayed responsive and reported an empty jitter buffer while the
simulation worker consumed a core. Perf attributed 72.4 percent to rotation drift
inside collision prediction, followed by shape-pose and distance queries. This
is a collision-progress defect exposed by the actual two-launcher encounter;
it blocks the milestone until reproduced and fixed. The live process was kept
for stack and body-state inspection. Screenshots are under
`/tmp/toy-sequential-playtests/missiles-live-*.png`.

The current desktop outputs are configured at 60 Hz, despite supporting 120 Hz.
Live client readings here remain around 41–49 FPS; the final performance pass
must distinguish compositor pacing, rendering cost, and simulation stalls.

### Captured contact cascade — 2026-09-19 08:09 UTC

The saved live state rules out shield depletion. The target still had 50 kg of
deployed material, 100 kg in reserve, and only about 26 MJ of shield heat.
Instead, two missiles had accumulated approximately 383,000 and 391,000 contact
revisions while advancing only 24.14 ms into the tick. Their inertia was finite
and well conditioned. The solver detected contacts inside a 0.2 mm shell but
separated resolved bodies by only 0.01 mm, repeatedly admitting the same contacts.

A separation skin outside the detection shell reduced the captured replay to
134 impacts. That alone was insufficient: the replay still performed 13.4 million
distance queries and took 11.6 seconds. The whole-tick rotational-motion bound
became excessively loose after impacts caused the missiles to tumble. Work now
focuses on cached short-angle rotation segments with conservative collision
bounds, plus a bounded conservative geometry fallback for extreme spin. The
regression uses the actual captured three-body state and a second run with a
large common orbital velocity. Completion and live retesting remain pending.

### Rotation and contact regression results

All 44 collision tests passed (two existing benchmarks ignored), as did eight
rotation tests. The captured contact state completed 20 consecutive ticks in
192 ms total, with a maximum of 139 impacts and 112,066 detailed queries in one
tick. Adding a common velocity of (22,000, -2,000, 23,000) m/s produced comparable
results: 184 ms total, at most 140 impacts and 112,847 queries. A low-speed
three-body cluster ran 100 ticks per reference frame without energy gain or
penetration. The extreme-spin regression verified that the bounded rotational
envelope still intercepts a crossing body.

The fresh stock-firmware fight also progresses, but exceeded its provisional
100,000-query assertion with 179,336 queries in a tick. The test is being run
through the full minute to measure the actual peak before setting a justified
regression bound. The captured state's first tick took roughly 127 ms, so the
freeze is resolved in that replay but the worst contact cost still exceeds the
100 ms simulation budget. This is recorded as a performance limit, not presented
as a completed performance fix. A geometry-preserving shortcut for centred
spheres and immediate envelope refitting after centre-of-mass changes are also
being integrated before live verification.

### Contact tolerance and fresh combat

The full one-minute stock fight completed with twelve launches, correct packaged
ammunition and missile mass, and no VM fault. Its worst tick initially took
332.55 ms and performed 390,084 detailed queries. For collision geometry already
voxelized at one metre, the old 0.1 mm contact tolerance imposed unnecessary
near-contact work. The feature-scaled tolerance is now 1 mm for metre-scale
features, retaining the existing 10 micrometre minimum and 1 mm maximum; the
separation skin remains ten times that tolerance. Tiny projectiles retain their
small-scale precision.

All 44 collision tests pass with that change and unchanged energy/no-tunneling
assertions. The captured state now runs twenty ticks in 33 ms total, with at most
82 impacts and 18,610 detailed queries; adding common orbital motion gives nearly
identical results. A fresh minute of fighting completed in 9.27 seconds of test
wall time, with twelve launches and at most 137 impacts, 121,123 detailed queries,
and 117.30 ms in one tick. The occasional 17 ms overrun remains a performance
measurement for final review. The original freeze and multi-second contact work
have been removed from these cases. A second fresh run and live UI verification
remain before committing the missile milestone.

### Piece 7 completed — live missile fight

The second independent one-minute stock fight passed: twelve launches, exact
mass and ammunition accounting, no VM faults, and the shared gas allowance
respected. Its peak was 70,483 detailed queries and 82.26 ms per tick. The
regression allows 250,000 queries to accommodate stochastic resource consumption
and different contact histories; the deterministic captured-state test uses a
100,000-query bound and currently peaks near 18,600. All 44 collision tests and
eight rotation tests pass. Formatting and diff checks pass.

The final live test used real mouse input to select the hostile patrol, Mark,
Fire, Look at, Hold fire, and Unmark. Twenty rounds launched over about 97 seconds;
the inventory showed four of twenty-four remaining. Missile meshes, exhaust,
HUD labels, Overview rows, shield impacts and continued flight were visible.
Already launched missiles continued after holding fire. The application closed
normally after 3,210 logged ticks, with no guest fault, device loss or recurrence
of the frozen simulation. Screenshots and the archived log are in
`/tmp/toy-sequential-playtests/missiles-final-*`.

Live performance remains an open final-pass item: server tick p95 was 19.29 ms,
with a maximum of 292.38 ms (214.33 ms in collision) under concurrent rendering.
Client reporting ranged from roughly 27 to 53 FPS. The original multi-second and
nonterminating contact cases are addressed, but these figures do not meet the
requested 120 FPS experience. Two UI/rendering observations are also retained:
the Inventory window needed resizing to expose all consumable rows during this
run, and some celestial surface textures disappeared later in the camera-follow
session. These need targeted checks in the inventory and final rendering passes.

Missile implementation also includes the shared-display scheduling and expired
publication fixes described above. Prior verification for this piece includes
281 server and 123 client library tests, all 16 missile persistence tests, 21
sandbox tests, the stock launcher-only marking regression, and the authenticated
network/MFD test. The collision and fresh-fight checks were rerun after the live
freeze fixes. The next piece is industry and physical cargo logistics.

## Piece 8 — industry and physical logistics

Missiles were committed as `f6972a5`. The main checkout has now been fast-forwarded
to that verified commit. The interrupted early parallel drafts were preserved
both in Git stash `6921c85daee9121856d45c821fa4ff5716331060` and a byte-verified
copy at `/tmp/toy-sim-mvp-interrupted-drafts-20260919` (143 paths, about 3.7 MB).
The earlier stash remains available as well. Active development continues in
the sequential worktree, keeping the main checkout at completed milestones.

Industry work is divided among shared inventory/recipes/catalogue, authoritative
ECS jobs and mines, protocol/session access, client UI, persistence, and independent
transaction checks. Ingredients will remain physically in inventory while jobs
reserve their quantities. Transfer, refilling, docking services and other jobs
must all honor those reservations. Completion consumes the bill and creates all
outputs atomically; lack of space or construction admission pauses completion.
Cancellation releases the reservation.

Shipyards must pay for the complete dry assembly, avionics and installed tank
containment. Fuel, battery charge and shield coolant cannot appear for free.
New ships start in docked inventory, and the client must support selecting,
refilling, charging and undocking them. Factories are powered installed station
modules; fixed-rate starter mines provide explicit raw-material sources.

Access policy needs one additional check: Industry permission may build a ship
for the facility's owner. Giving that ship to a different principal additionally
requires permission to withdraw cargo from the facility and authority over the
chosen recipient. Otherwise an operator could bypass a locked warehouse simply
by manufacturing its contents into a personally owned ship. These checks model
machine and storage credentials. Territorial law remains enforced by physical
actors.

### Industry integration checks in progress

The protocol 22 suite passes all twenty tests. Publication sends complete selected
inventories within a bounded message, explicitly identifying omissions instead
of silently truncating cargo rows. The client retains industry publications
across jitter-buffer catch-up so that consuming two frames cannot lose a one-time
catalogue update or access revocation.

Independent review found two concrete transaction issues before playtesting:
transferring cargo and undocking in the same input batch could leave stale mass,
and an odd mine batch could repeatedly favor the same recipient. Both have
regressions under development. Immediate mass updates are being restricted to
affected ships and their containment ancestors to avoid rescanning the entire
fleet for each warehouse operation.

The planned live journey starts at Neris Anchorage, makes repair material, builds
a Kestrel service launch, refills and charges the empty result, and undocks it.
Factory and shipyard restart tests also exercise completion immediately after a
snapshot. These checks are still pending; the protocol pass alone does not mark
industry complete.

The shared ship/inventory suite now passes all 52 tests, including eight new
checks for reservations, atomic completion, exact material conservation, fuel
recovery limits, missile assembly and cold ship construction. Three shared part
specification tests also pass. Review found one missing gameplay connection:
manufactured shielded ships had no way to replenish their empty coolant reserve.
Shield coolant is now manufactured cargo, paid into the installed reserve through
the same refill command. Its UI and server acceptance checks join the combined
integration build.

### Client and first integration results

All 129 client tests pass, including manufacturing/build intents, physical-unit
quantity editing, scrolling through long tank lists, switching focused ships and
preserving industry publications during catch-up. The combined build succeeds;
13 shared model tests, 20 protocol tests and two session byte-budget tests pass.

The first scenario-based server checks found a real assembly error: the new
industrial branch intersected Neris Anchorage's existing habitat ring. The
industrial backbone was moved aft of the station beacon using its normal
attachment connectors. Loading and compiling the corrected asset now succeeds
with 18 parts, about 5.33 million kilograms dry mass and a 347 metre bounding
radius. The server tests are being rebuilt against that corrected asset. This
was a startup failure, so live verification waits for those checks.

README claims about a four-system map and lack of persistence were stale. The
README now describes the implemented map, persistent launcher, resumable gas
execution and industry guide, while retaining the measured performance limits.

All 17 persistence tests now pass, including checkpointing one tick before ship
completion and restoring again after completion. The real FixedUpdate factory
test passes, as do the eleven transaction tests and the real-network inventory
privacy/revocation test. The broader server run passed 303 tests with five
existing ignored tests; its two failures were then fixed and individually rerun.
Mine allocation now equalizes capacity-limited shares and rotates only indivisible
extras. The other failure was test setup: the enlarged resource catalogue requires
an additional bounded hardware-discovery callback, so the tumble test now waits
for its specific Manual command acknowledgement before its unchanged 100 ticks
of control assertions. Both focused reruns pass.

The playable build succeeds. Live testing has opened Industry and loaded Neris's
real authorized facilities, available materials, recipes and catalogue over the
network. Manufacturing, cargo transfer and commissioning are the next checks.

### Live commissioning and restart verification

The native client produced ten batches of repair material, admitted the Kestrel
service launch build, and queued docking at Neris. A graceful shutdown during
construction and approach preserved both operations. After restart, the build
finished and exactly one Kestrel appeared in the hangar beside the patrol ship.
Construction initially waited behind an inbound docking reservation; internal
assembly now ignores aperture reservations while retaining host, ownership,
containment, size and mass checks. A regression restores an undersized bay while
leaving its inbound reservation intact and proves atomic completion succeeds.

The new hull had empty water and reactor tanks, empty shield reserve and zero
battery charge. Through the live inventory window I enabled dock power, loaded
8,023 kg water, 45 kg reactor fuel and 100 kg shield coolant from the warehouse,
and dragged repair material to the ship before confirming ten units. The
warehouse decreased by ten and the ship showed ten. A second graceful restart
preserved the ship, fuel, battery and transferred cargo. Undocking resumed its
flight computer and returned the camera to space; the cargo remained aboard.

Private hangar cameras also retained the external planet's atmospheric lighting.
They now remove that state while docked or in slip transit and restore it when
returning to space. The dock/undock ECS regression passes; the rebuilt native
client shows both patrol and Kestrel hulls clearly lit in the hangar.

The focused construction and atmosphere regressions pass, the final playable
build succeeds, and formatting/diff checks pass. Captures and archived native
logs are under `/tmp/toy-sequential-playtests/industry-*`. These checks exercise
real controls and the normal server connection. Display performance remains a
separate unresolved item for the final integration pass.
