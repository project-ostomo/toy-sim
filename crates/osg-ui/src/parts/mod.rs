mod specs;

pub use specs::{Category, PartDescription, SpecRow, SpecSection, SpecValue, Unit};

use crate::egui::{
    self, Align2, Color32, FontId, Rect, Response, Sense, StrokeKind, TextureId, Vec2,
};

#[derive(Clone, Copy)]
pub struct PartImage {
    pub texture: TextureId,
    pub uv: Rect,
}

pub fn tile(
    ui: &mut egui::Ui,
    title: &str,
    image: Option<PartImage>,
    selected: bool,
    width: f32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, width + 42.), Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
    if response.has_focus() {
        ui.painter()
            .rect_stroke(rect, 0., ui.visuals().selection.stroke, StrokeKind::Inside);
    }

    let picture = Rect::from_min_size(rect.min + Vec2::splat(4.), Vec2::splat(width - 8.));
    paint_image(ui, picture, image);

    let mut job = egui::text::LayoutJob::simple(
        title.to_owned(),
        FontId::proportional(12.),
        visuals.text_color(),
        width - 10.,
    );
    job.wrap.max_rows = 2;
    job.halign = egui::Align::Center;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let text_position = egui::pos2(rect.center().x, picture.bottom() + 6.);
    ui.painter()
        .galley(text_position, galley, visuals.text_color());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, title)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_image(ui: &egui::Ui, rect: Rect, image: Option<PartImage>) {
    if let Some(image) = image {
        ui.painter()
            .image(image.texture, rect, image.uv, Color32::WHITE);
    } else {
        ui.painter()
            .rect_filled(rect, 0., ui.visuals().extreme_bg_color);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "…",
            FontId::proportional(20.),
            ui.visuals().weak_text_color(),
        );
    }
}

pub fn heading(
    ui: &mut egui::Ui,
    title: &str,
    description: &PartDescription,
    image: Option<PartImage>,
    image_size: f32,
) {
    ui.heading(title);
    ui.label(egui::RichText::new(description.kind).color(ui.visuals().hyperlink_color));
    ui.add_space(4.);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(image_size), Sense::hover());
        paint_image(ui, rect, image);
        ui.add(egui::Label::new(description.summary).wrap());
    });
}

pub fn specifications(ui: &mut egui::Ui, description: &PartDescription) {
    for section in &description.sections {
        ui.add_space(8.);
        ui.separator();
        ui.label(egui::RichText::new(section.title).strong());
        for row in &section.rows {
            let width = ui.available_width();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new((width - ui.spacing().item_spacing.x) * 0.52, 0.),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(row.label).weak()).wrap());
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    ui.add(egui::Label::new(row.value.to_string()).wrap());
                });
            });
        }
        if let Some(note) = section.note {
            ui.add_space(2.);
            ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).wrap());
        }
    }
}

pub fn tooltip(
    ui: &mut egui::Ui,
    title: &str,
    description: &PartDescription,
    image: Option<PartImage>,
) {
    let viewport = ui.ctx().viewport_rect();
    ui.set_width(320_f32.min(viewport.width() - 32.).max(120.));
    let height = (viewport.height() - 64.).max(120.);
    ui.set_max_height(height);
    ui.spacing_mut().scroll.floating = false;
    egui::ScrollArea::vertical()
        .id_salt("part-tooltip")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .max_height(height)
        .show(ui, |ui| {
            heading(ui, title, description, image, 80.);
            specifications(ui, description);
            ui.add_space(8.);
            ui.label(
                egui::RichText::new("Click to pick up")
                    .small()
                    .color(ui.visuals().hyperlink_color),
            );
        });
}

pub fn show_tooltip(
    response: &Response,
    title: &str,
    description: &PartDescription,
    image: Option<PartImage>,
) {
    let mut card = egui::Tooltip::for_enabled(response);
    card.popup = card
        .popup
        .align(egui::emath::RectAlign::RIGHT)
        .align_alternatives(&[egui::emath::RectAlign::LEFT]);
    card.show(|ui| tooltip(ui, title, description, image));
}
