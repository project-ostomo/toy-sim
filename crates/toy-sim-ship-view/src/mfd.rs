//! Client presentation of complete programmable screen frames.
mod font;
mod geometry;
use bevy_egui::egui;
pub use font::{MfdFontPlugin, install as install_font};
use toy_sim_ship_wasm::screens::{
    BezelKey, Draw, FONT_HEIGHT, FONT_WIDTH, Ink, ScreenId, ScreenImage,
};

fn rgb(c: Ink) -> egui::Color32 {
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

struct Surface {
    painter: egui::Painter,
    screen: egui::Rect,
    scale: f32,
    font: egui::FontId,
}
impl Surface {
    fn point(&self, p: egui::Pos2) -> egui::Pos2 {
        self.screen.min + p.to_vec2() * self.scale
    }
    fn rect(&self, r: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_max(self.point(r.min), self.point(r.max))
    }
    fn pixel(&self, at: egui::Pos2, color: Ink) {
        let rect = self.rect(egui::Rect::from_min_size(at, egui::Vec2::ONE));
        if self.painter.clip_rect().intersects(rect) {
            self.painter.rect_filled(rect, 0., rgb(color));
        }
    }
    fn line(&self, from: egui::Pos2, to: egui::Pos2, color: Ink) {
        if from == to {
            self.pixel(from, color);
            return;
        }
        let clip = self.painter.clip_rect().expand(self.scale);
        if let Some([from, to]) = geometry::clip_line(
            self.point(from + egui::vec2(0.5, 0.5)),
            self.point(to + egui::vec2(0.5, 0.5)),
            clip,
        ) {
            self.painter
                .line_segment([from, to], egui::Stroke::new(self.scale, rgb(color)));
        }
    }
    fn text(&self, at: [i16; 2], text: &str, color: Ink) {
        let origin = self.point(egui::pos2(at[0] as f32, at[1] as f32));
        let mut cursor = origin;
        let cell = egui::vec2(FONT_WIDTH as f32, FONT_HEIGHT as f32) * self.scale;
        // Each Unicode scalar occupies one cell; no shaping/kerning across cells.
        let shapes = self.painter.fonts_mut(|fonts| {
            let mut shapes = Vec::new();
            for ch in text.chars() {
                if ch == '\n' {
                    cursor.x = origin.x;
                    cursor.y += cell.y;
                    continue;
                }
                let rect = egui::Rect::from_min_size(cursor, cell);
                if !ch.is_ascii_control() && ch != ' ' && self.painter.clip_rect().intersects(rect)
                {
                    let ch = if font::has_glyph(ch) { ch } else { '?' };
                    let galley =
                        fonts.layout_no_wrap(ch.to_string(), self.font.clone(), rgb(color));
                    let pos = cursor + (cell - galley.size()) * 0.5;
                    shapes.push(egui::Shape::galley(pos, galley, rgb(color)));
                }
                cursor.x += cell.x;
            }
            shapes
        });
        self.painter.extend(shapes);
    }
}
/// Paint a complete frame in a square destination. Install the font before the
/// egui pass (or add MfdFontPlugin). Returns false without painting invalid input.
pub fn paint(painter: &egui::Painter, screen: egui::Rect, frame: &ScreenImage) -> bool {
    if !frame.valid() || !screen.is_finite() || screen.width() <= 0. || screen.height() <= 0. {
        return false;
    }
    let scale = (screen.width() / frame.width as f32).min(screen.height() / frame.height as f32);
    let screen = egui::Rect::from_center_size(
        screen.center(),
        egui::vec2(frame.width as f32, frame.height as f32) * scale,
    );
    let painter = painter.with_clip_rect(screen);
    if !painter.clip_rect().is_positive() {
        return true;
    }
    let font = painter.fonts_mut(|fonts| {
        let base = egui::FontId::new(16. * scale, font::family());
        let fit = ((FONT_WIDTH as f32 * scale) / fonts.glyph_width(&base, 'M'))
            .min((FONT_HEIGHT as f32 * scale) / fonts.row_height(&base));
        egui::FontId::new(base.size * fit, font::family())
    });
    let surface = Surface {
        painter,
        screen,
        scale,
        font,
    };
    surface
        .painter
        .rect_filled(screen, 0., rgb(frame.background));
    let pos = |p: [i16; 2]| egui::pos2(p[0] as f32, p[1] as f32);
    for draw in &frame.draws {
        match draw {
            Draw::Pixel { at, color } => surface.pixel(pos(*at), *color),
            Draw::Text { at, text, color } => surface.text(*at, text, *color),
            Draw::Line { from, to, color } => surface.line(pos(*from), pos(*to), *color),
            Draw::Polyline { points, color } => {
                for pair in points.windows(2) {
                    surface.line(pos(pair[0]), pos(pair[1]), *color);
                }
            }
            Draw::Rect {
                at,
                size,
                filled,
                color,
            } => {
                if size.contains(&0) {
                    continue;
                }
                let rect = surface.rect(egui::Rect::from_min_size(
                    pos(*at),
                    egui::vec2(size[0] as f32, size[1] as f32),
                ));
                if surface.painter.clip_rect().intersects(rect) {
                    if *filled {
                        surface.painter.rect_filled(rect, 0., rgb(*color));
                    } else {
                        surface.painter.rect_stroke(
                            rect,
                            0.,
                            egui::Stroke::new(scale, rgb(*color)),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }
            Draw::Ellipse {
                centre,
                radii,
                filled,
                color,
            } => {
                let centre = pos(*centre);
                let radii = egui::vec2(radii[0] as f32, radii[1] as f32);
                if radii.x == 0. || radii.y == 0. {
                    surface.line(centre - radii, centre + radii, *color);
                    continue;
                }
                let centre = surface.point(centre + egui::vec2(0.5, 0.5));
                let radii = radii * scale;
                let clip = surface
                    .painter
                    .clip_rect()
                    .expand(scale + 1. / surface.painter.pixels_per_point());
                if !egui::Rect::from_center_size(centre, 2. * radii).intersects(clip) {
                    continue;
                }
                let points =
                    geometry::ellipse(centre, radii, clip, surface.painter.pixels_per_point());
                surface.painter.add(egui::epaint::PathShape {
                    points,
                    closed: true,
                    fill: if *filled {
                        rgb(*color)
                    } else {
                        egui::Color32::TRANSPARENT
                    },
                    // The old filled ellipse includes its boundary pixels as well.
                    stroke: egui::Stroke::new(scale, rgb(*color)).into(),
                });
            }
        }
    }
    true
}

#[derive(Default)]
pub struct MfdRenderer;
#[derive(Default)]
pub struct MfdResponse {
    pub keys: Vec<BezelKey>,
    pub focused: bool,
}
impl MfdRenderer {
    /// Mouse input targets the fixed bezel. The screen itself only takes focus.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: ScreenId,
        frame: Option<&ScreenImage>,
    ) -> MfdResponse {
        let frame = frame.filter(|f| f.screen_id == id && f.valid());
        let blank = ScreenImage {
            screen_id: id,
            width: 512,
            height: 512,
            background: [0; 3],
            draws: Vec::new(),
            buttons: Default::default(),
        };
        let current = frame.unwrap_or(&blank);
        let key_width = 42.;
        let gap = 4.;
        let size = (ui.available_width() - 2. * (key_width + gap)).clamp(128., 1024.);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(size + 2. * (key_width + gap), size),
            egui::Sense::hover(),
        );
        let screen = egui::Rect::from_min_size(
            rect.min + egui::vec2(key_width + gap, 0.),
            egui::vec2(size, size),
        );
        let mut result = MfdResponse::default();
        let screen_response =
            ui.interact(screen, ui.id().with(("screen", id)), egui::Sense::click());
        result.focused = screen_response.clicked();
        if result.focused {
            screen_response.surrender_focus();
        }
        paint(ui.painter(), screen, current);
        for (index, key) in BezelKey::ALL.into_iter().enumerate() {
            let height = (size / 6. - 8.).min(36.);
            let left = if index < 6 {
                rect.left()
            } else {
                screen.right() + gap
            };
            let top = rect.top() + size * (index % 6) as f32 / 6. + (size / 6. - height) / 2.;
            let r = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(key_width, height));
            let label = current.buttons[index].as_deref();
            let button = egui::Button::new(
                egui::RichText::new(label.unwrap_or("—"))
                    .font(egui::FontId::new(13., font::family())),
            )
            .sense(if label.is_some() {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            });
            let response = ui
                .push_id((id, index), |ui| ui.put(r, button))
                .inner
                .on_hover_text(format!(
                    "{key:?} · {}F{}",
                    if index < 6 { "" } else { "Shift+" },
                    index % 6 + 1
                ));
            if label.is_some() && response.clicked() {
                response.surrender_focus();
                result.keys.push(key);
                result.focused = true;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests;
