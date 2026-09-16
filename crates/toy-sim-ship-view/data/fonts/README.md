# MFD font

`IosevkaFixed-Regular.ttf` is the only font embedded for programmable screens and bezel labels.

## Attribution and license

- Font: Iosevka Fixed, Regular. The font's name table lists the family "Iosevka Fixed", the full name "Iosevka Fixed Regular" and version 34.8.1.
- Copyright (c) 2015-2026, Renzhi Li (aka. Belleve Invis, belleve@typeof.net).
- License: SIL Open Font License, Version 1.1. The full text, including the copyright notice, is in [LICENSE.md](LICENSE.md).

The OFL permits bundling and embedding the font with software as long as each copy carries the copyright notice and license. Keep `LICENSE.md` next to the font file. Under the license, the font may not be sold by itself, and modified versions may not use any Reserved Font Name without permission. Read the license text for the full conditions.

## How the code uses it

[src/mfd/font.rs](../../src/mfd/font.rs) includes the file with `include_bytes!` and registers it with egui as a dedicated font family named `toy-sim-mfd-iosevka-fixed`. Other egui fonts stay unchanged.

- `MfdFontPlugin` installs the family into every egui context before its first pass. `toy-sim` adds this plugin in its GUI plugin.
- `install(ctx)` does the same for code that uses the painter without Bevy.
- Screen text is laid out one character per 8 × 16 pixel cell. The font size is fitted so the glyph advance and row height fill the cell at the current scale ([src/mfd.rs](../../src/mfd.rs)).
- Glyph coverage is checked against the font's character map through `skrifa`. Characters without a glyph are drawn as `?`.

Replacing the font changes every screen's appearance. If you replace it, keep its license file with it and update the family registration. See [docs/mfds.md](../../../../docs/mfds.md) for the screen format.
