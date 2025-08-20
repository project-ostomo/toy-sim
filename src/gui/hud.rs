use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use bevy_egui::{
    EguiContexts,
    egui::{self, Id},
};

use crate::{
    camera::{CameraFocus, MainCamera},
    physics::AeroParams,
    precision::PreciseTransform,
};

pub fn hud(
    mut contexts: EguiContexts,
    camera: Single<(&PreciseTransform, &Projection), With<MainCamera>>,
    focus: Single<&PreciseTransform, With<CameraFocus>>,
    aero: Single<&AeroParams, With<CameraFocus>>,
) {
    let aero = aero.into_inner();
    let (cam_xform, projection) = camera.into_inner();
    let focus = focus.into_inner();

    let ctx = contexts.ctx_mut().unwrap();
    let screen_rect = ctx.screen_rect();
    let centre = screen_rect.center();

    // Any world-space direction you like:
    let dir_to_focus = focus.rotation * DVec3::NEG_Z;

    let nose_offset =
        dir_to_screen_offset(dir_to_focus, cam_xform.rotation, projection, screen_rect);
    let airspeed_offset = dir_to_screen_offset(
        aero.airspeed.normalize(),
        cam_xform.rotation,
        projection,
        screen_rect,
    );
    egui::Area::new(Id::new("crosshair"))
        .interactable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            if let Some(nose_offset) = nose_offset {
                ui.painter().add(egui::Shape::circle_filled(
                    centre + nose_offset,
                    10.0,
                    egui::Rgba::from_rgba_unmultiplied(0., 1., 0., 0.5),
                ));
            }
            if let Some(airspeed_offset) = airspeed_offset {
                ui.painter().add(egui::Shape::circle_filled(
                    centre + airspeed_offset,
                    5.0,
                    egui::Rgba::from_rgba_unmultiplied(1., 0., 0., 0.5),
                ));
            }
            ui.painter().add(egui::Shape::circle_stroke(
                centre,
                30.0,
                egui::Stroke {
                    color: egui::Rgba::from_rgba_unmultiplied(0., 1., 0., 0.5).into(),
                    width: 2.0,
                },
            ));
        });

    // Optional debug window
    egui::Window::new("Camera").show(ctx, |ui| {
        ui.label(format!("Direction (world): {:?}", dir_to_focus));
    });
}

fn dir_to_screen_offset(
    dir_world: DVec3,
    camera_rot: DQuat,
    projection: &Projection,
    screen_rect: egui::Rect,
) -> Option<egui::Vec2> {
    // --- -x--------------------- Local space conversion & back-facing test
    let dir_cam = camera_rot.inverse() * dir_world;
    if dir_cam.z >= 0.0 {
        // Camera in Bevy looks toward –Z; +Z is behind us.
        return None;
    }

    // --- yaw / pitch (in radians)
    let yaw = dir_cam.x.atan2(-dir_cam.z) as f32;
    let pitch = dir_cam.y.clamp(-1.0, 1.0).asin() as f32;

    // --- extract horizontal & vertical FOV
    let (fov_y, fov_x) = match projection {
        Projection::Perspective(p) => {
            let fov_y = p.fov;
            let aspect = screen_rect.aspect_ratio();
            let fov_x = 2.0 * ((fov_y / 2.0).tan() * aspect).atan();
            (fov_y, fov_x)
        }
        // Extend here for other projection types.
        _ => return None,
    };

    // --- normalised screen coordinates in range [-1,1]
    let nx = (yaw.tan()) / (fov_x / 2.0).tan();
    let ny = (pitch.tan()) / (fov_y / 2.0).tan();

    // --- clamp so icons don’t fly kilometres off screen
    let nx = nx.clamp(-1.5, 1.5);
    let ny = ny.clamp(-1.5, 1.5);

    let half_w = screen_rect.width() * 0.5;
    let half_h = screen_rect.height() * 0.5;

    // Return offset **from** screen centre
    Some(egui::vec2(nx * half_w, -ny * half_h))
}
