#!/usr/bin/env python3
"""Subset Sarasa's CJK glyphs and align their metrics with Charon Mono.

Requires fonttools==4.57.0. See crates/osg-ui/data/fonts/README.md for sources.
"""

import argparse
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont


CJK_RANGES = (
    (0x1100, 0x11FF),  # Hangul jamo
    (0x2E80, 0x30FF),  # Radicals, punctuation, kana
    (0x3100, 0x312F),  # Bopomofo
    (0x3130, 0x318F),  # Hangul compatibility jamo
    (0x3190, 0x33FF),  # Kanbun, Bopomofo, strokes, kana, enclosed/compatibility forms
    (0x3400, 0x9FFF),  # Unified ideographs
    (0xA960, 0xA97F),  # Hangul jamo extensions A
    (0xAC00, 0xD7FF),  # Hangul syllables and jamo extensions B
    (0xF900, 0xFAFF),  # Compatibility ideographs
    (0xFE10, 0xFE1F),  # Vertical forms
    (0xFE30, 0xFE4F),  # Compatibility forms
    (0xFF00, 0xFFEF),  # Fullwidth and halfwidth forms
    (0x1AFF0, 0x1B16F),  # Kana extensions
    (0x20000, 0x3FFFF),  # Supplementary ideographs
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sarasa", type=Path)
    parser.add_argument("charon_mono", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    font = TTFont(args.sarasa, recalcTimestamp=False)
    mono = TTFont(args.charon_mono)
    cmap = font.getBestCmap()
    codepoints = {
        codepoint for codepoint in cmap
        if any(start <= codepoint <= end for start, end in CJK_RANGES)
    }

    # Both upstream families use a 1000-unit em and 500-unit Latin cell.
    # Verify this before modifying anything: visual-only egui font tweaks
    # cannot establish the required layout ratio.
    assert font["head"].unitsPerEm == mono["head"].unitsPerEm == 1000
    mono_cell = mono["hmtx"].metrics[mono.getBestCmap()[ord("M")]][0]
    assert mono_cell == 500
    for codepoint in codepoints:
        width = font["hmtx"].metrics[cmap[codepoint]][0]
        assert width in (0, mono_cell, 2 * mono_cell), (hex(codepoint), width)

    options = subset.Options()
    options.hinting = False
    options.recalc_timestamp = False
    options.name_IDs = ["*"]
    options.name_legacy = True
    options.name_languages = ["*"]
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(unicodes=codepoints)
    subsetter.subset(font)

    # Match line metrics so fallback runs do not increase the line height.
    for attribute in ("ascent", "descent", "lineGap"):
        setattr(font["hhea"], attribute, getattr(mono["hhea"], attribute))
    for attribute in ("sTypoAscender", "sTypoDescender", "sTypoLineGap",
                      "usWinAscent", "usWinDescent"):
        setattr(font["OS/2"], attribute, getattr(mono["OS/2"], attribute))

    # Give the derivative its own family name and preserve OFL copyright records.
    names = {
        1: "Toy Sim CJK", 2: "Regular", 3: "ToySimCJK-Regular-1.0.41",
        4: "Toy Sim CJK Regular", 6: "ToySimCJK-Regular",
        16: "Toy Sim CJK", 17: "Regular",
    }
    for record in font["name"].names:
        if record.nameID in names:
            record.string = names[record.nameID].encode(record.getEncoding())
    font.save(args.output)
    print(f"Wrote {args.output}: {len(codepoints)} CJK codepoints, "
          f"full-width advance {2 * mono_cell}/{font['head'].unitsPerEm} em")


if __name__ == "__main__":
    main()
