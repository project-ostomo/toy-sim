# Shared UI

`toy-sim-ui` owns the workspace's egui integration, theme, fonts and reusable
widgets. The ship editor and simulator client both install `UiPlugin` after
Bevy's default plugins. This plugin installs `EguiPlugin`, configures each new
context before its first egui pass, and registers the dedicated MFD font.
Applications use the `egui` and `bevy_egui` exports from this crate.

The default theme uses square corners and dark blue-gray surfaces. Widget spacing
is 12 × 8 points, button padding is 10 × 6 points, window margins are 12 points,
and the minimum control height is 28 points. Body and button text use 14-point
Sarasa UI SC. Change these defaults in [theme.rs](src/theme.rs).

The font is bundled in the crate and embedded with `include_bytes!`; it does not
require a system installation or a runtime download. Sarasa is the primary
proportional font and a fallback for missing monospace glyphs. Programmable
screens retain their dedicated Iosevka Fixed family and fixed cell layout.
Font versions, provenance and licenses are in [data/fonts](data/fonts/README.md).

Reusable widgets live in `instruments`, `mfd`, `screens` and `parts`. Application-specific
panels, ECS queries and scene HUD logic stay with their application. The 3D ship
renderer lives in `toy-sim-ship-view` and has no egui dependency.

`parts::PartDescription` builds typed specification rows from a `PartDef` and
`Catalogue`. Builders match equipment types, so new catalogue entries using an
existing type need no UI code. Tiles, hover cards and inspector sections share
these descriptions and accept texture IDs with atlas UV coordinates. The editor
owns selection, filtering, placement and the offscreen preview scene.

Code using egui without Bevy can call `theme::install(ctx)` before its first pass,
and `mfd::install_font(ctx)` when it uses the programmable screen widgets.

```sh
cargo test -p toy-sim-ui --offline
cargo run -p toy-ship-editor
```
