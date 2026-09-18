use crate::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle,
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
    style::WidgetVisuals,
    vec2,
};

const FONT: &[u8] = include_bytes!("../data/fonts/SarasaUiSC-Regular.ttf");

pub fn install(ctx: &egui::Context) {
    crate::icons::install(ctx);
    ctx.add_font(FontInsert::new(
        "Sarasa UI SC",
        egui::FontData::from_static(FONT),
        vec![
            InsertFontFamily {
                family: FontFamily::Name("Phosphor".into()),
                priority: FontPriority::Lowest,
            },
            InsertFontFamily {
                family: FontFamily::Proportional,
                priority: FontPriority::Highest,
            },
            InsertFontFamily {
                family: FontFamily::Monospace,
                priority: FontPriority::Lowest,
            },
        ],
    ));

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style());
}

fn style() -> egui::Style {
    let mut style = egui::Style::default();
    style.spacing.item_spacing = vec2(12., 8.);
    style.spacing.window_margin = Margin::same(12);
    style.spacing.menu_margin = Margin::same(10);
    style.spacing.button_padding = vec2(10., 6.);
    style.spacing.interact_size = vec2(44., 28.);
    style.spacing.indent = 24.;
    style.spacing.icon_width = 16.;
    style.spacing.icon_spacing = 8.;
    style.spacing.extra_text_line_spacing = 2.;

    style.text_styles.extend([
        (TextStyle::Heading, FontId::proportional(20.)),
        (TextStyle::Body, FontId::proportional(14.)),
        (TextStyle::Button, FontId::proportional(14.)),
        (TextStyle::Small, FontId::proportional(12.)),
    ]);

    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.panel_fill = rgb(25, 29, 36);
    visuals.window_fill = rgb(28, 32, 40);
    visuals.window_stroke = Stroke::new(1., rgb(60, 71, 86));
    visuals.faint_bg_color = rgb(31, 37, 46);
    visuals.extreme_bg_color = rgb(16, 20, 27);
    visuals.code_bg_color = rgb(35, 42, 53);
    visuals.weak_text_color = Some(rgb(147, 161, 180));
    visuals.hyperlink_color = rgb(130, 180, 224);
    visuals.selection.bg_fill = rgb(44, 79, 110);
    visuals.selection.stroke = Stroke::new(1., rgb(219, 235, 250));
    visuals.text_cursor.stroke = Stroke::new(2., rgb(130, 180, 224));
    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.menu_corner_radius = CornerRadius::ZERO;

    visuals.widgets.noninteractive = widget(rgb(28, 32, 40), rgb(52, 63, 78), rgb(216, 223, 232));
    visuals.widgets.inactive = widget(rgb(42, 49, 60), rgb(65, 78, 95), rgb(204, 215, 229));
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered = widget(rgb(57, 71, 89), rgb(108, 149, 184), rgb(235, 242, 250));
    visuals.widgets.active = widget(rgb(57, 91, 121), rgb(130, 180, 224), rgb(241, 247, 253));
    visuals.widgets.open = widget(rgb(45, 62, 81), rgb(100, 137, 172), rgb(223, 235, 248));

    style
}

fn rgb(red: u8, green: u8, blue: u8) -> Color32 {
    Color32::from_rgb(red, green, blue)
}

fn widget(background: Color32, border: Color32, foreground: Color32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: background,
        weak_bg_fill: background,
        bg_stroke: Stroke::new(1., border),
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1., foreground),
        expansion: 0.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hovering_buttons_preserves_their_bounds_and_neighbor_positions() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        ctx.set_style_of(egui::Theme::Dark, style());

        let draw = |pointer| {
            let mut rects = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(800., 600.),
                )),
                events: vec![egui::Event::PointerMoved(pointer)],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                rects.clear();
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        rects.push(ui.button("Button").rect);
                        rects.push(ui.selectable_label(false, "Unselected").rect);
                        rects.push(ui.selectable_label(true, "Selected").rect);
                        rects.push(ui.label("Neighbor").rect);
                    });
                });
            });
            output.textures_delta.clear();
            rects
        };

        let away = egui::pos2(700., 500.);
        draw(away);
        let idle = draw(away);
        for rect in &idle[..3] {
            for _ in 0..3 {
                assert_eq!(draw(rect.center()), idle);
            }
            for _ in 0..3 {
                assert_eq!(draw(away), idle);
            }
        }
    }
}
