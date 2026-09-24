use super::ViewCamera;
use crate::state::{DisplayPose, Optical, OwnedShip, ShipDetails, SocietyState, ViewObservation};
use crate::ui::{Selection, shell::Shell};
use bevy::{prelude::*, window::PrimaryWindow};
use osg_ui::bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use osg_ui::units::distance;

pub(super) fn install(app: &mut App) {
    app.add_systems(
        EguiPrimaryContextPass,
        overlay.after(crate::ui::shell::ShellDraw),
    );
}

fn overlay(
    mut contexts: EguiContexts,
    mut selection: ResMut<Selection>,
    shell: Res<Shell>,
    session: Res<SocietyState>,
    optical: Query<(&Optical, &DisplayPose)>,
    cameras: Query<(&Camera, &GlobalTransform, &ViewCamera, &ViewObservation)>,
    owned: Query<(&OwnedShip, Option<&ShipDetails>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let scale = windows
        .iter()
        .next()
        .map_or(1.0, |window| window.scale_factor())
        / ctx.pixels_per_point();
    let pointer = ctx.input(|input| input.pointer.interact_pos());
    let clicked = crate::ui::input::pointer_available(ctx)
        && ctx.input(|input| input.pointer.primary_clicked());
    let marked = owned
        .iter()
        .find(|(ship, _)| Some(ship.0.ship) == selection.ship)
        .and_then(|(_, details)| details)
        .and_then(|details| details.0.instruments.as_ref())
        .and_then(|instruments| instruments.weapons_state.as_ref())
        .and_then(|weapons| weapons.target);
    let mut picked = None;
    let mut nearest = f32::INFINITY;

    for (camera, transform, view_camera, observation) in &cameras {
        if view_camera.private || Some(observation.0.id) != selection.view {
            continue;
        }
        let Some(viewport) = camera.logical_viewport_rect() else {
            continue;
        };
        let clip = egui::Rect::from_min_max(
            egui::pos2(viewport.min.x * scale, viewport.min.y * scale),
            egui::pos2(viewport.max.x * scale, viewport.max.y * scale),
        );
        let painter =
            osg_ui::desktop::hud_painter(ctx, egui::Id::new(("objects", observation.0.id)))
                .with_clip_rect(clip);

        for (object, pose) in &optical {
            if object.0.view != observation.0.id
                || object.0.contact.is_some()
                || object.0.known_entity == observation.0.focused_ship
            {
                continue;
            }
            let Some(iff) = &object.0.iff else { continue };
            let relative = pose.0.position.relative_to(view_camera.origin);
            let Ok(projected) = camera.world_to_viewport(transform, relative.as_vec3()) else {
                continue;
            };
            let center = egui::pos2(projected.x * scale, projected.y * scale);
            if !clip.contains(center) {
                continue;
            }
            let name = iff
                .labels
                .first()
                .cloned()
                .unwrap_or_else(|| "Identified ship".into());
            let standing = session
                .society
                .directory
                .contact_standing(session.society.account, Some(iff));
            painter.text(
                center + egui::vec2(10., 8.),
                egui::Align2::LEFT_TOP,
                format!("{}\n{}", name, distance(relative.length())),
                egui::FontId::proportional(11.),
                crate::ui::standing::color(standing).gamma_multiply(0.55),
            );
        }

        for object in shell.hud() {
            let relative = object.position.relative_to(view_camera.origin);
            let Ok(projected) = camera.world_to_viewport(transform, relative.as_vec3()) else {
                continue;
            };
            let center = egui::pos2(projected.x * scale, projected.y * scale);
            if !clip.contains(center) {
                continue;
            }
            let square = egui::Rect::from_center_size(center, egui::vec2(14., 14.));
            let selected = selection.target == Some(object.target);
            let alpha = if selected {
                1.
            } else if object.visible {
                0.55
            } else {
                0.
            };
            let color = crate::ui::standing::color(object.standing).gamma_multiply(alpha);
            painter.rect_stroke(
                square,
                0.,
                egui::Stroke::new(if selected { 2. } else { 1. }, color),
                egui::StrokeKind::Inside,
            );
            painter.text(
                square.right_bottom() + egui::vec2(3., 2.),
                egui::Align2::LEFT_TOP,
                format!("{}\n{}", object.name, distance(relative.length())),
                egui::FontId::proportional(11.),
                color,
            );
            if object.contact.is_some() && object.contact == marked {
                painter.circle_stroke(
                    center,
                    15.,
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 65, 65)),
                );
            }
            if clicked && let Some(pointer) = pointer.filter(|position| square.contains(*position))
            {
                let distance = center.distance_sq(pointer);
                if distance < nearest {
                    nearest = distance;
                    picked = Some(object.target);
                }
            }
        }
    }
    if let Some(target) = picked {
        selection.target = Some(target);
    }
    Ok(())
}
