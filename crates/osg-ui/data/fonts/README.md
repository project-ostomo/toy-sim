# Embedded fonts

The client embeds these fonts with `include_bytes!`; it needs no installed fonts
or runtime font downloads. The shared theme uses **Iosevka Charon Regular** for
proportional text and **Iosevka Charon Mono Regular** for monospace text and MFDs.
Both families fall back to **Toy Sim CJK**, derived from Sarasa Mono SC.

## Sources and licenses

- Charon: [v34.801](https://github.com/jul-sh/iosevka-charon/releases/tag/v34.801),
  `iosevka-charon.zip`. Archive SHA-256:
  `e00b1c41d69f045be34eda028959bf94367d0b053fe7e2bd4ccab8b1aa58ed32`.
  The two Regular faces are unmodified. License: [CHARON-OFL.txt](CHARON-OFL.txt).
- Sarasa: [v1.0.41](https://github.com/be5invis/Sarasa-Gothic/releases/tag/v1.0.41),
  `SarasaMonoSC-TTF-Unhinted-1.0.41.7z`. Archive SHA-256:
  `6e3ac724c4bf7d099aa44a2cc24ccdd4a3234c3248b13b3a4a76d570e8c79a26`.
  Source face: `SarasaMonoSC-Regular.ttf`. License: [SARASA-OFL.txt](SARASA-OFL.txt).

Archive hashes were verified against GitHub release metadata. Bundled file hashes:

| File | SHA-256 |
| --- | --- |
| IosevkaCharon-Regular.ttf | `7f6bc20d06a3d879f92d4635d962f575b07fbafda601b8b318252c4196b62c61` |
| IosevkaCharonMono-Regular.ttf | `145f0e34190048bcabb05be445351b77f5ac24b95681f7bb72d11642dbb00461` |
| ToySimCJK-Regular.ttf | `adbd744cad28d91737b8fc2456d2015956f0097b263149251c93d5fabcbf108f` |

## CJK derivation and width

The fallback retains all 43,527 mapped characters in the CJK ranges listed by
[build-cjk-font.py](../../../../scripts/build-cjk-font.py), including kana,
Hangul, Bopomofo, radicals, ideographs, and fullwidth/halfwidth punctuation.
Coverage is bounded by the upstream Sarasa face. It uses SC regional glyph forms;
this does not provide automatic Japanese/Korean/Traditional Chinese regional
shape selection for shared Han characters.

Charon Mono's cell is 500 units in a 1000-unit em. Sarasa's fullwidth glyphs
advance 1000 units in the same em: **exactly two mono cells** at equal font size.
Halfwidth forms retain one cell; combining forms retain zero advance. The build
verifies these metrics and aligns Sarasa's vertical metrics with Charon Mono.
Glyph outlines and horizontal advances are preserved. The derivative is renamed
Toy Sim CJK. No visual-only scaling workaround is used.

To reproduce with Python and `fonttools==4.57.0`, extract the archives and run:

```sh
python3 scripts/build-cjk-font.py \
  /path/to/SarasaMonoSC-Regular.ttf \
  /path/to/IosevkaCharonMono-Regular.ttf \
  crates/osg-ui/data/fonts/ToySimCJK-Regular.ttf
```

The `osg-ui` font test checks actual egui glyph and shaped text advances at
multiple font sizes and display scales. Pixel rasterization can still round
individual glyph positions to physical pixels; layout advances retain the ratio.

MFDs use the `osg-mfd-charon-mono` family. `UiPlugin` includes `MfdFontPlugin`;
standalone egui users call `mfd::install_font(ctx)` before drawing. Fullwidth
glyphs occupy two 8 × 16 cells; other scalars occupy one. Missing glyphs become
`?`. See [the MFD guide](../../../../docs/mfds.md).

## Phosphor

`Phosphor.ttf` is the unmodified regular icon font from
[Phosphor Web v2.1.1](https://github.com/phosphor-icons/web/tree/v2.1.1/src/regular).
The MIT license is included as [PHOSPHOR-LICENSE](PHOSPHOR-LICENSE).
[icons.rs](../../src/icons.rs) embeds the font and exposes named icons and a
separate `Phosphor` font family. The theme installs Charon and CJK as that family's
fallbacks, with Phosphor retaining priority for icons.
