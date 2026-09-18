use super::ViewCamera;
use crate::ui::SelectedTarget;
use crate::ui::{
    Selection,
    state::{
        Celestial, CelestialSystem, Contact, DisplayPose, SystemSubscription, ViewObservation,
    },
};
use bevy::{prelude::*, window::PrimaryWindow};
use toy_sim_model::Tag;
use toy_sim_ui::bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub(super) fn install(app: &mut App) {
    app.add_systems(EguiPrimaryContextPass, overlay);
}

fn overlay(
    mut contexts: EguiContexts,
    mut active: ResMut<Selection>,
    mut cameras: Query<(
        Entity,
        &Camera,
        &GlobalTransform,
        &ViewCamera,
        &ViewObservation,
        &mut super::camera::CameraOptions,
        &SystemSubscription,
    )>,
    contacts: Query<(&Contact, &DisplayPose)>,
    celestials: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let scale = windows
        .iter()
        .next()
        .map_or(1.0, |window| window.scale_factor())
        / ctx.pixels_per_point();
    let pointer = ctx.input(|input| input.pointer.interact_pos());
    let clicked = ctx.input(|input| input.pointer.primary_clicked());
    let mut selection = None;
    let mut selection_distance = f32::INFINITY;

    for (view_entity, camera, transform, view_camera, observation, framing, systems) in &mut cameras
    {
        let view = &observation.0;
        let Some(viewport) = camera.logical_viewport_rect() else {
            continue;
        };
        let clip = egui::Rect::from_min_max(
            egui::pos2(viewport.min.x * scale, viewport.min.y * scale),
            egui::pos2(viewport.max.x * scale, viewport.max.y * scale),
        );
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new(("sensor_hud", view.id)),
            ))
            .with_clip_rect(clip);
        let mut bodies: Vec<_> = celestials
            .iter()
            .filter(|(_, _, system)| {
                systems
                    .0
                    .iter()
                    .any(|reference| reference.system == system.0)
            })
            .collect();
        bodies.sort_unstable_by_key(|(body, _, _)| body.0.entity);
        for (body, pose, _) in bodies {
            let relative = pose.0.position.relative_to(view_camera.origin);
            let Ok(projected) = camera.world_to_viewport(transform, relative.as_vec3()) else {
                continue;
            };
            let center = egui::pos2(projected.x * scale, projected.y * scale);
            if !clip.contains(center) {
                continue;
            }
            let selected =
                matches!(framing.focus, Some(SelectedTarget::Celestial(id)) if id == body.0.entity);
            let color = if selected {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgb(255, 210, 125)
            };
            let marker = egui::Rect::from_center_size(center, egui::vec2(14., 14.));
            painter.circle_stroke(
                center,
                7.,
                egui::Stroke::new(if selected { 2. } else { 1. }, color),
            );
            painter.text(
                marker.right_bottom() + egui::vec2(2., 2.),
                egui::Align2::LEFT_TOP,
                format!("{}\n{}", body.0.name, distance(relative.length())),
                egui::FontId::proportional(11.),
                color,
            );
            if clicked {
                if let Some(pointer) =
                    pointer.filter(|position| marker.expand(4.).contains(*position))
                {
                    let available = ctx
                        .layer_id_at(pointer)
                        .is_none_or(|layer| layer.order == egui::Order::Background);
                    let distance = center.distance_sq(pointer);
                    if available && distance < selection_distance {
                        selection = Some((
                            view_entity,
                            view.id,
                            SelectedTarget::Celestial(body.0.entity),
                        ));
                        selection_distance = distance;
                    }
                }
            }
        }
        for (contact, pose) in contacts
            .iter()
            .filter(|(contact, _)| {
                contact.1.group == view.group && view.tracks.contains(&contact.0.id)
            })
            .take(512)
        {
            let track = &contact.0;
            if track.entity.is_some() && track.entity == view.focused_ship {
                continue;
            }
            let pose = &pose.0;
            let relative = pose.position.relative_to(view_camera.origin);
            let Ok(projected) = camera.world_to_viewport(transform, relative.as_vec3()) else {
                continue;
            };
            let center = egui::pos2(projected.x * scale, projected.y * scale);
            if !clip.contains(center) {
                continue;
            }
            let selected = active.contact() == Some(contact.1);
            let color = if selected {
                egui::Color32::WHITE
            } else if track
                .tags
                .iter()
                .any(|tag| matches!(tag, Tag::Kind(kind) if kind == "celestial"))
            {
                egui::Color32::from_rgb(255, 210, 125)
            } else {
                egui::Color32::from_rgb(125, 220, 255)
            };
            let square = egui::Rect::from_center_size(center, egui::vec2(14.0, 14.0));
            painter.rect_stroke(
                square,
                0.0,
                egui::Stroke::new(if selected { 2.0 } else { 1.0 }, color),
                egui::StrokeKind::Inside,
            );
            let name = track
                .tags
                .iter()
                .find_map(|tag| match tag {
                    Tag::Advertised(name) => Some(name.as_str()),
                    _ => None,
                })
                .unwrap_or("Contact");
            let text = format!("{name}\n{}", distance(relative.length()));
            painter.text(
                square.right_bottom() + egui::vec2(2.0, 2.0),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(11.0),
                color,
            );
            if clicked {
                if let Some(pointer) =
                    pointer.filter(|position| square.expand(4.0).contains(*position))
                {
                    let available = ctx
                        .layer_id_at(pointer)
                        .is_none_or(|layer| layer.order == egui::Order::Background);
                    let distance = center.distance_sq(pointer);
                    if available && distance < selection_distance {
                        selection =
                            Some((view_entity, view.id, SelectedTarget::Contact(contact.1)));
                        selection_distance = distance;
                    }
                }
            }
        }
    }
    if let Some((entity, view, target)) = selection {
        active.view = Some(view);
        match target {
            SelectedTarget::Contact(contact) => {
                active.target = Some(SelectedTarget::Contact(contact));
            }
            SelectedTarget::Celestial(id) => {
                active.target = Some(SelectedTarget::Celestial(id));
                if let Ok((_, _, _, _, _, mut framing, _)) = cameras.get_mut(entity) {
                    framing.focus = Some(SelectedTarget::Celestial(id));
                }
            }
        }
    }
    Ok(())
}

fn distance(metres: f64) -> String {
    if metres >= 1.0e12 {
        format!("{:.2} AU", metres / 149_597_870_700.0)
    } else if metres >= 1.0e6 {
        format!("{:.2} Mm", metres / 1.0e6)
    } else if metres >= 1000.0 {
        format!("{:.1} km", metres / 1000.0)
    } else {
        format!("{metres:.0} m")
    }
}
