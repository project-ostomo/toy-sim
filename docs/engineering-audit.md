# OpenSpaceGame engineering audit

Date: 2026-09-20. Baseline: `36009cf`.

## Scope and method

This is a read-only investigation of the OpenSpaceGame repository requested to
identify avoidable engineering and testing work. The only file created by this
audit is this report. The history simulator and other repositories are outside
scope. No source, tests, manifests, assets, or existing documentation were changed.

I inventoried the workspace and inspected representative protocol, persistence,
WASM, server, client UI, industry, route planning, and documentation code. This is
a targeted audit across subsystems, not a claim that every function was reviewed.
No broad test suite or benchmark was run for this report. Line references identify
the inspected baseline and will move when fixes are applied.

The parent task supplied these execution results: server library suite: 372
passed, 39 failed, five ignored; six asset tests and three network crate tests
passed. It also independently reproduced
`vessel::tests::fault_reboot_budget_pauses_without_power` failing with
`unsupported controller import ship_v30.tick_read`. These are reported execution
results, not tests executed by the auditor. The other failures have not been
individually diagnosed here. They must not be attributed collectively to the
rename, picomux, or a single stale fixture.

Measurements from the working tree:

| Measurement | Result | Interpretation |
| --- | ---: | --- |
| Rust lines under `crates` and `apps` | 128,871 | `rg --files -g '*.rs' crates apps` piped into `wc -l`; includes tests, comments, blank lines, and generated Rust if present |
| Test annotation occurrences | 889 | `rg -n '#\[(test|[^]]*::test)' crates apps`; includes qualified test macros, not compiled test discovery |
| Plain `#[test]` occurrences | 865 | A subset of the preceding measurement |
| Server test annotations | 422 | Includes integration files and feature/configuration dependent tests |
| Client test annotations | 162 | Includes UI tests that need the UI feature |
| WASM test annotations | 67 | Does not imply these are redundant |
| Ignore annotations | 6 | Across the workspace; five are in the server |
| Existing documentation lines | 6,593 | `docs/*.md`, `DEVLOG.md`, and `README.md`, before this report |
| Trait declaration matches | 8 | `rg -n '^pub trait|^trait ' crates -g '*.rs'`; a limited structural observation, not a semantic count |

These counts are scale indicators. They are not test case counts, test coverage,
runtime measurements, or evidence of waste by themselves. In particular,
`sim/services.rs` is 2,678 lines but its test module starts at line 1,530;
`persistence/world.rs` is 2,154 lines and its test sections start near line 1,421.
Calling these entire files production monoliths would exaggerate the problem.

## Assessment

There is concrete avoidable complexity, especially in two custom container
formats and in test maintenance. The strongest immediate concern is that some
behavior tests cannot reach the behavior they claim to check because their WASM
fixtures target a rejected ABI. A large suite with that failure mode gives poor
feedback even if many of its assertions are valuable.

The repository does not show pervasive interface factories or generic trait
hierarchies. Much of its complexity follows directly from physics, programmable
ships, multiplayer authority, persistence, and asynchronous planning. The useful
cleanup is selective: remove unnecessary format machinery, repair drifting
fixtures, and make tests assert stable contracts with smaller setups.

I cannot establish which code was authored by agents from this inspection, nor
whether agent use caused a particular design. The observable patterns are
consistent with accumulating feature work without periodically removing its
scaffolding: tests for unsupported fixture versions, generic section machinery
with a single production consumer, and historical feature descriptions embedded
in current operational documentation. Those are actionable regardless of author.

## Findings

### 1. Stale WASM fixture programs invalidate behavior tests

Priority: P0. Impact: high feedback failure. Confidence: high.

Evidence:

- [ABI declaration](../crates/osg-ship-api/src/abi.rs#L5) declares `ship_v31` and
  version 31; the import attribute also spells out `ship_v31` near line 1,316.
- [Vessel fixtures](../crates/osg-server/src/sim/vessel/tests.rs#L9) import
  `ship_v30` at lines 9–13, 627, and 714–715.
- [Missile integration fixture](../crates/osg-server/src/sim/missiles/integration_tests.rs#L20)
  also imports `ship_v30`.
- The isolated failure quoted above occurs while constructing/loading a
  controller, before the power/reboot behavior is exercised.

This is confirmed test maintenance debt. The number of tests is not the defect;
the fixture version and host interface have drifted apart. Merely updating a
string may be insufficient if the ABI record sizes or syscall semantics changed.

Before: individual WAT snippets embed the ABI module string and sometimes build
their own record layouts. After: current-behavior WAT construction takes the
module name from `osg_ship_api::abi::IMPORT_MODULE` through a small helper local
to the fixture users. An explicit unsupported-version test should deliberately
retain a wrong module name. Existing layout checks should remain independent.

Keep power interruption, reboot budget, metering, missile authority, and resource
consumption tests. Repair their setup; do not delete them to reduce failures or
add a v30 compatibility path. One small current fixture helper is sufficient;
this does not justify a general WASM fixture framework.

Acceptance: affected fixtures instantiate on the supported ABI, focused tests
exercise their named behavior, and an intentionally obsolete import is still
rejected. Then classify the remaining server failures individually. No arbitrary
pass-count target should replace diagnosis.

### 2. Persistence provides a multi-section format for exactly one section

Priority: P1. Impact: medium structural complexity. Confidence: high.

Evidence:

- [Production capture](../crates/osg-server/src/persistence/mod.rs#L38) builds a
  `BTreeMap` containing one entry named `world` with version 8.
- [Production restore](../crates/osg-server/src/persistence/mod.rs#L60) requires
  exactly one section and specifically looks up `world`.
- [Storage model](../crates/osg-server/src/persistence/storage.rs#L8) exposes
  `Snapshot.sections` and `SectionData`.
- [SQL schema](../crates/osg-server/src/persistence/storage.rs#L74) has separate
  snapshot and section tables, section counts, per-section versions and hashes,
  plus an aggregate checksum. Save iterates sections near line 120.
- [Storage test fixture](../crates/osg-server/src/persistence/storage.rs#L233)
  constructs `world` and `economy` sections, although production restore rejects
  that section set.

The storage fixture is testing generality that the application currently cannot
use. There is no need to preserve that generality solely because a test covers it.

Before: `Snapshot -> BTreeMap<String, SectionData> -> two SQL tables -> checked
section set -> world bytes`. After: `Snapshot { tick, saved_at, version, bytes }
-> one snapshot row -> checked version -> world bytes`. One checksum can cover
the exact metadata and payload that need integrity protection.

Preserve transactional saves, the current retention policy, refusal to load a
corrupt latest generation, file ownership/application checks, and exclusive
database use. SQLite transactions and locking solve real durability/concurrency
problems; replacing SQLite is not required for this simplification.

Tradeoff: future independent section loading would require another design change.
There is no inspected production consumer for it today. This is a storage format
change; make the new format explicit and require a fresh development database
under the project policy. Do not silently overwrite existing saves or introduce
an unrequested migration subsystem.

Acceptance: one saved payload and one application schema version represent a
world checkpoint; restart restores the world; corruption fails clearly; two
writers cannot open the same world; retention still behaves as specified. Adapt
the existing storage tests to these contracts and remove multi-section cases.

### 3. Network framing has unused extension machinery

Priority: P1. Impact: medium complexity and allocations. Confidence: high about
the current shape; medium about the best replacement until consumers are checked.

Evidence in [protocol implementation](../crates/osg-protocol/src/lib.rs#L34):

- `payload_length` requires the exact protocol version (currently 34).
- `section` near line 54 always sets the required flag to 1.
- State encoding near lines 68–90 emits all 13 sections; input emits one.
- Decoding near lines 117–139 parses a separate section language, validates flags
  and count, handles optional unknown sections, and fills a `BTreeMap`.
- Frame reconstruction near lines 142–160 explicitly retrieves every known
  section. Missing any required field fails.
- Tests such as `roundtrip_and_optional_sections` near line 653 and
  `presentation_is_required_and_old_version_rejected` near line 877 maintain this
  machinery.

Strict version checking alone does not make optional sections meaningless:
same-version optional extensions are possible. The narrower finding is that the
inspected encoder emits no optional sections and reconstructs a complete frame,
with no demonstrated partial decode or independent extension consumer.

Before: outer frame header plus numbered section headers, individually encoded
values, an intermediate map, and manual reconstruction of `Frame`. After:
retain a bounded versioned header and encode a complete state/input payload with
the existing serializer. Validate the decoded domain object and reject trailing
bytes. Remove `Clock` only if its role disappears with direct frame encoding.

Preserve version rejection, message size limits before allocation/read, finite
numeric validation, authority checks at the server, truncation rejection, and
round trips of relevant domain variants. The redundant object construction and
extension handling are the target; hostile-input validation remains necessary.

Tradeoff: this changes the wire format and removes same-version extensions.
Update client and server together, bump the version, and confirm there are no
external clients that need a transition. Do not introduce a dual decoder in this
prototype absent a specific requirement.

Acceptance: existing authorized network flows work, wrong versions and malformed
or oversized payloads fail, and domain validation retains coverage. Remove tests
whose sole contract was skipping unknown optional sections. Measure allocations
only if a performance claim is made; this report has not measured a speedup.

### 4. UI workflow tests contain brittle renderer assumptions

Priority: P1. Impact: medium maintenance burden. Confidence: high.

The industry tests exercise real workflows: production commands, reserved cargo,
drag/drop, quantity selection, permission revocation, and clipping. Those are
useful integration checks. The concern is their coupling to visual details:

- [Shape collector](../crates/osg-client/src/ui/shell/industry/tests.rs#L131)
  traverses `egui::Shape`, extracts literal labels, and derives click locations
  from glyph bounds.
- [Cargo harness](../crates/osg-client/src/ui/shell/industry/tests.rs#L334) builds
  two fixed-position windows, settles for three frames, and uses `x < 500` and
  `x > 500` to distinguish source/destination near lines 437–447.
- [Clipping test](../crates/osg-client/src/ui/shell/industry/tests.rs#L599)
  recognizes inventory tiles by an 88-by-88 rectangle size, matches the exact
  product label, and simulates 50 frames of scrolling.
- [Map search test](../crates/osg-client/src/ui/shell/map/tests.rs#L396) builds
  another label/shape traversal near line 431.

Renaming a label, changing tile size, or moving a window can break these tests
without violating the workflow. The clipping test also protects a real defect:
content that cannot be reached by scrolling. Its purpose should survive cleanup.

Before: business-rule scenarios drive a full UI through text and pixel geometry.
After: most transfer/permission/quantity scenarios call the existing decision or
intent logic with small data fixtures. Keep a small number of headless widget
flows proving that pointer input reaches those decisions, including drag/drop,
shift quantity selection, and accessible scrolling.

Where geometry is essential, derive bounds from the widget response or existing
layout output instead of recognizing paint primitives by dimensions. Prefer
existing stable IDs or a small local test hook if available. Do not build an
application-wide accessibility automation system merely to reduce these tests.
If exposing internals would cost more than the tests save, retain the small
workflow harness and reduce its scenario matrix first.

Acceptance: a cosmetic label/tile-size change does not break business-rule tests;
a broken drag handler or inaccessible inventory content still fails a retained
workflow check. Keep UI verification headless as required by AGENTS.md.

### 5. An ordinary UI test also performs a profiling workload

Priority: P2. Impact: low to medium recurring work. Confidence: high.

[Map test at line 284](../crates/osg-client/src/ui/shell/map/tests.rs#L284)
draws a 3,000-system map for 80 frames, records elapsed times, sorts them, prints
p50/p95, counts shapes, and compares fit/zoom output. It is a normal `#[test]`,
not an ignored benchmark. It does not assert a wall-time threshold, so this is
not evidence of timing flakiness.

Split the purposes conceptually: a short deterministic check should verify that
zoomed rendering culls irrelevant geometry and the camera remains valid. The
80-frame timing sample belongs with explicitly invoked profiling work. Existing
ignored scale tests already establish that convention, for example
[spatial tests](../crates/osg-spatial/src/tests.rs#L199) and
[collision solver tests](../crates/osg-server/src/sim/physics/collision/solver_tests.rs#L438).

Do not pick a shorter frame count blindly: establish the minimum settling needed
by the retained rendering assertion. Prefer a visibility/culling result over an
egui shape ratio when an existing interface exposes it. Preserve one integration
check that the renderer actually uses the culling result.

Acceptance: ordinary test runs verify culling without collecting timing
percentiles; explicit profiling still measures the representative workload. No
runtime savings estimate is claimed without measuring it.

### 6. Valuable industry invariants depend on incidental catalogue contents

Priority: P1. Impact: medium fixture fragility. Confidence: high.

[Industry acceptance tests](../crates/osg-server/src/sim/industry/acceptance_tests.rs)
contain essential economic invariants, including no duplicate completion,
reservation integrity, permission boundaries, and paid mass/energy consumption.
Their 1,776 lines are not sufficient grounds for deletion.

There are concrete setup dependencies worth removing:

- Near [line 777](../crates/osg-server/src/sim/industry/acceptance_tests.rs#L777),
  the full-hold/cancellation test searches the production catalogue for a recipe
  whose output volume exceeds its input volume, then unwraps the result. Balancing
  recipes can break setup before cancellation is exercised.
- Near [line 1519](../crates/osg-server/src/sim/industry/acceptance_tests.rs#L1519),
  the concurrent-lane test requires the fixture to contain at least two lanes of
  the selected recipe capability. It computes initial energy through the
  production `cumulative_energy` helper. The concurrency invariant is useful,
  but that shared oracle cannot independently detect a defect in the energy
  formula itself.

Before: load substantial production data and search for accidental preconditions.
After: construct the smallest valid facility and explicit recipe needed for each
invariant, using normal domain validation. Use known simple quantities for
independent accounting expectations. Retain a smaller stock-catalogue integration
test proving shipped content can actually construct/operate facilities.

Keep cross-system tests for reserved fuel, docked mass propagation, permission
revocation, no free progress, and atomic retries. These span real interactions
that isolated getters/setters cannot verify. Do not collapse all scenarios into
one long test whose first assertion hides later failures.

Acceptance: a recipe balance change does not invalidate unrelated lifecycle
fixtures; deliberately duplicated output or overspent energy still fails the
appropriate invariant. Keep fixture helpers local until multiple real callers
justify sharing them.

### 7. Current documentation carries a historical feature ledger

Priority: P2. Impact: medium maintenance and reader cost. Confidence: high.

[server-client.md](server-client.md) is 1,137 lines, alongside a 2,007-line
`DEVLOG.md`, a 607-line ABI reference, and focused subsystem documents. Some
length is warranted. The concrete duplication is current-reference material
mixed with incremental version notes:

- [Lines 1039–1047](server-client.md#L1039) describe what protocol versions 7,
  8, and 9 add, while the implementation is version 34.
- [Lines 930–952](server-client.md#L930) enumerate individual test behaviors and
  a large multi-package command. Many of those details duplicate test names and
  subsystem contracts.

These statements are historical, not necessarily false. Their placement makes
it harder to distinguish the current interface from the sequence that created
it, and every behavior change invites edits to code, tests, a subsystem document,
this overview, and the development log.

Before: large architecture document plus accumulated release narrative and test
inventory. After: current architecture/operations overview that links to the
authoritative ABI, persistence, navigation, and asset documents. Preserve useful
historical rationale in the existing development log; avoid duplicating it.
Keep a short test-command guide grouped by subsystem, not a prose inventory of
every assertion. Do not add another mandatory process document as the remedy.

Acceptance: a reader can find the currently supported versions and workflows
without reconciling historical versions, and each substantial contract has one
canonical reference. Update documentation when behavior changes rather than
requiring every small edit to append another narrative.

## Complexity to preserve and areas requiring more evidence

### Security and authority checks

Repeated checks across layers can be intentional. Protocol decoding validates
shape/numeric limits; command execution validates authority in the current world;
publication controls disclosure; the UI gives early feedback. Removing a server
check because the client also checks permissions creates an authorization bug.

[Route caller state](../crates/osg-server/src/sim/route_service.rs#L44) records
world, ship, principal, authority, travel, and topology revisions. The bounded
queue/worker state near lines 32–36 and 86–114 supports asynchronous work that can
outlive a world reset or authority change. I found no basis to replace this with
a synchronous call or an unbounded task spawn. If simplifying it later, retain
stale-result rejection, bounded work, cancellation, and ownership isolation.

### Domain, persistence, publication, and ABI representations

[World records](../crates/osg-server/src/persistence/world.rs#L15) duplicate some
runtime information, and [published service views](../crates/osg-server/src/sim/services.rs#L124)
copy selected data into shareable snapshots. These representations serve distinct
lifetimes and trust boundaries. ECS entity handles cannot simply be serialized as
stable identities; a public appearance must not expose private ship contents;
guest ABI records have fixed layouts. A global merged model would couple these
concerns and could leak data.

The 2,678-line service file is a navigation/ownership hotspot, but much of that
length is tests. Moving tests into a sibling file could improve navigation; it
does not remove architectural complexity. Splitting the production code by
existing query families may be reasonable when touching it. Do not introduce
service registries or adapter traits merely to distribute lines among files.

### WASM bindings and sandbox tests

[The binding generator](../tools/generate_ship_bindings.py#L7) already derives
C/AssemblyScript records from Rust ABI sources. Fixed-size/offset assertions in
[abi.rs](../crates/osg-ship-api/src/abi.rs#L1243) are meaningful contract checks,
even when they resemble record declarations: C, Rust, and guest memory must agree.
Generated header length should not be counted as hand-maintained duplication.

The regex-based generator accepts a constrained source grammar; that is a real
maintenance constraint, but replacing it with an AST/schema tool is not justified
by an observed failure here. Retain regeneration/reproducibility checks and
representative guest interoperability. Diagnose drift before proposing a new
binding framework.

[ProgramServices](../crates/osg-ship-wasm/src/program_services.rs#L4) is a small
interface separating guest runtime calls from host chat/LLM services. It has an
actual server implementation and a test fake. This is a justified seam; one
production implementation alone does not make a trait unnecessary.

### Physics and end-to-end network tests

Conservation, collision, numerical boundary, spatial-query reference comparison,
WASM metering, and authority tests protect failures that are difficult to detect
visually. Keep them. The network integration suite at
[tests/network.rs](../crates/osg-server/tests/network.rs#L45) covers authentication,
world reset, industry disclosure, society permissions, process restart, and chat
privacy. Those contracts are broader than unit serialization tests.

Its length warrants checking setup reuse when maintaining it, but this audit has
not demonstrated that its scenarios are redundant. Likewise, target-observation
and physics expectation failures need individual investigation: changing an
expected value to make a test pass can conceal a simulation regression.

## Proposed cleanup batches

| Order | Scope | Concrete deliverable | Required evidence and stop condition |
| --- | --- | --- | --- |
| A | Vessel/missile WAT fixtures | Supported ABI fixtures; explicit obsolete-import rejection case | Focused behavior tests reach their assertions; classify other known server failures without broad expectation rewrites |
| B | Industry acceptance fixtures | Explicit minimal recipes/facilities and independent accounting values | Existing reservation, atomicity, authority, and conservation cases pass; one stock-content integration remains |
| C | Industry/map UI tests | Smaller business-rule setups, retained workflow smoke coverage, separate profiling workload | Drag/drop, quantity selection, clipping, and culling work headlessly; cosmetic constants no longer define business assertions |
| D | Persistence storage | One versioned world payload; remove section model/table/test generality | Restart, corruption refusal, retention, and exclusivity checks pass; format reset is explicit |
| E | Wire protocol | One bounded versioned payload per message kind; remove section parser and optional-extension tests | Protocol validation and selected real-client/server flows pass; all consumers updated together |
| F | Current documentation | Canonical subsystem references and concise verification commands | Current behavior is discoverable without historical reconciliation; no new mandatory reporting ritual |

Batches D and E are independent format changes and should be reviewed separately.
They should follow restoration of useful test feedback, since otherwise existing
failures will make regressions harder to attribute. C should begin with the
explicit profiling split and incidental geometry assumptions, not a redesign of
the whole UI.

For each batch, record the user-visible or integrity contract before editing,
retain the smallest set of tests that distinguishes a broken implementation from
a correct one, run checks appropriate to changed boundaries, and stop when those
checks pass. A test that would also pass a deliberately broken implementation is
a candidate for improvement or removal. There is no recommended quota for lines,
crates, traits, or tests to delete.

## Limits and follow-up questions for implementation

The repository inventory found 19 workspace crates in the parent task, but this
audit did not prove that any particular crate boundary is wasteful. Shared math,
guest ABI, server runtime, and optional UI dependencies have different build and
runtime requirements. A crate-merging proposal needs dependency/build-time
evidence before it earns a place in these batches.

No runtime profile, test duration distribution, flakiness history, or individual
failure census was collected. Those measurements are appropriate only if needed
to decide a specific cleanup. The next concrete evidence should be the remaining
failure classification after repairing stale WASM setup, plus confirmation of
wire-format consumers before removing section support. This report establishes
actionable locations and preserved contracts; it does not certify uninspected
subsystems or authorize expanding the feature scope.
