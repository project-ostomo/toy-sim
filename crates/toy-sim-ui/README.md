# Shared UI

`toy-sim-ui` owns the workspace's egui integration, theme, fonts and reusable
widgets. The ship editor and simulator client both install `UiPlugin` after
Bevy's default plugins. This plugin installs `EguiPlugin`, configures each new
context before its first egui pass, and registers the dedicated MFD font.
Applications use the `egui` and `bevy_egui` exports from this crate.

The default theme uses square corners and dark blue-gray surfaces. Widget spacing
is 12 × 8 points, button padding is 10 × 6 points, window margins are 12 points,
and the minimum control height is 28 points. Body and button text use 14-point
Iosevka Aile. Change these defaults in [theme.rs](src/theme.rs).

The fonts are bundled in the crate and embedded with `include_bytes!`; they do not
require a system installation or a runtime download. Iosevka Aile is the primary
proportional font and Iosevka is the primary monospace font. Programmable
screens retain their dedicated Iosevka Fixed family and fixed cell layout.
Font versions, provenance and licenses are in [data/fonts](data/fonts/README.md).

Reusable widgets live in `desktop`, `icons`, `instruments`, `mfd`, `screens` and `parts`. Application-specific
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

## Desktop toolkit

[desktop.rs](src/desktop.rs) provides the shared window manager, launcher rail,
status bar, icon buttons, action buttons, meters and palette. `WindowSpec` defines
a stable ID, title, initial size, minimum size, default anchor, offset and initial
visibility. Keep a `Desktop` in application state and call `show` each egui pass.

```rust,ignore
let overview = WindowSpec {
    id: "overview",
    title: "OVERVIEW",
    size: egui::vec2(390., 380.),
    min_size: egui::vec2(350., 230.),
    anchor: egui::Align2::RIGHT_TOP,
    offset: egui::Vec2::ZERO,
    open: true,
};
desktop.show(ctx, overview, |ui| {
    ui.label("Application content");
});
```

`toggle`, `open` and `is_open` connect launcher buttons to windows. Native egui
windows provide focus, dragging from titles or unused backgrounds, resizing and
close. Interactive controls retain their own input. On drag release,
nearby edges snap to the workspace or neighboring windows. The workspace reserves
space for the launcher and footer. Default windows shrink to the available height
when the viewport shrinks; offscreen windows return within the workspace.
`locked` disables dragging and resizing. `reset` restores initial visibility,
positions and sizes through a new layout generation. Layout state is held in memory.

`launcher` and `status_bar` provide shared chrome while applications supply their
contents. `Icon::text` and `Icon::font` use the embedded Phosphor family. Buttons
have fixed bounds across idle, hover, pressed and disabled states. Applications
own ECS queries, selection, filtering and command dispatch. The toolkit has no
simulation or transport dependency beyond the crate's existing widgets.

The client implementation is in [ui/shell](../toy-sim-client/src/ui/shell.rs).
Its panel proportions and information hierarchy were informed by
[EVE Online Photon UI screenshots](https://forums.eveonline.com/t/welcome-to-photon-ui-sisi/354885?page=7).
No EVE artwork is bundled.
