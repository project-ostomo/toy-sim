# Programmable screens and MFDs

Flight computer firmware can define up to eight programmable screens. Each screen is a 2D pixel surface that the firmware redraws with a small set of primitives, plus twelve optional labelled bezel keys. The host validates and stores complete frames. Clients replay them with egui, with no framebuffer texture. Keyboard and pointer input flows back to the firmware as screen events.

Programmable screens are separate from the fixed instruments (attitude, navigation, contacts, weapons). For those, the firmware publishes data records and the client draws a fixed presentation ([ship-abi.md](ship-abi.md#instruments)).

Screens are drawn by one of two entry points, depending on the host:

- **Local simulator (`apps/toy-sim`).** Screens come from the flight computer's `ship_tick` callback, as described in [In the simulator](#in-the-simulator).
- **Authoritative server.** Screens come from `ship_display`, run in a separate display instance only while a network client subscribes. The frames are sent to that client in state frames ([Over the network](#over-the-network)).

The drawing imports and limits are the same for both.

Source:

- Syscalls: [imports/drawing.rs](../crates/toy-sim-ship-wasm/src/imports/drawing.rs), with event handling in [imports.rs](../crates/toy-sim-ship-wasm/src/imports.rs) and [computer.rs](../crates/toy-sim-ship-wasm/src/computer.rs)
- Frame model and limits: [drawing.rs](../crates/toy-sim-model/src/drawing.rs) in `toy-sim-model`, re-exported as `toy_sim_ship_wasm::screens`. The types are serde-serializable so frames can be sent over the network.
- Server display instances and screen input: [session.rs](../crates/toy-sim-server/src/sim/displays.rs)
- Painter and bezel widget: [mfd.rs](../crates/toy-sim-ship-view/src/mfd.rs)
- Custom screen widget and input mapping: [screens.rs](../crates/toy-sim-ship-view/src/screens.rs)
- Simulator windows: [gui/mfds.rs](../crates/toy-sim-client/src/ui/mfd.rs)
- Font: [crates/toy-sim-ship-view/data/fonts/README.md](../crates/toy-sim-ship-view/data/fonts/README.md)

## Defining screens

`screen_define(definition, bytes)` takes a `ScreenDefinition` (96 bytes):

| Field | Rule |
| --- | --- |
| `id` | 0 to 7 (`MAX_SCREENS` = 8) |
| `width`, `height` | 32 to 2048 pixels |
| `title` | Non-empty `Text64`, used as the window title |

Defining an existing ID replaces its definition. If the size changes, the stored frame for that screen is cleared. `screen_remove(id)` removes the definition and its frame, and acknowledges that screen's pending events.

Definitions belong to the committed session. They survive between callbacks and are cleared when the computer reboots.

## Drawing a frame

A frame is built between `screen_begin` and `screen_end` in a single callback:

1. `screen_begin(frame, bytes)` with `ScreenFrame { id, background }`. The screen must be defined, and its bit must be set in `TickContext.requested_screens`, otherwise the call returns `ERR_UNAVAILABLE`. A screen can begin only once per callback.
2. `screen_draw(screen, kind, parameters, parameter_bytes, payload, payload_bytes)` appends one primitive.
3. `screen_button(screen, key, label, bytes)` assigns or clears a bezel key label.
4. `screen_end(screen)` validates the frame and stages it for commit.

A frame that is begun but not ended is discarded. A committed frame stays on display until the firmware replaces it, redefines the screen with a new size, or removes it. Firmware can therefore redraw only when content changes; the [custom screen example](../crates/toy-sim-example-controller/examples/custom_screen.rs) redraws at most every 10 ticks unless its state changes.

Colours are `0xRRGGBB` values up to `0xFFFFFF`. Coordinates are signed 16-bit pixel positions measured from the top-left corner, and may fall outside the surface.

| Kind | Code | Parameters | Payload |
| --- | --- | --- | --- |
| `DRAW_PIXEL` | 0 | `ScreenPixel { color, x, y }` | none |
| `DRAW_TEXT` | 1 | `ScreenText { color, x, y }` | UTF-8 text, at most 4096 bytes |
| `DRAW_LINE` | 2 | `ScreenLine { color, x1, y1, x2, y2 }` | none |
| `DRAW_POLYLINE` | 3 | `ScreenPolyline { color }` | 2 to 256 `ScreenPoint { x, y }` records |
| `DRAW_RECTANGLE` | 4 | `ScreenRectangle { color, filled, x, y, width, height }` | none |
| `DRAW_ELLIPSE` | 5 | `ScreenEllipse { color, filled, cx, cy, rx, ry }` | none |

`filled` must be 0 or 1. Widths, heights and radii must fit in 16 bits.

Per-frame limits:

- 256 primitives (`MAX_SCREEN_PRIMITIVES`)
- 65,536 payload bytes in total (`MAX_SCREEN_PAYLOAD`)
- parameter records of at most 48 bytes, and payloads of at most 4096 bytes per draw
- a raster work estimate of at most 1,048,576 units. Estimates: pixel 1, text 128 per byte, line 1024, polyline 1024 per point, outlined rectangle 4096, filled rectangle its area clipped to 512 × 512, outlined ellipse 2048, filled ellipse its clipped bounding area plus 1024.

`screen_draw` costs one gas per 8 bytes of parameters and payload, in addition to the call cost.

### Text

Text uses a fixed grid of 8 × 16 pixel cells (`FONT_WIDTH`, `FONT_HEIGHT`). Each Unicode scalar value takes one cell, and `\n` starts a new line at the original x position. Spaces and control characters advance without drawing. Characters missing from the embedded Iosevka Fixed font are drawn as `?`.

### Bezel keys

`screen_button(screen, key, label, bytes)` sets key `0..=11`. Keys L1–L6 are 0 to 5 and R1–R6 are 6 to 11. Labels are UTF-8, at most 24 bytes and 6 characters. An empty label clears the key.

## Input events

Pending events are visible through `TickContext.screen_event_count`:

- `screen_event_read(index, out, bytes)` reads one `ScreenEvent` (128 bytes).
- `screen_event_ack(id)` acknowledges one. The ID must belong to an event visible in this callback.

Unacknowledged events are delivered again in later callbacks. Acknowledgements take effect only when the callback commits. Screen events wake a sleeping program.

`ScreenEvent` fields: `id`, `screen`, `kind`, `code`, `modifiers`, `x`, `y` (screen pixels, as `f64`) and `text` (`Text64`).

| Kind | Code | Meaning |
| --- | --- | --- |
| `EVENT_POINTER_MOVE` | 0 | Pointer moved over the screen, or anywhere while dragging |
| `EVENT_POINTER_PRESS` | 1 | `code`: 0 primary, 1 secondary, 2 other button |
| `EVENT_POINTER_RELEASE` | 2 | Same codes. Delivered even if the pointer left the screen during a drag. |
| `EVENT_KEY_PRESS` | 3 | `code` is a key code (below). Requires keyboard focus on the screen. |
| `EVENT_TEXT` | 4 | Typed text in `text`. Long input is split into 64-byte chunks. |
| `EVENT_KEY_RELEASE` | 5 | Same key codes |
| `EVENT_BEZEL` | 6 | Defined in the ABI. The current clients do not emit it. |
| `EVENT_RESET` | 7 | The host dropped queued input for this screen |

Key codes: single-character key names use their ASCII code (for example `'R'` = 82). The other mapped keys are Space 32, Enter 13, Escape 27, Tab 9, Backspace 8 and Delete 127, plus Up `0x110000`, Down `0x110001`, Left `0x110002`, Right `0x110003`, Home `0x110004`, End `0x110005`, Page Up `0x110006` and Page Down `0x110007`. Other keys produce no event.

Modifier bits: Shift 1, Ctrl 2, Alt 4, Command 8.

**Queueing.** The host holds up to 256 pending events per computer. Consecutive pointer moves on the same screen collapse into the newest one. When the queue is full, the affected screen's queued events are replaced by one `EVENT_RESET`. If other screens still fill the queue, each screen is collapsed to a single reset carrying its newest event ID. Firmware should treat a reset as "release every held button and key for this screen". Rebooting clears all pending events.

Clicking the screen gives it keyboard focus. Key and text events are taken from egui while the screen is focused, so they do not reach other widgets. The simulator's own keyboard controls are also skipped while egui wants keyboard input.

## In the shared client

The shared frontend opens MFD windows from the server's screen definitions. The "MFD slots" section also offers slots 1–8 before definitions are available, allowing the first subscription to start the display instance. Each window renders drawing primitives and forwards pointer, drag, key and text events. Closing a window unsubscribes and suppresses older buffered screen frames. The same frontend runs in the remote client and the debug launcher.

Screens execute in separate server display instances through `ship_display`. The flight callback does not render an MFD. No host widgets are embedded in the drawing surface, so third-party clients can choose their own shell and visual presentation.

## Over the network

The server path is described in full in [server-client.md](server-client.md#display-instances). In outline:

1. A client sends `ScreenSubscribe { ship, slot, hz }` for a ship its account controls. `hz` is 1 to 10.
2. While the ship is in space and has at least one subscriber, the server keeps a display instance of the ship's firmware. The instance calls `ship_display` with the flight computer's latest observation. Its `requested_screens` holds the subscribed slots that are due at the highest requested rate.
3. Each completed frame for a subscribed slot becomes a `ScreenUpdate` in the client's state frames. Its `revision` identifies the current display instance; ownership changes revoke that instance. A failed callback replaces the frame with an error string.
4. `ScreenInput { slot, revision, kind, code, modifiers, xy, text }` queues a `ScreenEvent` on the display instance. The client must be subscribed, and `revision` must match the displayed update.
5. The instance is dropped 10 ticks after the last subscriber leaves, or when control of the ship changes.

The display instance cannot write devices or issue world commands. It does not share memory with the flight instance. Firmware without a `ship_display` export gets no display instance; each subscribed slot is reported with no frame and the error "Display unavailable". The standard firmware exports a drawing-only `ship_display`: each requested slot becomes a 512 × 256 "Ship status" screen with simulation time, speed, mass and battery energy as text. It does not read screen events, so clicks on it do nothing.

The shared UI supports every defined slot and complete input forwarding. [Display tests](../crates/toy-sim-server/src/sim/displays.rs) cover shared viewers, expiry, ownership and power revocation, input forwarding and stale instance revisions.


## The bezel MFD widget

`toy_sim_ship_view::MfdRenderer::show(ui, id, frame)` draws a square screen with six bezel buttons on each side. Buttons show their labels in the MFD font, show "—" when unassigned, and are clickable only when labelled. Each button's tooltip names the key and an F-key hint (`F1`–`F6` for the left column, `Shift+F1`–`F6` for the right). The widget returns a `MfdResponse` with the clicked `BezelKey` values and whether the screen was clicked. It does not bind F-keys itself. The simulator does not use this widget at present. Its behaviour is covered by the tests in [mfd/tests.rs](../crates/toy-sim-ship-view/src/mfd/tests.rs).

## Painting

`toy_sim_ship_view::mfd::paint(painter, rect, frame)` draws a complete frame:

- It returns false and draws nothing for an invalid frame or an empty destination.
- The frame is scaled uniformly to fit the destination and centred. Drawing is clipped to the scaled surface.
- The background fills the surface. Primitives are drawn in order: pixels as scaled squares, lines as strokes one scaled pixel wide through pixel centres (clipped before drawing), rectangles filled or stroked inside, ellipses as tessellated paths with an outline.
- Text is fitted to the scaled cell size.

Install the font first, either with `MfdFontPlugin` in Bevy or `mfd::install_font(ctx)` without it.

## Tests

```sh
cargo test -p toy-sim-ship-view
cargo test -p toy-sim-ship-wasm
```

The view tests check:

- logical coordinates and draw order
- the text grid at different DPIs
- bounded curve work and off-screen fills
- rejection of invalid frames
- replacing and clearing without textures
- bezel clicks
- coordinate mapping, focus, text chunking and drag release in the custom screen widget

The WASM tests cover unfinished frames and the custom screen firmware responding to a click. The firmware runs through `instantiate_display` in that test.

```sh
cargo test -p toy-sim-server --lib sim::displays::tests
```
