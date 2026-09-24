# Original 32 screens: appearance repairs

Scope confirmed by the user: repair the original 32 reviewed screens first.
New Settings categories and direct messages are outside this pass.

## Implementation

- Shared compact window chrome, left-aligned titles, navy surfaces, smaller
  control spacing, numeric table alignment and semantic icons.
- Actual workspace framing in software screenshots, disabled animations,
  representative records and bounds checks against each window's allocation.
- Three-column map with star list, destination details and route colors;
  accessible compact launcher, selected-item content and transfer dialog.
- Industry facility sidebar, overview metrics and alerts, lane dashboard,
  pricing and public comparison tables, and attached job/build sheets.
- Directory hierarchy, principal headers and contextual tabs, posture tables,
  declaration/agreement cards, officer requests, trust matrix, organization
  members and contact affiliation card.
- Commodity catalogue and station offers, exchange chart/book/ticket columns,
  depth colors, order progress and Open/Filled/Cancelled history.
- Wallet account rows, executed FX history sparkline, gas reservation meter,
  structured tax strip, ledger details and separate Send/Move actions.
- Assets tree columns, full-row selection, measured capacity meters and
  selected-group inspector with bottom actions.

## Data supporting the screens

Station comparisons use accessible books and actual executed FX prices. Order
history retains the most recent 10,000 closures, including filled and cancelled
orders, with original and executed quantities. Asset capacity telemetry requires
View permission. Industry summaries report actual generation, consumption,
lane/job counts and berths; facility revenue retains cumulative net receipts in
the currency received. Bloc withdrawal requests can be cancelled, granted or
refused through typed RPC calls while membership remains active pending a grant.

These wire and persistence changes use protocol/database version 55. Bundled
firmware was rebuilt. Prototype worlds from earlier versions are rejected.

The requested Charon/Sarasa fonts and economic rules are retained. Gas meters
describe actual reservations; they do not claim a subscription budget. Dates,
tags and automation metadata absent from the game model are not invented.

## Verification

Verification completed:

- Workspace compilation across all targets passed.
- All 164 client tests passed, including all seven gallery checks.
- All 14 shared UI tests passed, including window close behavior, stable hover
  geometry and CJK advances.
- All five server network integration tests passed.
- Targeted server checks passed: nine exchange tests, the comparison/history
  query test, six diplomacy tests, two asset tests and the public-service
  charge/revenue/persistence test. Model and network library tests also passed.
- The final compact Market layout passed a targeted rerun after image review.
- `git diff --check` passed.

The screenshot gallery contains 66 linked captures of the original 32 workflows
at [target/ui-gallery/index.html](../target/ui-gallery/index.html).
Captures include the real workspace UI on a plain background; the 3D world scene
is omitted. Desktop images are reviewed against Figma, and 900×650 images are
checked for clipping and reachable controls.

Remaining model differences are recorded in the detailed reports: gas has no
subscription period; asset tags and contract reservation provenance are not
modeled; market comparison has no station fee/distance model; Directory has no
geographic claim records or dated officer policy metadata. The UI presents real
available values rather than populating those fields with fictional data.

The four previously documented broader server failures in
[implementation notes](client-ui-implementation.md#verification-results) were
outside this appearance pass; the complete server unit suite was not rerun.

Detailed screen reviews:

- [Workspace](ui-audit-workspace.md)
- [Economy](ui-audit-economy.md)
- [Industry](ui-audit-industry.md)
- [Directory](ui-audit-directory.md)

Icon mappings were checked against the [Phosphor source](https://raw.githubusercontent.com/phosphor-icons/web/master/src/regular/style.css)
and the bundled font's character map.
