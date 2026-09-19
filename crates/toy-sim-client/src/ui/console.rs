mod computer;

use super::selection::Selection;
use crate::state::{Outgoing, OwnedShip, RenderTime, SessionInfo, SessionReset, ShipDetails};
use bevy::prelude::*;
use toy_sim_model::*;
use toy_sim_ui::{
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
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Console>()
        .add_observer(|_: On<SessionReset>, mut state: ResMut<Console>| *state = Console::default())
        .add_systems(
            EguiPrimaryContextPass,
            (
                draw.in_set(ConsoleDraw),
                input.after(super::shell::ShellDraw),
            ),
        );
}

fn manual(ship: &ShipTelemetry, details: &ShipPresentation, connected: bool) -> bool {
    connected
        && !ship.travel.autopilot_enabled
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

fn readout(ui: &mut egui::Ui, label: &str, amount: f64, capacity: f64, unit: &str) {
    ui.horizontal(|ui| {
        let icon = if label == "BATTERY" {
            Icon::Power
        } else if label == "SHIELD RESERVE" {
            Icon::Shield
        } else {
            Icon::Cargo
        };
        let fraction = if capacity > 0. { amount / capacity } else { 0. };
        ui.label(icon.text(14.).color(gauges::Tone::Reserve.color(fraction)));
        ui.label(egui::RichText::new(label).monospace().size(11.));
    });
    gauges::gauge(
        ui,
        &format!("{} / {}", units(amount, unit), units(capacity, unit)),
        22.,
        if capacity > 0. { amount / capacity } else { 0. },
        None,
        None,
        false,
        gauges::Tone::Reserve,
    );
}

fn reserves(ui: &mut egui::Ui, ship: &ShipTelemetry, d: &ShipPresentation) {
    let p = &d.propulsion;
    let propellant: Vec<_> = d
        .inventory
        .iter()
        .filter(|r| p.propellants.contains(&r.resource))
        .collect();
    if !propellant.is_empty() {
        readout(
            ui,
            "PROPELLANT",
            propellant.iter().map(|r| r.amount_kg).sum(),
            propellant.iter().map(|r| r.capacity_kg).sum(),
            "kg",
        );
        ui.label(egui::RichText::new("Tank contents ⓘ").size(10.).weak())
            .on_hover_ui(|ui| {
                for r in &propellant {
                    ui.label(format!(
                        "{}: {} / {}",
                        r.name,
                        units(r.amount_kg, "kg"),
                        units(r.capacity_kg, "kg")
                    ));
                }
            });
    }
    readout(
        ui,
        "BATTERY",
        ship.battery_j as f64,
        d.battery_capacity_j as f64,
        "J",
    );
    for r in d
        .inventory
        .iter()
        .filter(|r| p.fuels.contains(&r.resource) || p.charges.contains(&r.resource))
    {
        readout(ui, &r.name.to_uppercase(), r.amount_kg, r.capacity_kg, "kg");
    }
    if let Some(h) = &d.health {
        if h.shield_reserve_capacity_kg > 0. {
            readout(
                ui,
                "SHIELD RESERVE",
                ship.coolant_reserve_kg,
                h.shield_reserve_capacity_kg,
                "kg",
            );
        }
    }
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
    let width = (screen.width() - RAIL_WIDTH - 24.).clamp(1., 840.);
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
    egui::Area::new(egui::Id::new("ship_console"))
        .fixed_pos(rect.min)
        .movable(false)
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(8, 13, 18, 238))
                .stroke(egui::Stroke::new(1., egui::Color32::from_rgb(63, 79, 87)))
                .inner_margin(10.)
                .show(ui, |ui| {
                    ui.set_height((height - 20.).max(1.));
                    let Some((ship, details)) = selected else {
                        ui.label("SHIP CONSOLE // awaiting connection");
                        return;
                    };
                    let ship = &ship.0;
                    let available = ui.available_rect_before_wrap();
                    let gap = 12.;
                    let left_width = (available.width() - gap) * 0.45;
                    let left = egui::Rect::from_min_size(
                        available.min,
                        egui::vec2(left_width, available.height()),
                    );
                    let right = egui::Rect::from_min_max(
                        left.right_top() + egui::vec2(gap, 0.),
                        available.max,
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                        ui.label(egui::RichText::new("RESERVES").monospace().strong());
                        egui::ScrollArea::vertical()
                            .id_salt("reserves")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                if let Some(d) = details {
                                    reserves(ui, ship, &d.0);
                                } else {
                                    ui.weak("Awaiting instruments…");
                                }
                            });
                    });
                    ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                        ui.label(egui::RichText::new("OPERATIONS").monospace().strong());
                        egui::ScrollArea::vertical()
                            .id_salt("operations")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 3.;
                                let Some(details) = details else {
                                    ui.weak("Awaiting instruments…");
                                    return;
                                };
                                let d = &details.0;
                                computer::draw(ui, d, clock.display_ns);
                                if !matches!(d.computer, ComputerStatus::Running { .. }) {
                                    state.pending = None;
                                    state.requested = None;
                                    state.feedback = None;
                                }
                                let command = throttle(d, clock.display_ns);
                                let enabled =
                                    manual(ship, d, session.status.is_empty()) && command.is_some();
                                if ship.travel.autopilot_enabled {
                                    state.pending = None;
                                }
                                if state.smooth.stamp != d.sim_time_ns {
                                    state.smooth.previous_force = state.smooth.force;
                                    state.smooth.previous_torque = state.smooth.torque;
                                    state.smooth.force =
                                        Vec3::from_array(d.propulsion.force_n.map(|v| v as f32));
                                    state.smooth.torque =
                                        Vec3::from_array(d.propulsion.torque_nm.map(|v| v as f32));
                                    if state.smooth.stamp == 0 {
                                        state.smooth.previous_force = state.smooth.force;
                                        state.smooth.previous_torque = state.smooth.torque;
                                    }
                                    state.smooth.stamp = d.sim_time_ns;
                                }
                                let rotation =
                                    Quat::from_array(d.control_rotation.map(|v| v as f32))
                                        .inverse();
                                let force = rotation
                                    * state
                                        .smooth
                                        .previous_force
                                        .lerp(state.smooth.force, fixed.overstep_fraction());
                                let torque = rotation
                                    * state
                                        .smooth
                                        .previous_torque
                                        .lerp(state.smooth.torque, fixed.overstep_fraction());
                                ui.label(
                                    egui::RichText::new(if ship.travel.autopilot_enabled {
                                        "THRUST // AP CONTROL"
                                    } else {
                                        "THRUST // MANUAL"
                                    })
                                    .monospace()
                                    .size(11.),
                                );
                                let max = d.propulsion.rated_forward_n;
                                let label = if max <= 0. {
                                    "NO FORWARD PROPULSION".into()
                                } else {
                                    format!(
                                        "{} / {} · {}",
                                        units(-force.z as f64, "N"),
                                        units(max, "N"),
                                        command
                                            .map_or("—".into(), |v| format!("{:.0}%", v * 100.))
                                    )
                                };
                                let response = gauges::gauge(
                                    ui,
                                    &label,
                                    36.,
                                    if max > 0. { -force.z as f64 / max } else { 0. },
                                    command,
                                    state.pending.map(|(_, value, _)| value),
                                    enabled,
                                    gauges::Tone::Normal,
                                );
                                if enabled && (response.clicked() || response.dragged()) {
                                    if let Some(pos) = response.interact_pointer_pos() {
                                        state.requested = Some(
                                            ((pos.x - response.rect.left()) / response.rect.width())
                                                .clamp(0., 1.)
                                                as f64,
                                        );
                                    }
                                }
                                if let Some(message) = &state.feedback {
                                    ui.colored_label(egui::Color32::LIGHT_RED, message);
                                }
                                for (axis, name) in ["PITCH", "YAW", "ROLL"].iter().enumerate() {
                                    gauges::bipolar(
                                        ui,
                                        &format!("{name} {}", units(torque[axis] as f64, "Nm")),
                                        torque[axis] as f64,
                                        d.propulsion.negative_torque_nm[axis],
                                        d.propulsion.positive_torque_nm[axis],
                                    );
                                }
                                ui.label(
                                    egui::RichText::new(format!(
                                        "POWER +{} / −{}",
                                        units(d.power_generated_w, "W"),
                                        units(d.power_consumed_w, "W")
                                    ))
                                    .monospace()
                                    .size(11.),
                                );
                                let hull = d
                                    .health
                                    .as_ref()
                                    .map_or(0., |h| h.hull_hp / h.hull_max_hp.max(1.));
                                let heat = ship.hull_heat_j / d.hull_heat_capacity_j.max(1.);
                                for (label, fraction) in [("HULL", hull), ("HEAT", heat)] {
                                    gauges::gauge(
                                        ui,
                                        &format!("{label} {:.0}%", fraction * 100.),
                                        18.,
                                        fraction,
                                        None,
                                        None,
                                        false,
                                        if label == "HULL" {
                                            gauges::Tone::Reserve
                                        } else {
                                            gauges::Tone::Heat
                                        },
                                    );
                                }
                                let temperature = ship.shield_temperature_k;
                                gauges::gauge(
                                    ui,
                                    &format!(
                                        "SHIELD {:.0} K · {:.0}% coverage",
                                        temperature,
                                        d.health.as_ref().map_or(0., |h| h.shield_strength) * 100.
                                    ),
                                    20.,
                                    temperature / toy_sim_ships::thermal::VAPORIZATION_K,
                                    None,
                                    None,
                                    false,
                                    gauges::Tone::Heat,
                                );
                            });
                    });
                });
        });
    Ok(())
}

fn input(
    mut contexts: EguiContexts,
    mut state: ResMut<Console>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<(&OwnedShip, &ShipDetails)>,
    session: Res<SessionInfo>,
    clock: Res<RenderTime>,
    time: Res<Time<Real>>,
    windows: Query<&Window>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
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
        if let Some(result) = session.results.iter().find(|r| r.id == id) {
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
    if windows.iter().any(|w| w.focused)
        && !ctx.egui_wants_keyboard_input()
        && !ctx.is_pointer_over_egui()
    {
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
    if let Some(value) = state.requested.take() {
        let id = outgoing.ship(&ship.0, ShipCommand::SetThrottle(value));
        state.pending = Some((id, value, now));
        state.feedback = None;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
