use super::{ViewCamera, projection};
use crate::state::{DisplayPose, OwnedShip, RenderTime, ShipDetails, ViewObservation};
use bevy::prelude::*;
use osg_model::{GalacticPosition, Pose, travel};
use osg_ui::{
    bevy_egui::{EguiContexts, EguiPrimaryContextPass},
    egui,
    units::distance,
};

const ROUTE_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 202, 110);
const SLIP_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 240, 145);

fn slip_destination(
    ship: &osg_model::ShipTelemetry,
    pose: &Pose,
    details: &osg_model::presentation::ShipPresentation,
    display_ns: u64,
) -> Option<(GalacticPosition, f64, Option<f64>, f64)> {
    if !matches!(ship.presence, travel::Presence::SlipTransit(_)) {
        return None;
    }
    let transit = details.slip_transit.as_ref()?;
    let remaining = transit.destination.relative_to(pose.position).length();
    let speed = ship.pose.as_ref().map_or(0.0, |pose| {
        bevy::math::DVec3::from_array(pose.velocity).length()
    });
    let eta = ship
        .travel
        .status
        .estimated_arrival_tick
        .filter(|_| details.slip_navigation_lock != Some(false))
        .map(|tick| (tick as f64 * osg_model::TICK_SECONDS - display_ns as f64 * 1e-9).max(0.0))
        .or_else(|| (speed > 0.0).then(|| remaining / speed));
    Some((transit.destination, remaining, eta, transit.failure_ppm))
}

pub fn install(app: &mut App) {
    app.add_systems(
        EguiPrimaryContextPass,
        draw.after(crate::ui::shell::ShellDraw)
            .in_set(crate::state::ClientSystems::Gameplay),
    );
}

struct Waypoint {
    position: GalacticPosition,
    name: String,
}

fn draw(
    mut contexts: EguiContexts,
    clock: Res<RenderTime>,
    details: Query<&ShipDetails>,
    cameras: Query<(
        &Camera,
        &GlobalTransform,
        &Projection,
        &ViewCamera,
        &ViewObservation,
    )>,
    ships: Query<(&OwnedShip, &DisplayPose)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    for (camera, transform, lens, view, observation) in &cameras {
        if view.private {
            continue;
        }
        let (Some(viewport), Projection::Perspective(lens)) =
            (camera.physical_viewport_rect(), lens)
        else {
            continue;
        };
        let ppp = ctx.pixels_per_point();
        let rect = egui::Rect::from_min_max(
            egui::pos2(viewport.min.x as f32 / ppp, viewport.min.y as f32 / ppp),
            egui::pos2(viewport.max.x as f32 / ppp, viewport.max.y as f32 / ppp),
        );
        let sy = 1. / (lens.fov as f64 * 0.5).tan();
        let projection = projection::View {
            rect,
            eye: transform.translation().as_dvec3(),
            rotation: transform.rotation().as_dquat().inverse(),
            sx: sy / rect.aspect_ratio() as f64,
            sy,
        };
        let ship = ships
            .iter()
            .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship);
        let origin = ship.map_or(view.origin, |(_, pose)| pose.0.position);
        let transit = ship.and_then(|(ship, pose)| {
            let details = details
                .iter()
                .find(|details| details.0.ship == ship.0.ship)?;
            slip_destination(&ship.0, &pose.0, &details.0, clock.display_ns)
        });
        let queue: Vec<_> = ship
            .into_iter()
            .filter(|(ship, _)| ship.0.travel.enabled)
            .flat_map(|(ship, _)| ship.0.travel.status.markers.iter().enumerate())
            .map(|(index, marker)| {
                (
                    index,
                    Some(Waypoint {
                        position: marker.position,
                        name: marker.label.clone(),
                    }),
                    ROUTE_COLOR,
                )
            })
            .collect();
        let painter =
            osg_ui::desktop::hud_painter(ctx, egui::Id::new(("navigation_hud", view.view)))
                .with_clip_rect(rect);
        let label_bounds = rect.intersect(osg_ui::desktop::workspace_in(ctx));

        let mut previous = Some(origin);
        for (_, waypoint, color) in &queue {
            if let Some(waypoint) = waypoint {
                if let Some(line) = previous.and_then(|previous| {
                    projection.segment(
                        previous.relative_to(view.origin),
                        waypoint.position.relative_to(view.origin),
                    )
                }) {
                    painter.extend(egui::Shape::dotted_line(
                        &line,
                        color.gamma_multiply(0.65),
                        8.,
                        1.2,
                    ));
                }
                previous = Some(waypoint.position);
            }
        }

        let mut labels: Vec<egui::Pos2> = Vec::new();
        for (index, waypoint, color) in queue
            .into_iter()
            .filter_map(|(i, w, color)| w.map(|w| (i, w, color)))
        {
            if transit.is_some() && index == 0 {
                continue;
            }
            let (point, offscreen) = projection.marker(waypoint.position.relative_to(view.origin));
            marker(&painter, point, color);
            let mut text_point = point;
            while labels
                .iter()
                .any(|p| (p.x - text_point.x).abs() < 220. && (p.y - text_point.y).abs() < 15.)
            {
                text_point.y += if point.y > rect.center().y { -16. } else { 16. };
            }
            labels.push(text_point);
            if text_point != point {
                painter.line_segment(
                    [point, text_point],
                    egui::Stroke::new(0.5, color.gamma_multiply(0.5)),
                );
            }
            label(
                &painter,
                label_bounds,
                text_point,
                &format!(
                    "{}. {}{} · {}",
                    index + 1,
                    if offscreen { "Offscreen · " } else { "" },
                    waypoint.name,
                    distance(waypoint.position.relative_to(origin).length())
                ),
                color,
            );
        }
        if let Some((destination, remaining, eta, failure_ppm)) = transit {
            let (point, offscreen) = projection.marker(destination.relative_to(view.origin));
            painter.circle_stroke(point, 10.0, egui::Stroke::new(1.5, SLIP_COLOR));
            let eta = eta.map_or_else(|| "—".to_owned(), |seconds| format!("{seconds:.1} s"));
            label(
                &painter,
                label_bounds,
                point,
                &format!(
                    "{}Slip destination · {} · ETA {}",
                    if offscreen { "Offscreen · " } else { "" },
                    distance(remaining),
                    eta
                ),
                SLIP_COLOR,
            );
            label(
                &painter,
                label_bounds,
                point + egui::vec2(0.0, 17.0),
                &format!(
                    "Failure chance: {}",
                    crate::ui::travel_risk::odds(failure_ppm)
                ),
                crate::ui::travel_risk::color(Some(failure_ppm)),
            );
            let locked = details
                .iter()
                .find(|details| Some(details.0.ship) == observation.0.focused_ship)
                .is_some_and(|details| details.0.slip_navigation_lock == Some(true));
            label(
                &painter,
                label_bounds,
                point + egui::vec2(0.0, 34.0),
                if locked {
                    "[BEACON LOCKED]"
                } else {
                    "[NO BEACON]"
                },
                if locked {
                    SLIP_COLOR
                } else {
                    egui::Color32::from_rgb(255, 75, 75)
                },
            );
        }
    }
    Ok(())
}

fn marker(painter: &egui::Painter, point: egui::Pos2, color: egui::Color32) {
    painter.add(egui::Shape::closed_line(
        vec![
            point + egui::vec2(0., -7.),
            point + egui::vec2(7., 0.),
            point + egui::vec2(0., 7.),
            point + egui::vec2(-7., 0.),
        ],
        egui::Stroke::new(1.5, color),
    ));
}

fn label(
    painter: &egui::Painter,
    bounds: egui::Rect,
    point: egui::Pos2,
    text: &str,
    color: egui::Color32,
) {
    let galley = painter.layout(
        text.to_owned(),
        egui::FontId::proportional(11.),
        color,
        bounds.width(),
    );
    let size = galley.size();
    let desired_x = if point.x < bounds.center().x {
        point.x + 12.
    } else {
        point.x - 12. - size.x
    };
    let x = desired_x.clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left()));
    painter.galley(egui::pos2(x, point.y - size.y * 0.5), galley, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use osg_model::Id;

    #[test]
    fn active_slip_destination_survives_missing_route_and_updates_eta_after_beacon_loss() {
        let id = Id([1; 16]);
        let mut pose = Pose::default();
        pose.velocity = [1000.0, 0.0, 0.0];
        let mut ship = osg_model::ShipTelemetry {
            location: Default::default(),
            can_control: true,
            appearance: None,
            radius_m: 10.0,
            dock_services: default(),
            iff: osg_model::IffIdentity {
                owner: id,
                faction: None,
                labels: default(),
                enabled: true,
            },
            ship: id,
            authority_revision: 1,
            spatial_instance: id,
            presence: travel::Presence::SlipTransit(id),
            pose: Some(pose.clone()),
            battery_j: 0,
            hull_heat_j: 0.0,
            shield_temperature_k: 0.0,
            coolant_reserve_kg: 0.0,
            travel: default(),
        };
        let destination = pose.position.offset_by(DVec3::X * 10000.0);
        let mut details = crate::ui::console::tests::details();
        details.slip_transit = Some(osg_model::presentation::SlipTransitTelemetry {
            departed_ns: 0,
            destination,
            failure_ppm: 5000.0,
            direction: [1.0, 0.0, 0.0],
        });
        assert_eq!(
            slip_destination(&ship, &pose, &details, 0),
            Some((destination, 10000.0, Some(10.0), 5000.0))
        );

        ship.travel.status.estimated_arrival_tick = Some(100);
        pose.position = pose.position.offset_by(DVec3::X * 2000.0);
        assert_eq!(
            slip_destination(&ship, &pose, &details, 2_000_000_000),
            Some((destination, 8000.0, Some(8.0), 5000.0))
        );

        details.slip_navigation_lock = Some(false);
        ship.pose.as_mut().unwrap().velocity = [100.0, 0.0, 0.0];
        assert_eq!(
            slip_destination(&ship, &pose, &details, 2_000_000_000),
            Some((destination, 8000.0, Some(80.0), 5000.0))
        );
        ship.presence = travel::Presence::Space;
        assert!(slip_destination(&ship, &pose, &details, 2_000_000_000).is_none());
    }

    #[test]
    fn long_waypoint_labels_fit_the_viewport_without_covering_the_toolbar() {
        let context = egui::Context::default();
        osg_ui::theme::install(&context);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600., 900.));
        let bounds = osg_ui::desktop::workspace(viewport);
        let text = "3. Offscreen · Slip arrival: Near Gaia EDR3 5983356259047553792 · transfer via Gaia EDR3 551862790386030400 · 179.23 ly";
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            },
            |ui| {
                let painter = ui.painter().with_clip_rect(viewport);
                for (index, x) in [22., 790., 810., 1578.].into_iter().enumerate() {
                    let point = egui::pos2(x, 300. + index as f32 * 100.);
                    marker(&painter, point, ROUTE_COLOR);
                    label(&painter, bounds, point, text, ROUTE_COLOR);
                }
            },
        );
        output.textures_delta.clear();
        let mut labels = 0;
        for shape in output.shapes {
            if let egui::Shape::Text(shape) = shape.shape {
                if shape.galley.job.text != text {
                    continue;
                }
                let rendered = shape.galley.rect.translate(shape.pos.to_vec2());
                assert!(
                    bounds.contains_rect(rendered),
                    "{rendered:?} exceeds {bounds:?}"
                );
                labels += 1;
            }
        }
        assert_eq!(labels, 4);
    }
}
