//! Embedded Charon families with a Sarasa-derived CJK fallback.

use crate::egui::{
    self, FontFamily,
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
};

pub(crate) const MONO: &[u8] = include_bytes!("../data/fonts/IosevkaCharonMono-Regular.ttf");
pub(crate) const CJK: &[u8] = include_bytes!("../data/fonts/ToySimCJK-Regular.ttf");
pub(crate) const PROPORTIONAL: &[u8] = include_bytes!("../data/fonts/IosevkaCharon-Regular.ttf");

pub(crate) fn install_family(ctx: &egui::Context, family: FontFamily, mono: bool) {
    // Insert fallback first, then the primary face ahead of it and egui defaults.
    for (name, bytes) in [
        ("Toy Sim CJK", CJK),
        if mono {
            ("Iosevka Charon Mono", MONO)
        } else {
            ("Iosevka Charon", PROPORTIONAL)
        },
    ] {
        ctx.add_font(FontInsert::new(
            name,
            egui::FontData::from_static(bytes),
            vec![InsertFontFamily {
                family: family.clone(),
                priority: FontPriority::Highest,
            }],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_advances_are_two_mono_cells_at_multiple_sizes_and_scales() {
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(pixels_per_point);
            crate::theme::install(&ctx);
            crate::mfd::install_font(&ctx);
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.fonts_mut(|fonts| {
                    for size in [10.0, 12.0, 13.0, 14.0, 17.5, 20.0] {
                        let mono = egui::FontId::monospace(size);
                        let cell = fonts.glyph_width(&mono, 'M');
                        for family in [
                            FontFamily::Monospace,
                            FontFamily::Proportional,
                            FontFamily::Name("osg-mfd-charon-mono".into()),
                        ] {
                            let font = egui::FontId::new(size, family);
                            for ch in "中文漢字あア한글，。！".chars() {
                                let width = fonts.glyph_width(&font, ch);
                                assert!(
                                    (width - 2.0 * cell).abs() < 0.0001,
                                    "{ch} at {size}pt/{pixels_per_point}: {width} != {}",
                                    2.0 * cell
                                );
                            }
                        }

                        // Layout goes through the shaper, unlike glyph_width.
                        let latin =
                            fonts.layout_no_wrap("MMMM".into(), mono.clone(), egui::Color32::WHITE);
                        let cjk = fonts.layout_no_wrap("中文".into(), mono, egui::Color32::WHITE);
                        let advance = |galley: &egui::Galley| {
                            galley.rows[0]
                                .glyphs
                                .iter()
                                .map(|glyph| glyph.advance_width)
                                .sum::<f32>()
                        };
                        assert!((advance(&latin) - advance(&cjk)).abs() < 0.0001);
                    }
                });
            });
            output.textures_delta.clear();
        }
    }
}
