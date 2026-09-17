use super::Selection;
use crate::state::{Outgoing, SessionInfo};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use toy_sim_model::*;

#[derive(Resource)]
pub(super) struct DebugControls {
    heat_j: f64,
    inspect: bool,
    sensor_range_m: f64,
    sensor_occlusion: bool,
}

impl Default for DebugControls {
    fn default() -> Self {
        Self {
            heat_j: 1e9,
            inspect: false,
            sensor_range_m: 1e8,
            sensor_occlusion: true,
        }
    }
}

pub(super) fn window(
    mut contexts: EguiContexts,
    mut controls: ResMut<DebugControls>,
    selection: Res<Selection>,
    mut outgoing: ResMut<Outgoing>,
    session: Res<SessionInfo>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if session.capabilities.is_empty() {
        return Ok(());
    }
    egui::Window::new("Server debug")
        .default_pos([380., 10.])
        .show(ctx, |ui| {
            if session.capabilities.contains(&DebugCapability::Clock) {
                ui.horizontal(|ui| {
                    for (label, rate) in [("Pause", 0.), ("1×", 1.), ("10×", 10.)] {
                        if ui.button(label).clicked() {
                            outgoing.push(Action::Debug(DebugCommand::SetRate(rate)));
                        }
                    }
                    if ui.button("Step").clicked() {
                        outgoing.push(Action::Debug(DebugCommand::Step));
                    }
                });
            }
            if session.capabilities.contains(&DebugCapability::Reset)
                && ui.button("Reset encounter").clicked()
            {
                outgoing.push(Action::Debug(DebugCommand::Reset));
            }
            if let Some(ship) = selection.ship {
                if session.capabilities.contains(&DebugCapability::Recover)
                    && ui.button("Recover focused ship").clicked()
                {
                    outgoing.push(Action::Debug(DebugCommand::Recover { ship }));
                }
                if session
                    .capabilities
                    .contains(&DebugCapability::ConfigureSensor)
                {
                    ui.add(
                        egui::Slider::new(&mut controls.sensor_range_m, 1e3..=1e10)
                            .logarithmic(true)
                            .text("Sensor range (m)"),
                    );
                    ui.checkbox(&mut controls.sensor_occlusion, "Celestial occlusion");
                    if ui.button("Apply sensor settings").clicked() {
                        outgoing.push(Action::Debug(DebugCommand::ConfigureSensor {
                            ship,
                            range_m: controls.sensor_range_m,
                            occlusion: controls.sensor_occlusion,
                        }));
                    }
                }
                if session.capabilities.contains(&DebugCapability::InjectHeat) {
                    ui.add(
                        egui::DragValue::new(&mut controls.heat_j)
                            .range(0. ..=1e18)
                            .suffix(" J"),
                    );
                    if ui.button("Add shield heat").clicked() {
                        outgoing.push(Action::Debug(DebugCommand::InjectShieldHeat {
                            ship,
                            joules: controls.heat_j,
                        }));
                    }
                    if ui.button("Add hull heat").clicked() {
                        outgoing.push(Action::Debug(DebugCommand::InjectHeat {
                            ship,
                            joules: controls.heat_j,
                        }));
                    }
                }
            }
            if session.capabilities.contains(&DebugCapability::Inspect)
                && ui
                    .checkbox(&mut controls.inspect, "Server diagnostics")
                    .changed()
            {
                outgoing.push(Action::Debug(DebugCommand::Inspect(controls.inspect)));
            }
            if let Some(data) = &session.diagnostics {
                ui.label(format!(
                    "{} entities · {} active · {} dormant",
                    data.entity_count, data.active_ships, data.dormant_ships
                ));
                ui.label(format!(
                    "Tick {} · {:.2} ms",
                    session.tick, data.tick_duration_ms
                ));
                for (name, milliseconds) in &data.systems {
                    ui.label(format!("{name}: {milliseconds:.3} ms"));
                }
            }
        });
    Ok(())
}
