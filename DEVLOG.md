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
