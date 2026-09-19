# Embedded fonts

The fonts are included with `include_bytes!`. Applications need no installed
fonts or runtime font files.

## Iosevka Aile and Iosevka

The shared theme uses Iosevka Aile Regular for proportional text and Iosevka
Regular for monospace text. Both are unmodified TTF files from the latest upstream
release checked on September 18, 2026: [v34.8.1](https://github.com/be5invis/Iosevka/releases/tag/v34.8.1),
published August 22, 2026. Their SIL Open Font License 1.1 is included as
[LICENSE.md](LICENSE.md), identical to the license in that release.

- `IosevkaAile-Regular.ttf`: [upstream archive](https://github.com/be5invis/Iosevka/releases/download/v34.8.1/PkgTTF-IosevkaAile-34.8.1.zip).
  File SHA-256: `3f4426136e9d90706e40aa45c83947eaed0007261a67f01488ba2f14f38117b1`.
- `Iosevka-Regular.ttf`: [upstream archive](https://github.com/be5invis/Iosevka/releases/download/v34.8.1/PkgTTF-Iosevka-34.8.1.zip).
  File SHA-256: `8b6065f04ca4ff4ce95ae48cf9f2f31b584823557adc38ec817dbe6f3745624b`.

The downloaded archives were verified against the SHA-256 digests in GitHub's
release metadata. Only the regular face from each archive is bundled.

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
separate `Phosphor` font family. The theme also installs Iosevka Aile as that family's
fallback, so icon codepoints do not change the application's text font selection.
