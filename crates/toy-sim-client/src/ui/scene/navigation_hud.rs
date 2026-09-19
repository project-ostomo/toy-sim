use super::{ViewCamera, projection};
use crate::{
    state::{Celestial, DisplayPose, NavigationObject, OwnedShip, ViewObservation},
    ui::{SelectedTarget, Selection},
};
use bevy::{math::DQuat, prelude::*};
use toy_sim_model::{GalacticPosition, Id, Pose, travel};
use toy_sim_ui::{
    bevy_egui::{EguiContexts, EguiPrimaryContextPass},
    egui,
    units::distance,
};

const ROUTE_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 202, 110);
const BEACON_COLOR: egui::Color32 = egui::Color32::from_rgb(113, 206, 229);

pub(super) fn install(app: &mut App) {
    app.add_systems(EguiPrimaryContextPass, draw);
}

struct Waypoint {
    position: GalacticPosition,
    name: String,
    target: Option<SelectedTarget>,
}

fn waypoint(
    order: &travel::Order,
    beacon: impl Fn(Id) -> Option<(String, Pose)>,
    celestial: impl Fn(Id) -> Option<(String, Pose)>,
) -> Option<Waypoint> {
    use travel::{Axes, Destination, Order, Reference, Target};

    let destination = match order {
        Order::Jump(id) | Order::Dock(id) => Destination::Beacon(*id),
        Order::TravelTo(destination)
        | Order::Sublight(destination)
        | Order::Slip { destination } => destination.clone(),
        Order::Guidance(travel::Guidance {
            target: Target::Destination(destination),
            ..
        }) => destination.clone(),
        _ => return None,
    };
    let (position, name, target) = match destination {
        Destination::Beacon(id) => {
            let (name, pose) = beacon(id)?;
            (pose.position, name, Some(SelectedTarget::Beacon(id)))
        }
        Destination::Galactic(position) => (position, "Coordinates".into(), None),
        Destination::Relative {
            reference,
            offset,
            axes,
        } => {
            let ((name, pose), target) = match reference {
                Reference::Beacon(id) => (beacon(id)?, SelectedTarget::Beacon(id)),
                Reference::Celestial(id) => (celestial(id)?, SelectedTarget::Celestial(id)),
            };
            let offset = offset.relative_to(GalacticPosition::ZERO);
            let offset = match axes {
                Axes::Galactic => offset,
                Axes::BodyFixed => DQuat::from_array(pose.rotation) * offset,
            };
            (
                pose.position.offset_by(offset),
                format!("Near {name}"),
                Some(target),
            )
        }
    };
    let action = match order {
        Order::Jump(_) => "Gate",
        Order::Dock(_) => "Dock",
        Order::Slip { .. } => "Slip arrival",
        _ => "Waypoint",
    };
    Some(Waypoint {
        position,
        name: format!("{action}: {name}"),
        target,
    })
}

fn draw(
    mut contexts: EguiContexts,
    mut selected: ResMut<Selection>,
    beacons: Query<(&NavigationObject, &DisplayPose)>,
    celestials: Query<(&Celestial, &DisplayPose)>,
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
        let queue: Vec<_> = ship
            .into_iter()
            .flat_map(|(ship, _)| {
                ship.0
                    .travel
                    .orders
                    .iter()
                    .enumerate()
                    .skip(ship.0.travel.order)
            })
            .map(|(index, order)| {
                (
                    index,
                    waypoint(
                        &order.action,
                        |id| {
                            beacons
                                .iter()
                                .find(|(b, _)| b.0.id == id)
                                .map(|(b, p)| (b.0.name.clone(), p.0.clone()))
                        },
                        |id| {
                            celestials
                                .iter()
                                .find(|(c, _)| c.0.entity == id)
                                .map(|(c, p)| (c.0.name.clone(), p.0.clone()))
                        },
                    ),
                )
            })
            .collect();
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new(("navigation_hud", view.view)),
            ))
            .with_clip_rect(rect);
        let label_bounds = rect.intersect(toy_sim_ui::desktop::workspace_in(ctx));

        let mut previous = Some(origin);
        for (_, waypoint) in &queue {
            if let Some(waypoint) = waypoint {
                if let Some(line) = previous.and_then(|previous| {
                    projection.segment(
                        previous.relative_to(view.origin),
                        waypoint.position.relative_to(view.origin),
                    )
                }) {
                    painter.extend(egui::Shape::dotted_line(
                        &line,
                        ROUTE_COLOR.gamma_multiply(0.65),
                        8.,
                        1.2,
                    ));
                }
                previous = Some(waypoint.position);
            }
        }

        for (beacon, pose) in &beacons {
            if queue.iter().filter_map(|(_, w)| w.as_ref()).any(|w| {
                w.position == pose.0.position
                    && w.target == Some(SelectedTarget::Beacon(beacon.0.id))
            }) {
                continue;
            }
            let offset = pose.0.position.relative_to(view.origin);
            if offset.length() > 1e12 {
                continue;
            }
            let (point, offscreen) = projection.marker(offset);
            if offscreen {
                continue;
            }
            marker(&painter, point, BEACON_COLOR);
            label(
                &painter,
                label_bounds,
                point,
                &format!(
                    "{} · {}",
                    beacon.0.name,
                    distance(pose.0.position.relative_to(origin).length())
                ),
                BEACON_COLOR,
            );
            select(
                ctx,
                &mut selected,
                point,
                Some(SelectedTarget::Beacon(beacon.0.id)),
                view.view,
            );
        }

        let mut labels: Vec<egui::Pos2> = Vec::new();
        for (index, waypoint) in queue.into_iter().filter_map(|(i, w)| w.map(|w| (i, w))) {
            let (point, offscreen) = projection.marker(waypoint.position.relative_to(view.origin));
            marker(&painter, point, ROUTE_COLOR);
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
                    egui::Stroke::new(0.5, ROUTE_COLOR.gamma_multiply(0.5)),
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
                ROUTE_COLOR,
            );
            select(ctx, &mut selected, point, waypoint.target, view.view);
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

fn select(
    ctx: &egui::Context,
    selected: &mut Selection,
    point: egui::Pos2,
    target: Option<SelectedTarget>,
    view: u64,
) {
    if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
        if target.is_some()
            && ctx.input(|i| i.pointer.primary_clicked())
            && pointer.distance(point) < 13.
            && ctx
                .layer_id_at(pointer)
                .is_none_or(|l| l.order == egui::Order::Background)
        {
            selected.target = target;
            selected.view = Some(view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn long_waypoint_labels_fit_the_viewport_without_covering_the_toolbar() {
        let context = egui::Context::default();
        toy_sim_ui::theme::install(&context);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600., 900.));
        let bounds = toy_sim_ui::desktop::workspace(viewport);
        let text = "3. Offscreen · Slip arrival: Near Gaia EDR3 5983356259047553792 gate · transfer via Gaia EDR3 551862790386030400 gate · 179.23 ly";
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

    #[test]
    fn slip_waypoints_follow_moving_and_rotating_references() {
        let id = Id([7; 16]);
        let initial = GalacticPosition::new(1_i128 << 100, 0, 0);
        let offset = GalacticPosition::ZERO.offset_by(DVec3::X * 1000.);
        for reference in [
            travel::Reference::Beacon(id),
            travel::Reference::Celestial(id),
        ] {
            let action = travel::Order::Slip {
                destination: travel::Destination::Relative {
                    reference,
                    offset,
                    axes: travel::Axes::BodyFixed,
                },
            };
            let mut pose = Pose {
                position: initial,
                ..Default::default()
            };
            let point = waypoint(
                &action,
                |_| Some(("Moving beacon".into(), pose.clone())),
                |_| Some(("Orbiting body".into(), pose.clone())),
            )
            .unwrap();
            assert_eq!(point.position, initial.offset_by(DVec3::X * 1000.));

            pose.position = initial.offset_by(DVec3::Z * 5e6);
            pose.rotation = DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array();
            let point = waypoint(
                &action,
                |_| Some(("Moving beacon".into(), pose.clone())),
                |_| Some(("Orbiting body".into(), pose.clone())),
            )
            .unwrap();
            assert!(
                point
                    .position
                    .relative_to(pose.position)
                    .distance(DVec3::Y * 1000.)
                    < 1e-5
            );
            assert!(point.name.starts_with("Slip arrival: Near "));
        }
    }

    #[test]
    fn resolves_each_spatial_order_including_rotated_relative_waypoints() {
        let id = Id([1; 16]);
        let anchor = GalacticPosition::new(1_i128 << 100, 0, 0);
        let pose = Pose {
            position: anchor,
            rotation: DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array(),
            ..Default::default()
        };
        let lookup = |key| (key == id).then(|| ("Beacon".into(), pose.clone()));
        for action in [
            travel::Order::Jump(id),
            travel::Order::Dock(id),
            travel::Order::TravelTo(travel::Destination::Beacon(id)),
        ] {
            let point = waypoint(&action, lookup, lookup).unwrap();
            assert_eq!(point.position, anchor);
            assert_eq!(point.target, Some(SelectedTarget::Beacon(id)));
        }
        let far = anchor.offset_by(DVec3::Z * 9_460_730_472_580_800.);
        for action in [
            travel::Order::Slip {
                destination: travel::Destination::Galactic(far),
            },
            travel::Order::Sublight(travel::Destination::Galactic(far)),
        ] {
            let point = waypoint(&action, lookup, lookup).unwrap();
            assert_eq!(point.position, far);
            assert!(point.target.is_none());
            assert_eq!(
                distance(point.position.relative_to(anchor).length()),
                "1.00 ly"
            );
        }
        for (axes, expected) in [
            (travel::Axes::Galactic, DVec3::X * 10.),
            (travel::Axes::BodyFixed, DVec3::Y * 10.),
        ] {
            let action = travel::Order::Sublight(travel::Destination::Relative {
                reference: travel::Reference::Celestial(id),
                offset: GalacticPosition::ZERO.offset_by(DVec3::X * 10.),
                axes,
            });
            let point = waypoint(&action, lookup, lookup).unwrap();
            assert!(point.position.relative_to(anchor).distance(expected) < 1e-5);
            assert_eq!(point.target, Some(SelectedTarget::Celestial(id)));
        }
        assert!(waypoint(&travel::Order::WaitUntil(100), lookup, lookup).is_none());
        assert!(waypoint(&travel::Order::Jump(Id([2; 16])), lookup, lookup).is_none());
    }
}
