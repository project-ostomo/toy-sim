use crate::{egui, icons::Icon};
use std::collections::HashMap;

pub const RAIL_WIDTH: f32 = 56.;
pub const STATUS_HEIGHT: f32 = 30.;
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(116, 207, 227);
pub const THREAT: egui::Color32 = egui::Color32::from_rgb(242, 92, 92);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(217, 226, 235);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(141, 159, 177);
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(15, 22, 31);
pub const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(21, 31, 40);
pub const INFO: egui::Color32 = egui::Color32::from_rgb(120, 184, 255);
pub const POSITIVE: egui::Color32 = egui::Color32::from_rgb(101, 224, 145);
pub const MINT: egui::Color32 = egui::Color32::from_rgb(99, 216, 181);
pub const WARNING: egui::Color32 = egui::Color32::from_rgb(242, 180, 75);
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
    loading: bool,
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

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedLayout {
    pub locked: bool,
    pub windows: std::collections::BTreeMap<String, SavedWindow>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedWindow {
    pub open: bool,
    pub rect: Option<[f32; 4]>,
    pub user_placed: bool,
}

impl Desktop {
    pub fn save_layout(&self) -> SavedLayout {
        SavedLayout {
            locked: self.locked,
            windows: self
                .windows
                .iter()
                .map(|(id, state)| {
                    (
                        (*id).into(),
                        SavedWindow {
                            open: state.open,
                            rect: state
                                .rect
                                .map(|rect| [rect.min.x, rect.min.y, rect.width(), rect.height()]),
                            user_placed: state.user_placed,
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn restore_layout(&mut self, saved: &SavedLayout, specs: &[WindowSpec]) {
        self.reset();
        self.locked = saved.locked;
        for spec in specs {
            let Some(window) = saved.windows.get(spec.id) else {
                continue;
            };
            let rect = window
                .rect
                .filter(|rect| {
                    rect.iter().all(|value| value.is_finite()) && rect[2] > 0. && rect[3] > 0.
                })
                .map(|rect| {
                    egui::Rect::from_min_size(
                        egui::pos2(rect[0], rect[1]),
                        egui::vec2(rect[2], rect[3]),
                    )
                });
            self.windows.insert(
                spec.id,
                WindowState {
                    open: window.open,
                    rect,
                    user_placed: window.user_placed,
                    ..Default::default()
                },
            );
        }
    }

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

    pub fn set_loading(&mut self, spec: WindowSpec, loading: bool) {
        self.state(spec).loading = loading;
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
        let initial = fit_rect(
            if state.user_placed {
                state.rect.unwrap_or(initial)
            } else {
                initial
            },
            bounds,
        );
        let mut window = egui::Window::new(egui::RichText::new(spec.title).size(12.).strong())
            .id(id)
            .title_bar(false)
            .default_rect(initial)
            .min_size(spec.min_size.min(bounds.size()))
            .max_size(maximum)
            .constrain_to(bounds)
            .drag_area(egui::WindowDrag::Anywhere)
            .movable(!locked)
            .resizable(!locked)
            .collapsible(false)
            .frame(
                egui::Frame::window(&ctx.style_of(egui::Theme::Dark))
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1., BORDER))
                    .inner_margin(0.)
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

        let mut close = false;
        let output = window.show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8., 4.);
            ui.spacing_mut().button_padding = egui::vec2(8., 3.);
            ui.visuals_mut().override_text_color = Some(TEXT);
            close = window_title(ui, spec.title, state.loading);
            egui::Frame::new()
                .inner_margin(10.)
                .show(ui, |ui| {
                    // Reserve the painted surface even when a short page does
                    // not fill the requested window height.
                    ui.set_min_size(ui.available_size());
                    contents(ui)
                })
                .inner
        })?;
        if close {
            state.open = false;
        }
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

/// Compact chrome shared by workspace windows and transient dialogs.
pub fn window_title(ui: &mut egui::Ui, title: &str, loading: bool) -> bool {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0., egui::Color32::from_rgb(25, 37, 49));
    let loading_id = ui.id().with("window_loading_since");
    if loading {
        const LOADING_DELAY_SECONDS: f64 = 3.;
        let now = ui.input(|input| input.time);
        let since = ui.data_mut(|data| *data.get_temp_mut_or_insert_with(loading_id, || now));
        let remaining = LOADING_DELAY_SECONDS - (now - since);
        if remaining <= 0. {
            loading_border(ui, rect);
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(remaining));
        }
    } else {
        ui.data_mut(|data| data.remove::<f64>(loading_id));
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(8., 0.),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(12.),
        TEXT,
    );
    let close_rect = egui::Rect::from_min_max(
        egui::pos2(rect.right() - 24., rect.top()),
        rect.right_bottom(),
    );
    let response = ui.interact(
        close_rect,
        ui.id().with("window_close"),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close"));
    let color = if response.hovered() { TEXT } else { MUTED };
    let center = close_rect.center();
    for sign in [-1., 1.] {
        ui.painter().line_segment(
            [
                center + egui::vec2(-3., -3. * sign),
                center + egui::vec2(3., 3. * sign),
            ],
            egui::Stroke::new(1., color),
        );
    }
    response.on_hover_text("Close").clicked()
}

/// Paint an indeterminate progress bar over the title's bottom border.
fn loading_border(ui: &egui::Ui, title: egui::Rect) {
    let track = egui::Rect::from_min_max(
        egui::pos2(title.left(), title.bottom() - 2.),
        title.right_bottom(),
    );
    let phase = ui.input(|input| (input.time / 1.6).rem_euclid(1.)) as f32;
    let length = track.width() * 0.24;
    let left = track.left() - length + phase * (track.width() + length);
    let segment = egui::Rect::from_min_size(
        egui::pos2(left, track.top()),
        egui::vec2(length, track.height()),
    );
    ui.painter()
        .rect_filled(track, 0., ACCENT.gamma_multiply(0.12));
    ui.painter()
        .rect_filled(segment.intersect(track), 0., ACCENT.gamma_multiply(0.65));
    ui.ctx().request_repaint();
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

pub fn hud_painter(ctx: &egui::Context, id: egui::Id) -> egui::Painter {
    let layer = egui::LayerId::new(egui::Order::Background, id);
    ctx.set_sublayer(egui::LayerId::background(), layer);
    ctx.layer_painter(layer)
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
                .inner_margin(6.)
                .show(ui, |ui| {
                    ui.set_width(RAIL_WIDTH - 14.);
                    ui.set_height((screen.height() - 14.).max(0.));
                    contents(ui);
                });
        });
}

pub fn status_bar(ctx: &egui::Context, contents: impl FnOnce(&mut egui::Ui)) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("desktop_status"))
        .fixed_pos(egui::pos2(
            screen.left() + RAIL_WIDTH,
            screen.bottom() - STATUS_HEIGHT,
        ))
        .movable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1., BORDER))
                .inner_margin(egui::Margin::symmetric(14, 0))
                .show(ui, |ui| {
                    ui.set_width((screen.width() - RAIL_WIDTH - 30.).max(0.));
                    ui.set_height(STATUS_HEIGHT - 2.);
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
    fn loading_title_animates_without_changing_layout() {
        let ctx = egui::Context::default();
        let draw = |loading, time| {
            let mut layout = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    window_title(ui, "Loading test", loading);
                    layout = ui.min_rect();
                },
            );
            output.textures_delta.clear();
            (layout, output.shapes)
        };
        let (idle, _) = draw(false, 0.);
        let (loading, initial) = draw(true, 0.4);
        let (_, waiting) = draw(true, 3.3);
        let (_, first) = draw(true, 3.5);
        let (later, second) = draw(true, 3.9);
        assert_eq!(idle, loading);
        assert_eq!(loading, later);

        let segment = |shapes: Vec<egui::epaint::ClippedShape>| {
            shapes.into_iter().find_map(|shape| match shape.shape {
                egui::Shape::Rect(rect) if rect.fill == ACCENT.gamma_multiply(0.65) => {
                    Some(rect.rect)
                }
                _ => None,
            })
        };
        assert!(segment(initial).is_none());
        assert!(segment(waiting).is_none());
        let first = segment(first).unwrap();
        let second = segment(second).unwrap();
        assert!(second.left() > first.left());
        assert_eq!(first.height(), 2.);
        assert_eq!(first.bottom(), idle.bottom());

        assert!(segment(draw(false, 4.).1).is_none());
        assert!(segment(draw(true, 4.1).1).is_none());
        assert!(segment(draw(true, 7.).1).is_none());
        assert!(segment(draw(true, 7.2).1).is_some());
    }

    #[test]
    fn compact_window_chrome_closes_the_correct_window() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
        let mut desktop = Desktop::default();
        let spec = WindowSpec {
            id: "chrome_test",
            title: "TEST WINDOW",
            size: egui::vec2(300., 180.),
            min_size: egui::vec2(200., 100.),
            anchor: egui::Align2::LEFT_TOP,
            offset: egui::Vec2::ZERO,
            open: true,
        };
        let draw = |desktop: &mut Desktop, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 650.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    desktop.show(ui.ctx(), spec, |ui| {
                        ui.label("Window contents");
                    });
                },
            );
            output.textures_delta.clear();
        };
        for _ in 0..4 {
            draw(&mut desktop, Vec::new());
        }
        let rect = desktop.rect(spec).unwrap();
        assert!(workspace_in(&ctx).contains_rect(rect));
        let position = egui::pos2(rect.right() - 13., rect.top() + 13.);
        for pressed in [true, false] {
            draw(
                &mut desktop,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(!desktop.is_open(spec));
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
