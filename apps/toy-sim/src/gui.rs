mod instruments;
mod mfds;
mod orbit_hud;
mod sensor_hud;
mod universe;
use std::time::Instant;

use bevy::{
    camera::Exposure,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};
use bevy_egui::{
    EguiContexts, EguiPrimaryContextPass,
    egui::{self, ProgressBar},
};

use crate::{
    camera::{CameraFocus, FollowTarget, MainCamera},
    orrery::{BodyClass, Celestial, Universe},
    physics::aerodynamics::AeroEnv,
    precision::{FloatingOrigin, PreciseTransform},
    simulation::{SimulationCounters, TICK_RATE_HZ},
    vessel::{
        ControlledVessel, ShipCatalogue, ShipDesign, ShipHardware, ShipSoftware, Vessel,
        VesselControlState,
    },
};

pub struct GuiPlugin;

impl Plugin for GuiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<sensor_hud::SensorHud>()
            .init_resource::<orbit_hud::OrbitHud>()
            .add_plugins(toy_sim_ship_view::mfd::MfdFontPlugin);
        app.configure_sets(
            PostUpdate,
            bevy_egui::EguiPostUpdateSet::EndPass
                .after(crate::precision::PrecisionSystems::WorldReady)
                .after(bevy::camera::CameraUpdateSystems),
        );
        app.add_systems(
            EguiPrimaryContextPass,
            (
                square_corners,
                (
                    camera_targets,
                    universe::window,
                    (sensor_hud::controls, sensor_hud::overlay).chain(),
                    flight,
                    consumables,
                    diagnostics,
                    recovery,
                    thrusters,
                    mfds::window,
                    (instruments::windows, orbit_hud::overlay).chain(),
                    time,
                    exposure,
                ),
            )
                .chain(),
        );
    }
}

fn exposure(
    mut contexts: EguiContexts,
    mut exposure: Single<&mut Exposure, With<MainCamera>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // Positive stops mean a brighter image; Bevy's EV100 runs in the opposite direction.
    let mut stops = Exposure::default().ev100 - exposure.ev100;
    if !ctx.egui_wants_keyboard_input() {
        ctx.input(|input| {
            if !input.modifiers.command && !input.modifiers.ctrl && !input.modifiers.alt {
                if input.key_pressed(egui::Key::Plus) || input.key_pressed(egui::Key::Equals) {
                    stops += 0.5;
                }
                if input.key_pressed(egui::Key::Minus) {
                    stops -= 0.5;
                }
            }
        });
    }
    egui::Window::new("Exposure")
        .default_pos(egui::pos2(330.0, 10.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("−")
                    .on_hover_text("Darker by half a stop")
                    .clicked()
                {
                    stops -= 0.5;
                }
                ui.label(format!("{stops:+.1} stops"));
                if ui
                    .button("+")
                    .on_hover_text("Brighter by half a stop")
                    .clicked()
                {
                    stops += 0.5;
                }
            });
            ui.small("+/− keys: ±0.5 stop (numpad also supported)");
        });
    exposure.ev100 = Exposure::default().ev100 - stops.clamp(-24.0, 32.0);
    Ok(())
}

fn time(
    mut contexts: EguiContexts,
    fixed_time: Res<Time<Fixed>>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut wall_time: Local<Option<Instant>>,
) {
    let wall_time = wall_time.get_or_insert_with(Instant::now);
    let ctx = contexts.ctx_mut().unwrap();
    egui::Window::new("Time")
        .default_open(false)
        .default_pos(egui::pos2(10.0, 240.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(virtual_time.is_paused(), "Pause")
                    .clicked()
                {
                    virtual_time.pause();
                }
                for (speed, label) in [(1.0, "1×"), (10.0, "10×")] {
                    let selected =
                        !virtual_time.is_paused() && virtual_time.relative_speed() == speed;
                    if ui.selectable_label(selected, label).clicked() {
                        virtual_time.set_relative_speed(speed);
                        virtual_time.unpause();
                    }
                }
            });
            ui.label(format!("Sim time: {:.2} s", fixed_time.elapsed_secs_f64()));
            ui.label(format!(
                "Wall time: {:.2} s",
                wall_time.elapsed().as_secs_f64()
            ));
        });
}

fn flight(
    mut contexts: EguiContexts,
    vessel: Single<(&VesselControlState, &AeroEnv), With<ControlledVessel>>,
) -> Result {
    let (ctrl, aero) = vessel.into_inner();
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Ship")
        .default_pos(egui::pos2(10.0, 10.0))
        .show(ctx, |ui| {
            ui.label(format!("Altitude: {:.1} m", aero.altitude));
            ui.label(format!("True airspeed: {:.1} m/s", aero.airspeed.length()));
            ui.label(format!("Density: {:.3e} kg/m³", aero.density));
            ui.label(format!("Pressure: {:.2} atm", aero.pressure / 101.3e3));
            ui.add(
                ProgressBar::new(ctrl.raw_throttle as f32)
                    .text("Throttle")
                    .corner_radius(0),
            );
            ui.add(
                ProgressBar::new(ctrl.raw_steering.x as f32 / 0.5 + 0.5)
                    .text("Pitch")
                    .corner_radius(0),
            );
            ui.add(
                ProgressBar::new(ctrl.raw_steering.y as f32 / 0.5 + 0.5)
                    .text("Yaw")
                    .corner_radius(0),
            );
            ui.add(
                ProgressBar::new(ctrl.raw_steering.z as f32 / 0.5 + 0.5)
                    .text("Roll")
                    .corner_radius(0),
            );
        });
    Ok(())
}

fn thrusters(
    mut contexts: EguiContexts,
    mut ship: Single<(&ShipDesign, &ShipHardware, &mut ShipSoftware), With<ControlledVessel>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let (design, hardware, software) = &mut *ship;
    egui::Window::new("Hardware diagnostics")
        .default_pos(egui::pos2(650.0, 10.0))
        .show(ctx, |ui| {
            ui.label(format!(
                "Hull {:.0}/{:.0} · Heat {:.1}/{:.1} MJ · Shield {:.0} K · Reserve {:.1}/{:.1} kg · Loss {:.3} kg/s · Strength {:.0}%",
                hardware.0.hull,
                design.0.hull,
                hardware.0.thermal.hull_energy_j / 1e6,
                design.0.hull_heat_capacity_j / 1e6,
                hardware.0.shield_temperature(&design.0),
                hardware.0.thermal.shield_reserve_kg,
                design.0.shield_reserve_capacity_kg,
                hardware.0.thermal.ablation_kg_s,
                hardware.0.shield_strength(&design.0) * 100.0
            ));
            ui.horizontal(|ui| {
                if ui.button("Hull +25 MJ").clicked() {
                    software.hull_energy_j += 25e6;
                }
                if ui.button("Shield +250 MJ").clicked() {
                    software.shield_energy_j += 250e6;
                }
                if ui.button("Reset encounter").clicked() {
                    software.reset = true;
                }
            });
            if let Some(fault) = &software.controller.fault {
                ui.colored_label(egui::Color32::RED, fault);
            }
            if software.controller.is_booting() {
                ui.label(format!(
                    "Computer booting: {:.0}%",
                    software.controller.boot_progress() * 100.
                ));
            }
            ui.label(format!(
                "Ship step: {:.1} µs / WASM memory: {} KiB",
                software.last_seconds * 1e6,
                software.controller.memory_bytes() / 1024
            ));
            let t = software.timings;
            ui.small(format!(
                "Prepare {:.1} / controller {:.1} / publish {:.1} / hardware {:.1} µs",
                t.prepare * 1e6,
                t.callback * 1e6,
                t.publish * 1e6,
                t.hardware * 1e6
            ));
            ui.small(format!(
                "Controller includes {:.1} µs native sensor query",
                t.scan * 1e6
            ));
            ui.small("Last update, wall time; excludes gravity, integration and fleet setup.");
            for &i in &design.0.active_parts {
                let part = &design.0.parts[i];
                let state = &hardware.0.devices[i];
                ui.label(format!(
                    "{}: {} / {:.2}",
                    part.definition.title,
                    if !state.operational {
                        "FAILED"
                    } else if !state.powered {
                        "OFF"
                    } else {
                        "OK"
                    },
                    state.actual
                ));
            }
            for result in &software.results {
                if result.result != toy_sim_ship_api::abi::REPLY_ACCEPTED {
                    ui.label(&result.message);
                }
            }
        });
    Ok(())
}
fn consumables(
    mut contexts: EguiContexts,
    tanks: Single<&ShipHardware, With<ControlledVessel>>,
    catalogue: Res<ShipCatalogue>,
) -> Result {
    egui::Window::new("Inventory")
        .default_open(false)
        .show(contexts.ctx_mut()?, |ui| {
            for (r, q) in catalogue
                .0
                .resources
                .iter()
                .zip(&tanks.0.inventory.quantities)
            {
                ui.label(format!("{}: {:.4}", r.title, q));
            }
            ui.label(format!("Energy: {:.1} J", tanks.0.inventory.energy_j));
        });
    Ok(())
}

fn diagnostics(
    mut contexts: EguiContexts,

    diagnostics: Res<DiagnosticsStore>,
    counters: Res<SimulationCounters>,
    collisions: Res<crate::physics::collision::CollisionStats>,

    objects: Query<(), With<PreciseTransform>>,
    camera: Single<&PreciseTransform, With<MainCamera>>,

    origin: Res<FloatingOrigin>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Diagnostics")
        .default_open(false)
        .default_pos(egui::pos2(10.0, 210.0))
        .show(ctx, |ui| {
            ui.label(format!("Frame: {}", counters.frames));
            ui.label(format!(
                "Simulation tick: {} ({TICK_RATE_HZ:.0} Hz)",
                counters.ticks
            ));
            ui.label(format!("Total: {} objects", objects.iter().len()));
            ui.label(format!(
                "Collision step: {:.2} ms · {} bodies",
                collisions.total_seconds * 1000.0,
                collisions.bodies
            ));
            ui.label(format!(
                "Index {:.2} / queries {:.2} / solve {:.2} ms",
                collisions.index_seconds * 1000.0,
                collisions.query_seconds * 1000.0,
                collisions.solve_seconds * 1000.0
            ));
            ui.label(format!(
                "{} pairs · {} geometry queries · {} impulses · {} contact reviews",
                collisions.candidates,
                collisions.detailed_queries,
                collisions.impacts,
                collisions.contact_reviews
            ));
            ui.label(format!(
                "Impact heat: {:.3} MJ",
                collisions.dissipated_j / 1e6
            ));
            let camera_xyz = camera.into_inner();
            ui.label(format!(
                "Camera translation (mm): {:?}",
                camera_xyz.translation_um
            ));
            ui.label(format!(
                "Floating origin (mm): {:?}",
                origin.0.translation_um
            ));

            if let Some(fps) = diagnostics
                .get(&FrameTimeDiagnosticsPlugin::FPS) // pick the diagnostic you want
                .and_then(|d| d.smoothed())
            {
                ui.label(format!("FPS: {fps:.1}"));
            }
        });

    Ok(())
}

fn square_corners(mut contexts: EguiContexts, mut configured: Local<bool>) -> Result {
    if !*configured {
        contexts.ctx_mut()?.all_styles_mut(|style| {
            let visuals = &mut style.visuals;
            visuals.window_corner_radius = egui::CornerRadius::ZERO;
            visuals.menu_corner_radius = egui::CornerRadius::ZERO;
            for widget in [
                &mut visuals.widgets.noninteractive,
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
                &mut visuals.widgets.open,
            ] {
                widget.corner_radius = egui::CornerRadius::ZERO;
            }
        });
        *configured = true;
    }
    Ok(())
}

fn camera_targets(
    mut contexts: EguiContexts,
    ships: Query<(Entity, &Vessel, Has<CameraFocus>)>,
    celestials: Query<(Entity, &Celestial, Has<CameraFocus>)>,
    orrery: Option<Res<Universe>>,
    mut follow: MessageWriter<FollowTarget>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Camera target")
        .default_pos(egui::pos2(340.0, 10.0))
        .default_width(220.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    ui.strong("Ships");
                    let mut ships: Vec<_> = ships.iter().collect();
                    ships.sort_by(|a, b| a.1.vessel_name.cmp(&b.1.vessel_name).then(a.0.cmp(&b.0)));
                    for (entity, ship, selected) in ships {
                        ui.push_id(entity, |ui| {
                            if ui
                                .selectable_label(selected, ship.vessel_name.as_str())
                                .clicked()
                            {
                                follow.write(FollowTarget(entity));
                            }
                        });
                    }
                    ui.separator();
                    ui.strong("Celestial bodies");
                    let mut bodies: Vec<_> = celestials.iter().collect();
                    bodies.sort_by(|a, b| a.1.0.cmp(&b.1.0).then(a.0.cmp(&b.0)));
                    for (entity, celestial, selected) in bodies {
                        let kind = orrery
                            .as_ref()
                            .and_then(|o| o.get_body(&celestial.0))
                            .map(|body| {
                                if matches!(body.class_params, BodyClass::Star { .. }) {
                                    "star"
                                } else if body
                                    .parent
                                    .as_ref()
                                    .and_then(|p| orrery.as_ref()?.get_body(p))
                                    .is_some_and(|p| matches!(p.class_params, BodyClass::Planet))
                                {
                                    "moon"
                                } else {
                                    "planet"
                                }
                            })
                            .unwrap_or("body");
                        ui.push_id(entity, |ui| {
                            if ui
                                .selectable_label(selected, format!("{} ({kind})", celestial.0))
                                .clicked()
                            {
                                follow.write(FollowTarget(entity));
                            }
                        });
                    }
                });
        });
    Ok(())
}

fn recovery(
    mut contexts: EguiContexts,
    ships: Query<(), With<ControlledVessel>>,
    mut recovery: ResMut<crate::vessel::ShipRecovery>,
) -> Result {
    if !ships.is_empty() {
        return Ok(());
    }
    egui::Window::new("Ship destroyed").show(contexts.ctx_mut()?, |ui| {
        if ui.button("Reset encounter").clicked() {
            recovery.requested = true;
            recovery.encounter = true;
        }
    });
    Ok(())
}
