# Ship editor

`toy-ship-editor` ([apps/toy-ship-editor](../apps/toy-ship-editor)) builds and edits `.ship` blueprints, installs custom firmware, configures avionics, and launches the simulator with the current design. The data model it edits is described in [ships.md](ships.md).

## Running

```sh
cargo run -p toy-ship-editor                       # empty design, file field set to ship.ship
cargo run -p toy-ship-editor -- path/to/design.ship  # open a design
```

The editor also has two non-interactive modes:

```sh
# Write the unarmed starter design (default path: starter.ship in the working directory)
cargo run -p toy-ship-editor -- --example my-starter.ship

# Load, compile against the built-in catalogue, validate the controller, and print a summary
cargo run -p toy-ship-editor -- --validate assets/ships/starter.ship
```

`--validate` prints `<name>: <n> parts, <mass> kg dry` and exits with an error if the design or program is invalid. Any other argument that starts with `--` is rejected.

The window opens at 1400 × 900. Assets load from the repository's `assets/` directory.

## Layout

- **Toolbar:** New, Starter, file path field, Open, Save (shows `Save *` when there are unsaved changes), Undo, Redo, the Assembly / Systems mode switch, and Launch sim.
- **Left panel:** in Assembly mode, the part palette, "Select mode", and the placement rotation with a "Rotate placement (R)" button. In Systems mode, a short description of the standard avionics.
- **Right inspector:** in Assembly mode, a Preview thrust slider. In both modes: ship name, flight computer selection, "Custom firmware (advanced)", "Launch settings", and details for the selected part.
- **Centre:** the 3D viewport in Assembly mode, or the systems panel in Systems mode.
- **Bottom status bar:** the validation result (green summary or red error), the last status message, and a controls reminder in Assembly mode.

The file path field is relative to the working directory. Open replaces the design (you can undo this) and clears the dirty flag. Save writes atomically through a temporary `.ship.tmp` file.

Undo keeps up to 100 previous states. Any edit clears the redo stack. New and Starter are ordinary edits and can be undone.

## Assembly mode

| Input | Effect |
| --- | --- |
| Click a palette entry | Choose a prototype to place |
| Click in the viewport while placing | Place the ghost part if it does not overlap |
| Click in the viewport with nothing selected for placement | Select the nearest part under the cursor, or clear the selection |
| Right-drag | Orbit the camera |
| Middle-drag | Pan |
| Mouse wheel over the viewport | Zoom (distance clamped between 0.3 and 100,000) |
| R | Advance placement rotation through the 24 orientations |
| Delete | Delete the selected part |
| Esc | Cancel placement |

Keyboard shortcuts are ignored while a text field has focus.

**Placement.** The ghost snaps to the face under the cursor: it is centred on the hit point and pushed flush against that face. When the cursor hits no part, the ghost sits on the y = 0 plane. It is outlined green when free and red when it overlaps a part. Placement checks only overlap; validation reports connectivity problems after the part is added. Placed parts get the next free ID. Device parts get a default alias `<prototype>_<id>`, with a numeric suffix if needed. Structural parts get no alias.

**Selected part inspector.** Shows the title, mass, hull, dimensions and equipment values. It also offers editable name (up to 64 characters), grid position, rotation (0 to 23), "Duplicate for placement", which selects the same prototype and rotation, and "Delete part".

Deleting a part also removes it from the actuator exclusion list.

**Preview thrust** drives engine plume visuals in the viewport between 0 and 1. It has no effect on the design.

The viewport draws a 40 m reference grid, a yellow outline around the selected part, and the ghost outline.

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

After every change, the editor compiles the blueprint and validates its controller program. On success, the status bar shows `<parts> parts · <dry mass> kg dry · <storage> m³ storage · <hull> hull`. Otherwise it shows the first error, for example `parts 3 and 5 overlap`, `assembly must be connected by faces`, or `duplicate device alias main_engine`.

## Launching the simulator

"Launch sim" is enabled when validation passes and no launched simulator is still running. It:

1. Compiles and validates again.
2. Saves a snapshot to `<temp>/toy-ship-editor/launch-<pid>-<revision>.ship`.
3. Starts `toy-sim --ship <snapshot>`.

By default it looks for `toy-sim` (or `toy-sim.exe`) next to the editor's own executable. Build both in the same profile first:

```sh
cargo build -p toy-sim -p toy-ship-editor
cargo run -p toy-ship-editor
```

To use a different binary, set its path under "Launch settings" in the inspector. When the child process exits, the status bar reports its exit status.

## Tests

```sh
cargo test -p toy-ship-editor
```

The tests check that `assets/ships/starter.ship` compiles with two weapons and a valid program, that deleting and undoing restores identical bytes that survive a save and reload, and that viewport picking returns the nearest face.
