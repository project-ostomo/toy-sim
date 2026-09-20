use crate::egui;

#[derive(Clone, Copy)]
pub enum Tone {
    Normal,
    Reserve,
    Heat,
}

impl Tone {
    pub fn color(self, fraction: f64) -> egui::Color32 {
        let severity = match self {
            Self::Normal => 0.,
            Self::Reserve => {
                if fraction <= 0.1 {
                    1.
                } else if fraction <= 0.25 {
                    0.5
                } else {
                    0.
                }
            }
            Self::Heat => {
                if fraction >= 1. {
                    1.
                } else if fraction >= 0.75 {
                    0.5
                } else {
                    0.
                }
            }
        };
        if severity >= 1. {
            egui::Color32::from_rgb(245, 87, 80)
        } else if severity > 0. {
            egui::Color32::from_rgb(240, 184, 88)
        } else {
            egui::Color32::from_rgb(224, 224, 218)
        }
    }
}

pub fn gauge(
    ui: &mut egui::Ui,
    label: &str,
    height: f32,
    fraction: f64,
    marker: Option<f64>,
    pending: Option<f64>,
    enabled: bool,
    tone: Tone,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        if enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        },
    );
    let p = ui.painter_at(rect);
    let accent = tone.color(fraction);
    p.rect_filled(rect, 0., egui::Color32::from_gray(12));
    let fill = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            rect.left() + rect.width() * fraction.clamp(0., 1.) as f32,
            rect.bottom(),
        ),
    );
    p.rect_filled(fill, 0., accent.linear_multiply(0.35));
    p.rect_stroke(
        rect,
        0.,
        egui::Stroke::new(1., accent.linear_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    for i in 1..10 {
        let x = rect.left() + rect.width() * i as f32 / 10.;
        p.line_segment(
            [
                egui::pos2(x, rect.bottom() - 4.),
                egui::pos2(x, rect.bottom()),
            ],
            egui::Stroke::new(1., accent),
        );
    }
    for (value, color) in [
        (marker, accent),
        (pending, egui::Color32::from_rgb(232, 180, 103)),
    ] {
        if let Some(value) = value {
            let x = rect.left() + rect.width() * value.clamp(0., 1.) as f32;
            p.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(2., color),
            );
        }
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(12.),
        if fraction <= 0.1 || matches!(tone, Tone::Heat) {
            accent
        } else {
            egui::Color32::from_rgb(215, 225, 228)
        },
    );
    response
}

pub fn bipolar(ui: &mut egui::Ui, label: &str, value: f64, negative: f64, positive: f64) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 17.), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let center = rect.center().x;
    let bound = if value < 0. { negative } else { positive };
    let fraction = if bound > 0. {
        (value / bound).clamp(-1., 1.)
    } else {
        0.
    } as f32;
    let end = center + fraction * rect.width() * 0.5;
    p.rect_filled(rect, 0., egui::Color32::from_gray(12));
    p.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(center.min(end), rect.top()),
            egui::pos2(center.max(end), rect.bottom()),
        ),
        0.,
        egui::Color32::from_gray(75),
    );
    p.line_segment(
        [
            egui::pos2(center, rect.top()),
            egui::pos2(center, rect.bottom()),
        ],
        egui::Stroke::new(1., egui::Color32::GRAY),
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(10.),
        egui::Color32::LIGHT_GRAY,
    );
}

pub fn vertical(
    ui: &mut egui::Ui,
    label: &str,
    text: &str,
    fraction: f64,
    tone: Tone,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let color = tone.color(fraction);
    let painter = ui.painter_at(rect);
    let meter = rect.shrink2(egui::vec2(3., 22.));
    painter.rect_stroke(
        meter,
        0.,
        egui::Stroke::new(1., color.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    for row in 0..20 {
        for col in 0..4 {
            let width = (meter.width() - 6.) / 4.;
            let height = (meter.height() - 6.) / 20.;
            let cell = egui::Rect::from_min_size(
                egui::pos2(
                    meter.left() + 3. + col as f32 * width,
                    meter.bottom() - 3. - (row + 1) as f32 * height,
                ),
                egui::vec2((width - 2.).max(1.), (height - 2.).max(1.)),
            );
            painter.rect_filled(
                cell,
                0.,
                if (row as f64 + 0.5) / 20. <= fraction.clamp(0., 1.) {
                    color
                } else {
                    egui::Color32::from_gray(28)
                },
            );
        }
    }
    painter.text(
        rect.center_top(),
        egui::Align2::CENTER_TOP,
        label,
        egui::FontId::monospace(10.),
        egui::Color32::LIGHT_GRAY,
    );
    painter.text(
        rect.center_bottom(),
        egui::Align2::CENTER_BOTTOM,
        text,
        egui::FontId::monospace(11.),
        color,
    );
    response
}
