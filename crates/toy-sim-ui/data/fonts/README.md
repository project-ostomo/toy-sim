# Embedded fonts

The fonts are included with `include_bytes!`. Applications need no installed
fonts or runtime font files.

## Sarasa UI SC

- File: `SarasaUiSC-Regular.ttf`, copied without modification from the locally
  installed Sarasa distribution.
- Family: Sarasa UI SC; style: Regular.
- Embedded version: `Version 1.0.5; ttfautohint (v1.8.4)`.
- SHA-256: `805b45397799f3c89b366a32e8dc53e4755ce53edfcf1b401fe713308247c9f4`.
- Upstream: [Sarasa Gothic](https://github.com/be5invis/Sarasa-Gothic).
- License: SIL Open Font License 1.1; the release's license is included as
  [SARASA-LICENSE.md](SARASA-LICENSE.md), copied from
  [the v1.0.5 source](https://github.com/be5invis/Sarasa-Gothic/blob/v1.0.5/LICENSE).

The font's embedded copyright notice is:

> Copyright (c) 2015-2024, Renzhi Li (aka. Belleve Invis, belleve@typeof.net).
> Portions Copyright (c) 2016-2020 The Inter Project Authors.
> Portions Copyright (c) 2014, 2015 Adobe Systems Incorporated (http://www.adobe.com/).
> Portions Copyright (c) 2012 Google Inc.

[theme.rs](../../src/theme.rs) registers Sarasa as the first proportional family
and the final monospace fallback. Egui's other fallback fonts remain available.
The complete font is embedded, including its Chinese glyphs.

## Iosevka Fixed

`IosevkaFixed-Regular.ttf` is the dedicated font for programmable screens and
bezel labels. Its embedded version is 34.8.1. Copyright (c) 2015-2026, Renzhi Li
(aka. Belleve Invis, belleve@typeof.net). Its SIL Open Font License 1.1 is included
as [LICENSE.md](LICENSE.md).

[src/mfd/font.rs](../../src/mfd/font.rs) registers the family
`toy-sim-mfd-iosevka-fixed`. `UiPlugin` includes `MfdFontPlugin`; standalone egui
users call `mfd::install_font(ctx)` before drawing.

Screen text occupies one 8 × 16 pixel cell per Unicode scalar, with glyphs fitted
to the cell at the current scale. Coverage is checked against the embedded font's
character map through `skrifa`; missing glyphs become `?`. See
[the MFD guide](../../../../docs/mfds.md) for the drawing protocol.

## Phosphor

`Phosphor.ttf` is the unmodified regular icon font from
[Phosphor Web v2.1.1](https://github.com/phosphor-icons/web/tree/v2.1.1/src/regular).
The MIT license is included as [PHOSPHOR-LICENSE](PHOSPHOR-LICENSE).
[icons.rs](../../src/icons.rs) embeds the font and exposes named icons and a
separate `Phosphor` font family. The theme also installs Sarasa as that family's
fallback, so icon codepoints do not change the application's text font selection.
