# Industry appearance audit

Audited 2026-09-24 by viewing the six Figma frames through the Figma screenshot tool at 1600 pixels, and all twelve corresponding software screenshots through `view_image`. Figma file: `kvxDvZDyEbkfgU25IDzyyd`. This is an appearance audit; no product source changes were made.

## Remediation, 2026-09-24

The findings below record the original audit. Subsequent implementation added the facility sidebar with system groups, overview cards and table, attention banner, module lane cards, right job/build sheets, pricing and comparison tables, import area, materials and construction checks, and a compact job footer that remains visible. The gallery now renders the actual desktop/window composition. Fixtures include running and stalled work, an offline module, a valid imported blueprint, shortages, and multiple facilities.

The server publishes facility power, lane, queue, system, and berth metrics. Outside revenue is persisted as cumulative net receipts in UEC and LAT and survives job completion. Public comparisons request actual quotes, and shortages link to Assets. Revenue is labeled cumulative; payment wording preserves the implemented reservation/charge behavior. The requested Charon fonts remain in use.

The industry gallery passed all sixteen desktop/compact captures after correcting a compact public-job overflow. Fresh public-job, console, shipyard, pricing, overview, and search images were visually inspected. Shipyard's queue action and the public-job footer are visible within the window. Compact pricing retains vertical scrolling to reach customer tiers and the quote preview.

The implementation has substantial composition differences in every industry screen. The intentional Iosevka Charon font change is accepted. Different fictional facility names and quantities are also accepted. The issues below concern layout, hierarchy, visible information, and state coverage.

## Evidence map

All local paths below are relative to the repository root. Each screenshot stem has both `-1600x900.png` and `-900x650.png` in `target/ui-gallery/`.

| Figma frame | Local screenshot stem | Main source |
| --- | --- | --- |
| [All facilities, 47:84](https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI?node-id=47-84) | `industry-facilities` | `crates/osg-client/src/ui/shell/industry.rs:91` |
| [Facility console, 47:493](https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI?node-id=47-493) | `industry-production` | `crates/osg-client/src/ui/shell/industry.rs:218` |
| [Service and pricing, 49:123](https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI?node-id=49-123) | `industry-pricing` | `crates/osg-client/src/ui/shell/industry/service.rs:80` |
| [Public facility search, 49:604](https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI?node-id=49-604) | `industry-public-search` | `crates/osg-client/src/ui/shell/industry/service.rs:302` |
| [New public job, 50:178](https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI?node-id=50-178) | `industry-public-job` | `crates/osg-client/src/ui/shell/industry/service.rs:394` |
| [Own shipyard build, 50:614](https://www.figma.com/design/kvxDvZDyEbkfgU25IDzyyd/toy-sim-Client-UI?node-id=50-614) | `industry-shipyard` | `crates/osg-client/src/ui/shell/industry.rs:381` |

## P0: Restore the industry workspace composition

All six Figma frames use an approximately 1180 × 800 floating industry window with a persistent 265 px facility navigation column. Its search field, system groups, state dots, selected rows, and alert badge remain available while the content changes. Current `industry::draw` renders a heading, mode tabs, facility dropdown, paging buttons, owner, content tabs, and capabilities in one vertical stack. There is no facility sidebar. About 285 px of the screenshot height is consumed before pricing or production content begins. See `industry.rs:56–257`.

Implement the sidebar and content split first; the current stacked structure prevents the subsequent screens from resembling their references even with correct colors and data.

## P0: Replace the overview list and production form with the designed dashboard

**All facilities:** Figma 47:84 shows four KPI cards (power, lanes, stalled work, outside revenue), an amber attention banner, and a striped facility table with modules, power bars, lanes, queue, and outside work. Current `industry-facilities-1600x900.png` contains a single outlined horizontal row and pagination, occupying only the top 220 px. Its renderer only emits facility name, owner, capabilities, and an Open console button (`industry.rs:103–122`). The absent cards, alert, and table are implementation omissions, beyond the fixture having one facility.

**Facility console:** Figma 47:493 shows a facility header, three resource cards, and four bordered module sections with running lane cards, progress bars, time, power, queue chips, and green/amber/red status tags. Current `industry-production-1600x900.png` shows two capability text lines followed by recipe selection and an input/output form. The production and jobs tabs separate information that the design displays together. There is no console dashboard layout in `industry.rs:268–379`; `jobs` at line 674 is a separate tab. A rich fixture alone cannot reproduce the Figma console.

## P1: Rebuild public search as a comparison table

Figma 49:604 places recipe/module selection, batches, and sorting above a table with facility, customer tier, quote, free lanes, queue, and stock columns. Selecting a facility reveals a broad summary card and a highlighted New job action. Current `industry-public-search-1600x900.png` shows a 357 px wide outlined card containing a facility name and a raw rate sentence, followed by an empty selection message. No comparison columns, quote sorting controls, stock column, or selected detail card are rendered in `service.rs:302–391`. The fixture also deliberately selects no facility (`tests.rs:207`), leaving the reference's selected state unverified.

## P1: Make new job and ship construction proper detail sheets

Figma 50:178 keeps the facility overview visible and opens a roughly 480 px right sheet. It contains a recipe search, categorized recipe rows, batches, aligned Need/Here/Short columns, a stock-elsewhere warning with an Assets action, output fit information, itemized price card, payer selector, and full-width queue action. Current `industry-public-job-1600x900.png` adds the entire job form below the public result list. Inputs and price are unaligned text sentences, the payer is above the form, and there is no dedicated sheet or stock-elsewhere callout (`service.rs:394–481`). At 900 × 650, the primary queue action is below the visible area.

Figma 50:614 similarly keeps the console visible behind a New build sheet with Blueprint/Import tabs, a dashed import drop zone, valid-file badge and metadata, materials table, grouped fit/berth/queue checks, price notice, and full-width action. Current `industry-shipyard-1600x900.png` uses dropdowns, short text rows, then a separate import path field. File dropping is supported in code, but has no visible drop-zone treatment (`industry.rs:381–525`). The existing fixture selects a builtin blueprint with missing inputs, so it also fails to exercise the reference's successfully imported, valid design state.

## P1: Restore pricing table hierarchy and contextual color

Figma 49:123 presents the accept-jobs toggle and currency in one raised card, rates in a striped table with aligned numeric columns and revenue, ordered customer tiers in another striped table, and a quote-preview strip at the bottom. Current `industry-pricing-1600x900.png` uses a checkbox, separate currency row, repeated inline rate labels/fields, and compact tier rows with large arrow/removal controls (`service.rs:80–285`). Revenue and quote preview are absent. All prices and tiers visually compete at one level. At 900 × 650, the heading/capability stack means only the first customer tier is initially visible.

The shared palette already exposes cyan, green, amber, red, and raised blue surfaces in `crates/osg-ui/src/desktop.rs:6–17`. The industry renderer uses few of the designed surfaces, status badges, progress indicators, warning fills, or striped rows, so the semantic colors occupy much less of the page. Applying these surfaces and state treatments is more consequential than changing a single global background color. Keep the requested Charon font, while adding small section labels, aligned numerical columns, and larger KPI values.

## P0 verification gap: screenshots are component captures with sparse fixtures

The industry gallery directly calls `industry::draw` on the root UI (`crates/osg-client/src/ui/shell/tests.rs:212–226`), whereas the actual application calls it through `shell.desktop.show` (`crates/osg-client/src/ui/shell/panels.rs:390`). The software rasterizer starts unpainted pixels at `#0f0f0f` (`tests/software.rs:44`). Consequently these screenshots omit the production window surface, title bar, margins, rail, status bar, and surrounding scene. Their black background and edge-to-edge content cannot establish how the actual application compares to the full Figma screens.

The fixture has one facility, two operational capabilities, empty items/products/jobs, no location, one recipe, one builtin blueprint, two customer tiers, and sufficient public customer stock (`tests.rs:14–155`). It does not cover the reference's multiple system groups, stalled job, offline module, mixed currencies, public customer work, insufficient stock warning, or validated import. The tests check outer bounds and no emitted intents; they do not establish visual correspondence.

Render the real desktop composition at 1600 × 900 with representative data for those states. Retain compact captures to check scroll access and actions, but do not claim a 900 × 650 Figma match: the audited references are 1600 × 900. Existing overflow assertions are useful and insufficient for appearance verification.

## Suggested correction order

1. Capture the actual desktop/window composition with populated fixtures so the next comparison measures the product appearance.
2. Add the facility sidebar, overview KPI cards/table, and module lane dashboard.
3. Add the right job/build sheets and public comparison table.
4. Apply shared table/card/status components to pricing, inputs, checks, and quote summaries.
5. Recheck all six desktop states and compact access to primary actions through software screenshots.
