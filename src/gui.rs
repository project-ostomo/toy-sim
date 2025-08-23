mod hud;

use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};
use bevy_egui::{
    EguiContexts, EguiPrimaryContextPass,
    egui::{self, ProgressBar},
};

use crate::{
    camera::{CameraFocus, MainCamera},
    gui::hud::{bottom_hud, overlay_hud},
    physics::AeroEnv,
    precision::{FloatingOrigin, PreciseTransform},
    vessel::{ConsumableTanks, Thruster, VesselControls},
};

pub struct GuiPlugin;

impl Plugin for GuiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            (
                flight,
                consumables,
                diagnostics,
                thrusters,
                overlay_hud,
                bottom_hud,
            ),
        );
    }
}

fn flight(
    mut contexts: EguiContexts,
    vessel: Single<(&VesselControls, &AeroEnv), With<CameraFocus>>,
) -> Result {
    let (ctrl, aero) = vessel.into_inner();
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Flight").show(ctx, |ui| {
        ui.label(format!("Altitude: {:.1} m", aero.altitude));
        ui.label(format!("True airspeed: {:.1} m/s", aero.airspeed.length()));
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
    focused: Single<&Children, With<CameraFocus>>,
    thrusters: Query<&Thruster>,
) -> Result {
    let children = focused.into_inner();
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Thrusters").show(ctx, |ui| {
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
    tanks: Single<&ConsumableTanks, With<CameraFocus>>,
) -> Result {
    let tanks = tanks.into_inner();
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Consumables").show(ctx, |ui| {
        for (cs, (val, _)) in tanks.iter() {
            ui.label(format!("{cs:?}: {val:.2}"));
        }
    });
    Ok(())
}

fn diagnostics(
    mut contexts: EguiContexts,

    diagnostics: Res<DiagnosticsStore>,

    objects: Query<(), With<PreciseTransform>>,
    camera: Single<&PreciseTransform, With<MainCamera>>,

    origin: Res<FloatingOrigin>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Diagnostics").show(ctx, |ui| {
        ui.label(format!("Total: {} objects", objects.iter().len()));
        let camera_xyz = camera.into_inner();
        ui.label(format!(
            "Camera translation (mm): {:?}",
            camera_xyz.translation_mm
        ));
        ui.label(format!(
            "Floating origin (mm): {:?}",
            origin.0.translation_mm
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
