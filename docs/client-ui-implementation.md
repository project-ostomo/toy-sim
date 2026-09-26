# Client UI implementation

Design: https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI

Implement the 32 reviewed screen variants with shared components and authoritative
commands usable by graphical and headless clients. Bloc officers are ordinary
accounts operated by external agents.

## Delivery checklist

The following screen workflows and shared services have implementations. The
[appearance audit](ui-appearance-audit.md) prompted a repair pass covering the
original 32 screens; see [repair results](ui-repair-results.md) for verification.

- Shared `StatStrip`, `FilterBar`, `ActionHeader`, `FlowRow`, table row and empty
  state components using egui-flex 0.8 where appropriate.
- Shell intents collected during rendering and dispatched after the egui passes.
- Wallet window, account selection, transfers, licence restriction display,
  daily charge estimates, currency filters and paginated ledger history.
- Integer UEC/LAT accounting, market conversion, gas transfers, persisted
  ledger entries and settlement dates.
- Directory diplomacy tab: declarations, ordered sources, agreements, dynamic
  blocs, application inbox, officers and posture commands. Existing USE and LFS
  membership is seeded into bloc records. Structured agreements govern docking,
  basing, wanted-list inheritance, mutual defence standings and transfer tariffs.
  Declaration history is paginated; standing cards show the rule's source.
- Assets window grouped by owner, bulk transfers and linked permission profiles.
  Profile edits propagate to attached assets. Additional asset grants and explicit
  denied permissions override the profile. Unlinking retains effective permissions.
- Assets also groups by location and type, with an attached details pane, owner
  filtering and server search. Goods aggregate across authorized holds and
  principal station storage, with separate available/reserved quantities and
  links to cargo or station markets. Assets, goods and holding lists each have
  independent cursors and a maximum page size of 128. Custody is counted once.
- Global LAT/UEC exchange: price/time matching, partial fills, reserved balances,
  limit and immediate orders, cancellations, bounded subscriptions, trade history,
  candles and persisted order history. The USE supplies an unlimited LAT bid.
  Charts provide price and UTC axes, 1-minute/5-minute/hourly intervals and
  OHLC/volume hover details for the loaded history page.
- Station commodity books in UEC or LAT, with reserved stock and delivery into
  principal storage. Deposits and withdrawals retain physical cargo capacity and
  mass accounting at the station.
- Sovereignty officers can set turnover tax. Ordinary transfer receipts and
  buy order placement are taxed; each charge rounds up to a microcurrency
  unit. Buyers pay upfront on their full limit value in the quoted currency,
  with no refunds for cancellation or unfilled quantity. Fills transfer full
  amounts, and sellers pay no market tax. Tax remittance does not recursively
  incur another charge.
- Workspace window visibility, position, size and lock state persist locally in
  `$XDG_CONFIG_HOME/toy-sim/workspace.toml` (or `~/.config/toy-sim/workspace.toml`).
- `OsgNetClient` owns picomux, main events, simulation inputs and typed RPC calls.
  Screens load their data through independent queries. Mutations return
  `Result<(), GameError>`. Graphical and headless clients use the same API.
- Public industry search, ordered customer tiers, energy and time pricing,
  quotes, reserved payment and customer inputs, cancellation and delivery into
  customer station storage. Operators publish prices with revision checks;
  queued work retains its accepted quote.
- Wallet, Assets (location/type/bulk) and Market (FX/orders/commodity/restricted)
  workspace screenshots at 1600×900 and 900×650, rendered on the CPU. The gallery
  also covers workspace, navigation, cargo, hangar, industry, directory and chat.

Delivery checklist:

- [x] Shared workspace, navigation, tables, forms, status, identity, ordered rules,
  charts and feedback components; selective egui-flex layouts; screenshot gallery.
- [x] Directory: bloc applications and officers, declarations, agreements, ordered
  trust sources and standing provenance.
- [x] Assets: principal station storage, inventory aggregation, named access
  profiles, bulk actions and general reservations.
- [x] Wallet: fixed point UEC/LAT, transfers, ledger, gas transfers, entitlement,
  taxes, licences and market conversion with the USE backstop bid.
- [x] Markets: station commodity books, reservations, atomic partial fills,
  cancellations, physical delivery, global FX, charts and history.
- [x] Industry: facility statistics, rates, ordered customer tiers, public quotes,
  payment reservations, cancellation and delivery.
- [x] Bounded event subscriptions, paginated RPC, persistence invariants,
  headless parity and examples.
- [x] Gallery fixtures mapped to the 32 reviewed screens at 1600×900 and
  900×650, with bounds checks.
- [x] Representative workspace captures for every reviewed screen, including
  organization Members, hostile contact affiliation and officer withdrawals.
- [x] Audited composition, density, tables, cards, semantic colors and compact
  usability repaired and reviewed using rendered images. Remaining model
  differences are listed in [repair results](ui-repair-results.md).

## Economic decisions

UEC demurrage is 20% effective annual loss, settled at UTC midnight each day.
Apply `1 - 0.8^(1/D)` to the balance above 50,000 UEC per account, where D is
365 or 366 for the day being charged. Round each charge up to the smallest currency
unit. Persist the last settled day and catch up missed days before transactions.
Do not accumulate fractional charges. Turnover tax likewise rounds each charge up
and applies to ordinary transfer receipts and buy order placement.

The USE provides an unlimited standing buy order for LAT at a configurable
backstop price, initially 3.20 UEC/LAT. It receives LAT and issues UEC to pay for
fills. This is a bid in the FX order book; it does not supply unlimited LAT.
Sell orders must execute against better bids before reaching the backstop.
Unlicensed USE recipients must sell incoming LAT through this market.

Wallet valuations use the latest executed FX price, or “Market price unavailable”
before the first trade. Restricted receipts execute against market bids and then
the backstop. Available and reserved balances are separate wallet buckets.
Demurrage applies only to available UEC; open orders and unstarted public jobs
retain their commitments in the owner's aggregate reserved balance.

## Layout constraints

Use egui-flex for StatStrip, FilterBar, ActionHeader and FlowRow. Use stable IDs,
one wrapping layer, at most one shrinking item per container, explicit bounds
inside scroll areas and content IDs when measured text changes. Keep tables,
cargo grids, map canvases and attached drawers in explicit layouts. Dispatch
commands once per completed graphical frame.

## Verification

Test financial conservation, authorization, duplicate commands, concurrent
reservations, stale quotes, cancellation and checkpoint restoration. Check daily
demurrage across thresholds, 365/366 day years, changing balances and downtime.
Use software rendering for screenshots. Preserve existing workspace changes.

## Run and configure

The protocol and database version is now 55. Use a fresh prototype database;
older worlds are rejected. Rebuild the bundled ship controller with this version.

The server configuration supports an official conversion rate at the top level:

```toml
official_uec_per_lat = "3.20"
```

Each existing `[[accounts]]` entry can also specify initial values and officer
roles. These settings apply when creating a world; checkpoints retain subsequent
balances, licences and appointments.

```toml
initial_uec = "75000"
initial_lat = "1000.25"
lat_licence = true
polity_officer = ["Helion Commonwealth"]
bloc_officer = ["League of Free States"]
```

Balances default to zero, and officer lists default to empty. Keys and account IDs
continue to use the existing authentication configuration. USE licence
declarations override the initial LAT licence setting for their targets.

Open **Wallet**, **Assets**, or **Society → Diplomacy** from the client launcher.
The headless example is in `crates/osg-client/examples/wallet_watch.rs`.

Generate the screen gallery with:

```sh
cargo test -p osg-client --features ui --lib gallery --offline
python3 tools/ui_gallery_index.py
```

Images are written to `target/ui-gallery`; `OSG_UI_GALLERY` can select another
directory. These software-rendered fixtures include actual workspace windows
on a plain background; they do not include the 3D scene.
Open `target/ui-gallery/index.html` for the complete screen map and full-size images.

## Screen map

Each filename below has `-1600x900.png` and `-900x650.png` variants in
`target/ui-gallery`. Related workflows share windows and tabs.

| Figma screen | Gallery prefix |
|---|---|
| 01 Shell | `workspace-shell` |
| 02 Navigation | `workspace-navigation` |
| 03 Cargo hold | `workspace-cargo` |
| 04 Consumables | `workspace-consumables` |
| 05 Station storage | `workspace-storage` |
| 06 Docked ships | `workspace-hangar` |
| 07 Transfer quantity | `workspace-quantity` |
| 08 Gate network | `workspace-map` |
| 09 Polity | `society-polity` |
| 09b Declarations and agreements | `society-declarations`, `society-agreements` |
| 09c Bloc | `society-bloc-public` |
| 09d Bloc officer inbox | `society-bloc-inbox` |
| 09f Trust sources | `society-sources` |
| 09g Organization | `society-organizations` |
| 09h Standing card | `society-standing` |
| 10 All facilities | `industry-facilities` |
| 10b Facility console | `industry-production` |
| 10c Service pricing | `industry-pricing` |
| 10d Public search | `industry-public-search` |
| 10e New public job | `industry-public-job` |
| 10f Own shipyard | `industry-shipyard` |
| 11 Local chat | `workspace-chat` |
| 12 Interface settings | `workspace-settings` |
| 13 Wallet | `wallet-licensed` |
| 13b Unlicensed wallet | `wallet-restricted` |
| 14 Assets by location | `assets-location` |
| 14b Assets by type | `assets-type` |
| 14c Bulk assets | `assets-bulk` |
| 15 Commodity offers | `market-commodity` |
| 15b Unlicensed offers | `market-restricted` |
| 15c Orders and price history | `market-orders` |
| 15d UEC/LAT exchange | `market-exchange` |

See [Client networking and RPC](rpc.md) for the shared graphical/headless API.
The Wallet's itemized ledger contains monetary entries; computer gas displays
available, reserved and spent totals.

## Verification results

Workspace compilation across all targets passes. The client suite passes all
164 tests; model 23, network library 14 and protocol 6 tests also pass. All five
server network integration tests pass, including authenticated standing/history
queries, disconnected mutation replies, process restart and world replacement.

The server library has 326 passing tests and one ignored test when the following
four failures from the full run are excluded. These remain unresolved in the
shared workspace:

- `restore_rebuilds_handles_and_blocks_saved_contact_orders`: the initial fixture
  has no observed contact handle.
- `target_handles_and_director_queries_use_own_observed_tracks_and_fresh_docked_state`:
  the injected observed contact is unavailable to the query.
- `due_shots_launch_together_and_ccd_reports_their_impacts`: expected projectile
  impacts are missing.
- `stock_computer_docks_from_default_spawn_without_entering_station`: the
  autopilot intersects the station hull.

Public industry checks cover stale quotes, reservation contention, cancellation,
single charging, delivery to customer storage and checkpoints before and after
charging. Treaty checks cover port rights, inherited wanted/defence rules and
rounded tariffs. Declaration history retains ordered public revisions.
