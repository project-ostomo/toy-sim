# Ship editor

The editor uses the shared [toy-sim-ui](../crates/toy-sim-ui/README.md) theme: roomier controls, dark blue-gray surfaces and embedded Iosevka Aile and Iosevka fonts.

`toy-ship-editor` ([apps/toy-ship-editor](../apps/toy-ship-editor)) builds and edits `.ship` blueprints, installs custom firmware, configures avionics, and launches the simulator with the current design. The data model it edits is described in [ships.md](ships.md).

## Running

```sh
cargo run -p toy-ship-editor                       # empty design, file field set to ship.ship
cargo run -p toy-ship-editor -- path/to/design.ship  # open a design
```

The editor also provides non-interactive blueprint generation and validation:

```sh
# Write the unarmed starter design (default path: starter.ship in the working directory)
cargo run -p toy-ship-editor -- --example my-starter.ship

# Write a micropulse demonstrator with preloaded charges and supporting hardware
cargo run -p toy-ship-editor -- --micropulse-example my-micropulse.ship

# Load, compile against the built-in catalogue, validate the controller, and print a summary
cargo run -p toy-ship-editor -- --validate assets/ships/starter.ship
```

`--validate` prints `<name>: <n> parts, <mass> kg dry` and exits with an error if the design or program is invalid. Any other argument that starts with `--` is rejected.

The window opens at 1400 × 900. Loading or replacing a ship frames its physical bounds in the viewport. Assets load from the repository's `assets/` directory.

## Layout

- **Toolbar:** New, Starter, file path field, Open, Save (shows `Save *` when there are unsaved changes), Undo, Redo, the Assembly / Systems mode switch, and Launch sim.
- **Left panel:** in Assembly mode, a searchable catalogue with category filters and model thumbnails. Hover a tile for specifications; click it to pick up that part. Rotation, connector selection and selection controls stay below the scrolling catalogue. In Systems mode, a short description of the standard avionics.
- **Right inspector:** the Part tab shows the picked or selected part, with a larger model preview and grouped physical, performance and resource specifications. The Ship tab contains the ship name, Preview thrust slider, flight computer, firmware import and launch settings.
- **Centre:** the 3D viewport in Assembly mode, or the systems panel in Systems mode.
- **Bottom status bar:** the validation result (green summary or red error), the last status message, and a controls reminder in Assembly mode.

The file path field is relative to the working directory. Open replaces the design (you can undo this) and clears the dirty flag. Save writes atomically through a temporary `.ship.tmp` file.

Undo keeps up to 100 previous states. Any edit clears the redo stack. New and Starter are ordinary edits and can be undone. Starter loads the small water-NTR patrol.

## Assembly mode

| Input | Effect |
| --- | --- |
| Click a catalogue tile | Pick up a prototype and open its Part inspector |
| Click a matching socket while placing | Attach the preview if the assembly validates |
| Click in the viewport with nothing selected for placement | Select the nearest part under the cursor, or clear the selection |
| Right-drag | Orbit the camera |
| Middle-drag | Pan |
| Mouse wheel over the viewport | Zoom (distance clamped between 0.3 and 100,000) |
| R | Rotate around the connection in quarter turns |
| Delete | Delete the selected part and attached descendants when not placing |
| Esc | Cancel placement |

Keyboard shortcuts are ignored while a text field has focus.

**Placement.** A blueprint has one root at the assembly origin. Pick a catalogue part, choose its plug in the connector selector, then click a cyan marker on a compatible, unused socket. Connector types must match. Hull connections include their size; station backbones use a separate connector type. Parts may expose equipment sockets for weapons and other hardware.

The actual part model previews the resolved attachment transform, with a green outline when valid and red when rejected. R selects one of four quarter-turn rolls around the connection. The first part becomes the root when you click the empty viewport. Further parts require a socket; there is no placement grid or free surface placement. Collision compilation separately uses a coarse 1 m grid.

Placed parts get the next free ID. Device parts get a default alias `<prototype>_<id>`, with a numeric suffix if needed. Structural parts get no alias.

**Part inspector.** Picking a catalogue part or selecting an installed part opens the Part tab. Hovering other tiles leaves this pane unchanged. Specifications use SI units and describe the equipment type: capacities, thrust or torque, power, consumption, shield properties and weapon ratings. Derived figures include engine exhaust velocity, generator waste heat and weapon energy per shot. RCS consumption is labelled per active axis.

An installed part offers an editable name (up to 64 characters), its parent socket and plug, a quarter-turn roll slider, "Duplicate for placement", and "Delete part". The root is labelled separately. Picking up a prototype preserves the last installed selection; Escape returns to it. Placement remains active after adding a part. Searching or filtering does not change the active placement or edit the blueprint. New, Open and Starter clear placement and selection.

**Resource tanks.** Select an installed fuselage part and use **Add tank** in its Part inspector. Each tank has a resource, volume in m³, and starting fill slider. The capacity bar shows allocated volume, including empty tank space. The 8m section provides 500 m³ and the end provides 40 m³. The 4m variants provide 62.5 m³ and 5 m³, and the 2m variants provide 7.8125 m³ and 0.625 m³. These provisional capacities are catalogue values. Each part supports up to 32 tanks. Reduce one tank's volume to make room for another. Removing a tank releases its allocation.

The inspector shows each resource's density, loaded mass, and full mass. The Ship tab also shows the total starting tank contents mass. Tank settings survive saving, undo/redo, and duplication. Invalid resource IDs, volumes, fills, or allocations exceeding the part's capacity prevent launch.

Deleting a part removes its attached descendants and clears their actuator exclusions. Undo restores the assembly.

**Preview thrust** drives engine plume visuals in the viewport between 0 and 1. It has no effect on the design.

The viewport draws compatible socket markers, a yellow outline around the selected part when not placing, and the placement validity outline. Habitat rings animate around their fixed hubs in the editor.

Thumbnails share one offscreen atlas, with a 256 × 256 pixel cell per prototype. It uses isolated lighting and fits model bounds into each cell, including weapon barrels and model children that arrive asynchronously. The camera runs only while preview UI is visible. The grid, tooltips and inspector reuse the same texture.

## Systems mode

The systems panel ([devices.rs](../apps/toy-ship-editor/src/devices.rs)) edits `avionics` and device metadata:

- The standard avionics summary: 71 kg distributed mass, 101 W base power, 1000 W sensor power, 100,000 km sensor range.
- "Enable contact sensor by default" (`sensor_enabled`).
- Control orientation: a list of the 24 orientations labelled by their forward and up axes.
- Available forward thrust, summed from control-enabled engines and RCS axes along the forward axis. A warning appears when there is no forward propulsion, and another for each axis without bidirectional steering authority ("automatic rendezvous unavailable").
- "Actuator groups": one checkbox per group label on engines, RCS blocks, torquers and weapons. It enables or excludes all members at once.
- "Physical equipment": for each device part, a checkbox "Use for automatic flight control" (or "Use for automatic weapons control" on weapons). An advanced section edits name, script alias and groups (up to 16).

Authority estimates depend only on geometry. The panel notes that power, fuel and saturation also limit real authority.

## Firmware

- **Standard firmware** selects `Firmware::Standard`, the bundled flight computer ([ships.md](ships.md#the-standard-firmware)).
- **Custom firmware (advanced):** enter a path to a compiled `.wasm` file and press "Import WASM". The editor runs `ControllerRuntime::validate_program` on it: imports, exports, memory limits, and an actual instantiation that checks `ship_api_version`. On success the program bytes are embedded in the blueprint as `Firmware::Custom`, and the program size is shown. See [ship-abi.md](ship-abi.md).

## Validation

After every change, the editor compiles the blueprint and validates its controller program. On success, the status bar shows `<parts> parts · <dry mass> kg dry · <storage> m³ cargo · <tank space> m³ tank space · <hull> hull`. Otherwise it shows the first error, for example `parts 3 and 5 overlap`, `incompatible connectors`, or `duplicate device alias main_engine`.

## Launching the simulator

"Launch sim" is enabled when validation passes and no launched simulator is still running. It:

1. Compiles and validates again.
2. Saves a snapshot to `<temp>/toy-ship-editor/launch-<pid>-<revision>.ship`.
3. Starts `toy-sim-debug --ship <snapshot>`.

By default it looks for `toy-sim-debug` (or `toy-sim-debug.exe`) next to the editor's own executable. Build the editor, debug launcher and server in the same profile first:

```sh
cargo build -p toy-sim-debug -p toy-sim-server -p toy-ship-editor
cargo run -p toy-ship-editor
```

To use a different binary, set its path under "Launch settings" in the inspector. When the child process exits, the status bar reports its exit status.

## Tests

```sh
cargo test -p toy-ship-editor
```

The tests check that `assets/ships/starter.ship` compiles with two weapons and a valid program, that deleting and undoing restores identical bytes that survive a save and reload, that viewport picking returns the nearest face, that browsing preserves the blueprint and selection, and that preview bounds and render layers handle model children arriving later.

## Trying micropulse propulsion

Open `assets/ships/micropulse-demo.ship` in the editor and choose **Launch sim**, or run `cargo run -p toy-sim-debug -- --ship assets/ships/micropulse-demo.ship`. Build the server, debug launcher, and editor together first using the command above.

The demonstrator has an 8m engine, a fuselage tank allocated to micropulse charges at 20% starting fill, a forward cap, battery, command module, shield, coolant reserve, and torquer. All four micropulse engine sizes are also available under Propulsion in the catalogue. For a custom ship, configure a fuselage tank for **Micropulse charges (kg)** and include a battery to start the avionics. Micropulse thrust needs no separate bulk propellant or electrical input. Electricity is recovered while firing; there is no idle generation in this version.

## Station assembly

Open `assets/ships/neris-anchorage.ship` for a complete station or search for station parts in the catalogue. Its cylindrical backbone, habitat hub, docking hangar and beacon connect through station backbone nodes; ship-class weapons attach through equipment nodes. The model library is in `assets/models/stations/station-catalogue.blend`. See [stations-navigation.md](stations-navigation.md) for the catalogue and runtime behavior.
