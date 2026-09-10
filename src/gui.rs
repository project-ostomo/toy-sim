mod sensor_hud;
mod universe;
use std::time::Instant;

use bevy::{
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
    vessel::{ConsumableTanks, ControlledVessel, Thruster, Vessel, VesselControlState},
};

pub struct GuiPlugin;

impl Plugin for GuiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<sensor_hud::SensorHud>();
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
                    thrusters,
                    time,
                ),
            )
                .chain(),
        );
    }
}

fn time(
    mut contexts: EguiContexts,
    fixed_time: Res<Time<Fixed>>,
    mut wall_time: Local<Option<Instant>>,
) {
    let wall_time = wall_time.get_or_insert_with(Instant::now);
    let ctx = contexts.ctx_mut().unwrap();
    egui::Window::new("Time")
        .default_open(false)
        .default_pos(egui::pos2(10.0, 240.0))
        .show(ctx, |ui| {
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
    focused: Single<&Children, With<ControlledVessel>>,
    thrusters: Query<&Thruster>,
) -> Result {
    let children = focused.into_inner();
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Thrusters")
        .default_open(false)
        .default_pos(egui::pos2(10.0, 270.0))
        .show(ctx, |ui| {
            for (i, thruster) in thrusters.iter_many(children).enumerate() {
                ui.label(format!(
                    "{i}: {}% / {:.2} N",
                    (thruster.throttle * 100.0) as usize,
                    thruster.current_thrust
                ));
            }
        });
    Ok(())
}

fn consumables(
    mut contexts: EguiContexts,
    tanks: Single<&ConsumableTanks, With<ControlledVessel>>,
) -> Result {
    let tanks = tanks.into_inner();
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Consumables")
        .default_open(false)
        .default_pos(egui::pos2(10.0, 300.0))
        .show(ctx, |ui| {
            for (cs, (val, _)) in tanks.iter() {
                ui.label(format!("{cs:?}: {val:.2}"));
            }
        });
    Ok(())
}

fn diagnostics(
    mut contexts: EguiContexts,

    diagnostics: Res<DiagnosticsStore>,
    counters: Res<SimulationCounters>,

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
