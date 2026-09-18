use super::{
    ViewOptions, curves,
    geometry::RationalArc,
    projection::{View, occluded},
};
use bevy::math::DVec3;
use toy_sim_model::{GalacticPosition, Trajectory};
use toy_sim_ui::egui;

const COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(90, 221, 249),
    egui::Color32::from_rgb(255, 192, 87),
    egui::Color32::from_rgb(233, 129, 213),
];
const MAX_PATHS: usize = 32;
const MAX_SEGMENTS: usize = 4096;

struct Stroke<'a> {
    painter: &'a egui::Painter,
    color: egui::Color32,
    distance: f32,
    previous: Option<egui::Pos2>,
}

impl<'a> Stroke<'a> {
    fn new(painter: &'a egui::Painter, color: egui::Color32) -> Self {
        Self {
            painter,
            color,
            distance: 0.,
            previous: None,
        }
    }

    fn draw(&mut self, points: &[egui::Pos2], hidden: bool) {
        if points.len() < 2 {
            return;
        }
        if self
            .previous
            .is_none_or(|point| point.distance(points[0]) > 0.1)
        {
            self.distance = 0.;
        }
        self.previous = points.last().copied();

        if !hidden {
            self.painter.add(egui::Shape::line(
                points.to_vec(),
                egui::Stroke::new(4., self.color.gamma_multiply(0.09)),
            ));
            self.painter.add(egui::Shape::line(
                points.to_vec(),
                egui::Stroke::new(1.3, self.color),
            ));
        }
        for line in points.windows(2) {
            let delta = line[1] - line[0];
            let length = delta.length();
            if length <= 1e-6 {
                continue;
            }
            let direction = delta / length;
            if hidden {
                let mut at = 0.;
                while at < length {
                    let phase = (self.distance + at).rem_euclid(10.);
                    let step = (if phase < 4. { 4. - phase } else { 10. - phase })
                        .max(0.001)
                        .min(length - at);
                    if phase < 4. {
                        let dash = [line[0] + direction * at, line[0] + direction * (at + step)];
                        self.painter.line_segment(
                            dash,
                            egui::Stroke::new(3.3, egui::Color32::from_black_alpha(150)),
                        );
                        self.painter.line_segment(
                            dash,
                            egui::Stroke::new(1.3, self.color.linear_multiply(0.75)),
                        );
                    }
                    at += step;
                }
            } else {
                let mut at = 160. - self.distance.rem_euclid(160.);
                while at <= length {
                    let tip = line[0] + direction * at;
                    let side = egui::vec2(-direction.y, direction.x);
                    for sign in [-1., 1.] {
                        self.painter.line_segment(
                            [tip - direction * 6. + side * (3. * sign), tip],
                            egui::Stroke::new(1., self.color),
                        );
                    }
                    at += 160.;
                }
            }
            self.distance = (self.distance + length).rem_euclid(160.);
        }
    }

    fn arcs(&mut self, view: &View, arcs: &[RationalArc], bodies: &[(DVec3, f64)]) {
        let ppp = self.painter.ctx().pixels_per_point();
        for arc in curves::project(view, arcs, ppp, bodies) {
            let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                arc.points,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.3, self.color),
            );
            self.draw(&shape.flatten(Some(0.25 / ppp)), arc.hidden);
        }
    }
}

fn path_segments(
    path: &Trajectory,
    anchor: GalacticPosition,
    now: u64,
) -> impl Iterator<Item = [DVec3; 2]> + '_ {
    path.vertices
        .windows(2)
        .take(MAX_SEGMENTS)
        .filter_map(move |pair| {
            if path.timed && pair[1].sim_time_ns < now {
                return None;
            }
            let mut start = pair[0].position.relative_to(anchor);
            let end = pair[1].position.relative_to(anchor);
            if path.timed && pair[0].sim_time_ns < now && pair[1].sim_time_ns > pair[0].sim_time_ns
            {
                let fraction = (now - pair[0].sim_time_ns) as f64
                    / (pair[1].sim_time_ns - pair[0].sim_time_ns) as f64;
                start = start.lerp(end, fraction.clamp(0., 1.));
            }
            (start.is_finite() && end.is_finite()).then_some([start, end])
        })
}

pub(super) fn draw(
    painter: &egui::Painter,
    view: &View,
    options: &ViewOptions,
    bodies: &[(DVec3, f64)],
    now: u64,
    reserved: &[egui::Rect],
) {
    for (index, curve) in options.curves.iter().enumerate() {
        let end = curve
            .conic
            .impact(options.horizon)
            .unwrap_or(options.horizon);
        if index == 0 && curve.conic.mu > 0. {
            for step in 0..8 {
                if let Some((position, _)) = curve.conic.state(end * step as f64 / 8.) {
                    if let Some(line) = view.segment(curve.offset, position + curve.offset) {
                        painter.line_segment(
                            line,
                            egui::Stroke::new(0.5, COLORS[0].gamma_multiply(0.08)),
                        );
                    }
                }
            }
        }
        if let Some(arcs) = curve.arcs(0., end) {
            Stroke::new(painter, COLORS[if index == 0 { 0 } else { 2 }]).arcs(view, &arcs, bodies);
        }
    }

    let mut remaining = MAX_SEGMENTS;
    for path in options.instruments.paths.iter().take(MAX_PATHS) {
        if path.published_at_ns > now || path.valid_until_ns < now {
            continue;
        }
        let target = options
            .instruments
            .navigation
            .as_ref()
            .is_some_and(|nav| nav.target_path == Some(path.id));
        let mut stroke = Stroke::new(painter, COLORS[if target { 2 } else { 1 }]);
        for [a, b] in path_segments(path, options.curve_anchor, now).take(remaining) {
            remaining -= 1;
            stroke.arcs(
                view,
                &[RationalArc([
                    a.extend(1.),
                    ((a + b) * 0.5).extend(1.),
                    b.extend(1.),
                ])],
                bodies,
            );
        }
    }

    if let Some([a, b]) = options.encounter {
        if let Some(line) = view.segment(a, b) {
            Stroke::new(painter, COLORS[1]).draw(&line, true);
        }
    }

    markers(painter, view, options, bodies, reserved);
}

fn markers(
    painter: &egui::Painter,
    view: &View,
    options: &ViewOptions,
    bodies: &[(DVec3, f64)],
    reserved: &[egui::Rect],
) {
    let ctx = painter.ctx();
    let pointer = ctx.pointer_hover_pos();
    let mut occupied = reserved.to_vec();
    for marker in options.instruments.markers.iter().take(128) {
        let position = marker.position.relative_to(options.curve_anchor);
        let (screen, offscreen) = view.marker(position);
        let color = COLORS[match marker.kind {
            1 => 1,
            2 => 2,
            _ => 0,
        }]
        .gamma_multiply(if occluded(view.eye, position, bodies) {
            0.4
        } else {
            1.
        });
        if offscreen {
            let direction = (screen - view.rect.center()).normalized();
            let side = egui::vec2(-direction.y, direction.x);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    screen + direction * 5.,
                    screen - direction * 4. + side * 4.,
                    screen - direction * 4. - side * 4.,
                ],
                color,
                egui::Stroke::NONE,
            ));
        } else {
            painter.circle_stroke(screen, 3.5, egui::Stroke::new(1.3, color));
            painter.circle_filled(screen, 1., color);
        }
        let galley = painter.layout_no_wrap(
            marker.label.lines().next().unwrap_or("").into(),
            egui::FontId::proportional(11.),
            color,
        );
        let offsets = [9., 32., 72., 144., 240.].into_iter().flat_map(|distance| {
            [
                egui::vec2(distance, -16.),
                egui::vec2(distance, 7.),
                egui::vec2(-galley.size().x - distance, -16.),
                egui::vec2(-galley.size().x - distance, 7.),
                egui::vec2(-galley.size().x * 0.5, -distance - 16.),
                egui::vec2(-galley.size().x * 0.5, distance + 7.),
            ]
        });
        for offset in offsets {
            let position = screen + offset;
            let rect = egui::Rect::from_min_size(position, galley.size()).expand(3.);
            if view.rect.contains_rect(rect) && !occupied.iter().any(|other| other.intersects(rect))
            {
                if offset.length() > 35. {
                    painter.line_segment(
                        [screen, rect.clamp(screen)],
                        egui::Stroke::new(0.6, color.gamma_multiply(0.5)),
                    );
                }
                painter.rect_filled(rect, 2., egui::Color32::from_black_alpha(150));
                painter.galley(position, galley.clone(), color);
                occupied.push(rect);
                break;
            }
        }
        if let Some(pointer) = pointer.filter(|point| {
            point.distance(screen) < 12.
                && ctx
                    .layer_id_at(*point)
                    .is_none_or(|layer| layer.order <= egui::Order::Background)
        }) {
            egui::Area::new(painter.layer_id().id.with(("orbit_marker", marker.id)))
                .order(egui::Order::Tooltip)
                .fixed_pos(pointer + egui::vec2(16., 16.))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(&marker.label);
                    });
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::TrajectoryVertex;

    fn strokes(pieces: usize, hidden: bool) -> Vec<[egui::Pos2; 2]> {
        let context = egui::Context::default();
        context.begin_pass(egui::RawInput::default());
        {
            let painter = context.layer_painter(egui::LayerId::background());
            let mut stroke = Stroke::new(&painter, COLORS[0]);
            for index in 0..pieces {
                let start = index as f32 * 400. / pieces as f32;
                let end = (index + 1) as f32 * 400. / pieces as f32;
                stroke.draw(&[egui::pos2(start, 100.), egui::pos2(end, 100.)], hidden);
            }
        }
        let mut output = context.end_pass();
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::LineSegment { points, .. } => Some(points),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dashes_and_direction_arrows_keep_their_phase_across_curve_pieces() {
        for pieces in [1, 7, 400] {
            let dashed = strokes(pieces, true);
            for pixel in 0..400 {
                let x = pixel as f32 + 0.5;
                let painted = dashed.iter().any(|line| x >= line[0].x && x <= line[1].x);
                assert_eq!(painted, pixel % 10 < 4, "pixel {pixel}, {pieces} pieces");
            }
            let arrows = strokes(pieces, false);
            assert_eq!(arrows.len(), 4);
            for (arrow, x) in arrows.iter().zip([160., 160., 320., 320.]) {
                assert!((arrow[1].x - x).abs() < 0.01);
                assert!(arrow[0].x < arrow[1].x);
            }
        }
    }

    #[test]
    fn published_paths_keep_maneuver_corners_and_start_at_presentation_time() {
        let anchor = GalacticPosition::new(10_i128.pow(27), 0, 0);
        let mut path = Trajectory {
            id: 1,
            revision: 1,
            published_at_ns: 0,
            valid_until_ns: 200,
            timed: true,
            vertices: vec![
                TrajectoryVertex {
                    sim_time_ns: 0,
                    position: anchor,
                },
                TrajectoryVertex {
                    sim_time_ns: 100,
                    position: anchor.offset_by(DVec3::X * 10.),
                },
                TrajectoryVertex {
                    sim_time_ns: 200,
                    position: anchor.offset_by(DVec3::new(10., 20., 0.)),
                },
            ],
        };
        let segments: Vec<_> = path_segments(&path, anchor, 50).collect();
        assert_eq!(
            segments,
            vec![
                [DVec3::X * 5., DVec3::X * 10.],
                [DVec3::X * 10., DVec3::new(10., 20., 0.)],
            ]
        );
        assert!(path_segments(&path, anchor, 201).next().is_none());

        path.timed = false;
        assert_eq!(
            path_segments(&path, anchor, 201).next().unwrap()[0],
            DVec3::ZERO
        );
    }
}
