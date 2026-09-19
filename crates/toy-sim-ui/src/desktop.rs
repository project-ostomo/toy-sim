use crate::{egui, icons::Icon};
use std::collections::HashMap;

pub const RAIL_WIDTH: f32 = 56.;
pub const STATUS_HEIGHT: f32 = 30.;
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(116, 207, 227);
pub const THREAT: egui::Color32 = egui::Color32::from_rgb(242, 92, 92);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(217, 226, 235);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(141, 159, 177);
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(15, 22, 31);
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(54, 73, 89);

#[derive(Clone, Copy)]
pub struct WindowSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub size: egui::Vec2,
    pub min_size: egui::Vec2,
    pub anchor: egui::Align2,
    pub offset: egui::Vec2,
    pub open: bool,
}

#[derive(Default)]
struct WindowState {
    open: bool,
    rect: Option<egui::Rect>,
    move_to: Option<egui::Pos2>,
    dragged: bool,
    user_placed: bool,
}

#[derive(Default)]
pub struct Desktop {
    windows: HashMap<&'static str, WindowState>,
    generation: u64,
    pub locked: bool,
}

impl Desktop {
    fn state(&mut self, spec: WindowSpec) -> &mut WindowState {
        self.windows.entry(spec.id).or_insert_with(|| WindowState {
            open: spec.open,
            ..Default::default()
        })
    }

    pub fn is_open(&self, spec: WindowSpec) -> bool {
        self.windows
            .get(spec.id)
            .map_or(spec.open, |state| state.open)
    }

    pub fn rect(&self, spec: WindowSpec) -> Option<egui::Rect> {
        self.windows
            .get(spec.id)
            .filter(|state| state.open)
            .and_then(|state| state.rect)
    }

    pub fn toggle(&mut self, spec: WindowSpec) {
        let state = self.state(spec);
        state.open = !state.open;
    }

    pub fn open(&mut self, spec: WindowSpec) {
        self.state(spec).open = true;
    }

    pub fn reset(&mut self) {
        self.windows.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn show<R>(
        &mut self,
        ctx: &egui::Context,
        spec: WindowSpec,
        contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        let bounds = workspace_in(ctx);
        if bounds.width() < 1. || bounds.height() < 1. {
            return None;
        }
        let id = egui::Id::new(("desktop_window", spec.id, self.generation));
        let locked = self.locked;
        let neighbors: Vec<_> = self
            .windows
            .iter()
            .filter(|(key, state)| **key != spec.id && state.open)
            .filter_map(|(_, state)| state.rect)
            .collect();
        let state = self.state(spec);
        if !state.open {
            return None;
        }

        let available = egui::vec2(
            bounds.width(),
            (bounds.height() - spec.offset.y.max(0.)).max(spec.min_size.y),
        );
        let maximum = if state.user_placed {
            bounds.size()
        } else {
            available.min(bounds.size())
        };
        let size = spec.size.min(maximum);
        let initial = spec
            .anchor
            .align_size_within_rect(size, bounds)
            .translate(spec.offset);
        let initial = fit_rect(initial, bounds);
        let mut window = egui::Window::new(egui::RichText::new(spec.title).size(12.).strong())
            .id(id)
            .open(&mut state.open)
            .default_rect(initial)
            .min_size(spec.min_size.min(bounds.size()))
            .max_size(maximum)
            .constrain_to(bounds)
            .drag_area(egui::WindowDrag::Anywhere)
            .movable(!locked)
            .resizable(!locked)
            .collapsible(false)
            .title_frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(25, 37, 49))
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .frame(
                egui::Frame::window(&ctx.style_of(egui::Theme::Dark))
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1., BORDER))
                    .inner_margin(10.)
                    .corner_radius(0.),
            );

        let requested_position = state.move_to.take().or_else(|| {
            let rect = state.rect?;
            if !state.user_placed && !ctx.input(|input| input.pointer.any_down()) {
                Some(initial.min)
            } else if !bounds.contains_rect(rect) {
                Some(
                    fit_rect(
                        egui::Rect::from_min_size(rect.min, rect.size().min(maximum)),
                        bounds,
                    )
                    .min,
                )
            } else {
                None
            }
        });
        if let Some(position) = requested_position.filter(|position| {
            state
                .rect
                .is_none_or(|rect| rect.min.distance(*position) > 0.1)
        }) {
            window = window.current_pos(position);
        }

        let output = window.show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8., 6.);
            ui.spacing_mut().button_padding = egui::vec2(8., 4.);
            ui.visuals_mut().override_text_color = Some(TEXT);
            contents(ui)
        })?;
        let rect = output.response.rect;
        if state.rect.is_some_and(|previous| {
            previous.min.distance(rect.min) > 0.1 || (previous.size() - rect.size()).length() > 0.1
        }) && ctx.input(|input| input.pointer.any_down())
        {
            state.dragged = true;
            state.user_placed = true;
        }
        if state.dragged && ctx.input(|input| input.pointer.any_released()) {
            state.move_to = Some(snap_rect(rect, bounds, &neighbors).min);
            state.dragged = false;
        }
        state.rect = Some(rect);
        output.inner
    }
}

pub fn workspace_in(ctx: &egui::Context) -> egui::Rect {
    workspace(ctx.content_rect())
}

pub fn workspace(screen: egui::Rect) -> egui::Rect {
    let min = (screen.min + egui::vec2(RAIL_WIDTH + 10., 10.)).min(screen.max);
    let max = (screen.max - egui::vec2(10., STATUS_HEIGHT + 10.)).max(min);
    egui::Rect::from_min_max(min, max)
}

fn fit_rect(rect: egui::Rect, bounds: egui::Rect) -> egui::Rect {
    let size = rect.size().min(bounds.size());
    let min = egui::pos2(
        rect.left().clamp(bounds.left(), bounds.right() - size.x),
        rect.top().clamp(bounds.top(), bounds.bottom() - size.y),
    );
    egui::Rect::from_min_size(min, size)
}

fn snap_rect(rect: egui::Rect, bounds: egui::Rect, neighbors: &[egui::Rect]) -> egui::Rect {
    let mut delta = egui::Vec2::ZERO;
    let mut nearest = egui::vec2(10., 10.);
    let mut consider = |axis: usize, distance: f32| {
        if distance.abs() < nearest[axis] {
            nearest[axis] = distance.abs();
            delta[axis] = distance;
        }
    };
    consider(0, bounds.left() - rect.left());
    consider(0, bounds.right() - rect.right());
    consider(1, bounds.top() - rect.top());
    consider(1, bounds.bottom() - rect.bottom());
    for other in neighbors {
        if rect.bottom() >= other.top() && rect.top() <= other.bottom() {
            consider(0, other.left() - 8. - rect.right());
            consider(0, other.right() + 8. - rect.left());
        }
        if rect.right() >= other.left() && rect.left() <= other.right() {
            consider(1, other.top() - 8. - rect.bottom());
            consider(1, other.bottom() + 8. - rect.top());
        }
    }
    fit_rect(rect.translate(delta), bounds)
}

pub fn launcher(ctx: &egui::Context, contents: impl FnOnce(&mut egui::Ui)) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("desktop_launcher"))
        .fixed_pos(screen.min)
        .movable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1., BORDER))
                .inner_margin(7.)
                .show(ui, |ui| {
                    ui.set_width(RAIL_WIDTH - 14.);
                    ui.set_height((screen.height() - STATUS_HEIGHT - 14.).max(0.));
                    contents(ui);
                });
        });
}

pub fn status_bar(ctx: &egui::Context, contents: impl FnOnce(&mut egui::Ui)) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("desktop_status"))
        .fixed_pos(egui::pos2(screen.left(), screen.bottom() - STATUS_HEIGHT))
        .movable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1., BORDER))
                .inner_margin(egui::Margin::symmetric(14, 5))
                .show(ui, |ui| {
                    ui.set_width((screen.width() - 28.).max(0.));
                    ui.horizontal(contents);
                });
        });
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(42., 42.), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let color = if active { ACCENT } else { MUTED };
    if active || response.hovered() {
        ui.painter()
            .rect_filled(rect, 3., egui::Color32::from_rgb(32, 48, 62));
    }
    if active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(2., rect.height())),
            0.,
            ACCENT,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon.glyph(),
        Icon::font(23.),
        color,
    );
    response.on_hover_text(label)
}

pub fn action_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    hint: &str,
) -> egui::Response {
    let inner = ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(56., 52.), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let color = if !enabled {
            MUTED
        } else if response.hovered() {
            ACCENT
        } else {
            TEXT
        };
        if response.hovered() && enabled {
            ui.painter()
                .rect_filled(rect, 2., egui::Color32::from_rgb(32, 48, 62));
        }
        ui.painter().text(
            rect.center_top() + egui::vec2(0., 15.),
            egui::Align2::CENTER_CENTER,
            icon.glyph(),
            Icon::font(22.),
            color,
        );
        ui.painter().text(
            rect.center_bottom() - egui::vec2(0., 7.),
            egui::Align2::CENTER_BOTTOM,
            label,
            egui::FontId::proportional(10.),
            color,
        );
        response
    });
    inner.inner.on_hover_text(hint)
}

pub fn meter(
    ui: &mut egui::Ui,
    label: &str,
    value: f64,
    maximum: f64,
    detail: &str,
    color: egui::Color32,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).small().color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(detail).small().color(TEXT));
        });
    });
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0., BORDER);
    let fraction = if maximum > 0. {
        (value / maximum).clamp(0., 1.) as f32
    } else {
        0.
    };
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height())),
        0.,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_and_layout_lock_use_real_pointer_events() {
        let ctx = egui::Context::default();
        let mut desktop = Desktop::default();
        let spec = WindowSpec {
            id: "test",
            title: "Test window",
            size: egui::vec2(240., 160.),
            min_size: egui::vec2(160., 100.),
            anchor: egui::Align2::LEFT_TOP,
            offset: egui::Vec2::ZERO,
            open: true,
        };
        let frame = |desktop: &mut Desktop, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 700.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    desktop.show(ui.ctx(), spec, |ui| {
                        ui.label("Content");
                    });
                },
            );
            output.textures_delta.clear();
        };
        for _ in 0..3 {
            frame(&mut desktop, vec![]);
        }
        let drag = |desktop: &mut Desktop, delta: egui::Vec2| {
            let start = desktop.rect(spec).unwrap().min + egui::vec2(100., 15.);
            let end = start + delta;
            frame(desktop, vec![egui::Event::PointerMoved(start)]);
            frame(
                desktop,
                vec![egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            );
            for step in 1..=24 {
                frame(
                    desktop,
                    vec![egui::Event::PointerMoved(
                        start + delta * (step as f32 / 24.),
                    )],
                );
            }
            frame(desktop, vec![]);
            frame(
                desktop,
                vec![egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            for _ in 0..3 {
                frame(desktop, vec![]);
            }
        };
        let initial = desktop.rect(spec).unwrap();
        drag(&mut desktop, egui::vec2(10., 10.));
        let moved = desktop.rect(spec).unwrap();
        assert_eq!(moved.min - initial.min, egui::vec2(10., 10.));
        desktop.locked = true;
        drag(&mut desktop, egui::vec2(80., 60.));
        assert_eq!(desktop.rect(spec).unwrap(), moved);
        desktop.reset();
        for _ in 0..3 {
            frame(&mut desktop, vec![]);
        }
        assert_eq!(desktop.rect(spec).unwrap(), initial);
    }

    #[test]
    fn windows_snap_to_neighbors_and_remain_accessible_after_resize() {
        let bounds = workspace(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280., 720.),
        ));
        let neighbor = egui::Rect::from_min_size(egui::pos2(800., 20.), egui::vec2(300., 200.));
        let moved = egui::Rect::from_min_size(egui::pos2(806., 231.), egui::vec2(300., 250.));
        let snapped = snap_rect(moved, bounds, &[neighbor]);
        assert_eq!(snapped.top(), neighbor.bottom() + 8.);
        let smaller = workspace(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640., 480.),
        ));
        assert!(smaller.contains_rect(fit_rect(snapped, smaller)));
        assert!(smaller.contains_rect(fit_rect(
            egui::Rect::from_min_size(egui::pos2(-100., -100.), egui::vec2(2000., 1000.)),
            smaller
        )));
    }
}
