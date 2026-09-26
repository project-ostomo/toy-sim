# Economy UI appearance audit

Audited 2026-09-24 using the actual Figma screenshot images at 1600 × 900 and the current software-rendered gallery images. All nine desktop economy variants were inspected. Compact examples inspected: wallet licensed, assets bulk, market commodity, market exchange.

Figma file: `kvxDvZDyEbkfgU25IDzyyd`.

## Evidence and scope

| Screen | Figma node | Current desktop screenshot |
| --- | --- | --- |
| Wallet | `53:1433` | [wallet licensed](../target/ui-gallery/wallet-licensed-1600x900.png) |
| Wallet restricted | `53:1892` | [wallet restricted](../target/ui-gallery/wallet-restricted-1600x900.png) |
| Assets by location | `25:876` | [assets location](../target/ui-gallery/assets-location-1600x900.png) |
| Assets by type | `25:1341` | [assets type](../target/ui-gallery/assets-type-1600x900.png) |
| Assets bulk selection | `25:1890` | [assets bulk](../target/ui-gallery/assets-bulk-1600x900.png) |
| Market commodity | `55:74` | [market commodity](../target/ui-gallery/market-commodity-1600x900.png) |
| Market restricted | `55:651` | [market restricted](../target/ui-gallery/market-restricted-1600x900.png) |
| Market orders | `55:1235` | [market orders](../target/ui-gallery/market-orders-1600x900.png) |
| Currency exchange | `58:74` | [market exchange](../target/ui-gallery/market-exchange-1600x900.png) |

The current gallery draws these screen bodies directly into the full viewport. For example, `shell/tests.rs:618` calls `market::draw` inside `ctx.run_ui`. It does not show their floating window frame, navigation rail, scene, selected item, overview, or bottom status bar. Figma shows those elements around an approximately 1180 px wide window. Current screenshots therefore cannot establish full-screen visual parity. Their edge-to-edge presentation also changes all column widths and apparent density.

The following user decisions are intentional and must survive visual fixes: Iosevka Charon and Charon Mono with Sarasa CJK, 20% annual demurrage charged daily, turnover tax on ordinary transfer receipts and upfront buy orders, market LAT valuation, and the USE's unlimited UEC bid backstop for LAT. The earlier Figma demurrage and income-tax wording is obsolete. Different example names and numerical balances are not themselves defects.

## Priority 1: screen composition

1. **Commodity market needs its own layout.** Figma `55:74` has a persistent 230 px catalogue rail, then item title, Offers/Bids/History tabs, station comparison table, and a lower split between local depth and an order ticket. The current commodity image is the same large three-card dashboard and chart as currency exchange, with station and item dropdowns. The entire visible station offer table is missing: station/system, currency badges, price, comparable UEC price, available quantity, fee, distance. This changes the purpose and hierarchy of the screen. Source: [market.rs](../crates/osg-client/src/ui/shell/market.rs), especially `instrument_picker` and `draw`.

2. **Currency exchange needs a chart/book/ticket split.** Figma `58:74` places the compact price and 24-hour metrics above a large chart at left, depth at right, recent trades below the chart, and order entry below depth. The current screen vertically stacks three huge metric cards, a shallow full-width chart, a full-width order table, and a small order form. Recent trades begin near the desktop viewport bottom; at 900 × 650 even order entry is below the initial viewport. Restore the major columns before adjusting small spacing. Sources: [market.rs](../crates/osg-client/src/ui/shell/market.rs), [components.rs](../crates/osg-ui/src/components.rs).

3. **Assets needs aligned table/tree rows.** Figma `25:876` and `25:1341` align asset name, owner/location, status and contents/quantity across a dense grid, with group bars and full-row selection. Current rows are loose trees containing just name and status, with a large empty middle and goods in a separate list below. Selection colors only the small name button. This loses the visual scan path even with more realistic data. Source: [assets/browser.rs](../crates/osg-client/src/ui/shell/assets/browser.rs), `list`.

4. **Bulk selection has the wrong inspector and action placement.** Figma `25:1890` shows a selected-group inspector with count, profiles in use, locations, proposed profile rules, and apply controls; a prominent selection toolbar anchors the bottom. The current bulk screenshot retains the individual Kestrel inspector and puts profile/link/unlink/transfer controls above the filters. It does not visually explain the selected group or proposed result. Source: [assets/browser.rs](../crates/osg-client/src/ui/shell/assets/browser.rs), [assets.rs](../crates/osg-client/src/ui/shell/assets.rs).

5. **Orders needs its own table and history composition.** Figma `55:1235` presents open/filled/cancelled tabs, a compact table with item, station, price, fill progress, held assets and cancel actions, followed by price history. The current page retains the three balance cards, shows a single loose order sentence and cancel button, then a long recent-trades table. The intended hierarchy and visible fill progress are absent. Source: [market.rs](../crates/osg-client/src/ui/shell/market.rs), `state.orders` branch.

## Priority 2: missing visual information and affordances

- **Wallet cards:** Figma has two wide currency panels plus a narrower Gas panel. Each currency panel contains compact named account rows with right-aligned balances. Current equal-width cards show one large ungrouped number for the selected account. The LAT sparkline and Gas subscription/progress treatment are absent. Restore the multi-account overview and currency-specific content rather than using one generic stat card structure. Sources: [wallet.rs](../crates/osg-client/src/ui/shell/wallet.rs), `stat_strip` in [components.rs](../crates/osg-ui/src/components.rs).
- **Wallet ledger:** Figma labels the ledger, has account and entry-kind columns, outlined entry badges, descriptive transactions, and compact right-aligned amounts. The current view has a wide centered Entry column with bare `Issue`/`Demurrage` labels, no account column, and no kind filter. The richer account/transaction fixture is also missing, so the three example rows do not exercise the intended hierarchy. Source: [wallet.rs](../crates/osg-client/src/ui/shell/wallet.rs).
- **Tax strip:** Figma uses a separate raised band with account/polity relationships and amber rates. The current single amber line blends into the surrounding surface. Keep the correct turnover-tax semantics while restoring a structured band. Source: [wallet.rs](../crates/osg-client/src/ui/shell/wallet.rs).
- **Restricted states:** Figma's restricted LAT wallet has a dashed amber outline and explanatory hierarchy; the current card differs chiefly by a small badge and amber text. Restricted market has a full-width amber panel above offers and visibly disabled LAT rows with restriction badges. Current market has one amber sentence and the same normal book layout. Sources: [wallet.rs](../crates/osg-client/src/ui/shell/wallet.rs), [market.rs](../crates/osg-client/src/ui/shell/market.rs).
- **Asset inspectors:** Figma's ship inspector includes ship class, ownership and tags, hold/battery/fuel, access summary and reservations. Current screenshot contains title/status, owner, location and buttons. The goods inspector lacks Figma's total/available/reserved summary strip and reservation provenance. Source: [assets/browser.rs](../crates/osg-client/src/ui/shell/assets/browser.rs), `details`.
- **Filters and icons:** Assets lacks the visible system/status/tag filters and Mine/Shared/All segmentation; market lacks the catalogue's grouped resource/kit/currency icons. Sparse icons and missing colored status cells remove much of Figma's visual richness. Sources: [assets/browser.rs](../crates/osg-client/src/ui/shell/assets/browser.rs), [market.rs](../crates/osg-client/src/ui/shell/market.rs).

## Priority 3: shared presentation

- Colors are broadly in the intended dark blue family, but many regions that carry visual weight in Figma are absent: full-row cyan selection, red asks and green bids with depth bars, colored outlined category badges, amber restriction panels, and mint chart strokes. Increasing global saturation would not restore those missing regions. The current market also uses amber for sells where Figma uses red.
- Current generic table cells center names and amounts across very wide columns. Figma aligns labels left, numeric columns right, and uses compact fixed-purpose columns. Adjust the shared row primitive or supply explicit alignment per column. Source: [components.rs](../crates/osg-ui/src/components.rs), `table_row`.
- Charon is intentional. Typography hierarchy still differs: large bare balances dominate the wallet and market cards, whereas Figma emphasizes compact labels and aligned account rows. Group numbers for scanning and separate display precision from stored precision; the current `74985.720866` and `1250000` are visually noisy compared with grouped values. This should not change monetary arithmetic.
- Figma currency chart has moving-average lines, current-price guide, OHLC annotation, richer time ticks, interval/display controls, and depth bars. Current chart lacks those visible elements. Its thin green dashes are partly a fixture problem: a single trade per minute produces candles with equal OHLC. Populate multiple trades per interval, including rises and falls, before assessing candle geometry. Source: [components.rs](../crates/osg-ui/src/components.rs), `price_chart`; fixture in [shell/tests.rs](../crates/osg-client/src/ui/shell/tests.rs).

## Verification changes needed

Capture the real shell with the economy window open at the Figma dimensions, in addition to component snapshots. Populate enough accounts, assets, station offers, partially filled orders and price movements to exercise the layout. Verify one commodity, one currency exchange, one wallet and one bulk-assets screen first; then repeat their restricted and compact states. Existing viewport-bounds assertions demonstrate rendering bounds, not appearance parity or that primary actions remain visible.

Compact evidence: [wallet](../target/ui-gallery/wallet-licensed-900x650.png), [bulk assets](../target/ui-gallery/assets-bulk-900x650.png), [commodity](../target/ui-gallery/market-commodity-900x650.png), [exchange](../target/ui-gallery/market-exchange-900x650.png). The Figma frames inspected provide desktop references, so compact behavior should preserve the same information priorities rather than claim pixel equivalence to an unprovided compact design.

## Implementation follow-up

The gallery now captures these screens inside the real desktop frame. Wallet uses unequal currency/Gas panels, named account rows, an executed-FX sparkline, a thin allocation meter, a tax band, entry badges, aligned monetary columns, and separate Send/Move actions. Restricted LAT has a prominent amber outline and explanation. Gas labels describe current allocation because no subscription period is modeled.

Market now has a persistent item catalogue, actual cross-station offer comparisons, restricted offer treatment, a split depth/ticket composition, an exchange chart beside the book, and an orders grid with fill progress and Open/Filled/Cancelled filters. The backend supplies actual comparison values and persisted order status. At compact sizes the exchange ticket precedes depth and station offers scroll separately to keep order entry visible. Charts use actual trade history, with moving averages and a current-price line provided by the shared chart component.

Assets now has aligned columns, colored status cells, full-row selection, system/status/sharing filters, Phosphor icons, actual hold/battery/consumable telemetry, goods totals, and an aggregate selection inspector. Bulk actions occupy a dedicated bottom band. Fixture accounts, assets, orders and market books now exercise those layouts.

Known model differences remain explicit: asset tags, subscription plans and market commissions are not modeled; the UI does not fabricate them. Station comparison currently shows price, comparable UEC price and available quantity; distance and polity filters are not present. Goods reservations expose totals and locations, without per-contract provenance. These should be treated as product/data work if exact feature parity with the reference is required.

Verification: all seven gallery tests passed after the main changes, including every economy desktop and compact variant. All nine desktop and nine compact economy images were inspected. That inspection found compact order entry below the initial viewport, which received a focused responsive correction and targeted market-gallery rerun.
