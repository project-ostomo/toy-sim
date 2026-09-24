mod computer;
mod resources;
mod systems;

use super::selection::Selection;
use crate::state::{
    CommandState, Outgoing, OwnedShip, RenderTime, SessionInfo, SessionReset, ShipDetails,
};
use bevy::prelude::*;
use osg_model::*;
use osg_ui::{
    bevy_egui::{EguiContexts, EguiPrimaryContextPass},
    desktop::{RAIL_WIDTH, STATUS_HEIGHT},
    egui, gauges,
    icons::Icon,
};

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ConsoleDraw;

#[derive(Default)]
struct Smooth {
    stamp: u64,
    force: Vec3,
    previous_force: Vec3,
    torque: Vec3,
    previous_torque: Vec3,
}
#[derive(Resource, Default)]
struct Console {
    selected: Option<Id>,
    pending: Option<(Id, f64, f64)>,
    requested: Option<f64>,
    feedback: Option<String>,
    smooth: Smooth,
    input_frame: Option<f64>,
    power: systems::PowerDisplay,
}

pub(super) fn layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Background, egui::Id::new("ship_console"))
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Console>()
        .add_observer(|_: On<SessionReset>, mut state: ResMut<Console>| *state = Console::default())
        .add_systems(
            EguiPrimaryContextPass,
            (
                draw.in_set(ConsoleDraw),
                keyboard_throttle.in_set(super::input::GameplayInput::Keyboard),
                input
                    .after(super::shell::ShellDraw)
                    .after(keyboard_throttle),
            ),
        );
}

fn manual(ship: &ShipTelemetry, details: &ShipPresentation, connected: bool) -> bool {
    connected
        && ship.presence == travel::Presence::Space
        && matches!(details.computer, ComputerStatus::Running { .. })
        && details.propulsion.rated_forward_n > 0.
}

fn throttle(details: &ShipPresentation, time: u64) -> Option<f64> {
    details
        .instruments
        .as_ref()
        .filter(|i| i.valid_until_ns >= time)
        .and_then(|i| i.navigation.as_ref())
        .map(|i| i.throttle)
}

fn units(value: f64, unit: &str) -> String {
    if unit == "kg" && value.abs() >= 1e3 {
        return format!("{:.1} t", value / 1e3);
    }
    let (n, prefix) = if value.abs() >= 1e9 {
        (value / 1e9, "G")
    } else if value.abs() >= 1e6 {
        (value / 1e6, "M")
    } else if value.abs() >= 1e3 {
        (value / 1e3, "k")
    } else {
        (value, "")
    };
    format!("{n:.1} {prefix}{unit}")
}

fn draw(
    mut contexts: EguiContexts,
    mut state: ResMut<Console>,
    selection: Res<Selection>,
    session: Res<SessionInfo>,
    clock: Res<RenderTime>,
    fixed: Res<Time<Fixed>>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let screen = ctx.content_rect();
    let height = 280.;
    let width = 1108.;
    let x = screen.left() + RAIL_WIDTH + (screen.width() - RAIL_WIDTH - width) * 0.5;
    let rect = egui::Rect::from_min_size(
        egui::pos2(x, screen.bottom() - STATUS_HEIGHT - height - 5.),
        egui::vec2(width, height),
    );
    if state.selected != selection.ship {
        state.selected = selection.ship;
        state.pending = None;
        state.requested = None;
        state.feedback = None;
        state.smooth = Smooth::default();
    }
    let selected = ships
        .iter()
        .find(|(ship, _)| Some(ship.0.ship) == selection.ship);
    egui::Area::new(layer().id)
        .fixed_pos(rect.min)
        .movable(false)
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(9, 10, 12, 245))
                .stroke(egui::Stroke::new(1., egui::Color32::from_gray(85)))
                .inner_margin(10.)
                .show(ui, |ui| {
                    ui.set_height((height - 20.).max(1.));
                    let Some((ship, details)) = selected else {
                        ui.label("SHIP CONSOLE // awaiting connection");
                        return;
                    };
                    let ship = &ship.0;
                    let available = ui.available_rect_before_wrap();
                    let widths = [280., 456., 320.];
                    let mut x = available.left();
                    for (index, width) in widths.into_iter().enumerate() {
                        let area = egui::Rect::from_min_size(
                            egui::pos2(x, available.top()),
                            egui::vec2(width, available.height()),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                            ui.set_clip_rect(area.intersect(ui.clip_rect()));
                            ui.spacing_mut().item_spacing.y = 4.;
                            ui.label(
                                egui::RichText::new(["RESOURCES", "COMPUTER", "SYSTEMS"][index])
                                    .monospace()
                                    .strong(),
                            );
                            ui.separator();
                            let Some(details) = details else {
                                ui.weak("Awaiting instruments…");
                                return;
                            };
                            match index {
                                0 => resources::draw(ui, ship, &details.0),
                                1 => computer::panel(ui, &details.0, clock.display_ns),
                                _ => systems::draw(
                                    ui,
                                    &mut state,
                                    ship,
                                    &details.0,
                                    clock.display_ns,
                                    &fixed,
                                    session.status.is_empty(),
                                ),
                            }
                        });
                        x += width + 16.;
                    }
                });
        });
    Ok(())
}

fn keyboard_throttle(
    mut contexts: EguiContexts,
    mut state: ResMut<Console>,
    ships: Query<(&OwnedShip, &ShipDetails)>,
    session: Res<SessionInfo>,
    clock: Res<RenderTime>,
    time: Res<Time<Real>>,
    windows: Query<&Window>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if state.input_frame == Some(time.elapsed_secs_f64()) {
        return Ok(());
    }
    let Some((ship, details)) = ships
        .iter()
        .find(|(ship, _)| Some(ship.0.ship) == state.selected)
    else {
        return Ok(());
    };
    let command = throttle(&details.0, clock.display_ns);
    if !manual(&ship.0, &details.0, session.status.is_empty()) || command.is_none() {
        return Ok(());
    }
    if windows.iter().any(|w| w.focused) && !ctx.egui_is_using_pointer() {
        let modifiers = ctx.input(|i| i.modifiers);
        if !modifiers.alt && !modifiers.mac_cmd && modifiers.shift != modifiers.ctrl {
            let direction = if modifiers.shift { 1. } else { -1. };
            let base = state
                .pending
                .map_or(command.unwrap(), |(_, value, _)| value);
            state.requested =
                Some((base + direction * time.delta_secs_f64().min(0.1) * 0.25).clamp(0., 1.));
        }
    }
    Ok(())
}

fn input(
    mut state: ResMut<Console>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<(&OwnedShip, &ShipDetails)>,
    session: Res<SessionInfo>,
    feedback: Res<CommandState>,
    clock: Res<RenderTime>,
    time: Res<Time<Real>>,
) -> Result {
    let now = time.elapsed_secs_f64();
    if state.input_frame == Some(now) {
        return Ok(());
    }
    state.input_frame = Some(now);
    let Some((ship, details)) = ships
        .iter()
        .find(|(ship, _)| Some(ship.0.ship) == state.selected)
    else {
        return Ok(());
    };
    let command = throttle(&details.0, clock.display_ns);
    if let Some((id, value, started)) = state.pending {
        if let Some(result) = feedback.results.iter().find(|r| r.id == id) {
            if let Some(error) = &result.error {
                state.feedback = Some(error.clone());
                state.pending = None;
            }
        }
        if command.is_some_and(|v| (value - v).abs() < 1e-5) {
            state.pending = None;
        }
        if now - started > 5. {
            state.pending = None;
            state.feedback = Some("Throttle confirmation unavailable".into());
        }
    }
    if !manual(&ship.0, &details.0, session.status.is_empty()) || command.is_none() {
        state.pending = None;
        state.requested = None;
        return Ok(());
    }
    if let Some(value) = state.requested.take() {
        let id = outgoing.ship(&ship.0, ShipCommand::SetThrottle(value));
        state.pending = Some((id, value, now));
        state.feedback = None;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
