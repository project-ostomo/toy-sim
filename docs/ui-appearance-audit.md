# Client appearance audit — 2026-09-24

This records the findings before the repair pass. The user confirmed that the
pass covers the original 32 screens; newer Settings categories and direct
messages are separate work. See [repair results](ui-repair-results.md) for the
implementation and verification status.

Four agents inspected Figma images and local software screenshots across the
workspace, economy, industry and Directory. All four found major differences.
The existing implementation and gallery do not meet the reviewed visual design.
The earlier delivery checklist overstated visual completion.

## Findings and repair order

| Priority | Area | Observed difference | Required correction |
| --- | --- | --- | --- |
| 1 | Screenshot evidence | Most fixtures draw a screen body across the entire viewport, omitting the shell, actual window dimensions and chrome. Sparse records and some incorrect selected states conceal missing UI. | Capture through the real shell with populated, matched states and settled animations. Keep component captures clearly identified. |
| 2 | Shared shell | Window titles, control spacing and chrome differ. Selected-item status clips; the compact launcher loses its lower icons. | Match compact title/row metrics and ensure content and launcher actions remain accessible. |
| 3 | Market | Commodity offers reuse an exchange dashboard; catalogue and station comparison are missing. Exchange chart, depth and ticket stack vertically. | Build the commodity catalogue/table/ticket composition and the exchange chart/book columns. Restore order progress and history layout. |
| 3 | Industry | Missing persistent facility sidebar, overview table and lane dashboard. Public search and pricing use forms instead of the designed comparison/rate tables. | Build facility navigation, overview, console, pricing tables and attached job/build sheets. |
| 3 | Directory | Global tabs and equal columns replace the hierarchy sidebar and contextual details. Forms displace published summaries. Standing is four lines instead of the contact card. | Build principal navigation/header, summary tables, agreement/request cards, trust matrix, member view and complete contact card. |
| 3 | Map | Controls, canvas and destination stack vertically. | Restore preferences/star list, central map and destination/route columns, with an explicit compact arrangement. |
| 4 | Assets | Loose tree labels lack aligned columns and full-row selection. Bulk selection still shows one ship's inspector. | Build tree tables, richer inspectors and selected-group details/actions. |
| 4 | Wallet | Generic totals replace named account rows; sparkline, gas meter and structured tax strip are absent. Ledger lacks the designed descriptive hierarchy. | Build currency-specific panels and a dense, informative ledger. |
| 5 | Visual finish | Missing navy row surfaces, colored badges, selection bands, depth bars and warning cards reduce richness. | Restore these semantic regions, then compare spacing, typography scale and color against the references. |

The shared palette already contains the main cyan, mint, green, amber and red
colors. Missing surfaces and information treatments account for much of the
perceived color gap. Some bare gallery backgrounds also come from the screenshot
harness. Potential animation dimming remains unverified and should be checked
before changing runtime opacity.

## Acceptance for the corrections

- Match desktop references with the same selected screen, representative data,
  permissions, window dimensions and surrounding workspace.
- Review complete rendered images for the intended major regions, row alignment,
  density, selections, restrictions, warnings and available actions.
- Check 900×650 for readable content, reachable actions and clipping. There is no
  supplied compact Figma reference, so this is a usability check.
- Use bounds assertions as a supplement to image review. They cannot establish
  correct composition, visible text or presence of required information.

Preserve the requested Charon/Sarasa fonts, 20% effective annual demurrage computed
daily, rounding charges up, turnover tax on ordinary transfer receipts and upfront buy orders, market
LAT valuation and the unlimited UEC bid for LAT. Figma's older economic text
must not replace those decisions. Automated officers remain ordinary accounts
operated through headless clients.

## Detailed evidence

- [Workspace, navigation, inventory, chat and settings](ui-audit-workspace.md)
- [Wallet, Assets and Market](ui-audit-economy.md)
- [Industry](ui-audit-industry.md)
- [Directory and standing](ui-audit-directory.md)
- [Current screenshot gallery](../target/ui-gallery/index.html)

The live design now has expanded Settings and Chat frames with different node
IDs. The workspace report identifies them separately from the original reviewed
screen set. The audit changed documentation only; visual repairs remain open.
