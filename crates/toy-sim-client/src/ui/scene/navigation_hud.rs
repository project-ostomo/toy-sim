use super::ViewCamera;
use crate::{
    state::{OwnedShip, ViewObservation},
    ui::{SelectedTarget, Selection},
};
use bevy::{prelude::*, window::PrimaryWindow};
use toy_sim_model::{Id, travel};
use toy_sim_ui::{
    bevy_egui::{EguiContexts, EguiPrimaryContextPass},
    egui,
};

pub(super) fn install(app: &mut App) {
    app.add_systems(EguiPrimaryContextPass, draw);
}
fn draw(
    mut contexts: EguiContexts,
    mut selected: ResMut<Selection>,
    beacons: Query<(&crate::state::NavigationObject, &crate::state::DisplayPose)>,
    cameras: Query<(&Camera, &GlobalTransform, &ViewCamera, &ViewObservation)>,
    ships: Query<(&OwnedShip, &crate::state::DisplayPose)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let scale = windows.iter().next().map_or(1., |w| w.scale_factor()) / ctx.pixels_per_point();
    for (camera, transform, view, observation) in &cameras {
        if view.private {
            continue;
        }
        let Some(viewport) = camera.logical_viewport_rect() else {
            continue;
        };
        let rect = egui::Rect::from_min_max(
            egui::pos2(viewport.min.x * scale + 75., viewport.min.y * scale + 150.),
            egui::pos2(viewport.max.x * scale - 25., viewport.max.y * scale - 45.),
        );
        let ship = ships
            .iter()
            .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship);
        let queue: Vec<Id> = ship
            .into_iter()
            .flat_map(|(ship, _)| ship.0.travel.orders.iter().skip(ship.0.travel.order))
            .filter_map(|order| match order {
                travel::Order::Jump(id)
                | travel::Order::Dock(id)
                | travel::Order::TravelTo(travel::Destination::Beacon(id)) => Some(*id),
                _ => None,
            })
            .collect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new(("navigation_hud", view.view)),
        ));
        for (beacon, pose) in &beacons {
            let beacon = &beacon.0;
            let waypoint = queue.iter().position(|id| *id == beacon.id);
            let offset = pose.0.position.relative_to(view.origin);
            if offset.length() > 1e12 && waypoint.is_none() {
                continue;
            }
            let Some(ndc) = camera.world_to_ndc(transform, offset.as_vec3()) else {
                continue;
            };
            let projected = egui::pos2(
                (viewport.min.x + (ndc.x + 1.) * viewport.width() * 0.5) * scale,
                (viewport.min.y + (1. - ndc.y) * viewport.height() * 0.5) * scale,
            );
            let on_screen = rect.contains(projected) && ndc.z >= 0.;
            if !on_screen && waypoint.is_none() {
                continue;
            }
            let point = if on_screen {
                projected
            } else {
                let direction = (projected - rect.center()) * if ndc.z < 0. { -1. } else { 1. };
                let direction = direction.normalized();
                let extent = rect.size() * 0.5;
                let reach = (extent.x / direction.x.abs().max(0.001))
                    .min(extent.y / direction.y.abs().max(0.001));
                rect.center() + direction * reach
            };
            let color = if waypoint.is_some() {
                egui::Color32::from_rgb(255, 202, 110)
            } else {
                egui::Color32::from_rgb(113, 206, 229)
            };
            painter.add(egui::Shape::closed_line(
                vec![
                    point + egui::vec2(0., -9.),
                    point + egui::vec2(9., 0.),
                    point + egui::vec2(0., 9.),
                    point + egui::vec2(-9., 0.),
                ],
                egui::Stroke::new(1.5, color),
            ));
            let label = format!(
                "{}{} · {}",
                waypoint.map_or(String::new(), |i| format!("{}. ", i + 1)),
                beacon.name,
                super::sensor_hud::distance(
                    pose.0
                        .position
                        .relative_to(ship.map_or(view.origin, |(_, pose)| pose.0.position))
                        .length()
                )
            );
            painter.text(
                point + egui::vec2(13., 0.),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(11.),
                color,
            );
            if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
                if ctx.input(|i| i.pointer.primary_clicked())
                    && pointer.distance(point) < 13.
                    && ctx
                        .layer_id_at(pointer)
                        .is_none_or(|l| l.order == egui::Order::Background)
                {
                    selected.target = Some(SelectedTarget::Beacon(beacon.id));
                    selected.view = Some(view.view);
                }
            }
        }
    }
    Ok(())
}
