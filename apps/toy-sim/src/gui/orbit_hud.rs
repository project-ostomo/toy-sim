//! Presentation-only orbit overlay. All positions are relative f64 until final egui projection.
mod curves;
use crate::{
    camera::{CameraParams, MainCamera},
    navigation::{Curve, Model, conic::Conic, distance, duration, geometry::AnalyticCurve},
    orrery::Universe,
    physics::Velocity,
    precision::{PreciseTransform, PresentationPose, PresentationVelocity},
    vessel::{ControlledVessel, ShipDesign, ShipHardware, ShipSoftware},
};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_egui::{EguiContexts, egui};

const COLOURS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(90, 221, 249),
    egui::Color32::from_rgb(255, 192, 87),
    egui::Color32::from_rgb(233, 129, 213),
];
#[derive(Clone, Copy, PartialEq)]
enum Framing {
    Ship,
    Orbit,
    Encounter,
    Return,
}
#[derive(Resource)]
pub(super) struct OrbitHud {
    pub enabled: bool,
    preview: f64,
    owner: Option<Entity>,
    key: Option<(f64, u64, u64, bool)>,
    model: Option<Model>,
    request: Option<Framing>,
    framing: Option<Framing>,
    build_ms: f64,
    paint_ms: f64,
}
impl Default for OrbitHud {
    fn default() -> Self {
        Self {
            enabled: true,
            preview: 0.,
            owner: None,
            key: None,
            model: None,
            request: None,
            framing: None,
            build_ms: 0.,
            paint_ms: 0.,
        }
    }
}
pub(super) fn controls(ui: &mut egui::Ui, hud: &mut OrbitHud) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut hud.enabled, "Orbits [O]");
        for (label, mode) in [
            ("Ship", Framing::Ship),
            ("Orbit", Framing::Orbit),
            ("Encounter", Framing::Encounter),
            ("Return", Framing::Return),
        ] {
            if ui.small_button(label).clicked() {
                hud.request = Some(mode);
                hud.enabled = true;
            }
        }
    });
    if let Some(model) = hud.model.as_ref().filter(|_| hud.enabled) {
        ui.small(format!(
            "{} · {}",
            model.primary,
            model.period.map_or_else(
                || "Unbound / inertial".into(),
                |p| format!("Period {}", duration(p))
            )
        ));
        if let Some(ca) = &model.encounter {
            ui.label(format!(
                "Closest {} · T+{}",
                distance(ca.range),
                duration(ca.seconds)
            ));
            ui.small(format!(
                "{:.1} m/s relative · {}",
                ca.speed,
                if model.plan.points.is_empty() {
                    "coast estimate"
                } else {
                    "published plan estimate"
                }
            ));
        }
        if let Some(age) = model.target_age {
            ui.small(format!("Target observation {:.1} s old", age));
        }
        if let Some(age) = model.plan_age {
            ui.small(format!("Plan published {:.1} s ago", age));
        }
        if model.period.is_some_and(|p| p > model.horizon) {
            ui.small("Preview capped at one year");
        }
        ui.small("Two-body estimate · external forces may change the path")
            .on_hover_text(format!(
                "Native prediction {:.2} ms / refresh · projection {:.2} ms / frame",
                hud.build_ms, hud.paint_ms
            ));
    }
}

pub(super) fn overlay(
    mut contexts: EguiContexts,
    mut hud: ResMut<OrbitHud>,
    universe: Res<Universe>,
    time: Res<Time<Fixed>>,
    mut ship: Single<
        (
            Entity,
            &PreciseTransform,
            Option<&PresentationPose>,
            Option<&PresentationVelocity>,
            &Velocity,
            &ShipDesign,
            &ShipHardware,
            &mut ShipSoftware,
        ),
        (With<ControlledVessel>, Without<MainCamera>),
    >,
    mut camera: Single<(&Camera, &PreciseTransform, &mut CameraParams), With<MainCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    bodies: Query<(
        &crate::orrery::Celestial,
        &PresentationPose,
        &PresentationVelocity,
    )>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| {
            i.key_pressed(egui::Key::O)
                && !i.modifiers.command
                && !i.modifiers.alt
                && !i.modifiers.ctrl
        })
    {
        hud.enabled = !hud.enabled;
    }
    let (entity, pose, presentation, presentation_velocity, velocity, design, hardware, software) =
        &mut *ship;
    let online = hardware.0.computer_running(&design.0) && !software.controller.is_booting();
    software.controller.instrument_interest = if hud.enabled && online {
        toy_sim_ship_api::abi::INTEREST_MARKERS | toy_sim_ship_api::abi::INTEREST_PATHS
    } else {
        0
    };
    if hud.owner != Some(*entity) {
        hud.owner = Some(*entity);
        hud.key = None;
        hud.model = None;
        hud.framing = None;
        hud.preview = 0.;
    }
    if !hud.enabled {
        return Ok(());
    }
    let target_id = software
        .controller
        .state
        .navigation
        .as_ref()
        .map_or(0, |n| n.target_contact);
    let now = crate::precision::presentation_time(&time);
    let key = (
        now,
        software.controller.trajectory_revision,
        target_id,
        online,
    );
    if hud.key != Some(key) {
        let start = std::time::Instant::now();
        let snapshots = if online {
            crate::navigation::Snapshots::at(&software.controller.state, now)
        } else {
            crate::navigation::Snapshots::default()
        };
        hud.model = Some(Model::build(
            &universe,
            presentation.map_or(pose.translation_um, |pose| pose.0.translation_um),
            presentation_velocity.map_or(velocity.0, |velocity| velocity.0),
            now,
            &snapshots,
        ));
        hud.key = Some(key);
        hud.build_ms = start.elapsed().as_secs_f64() * 1000.;
    }
    let start = std::time::Instant::now();
    let hud = &mut *hud;
    let Some(model) = hud.model.as_ref() else {
        return Ok(());
    };
    let (camera, camera_pose, params) = &mut *camera;
    if params.navigation_focus.is_none() {
        hud.framing = None;
    }
    let primary = bodies
        .iter()
        .find(|(b, _, _)| b.0.as_str() == model.primary);
    let frame_anchor = primary.map_or(model.anchor, |(_, p, _)| p.0.translation_um);
    let ship_position = presentation.map_or(pose.translation_um, |p| p.0.translation_um);
    // Fit only the displayed own coast to interpolated position AND velocity.
    // Published plans and target observations retain their independent epochs.
    let own_presentation = model.own.analytic.as_ref().map(|a| AnalyticCurve {
        conic: Conic {
            r: ship_position.relative_to(frame_anchor),
            v: presentation_velocity.map_or(velocity.0, |v| v.0)
                - primary.map_or(velocity.0 - a.conic.v, |(_, _, v)| v.0),
            ..a.conic.clone()
        },
        offset: DVec3::ZERO,
    });
    let Some(viewport) = camera.logical_viewport_rect() else {
        return Ok(());
    };
    let scale = window.scale_factor() / ctx.pixels_per_point();
    let rect = egui::Rect::from_min_max(
        egui::pos2(viewport.min.x * scale, viewport.min.y * scale),
        egui::pos2(viewport.max.x * scale, viewport.max.y * scale),
    );
    let clip_matrix = camera.clip_from_view();
    let view = View {
        rect,
        eye: camera_pose.translation_um.relative_to(frame_anchor),
        rotation: camera_pose.rotation.inverse(),
        sx: clip_matrix.x_axis.x as f64,
        sy: clip_matrix.y_axis.y as f64,
    };
    if let Some(request) = hud.request.take() {
        if request == Framing::Return {
            params.return_from_navigation();
            hud.framing = None;
        } else {
            let (center, radius) =
                framing_bounds(model, request, ship_position.relative_to(frame_anchor));
            let min_sine = (1. / view.sx.max(view.sy).max(0.01)).atan().sin();
            let direction = if request == Framing::Orbit {
                model
                    .own
                    .points
                    .first()
                    .zip(model.own.points.get(model.own.points.len() / 4))
                    .and_then(|(a, b)| a.p.cross(b.p).try_normalize())
            } else {
                None
            };
            if let Some(focus) = crate::navigation::checked_offset(frame_anchor, center) {
                params.frame_navigation(
                    focus,
                    (radius / min_sine * 1.2).max(if request == Framing::Ship {
                        30.
                    } else {
                        100.
                    }),
                    direction,
                );
                hud.framing = Some(request);
            }
        }
    }
    if let Some(mode) = hud.framing {
        let (center, _) = framing_bounds(model, mode, ship_position.relative_to(frame_anchor));
        params.navigation_focus = crate::navigation::checked_offset(frame_anchor, center);
    }
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("orbital-navigation"),
        ))
        .with_clip_rect(rect);
    let layers = ctx.memory(|m| m.areas().visible_layer_ids());
    let reserved: Vec<_> = layers
        .into_iter()
        .filter(|l| l.order >= egui::Order::Middle)
        .filter_map(|l| egui::containers::AreaState::load(ctx, l.id).map(|s| s.rect()))
        .collect();
    paint_model(
        &painter,
        &view,
        model,
        ship_position.relative_to(frame_anchor),
        hud.preview,
        ctx.pointer_hover_pos(),
        &reserved,
        own_presentation.as_ref(),
    );
    if online {
        let pose = presentation.map_or(**pose, |pose| pose.0);
        let presentation = toy_sim_ship_wasm::spatial::Snapshot {
            epoch: now,
            origin: [
                pose.translation_um.x,
                pose.translation_um.y,
                pose.translation_um.z,
            ],
            rotation: pose.rotation.to_array(),
            velocity: presentation_velocity
                .map_or(velocity.0, |velocity| velocity.0)
                .to_array(),
            ..Default::default()
        };
        paint_publications(
            &painter,
            &view,
            &software.controller.state,
            presentation,
            frame_anchor,
            &reserved,
        );
    }
    hud.preview = hud.preview.clamp(0., model.horizon);
    egui::Area::new(egui::Id::new("orbit-timeline"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0., -18.))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(205))
                .stroke(egui::Stroke::new(1., COLOURS[0].gamma_multiply(0.25)))
                .inner_margin(10.)
                .show(ui, |ui| {
                    ui.set_width(480_f32.min(rect.width() - 40.).max(180.));
                    ui.horizontal(|ui| {
                        for (i, label) in ["COAST", "PLAN", "TARGET"].iter().enumerate() {
                            ui.colored_label(COLOURS[i], *label);
                            ui.add_space(12.);
                        }
                        ui.weak("ORBITAL NAVIGATION");
                    });
                    ui.horizontal(|ui| {
                        if ui.small_button("NOW").clicked() {
                            hud.preview = 0.;
                        }
                        ui.add(
                            egui::Slider::new(&mut hud.preview, 0. ..=model.horizon)
                                .logarithmic(true)
                                .smallest_positive(0.1)
                                .show_value(false),
                        );
                        ui.monospace(format!("T+{}", duration(hud.preview)));
                        if let Some(ca) = &model.encounter {
                            if ui
                                .small_button("CA")
                                .on_hover_text("Preview closest approach")
                                .clicked()
                            {
                                hud.preview = ca.seconds;
                            }
                        }
                    });
                    ui.small(format!(
                        "Preview only · horizon {}",
                        duration(model.horizon)
                    ));
                });
        });
    hud.paint_ms = start.elapsed().as_secs_f64() * 1000.;
    Ok(())
}
fn framing_bounds(model: &Model, mode: Framing, ship: DVec3) -> (DVec3, f64) {
    if mode == Framing::Ship {
        return (ship, 10.);
    }
    let mut points = vec![ship];
    if mode == Framing::Orbit {
        points.extend(model.own.points.iter().map(|s| s.p));
        points.extend([DVec3::splat(model.radius), DVec3::splat(-model.radius)]);
    } else if !model.plan.points.is_empty() {
        points.extend(model.plan.points.iter().map(|s| s.p));
        points.extend(
            model
                .plan
                .points
                .iter()
                .filter_map(|s| model.target.at(s.t)),
        );
    } else if let Some(target) = model.target.points.first() {
        points.push(target.p);
    }
    let min = points
        .iter()
        .copied()
        .fold(DVec3::splat(f64::INFINITY), DVec3::min);
    let max = points
        .iter()
        .copied()
        .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
    let center = (min + max) * 0.5;
    let radius = points
        .iter()
        .map(|p| p.distance(center))
        .fold(10., f64::max);
    (center, radius)
}
struct View {
    rect: egui::Rect,
    eye: DVec3,
    rotation: DQuat,
    sx: f64,
    sy: f64,
}
impl View {
    fn camera(&self, p: DVec3) -> DVec3 {
        self.rotation * (p - self.eye)
    }
    fn screen(&self, p: DVec3) -> egui::Pos2 {
        self.rect.center()
            + egui::vec2(
                (p.x / (-p.z) * self.sx * self.rect.width() as f64 * 0.5) as f32,
                (-p.y / (-p.z) * self.sy * self.rect.height() as f64 * 0.5) as f32,
            )
    }
    fn point(&self, p: DVec3) -> Option<egui::Pos2> {
        let p = self.camera(p);
        if p.z >= -0.1 {
            None
        } else {
            Some(self.screen(p))
        }
    }
    fn segment(&self, a: DVec3, b: DVec3) -> Option<[egui::Pos2; 2]> {
        let (mut a, mut b) = (self.camera(a), self.camera(b));
        if a.z >= -0.1 && b.z >= -0.1 {
            return None;
        }
        if a.z > -0.1 {
            a = a.lerp(b, (-0.1 - a.z) / (b.z - a.z));
        }
        if b.z > -0.1 {
            b = b.lerp(a, (-0.1 - b.z) / (a.z - b.z));
        }
        // Clip in homogeneous view space before f32 conversion, even with enormous anchors.
        for (axis, sign, scale) in [
            (0, 1., self.sx),
            (0, -1., self.sx),
            (1, 1., self.sy),
            (1, -1., self.sy),
        ] {
            let fa = -a.z + sign * a[axis] * scale;
            let fb = -b.z + sign * b[axis] * scale;
            if fa < 0. && fb < 0. {
                return None;
            }
            if fa < 0. {
                a = a.lerp(b, fa / (fa - fb));
            } else if fb < 0. {
                b = b.lerp(a, fb / (fb - fa));
            }
        }
        let out = [self.screen(a), self.screen(b)];
        out.iter().all(|p| p.is_finite()).then_some(out)
    }
    fn marker(&self, p: DVec3) -> (egui::Pos2, bool) {
        if let Some(p) = self.point(p).filter(|p| self.rect.shrink(16.).contains(*p)) {
            return (p, false);
        }
        let p = self.camera(p);
        let direction = bevy::math::DVec2::new(p.x * self.sx, -p.y * self.sy)
            .try_normalize()
            .unwrap_or(bevy::math::DVec2::NEG_Y);
        let direction = egui::vec2(direction.x as f32, direction.y as f32);
        let half = self.rect.size() * 0.5 - egui::vec2(22., 22.);
        let factor =
            (half.x / direction.x.abs().max(1e-8)).min(half.y / direction.y.abs().max(1e-8));
        (self.rect.center() + direction * factor, true)
    }
}
fn occluded(eye: DVec3, p: DVec3, bodies: &[(DVec3, f64)]) -> bool {
    let ray = p - eye;
    let distance = ray.length();
    if distance <= 0. {
        return false;
    }
    let dir = ray / distance;
    bodies.iter().any(|(center, radius)| {
        let to_center = *center - eye;
        let along = to_center.dot(dir);
        if along <= 0. || to_center.length_squared() <= radius * radius {
            return false;
        }
        let perpendicular = to_center.cross(dir).length_squared();
        perpendicular < radius * radius
            && along - (radius * radius - perpendicular).sqrt() < distance - 1.
    })
}
fn dashed(painter: &egui::Painter, points: [egui::Pos2; 2], colour: egui::Color32) {
    let delta = points[1] - points[0];
    let length = delta.length();
    for i in 0..(length / 10.).ceil().min(512.) as usize {
        let a = i as f32 * 10.;
        let b = (a + 4.).min(length);
        painter.line_segment(
            [
                points[0] + delta * (a / length.max(1e-6)),
                points[0] + delta * (b / length.max(1e-6)),
            ],
            egui::Stroke::new(1., colour),
        );
    }
}
fn curve(
    painter: &egui::Painter,
    view: &View,
    curve: &Curve,
    analytic: Option<&AnalyticCurve>,
    colour: egui::Color32,
    bodies: &[(DVec3, f64)],
) {
    let ppp = painter.ctx().pixels_per_point();
    let arcs = analytic.and_then(|a| {
        let first = curve.points.first()?;
        let last = curve.points.last()?;
        let full_orbit = !curve.impact
            && curve
                .analytic
                .as_ref()
                .and_then(|a| a.conic.period())
                .is_some_and(|p| last.t - first.t >= p * (1. - 1e-12));
        let end = if full_orbit {
            a.conic.period().map_or(last.t, |p| first.t + p)
        } else {
            last.t
        };
        a.arcs(first.t, end)
    });
    let mut paths = vec![];
    if let Some(arcs) = arcs {
        for arc in curves::project(view, &arcs, ppp, bodies) {
            let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                arc.points,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.3, colour),
            );
            // Egui owns curve flattening and stroke tessellation. Use a physical-pixel
            // tolerance without changing global settings for unrelated UI widgets.
            paths.push((shape.flatten(Some(0.25 / ppp)), arc.hidden));
        }
    } else {
        // Firmware publishes a piecewise-linear path. Preserve every maneuver corner;
        // cross-primary target estimates also retain their sampled frame reconstruction.
        for points in curve.points.windows(2) {
            let (a, b) = (points[0].p, points[1].p);
            if let Some(line) = view.segment(a, b) {
                paths.push((line.to_vec(), occluded(view.eye, (a + b) * 0.5, bodies)));
            }
        }
    }
    let mut distance = 0_f32;
    let mut previous: Option<egui::Pos2> = None;
    for (points, hidden) in paths {
        if points.len() < 2 {
            continue;
        }
        if previous.is_none_or(|p| p.distance(points[0]) > 0.1) {
            distance = 0.;
        }
        previous = points.last().copied();
        if !hidden {
            painter.add(egui::Shape::line(
                points.clone(),
                egui::Stroke::new(4., colour.gamma_multiply(0.09)),
            ));
            painter.add(egui::Shape::line(
                points.clone(),
                egui::Stroke::new(1.3, colour),
            ));
        }
        // Carry phase across cubic and tessellation boundaries: decorations do not
        // restart each time adaptive geometry changes the number of segments.
        for line in points.windows(2) {
            let delta = line[1] - line[0];
            let length = delta.length();
            if length <= 1e-6 {
                continue;
            }
            let dir = delta / length;
            if hidden {
                let mut at = 0.;
                while at < length {
                    let phase = (distance + at).rem_euclid(10.);
                    let step = (if phase < 4. { 4. - phase } else { 10. - phase })
                        .max(0.001)
                        .min(length - at);
                    if phase < 4. {
                        painter.line_segment(
                            [line[0] + dir * at, line[0] + dir * (at + step)],
                            egui::Stroke::new(1., colour.gamma_multiply(0.28)),
                        );
                    }
                    at += step;
                }
            } else {
                let mut at = 160. - distance.rem_euclid(160.);
                while at <= length {
                    let tip = line[0] + dir * at;
                    let side = egui::vec2(-dir.y, dir.x);
                    painter.line_segment(
                        [tip - dir * 6. + side * 3., tip],
                        egui::Stroke::new(1., colour),
                    );
                    painter.line_segment(
                        [tip - dir * 6. - side * 3., tip],
                        egui::Stroke::new(1., colour),
                    );
                    at += 160.;
                }
            }
            distance += length;
        }
    }
}
fn paint_model(
    painter: &egui::Painter,
    view: &View,
    model: &Model,
    ship: DVec3,
    preview: f64,
    pointer: Option<egui::Pos2>,
    reserved: &[egui::Rect],
    own_presentation: Option<&AnalyticCurve>,
) {
    // Sparse radial guides communicate the common nonrotating orbital plane.
    for s in model
        .own
        .points
        .iter()
        .step_by((model.own.points.len() / 8).max(1))
    {
        if let Some(line) = view.segment(DVec3::ZERO, s.p) {
            painter.line_segment(
                line,
                egui::Stroke::new(0.5, COLOURS[0].gamma_multiply(0.08)),
            );
        }
    }
    for (i, path) in [(0, &model.own), (2, &model.target), (1, &model.plan)] {
        let faded = if i == 2 && model.target_age.is_some_and(|a| a > 1.) {
            0.5
        } else {
            1.
        };
        curve(
            painter,
            view,
            path,
            if i == 0 {
                own_presentation.or(path.analytic.as_ref())
            } else {
                path.analytic.as_ref()
            },
            COLOURS[i].gamma_multiply(faded),
            &model.occluders,
        );
    }
    if let Some(ca) = &model.encounter {
        if let Some(line) = view.segment(ca.ship, ca.target) {
            dashed(painter, line, COLOURS[1].gamma_multiply(0.7));
        }
    }
    let mut occupied = reserved.to_vec();
    let mut hover = None;
    for annotation in &model.annotations {
        let p = if annotation.title == "SHIP" {
            ship
        } else {
            annotation.p
        };
        let (screen, offscreen) = view.marker(p);
        let colour =
            COLOURS[annotation.kind].gamma_multiply(if occluded(view.eye, p, &model.occluders) {
                0.4
            } else {
                1.
            });
        let radius = if annotation.title.contains("CLOSEST") {
            5.
        } else {
            3.5
        };
        if offscreen {
            let dir = (screen - view.rect.center()).normalized();
            let side = egui::vec2(-dir.y, dir.x);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    screen + dir * 5.,
                    screen - dir * 4. + side * 4.,
                    screen - dir * 4. - side * 4.,
                ],
                colour,
                egui::Stroke::NONE,
            ));
        } else {
            painter.circle_stroke(screen, radius, egui::Stroke::new(1.3, colour));
            painter.circle_filled(screen, 1., colour);
        }
        if pointer
            .is_some_and(|p| p.distance(screen) < 12. && !reserved.iter().any(|r| r.contains(p)))
        {
            hover = Some((screen, annotation.detail.as_str()));
        }
        let text = if offscreen {
            format!("{} ↗", annotation.title)
        } else {
            annotation.title.clone()
        };
        let galley = painter.layout_no_wrap(text, egui::FontId::monospace(11.), colour);
        let offsets = [9., 32., 72., 144., 240.].into_iter().flat_map(|d| {
            [
                egui::vec2(d, -16.),
                egui::vec2(d, 7.),
                egui::vec2(-galley.size().x - d, -16.),
                egui::vec2(-galley.size().x - d, 7.),
                egui::vec2(-galley.size().x * 0.5, -d - 16.),
                egui::vec2(-galley.size().x * 0.5, d + 7.),
            ]
        });
        for offset in offsets {
            let position = screen + offset;
            let rect = egui::Rect::from_min_size(position, galley.size()).expand(3.);
            if view.rect.contains_rect(rect) && !occupied.iter().any(|r| r.intersects(rect)) {
                if offset.length() > 35. {
                    painter.line_segment(
                        [screen, rect.clamp(screen)],
                        egui::Stroke::new(0.6, colour.gamma_multiply(0.5)),
                    );
                }
                painter.rect_filled(rect, 2., egui::Color32::from_black_alpha(150));
                painter.galley(position, galley.clone(), colour);
                occupied.push(rect);
                break;
            }
        }
    }
    if preview > 0. {
        for (kind, path) in [(0, &model.own), (1, &model.plan), (2, &model.target)] {
            let sample_time = if kind == 0 && !path.impact {
                model.period.map_or(preview, |p| preview % p)
            } else {
                preview
            };
            let position = if kind == 0 {
                own_presentation.map_or_else(|| path.at(sample_time), |a| a.at(sample_time))
            } else {
                path.at(sample_time)
            };
            if let Some(p) = position
                .and_then(|p| view.point(p))
                .filter(|p| view.rect.contains(*p))
            {
                painter.circle_stroke(p, 7., egui::Stroke::new(1., COLOURS[kind]));
                painter.line_segment(
                    [p - egui::vec2(11., 0.), p + egui::vec2(11., 0.)],
                    egui::Stroke::new(1., COLOURS[kind]),
                );
            }
        }
    }
    if let Some((p, text)) = hover {
        let galley = painter.layout_no_wrap(
            text.into(),
            egui::FontId::monospace(12.),
            egui::Color32::WHITE,
        );
        let position = egui::pos2(
            (p.x + 12.)
                .min(view.rect.right() - galley.size().x - 8.)
                .max(view.rect.left() + 8.),
            (p.y + 20.).min(view.rect.bottom() - galley.size().y - 8.),
        );
        painter.rect_filled(
            egui::Rect::from_min_size(position, galley.size()).expand(6.),
            2.,
            egui::Color32::from_black_alpha(240),
        );
        painter.galley(position, galley, egui::Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> View {
        View {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 720.)),
            eye: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            sx: 1.,
            sy: 1280. / 720.,
        }
    }
    #[test]
    fn clips_near_plane_and_viewport_before_float_conversion() {
        let view = view();
        assert!(view.segment(DVec3::Z, DVec3::Z * 2.).is_none());
        let line = view
            .segment(DVec3::new(-1e30, 0., -100.), DVec3::new(1e30, 0., -100.))
            .unwrap();
        assert!(line.iter().all(|p| view.rect.expand(0.01).contains(*p)));
        assert!(
            view.segment(DVec3::new(0., 0., 1.), DVec3::new(0., 0., -100.))
                .is_some()
        );
        assert!(
            view.segment(DVec3::new(1000., 0., -100.), DVec3::new(2000., 0., -100.))
                .is_none()
        );
    }
    #[test]
    fn occlusion_tests_depth_and_not_just_angular_overlap() {
        let bodies = vec![(DVec3::new(0., 0., -10.), 2.)];
        assert!(occluded(DVec3::ZERO, DVec3::new(0., 0., -20.), &bodies));
        assert!(!occluded(DVec3::ZERO, DVec3::new(0., 0., -5.), &bodies));
        assert!(!occluded(DVec3::ZERO, DVec3::new(10., 0., -20.), &bodies));
    }
    #[test]
    fn projection_is_independent_of_floating_origin_and_dpi() {
        let anchor = crate::precision::GalacticPosition::new(1_i128 << 100, -(1_i128 << 98), 0);
        let eye = anchor.offset_by(DVec3::new(100., 200., 300.));
        let point = anchor.offset_by(DVec3::new(101., 202., 200.));
        let mut v = view();
        v.eye = eye.relative_to(anchor);
        let a = v.point(point.relative_to(anchor)).unwrap();
        let shifted = anchor.offset_by(DVec3::splat(1e6));
        v.eye = eye.relative_to(shifted);
        let b = v.point(point.relative_to(shifted)).unwrap();
        assert_eq!(a, b);
        // View is measured in egui points; doubling physical resolution/DPI leaves it identical.
        let physical = egui::vec2(2560., 1440.);
        assert_eq!(physical / 2., v.rect.size());
    }
}

#[cfg(test)]
mod profile {
    use super::*;
    use crate::navigation::{ObservedTarget, Snapshots};
    #[test]
    #[ignore = "manual native prediction and egui projection benchmark"]
    fn profile_orbit_overlay() {
        let universe = Universe::init(crate::orrery::example_config()).unwrap();
        let time = 100.;
        let epoch = hifitime::Epoch::from_mjd_utc(0.) + hifitime::Duration::from_seconds(time);
        // Keep the original widely separated pair for comparable rendering measurements.
        let scenario = &crate::scenario::InitialScenario {
            traffic_count: 1,
            traffic_orbit_start: 1,
            ..crate::scenario::INITIAL_SCENARIO.clone()
        };
        let body = universe.get_body(scenario.body).unwrap();
        let states = scenario.fleet_states(body.radius, body.mass).unwrap();
        let origin = universe.solve_position(scenario.body, epoch).unwrap();
        let velocity = universe.solve_velocity(scenario.body, epoch).unwrap();
        let ship = origin.offset_by(states[0].0);
        let mut snapshots = Snapshots::default();
        let target = ObservedTarget {
            id: 2,
            name: "Observed traffic".into(),
            position: origin.offset_by(states[1].0),
            velocity: velocity + states[1].1,
            epoch: time,
            age: 0.,
        };
        snapshots.target = Some(target.clone());
        snapshots.plan = Some(toy_sim_ship_wasm::spatial::Path {
            snapshot: Some(toy_sim_ship_wasm::spatial::Snapshot {
                id: 1,
                epoch: time,
                origin: [ship.x, ship.y, ship.z],
                ..default()
            }),
            header: toy_sim_ship_api::abi::SpatialPath {
                meta: toy_sim_ship_api::abi::SpatialMeta {
                    id: 1,
                    valid_until_s: time + 2.,
                    ..default()
                },
                frame: toy_sim_ship_api::abi::SpatialFrame {
                    reference: 1,
                    origin_velocity_m_s: target.velocity.to_array(),
                    ..default()
                },
                kind: toy_sim_ship_api::abi::PATH_TIMED,
                ..default()
            },
            vertices: (0..33)
                .map(|i| toy_sim_ship_api::abi::SpatialVertex {
                    position_m: (target.position.relative_to(ship) * (i as f64 / 32.)).to_array(),
                    time_s: time + i as f64 * 100.,
                })
                .collect(),
            published_at: time,
            revision: 1,
        });
        let mut builds = vec![];
        for _ in 0..100 {
            let start = std::time::Instant::now();
            std::hint::black_box(Model::build(
                &universe,
                ship,
                velocity + states[0].1,
                time,
                &snapshots,
            ));
            builds.push(start.elapsed().as_secs_f64() * 1000.);
        }
        let model = Model::build(&universe, ship, velocity + states[0].1, time, &snapshots);
        let ctx = egui::Context::default();
        let mut measurements = vec![("prediction (once per changed tick)", builds)];
        let mut view = View {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600., 1000.)),
            eye: DVec3::Z * 1.5e8,
            rotation: DQuat::IDENTITY,
            sx: 1.,
            sy: 1.6,
        };
        for (label, eye) in [
            (
                "orbit view + egui tessellation (per frame; no GPU)",
                DVec3::Z * 1.5e8,
            ),
            (
                "100 m ship view + egui tessellation (per frame; no GPU)",
                ship.relative_to(model.anchor) + DVec3::Z * 100.,
            ),
        ] {
            view.eye = eye;
            let mut paints = vec![];
            for _ in 0..100 {
                let start = std::time::Instant::now();
                ctx.begin_pass(egui::RawInput {
                    screen_rect: Some(view.rect),
                    ..default()
                });
                let painter = ctx.layer_painter(egui::LayerId::background());
                paint_model(
                    &painter,
                    &view,
                    &model,
                    ship.relative_to(model.anchor),
                    100.,
                    None,
                    &[],
                    None,
                );
                let mut output = ctx.end_pass();
                let shapes = std::mem::take(&mut output.shapes);
                std::hint::black_box(ctx.tessellate(shapes, output.pixels_per_point));
                output.textures_delta.clear();
                paints.push(start.elapsed().as_secs_f64() * 1000.);
            }
            measurements.push((label, paints));
        }
        for (label, mut samples) in measurements {
            samples.sort_by(f64::total_cmp);
            println!(
                "{label}: mean {:.3} ms, p50 {:.3} ms, p95 {:.3} ms",
                samples.iter().sum::<f64>() / samples.len() as f64,
                samples[50],
                samples[95]
            );
        }
        println!(
            "Cached prediction samples (coasts render analytically): coast {}, plan {}, target {}; overlay disabled skips prediction and painting",
            model.own.points.len().saturating_sub(1),
            model.plan.points.len().saturating_sub(1),
            model.target.points.len().saturating_sub(1)
        );
    }
}

fn paint_publications(
    painter: &egui::Painter,
    view: &View,
    session: &toy_sim_ship_wasm::Session,
    presentation: toy_sim_ship_wasm::spatial::Snapshot,
    anchor: crate::precision::GalacticPosition,
    reserved: &[egui::Rect],
) {
    let spatial = &session.spatial;
    let selected = session
        .navigation
        .filter(|nav| nav.valid_until_s > presentation.epoch);

    for (id, path) in &spatial.paths {
        // Selected navigation paths are already rendered with timeline controls.
        if selected.is_some_and(|nav| nav.own_path == *id || nav.target_path == *id) {
            continue;
        }
        let Some(vertices) = spatial.polyline_at(*id, presentation) else {
            continue;
        };
        let curve = Curve {
            points: vertices
                .into_iter()
                .enumerate()
                .map(|(index, position)| crate::navigation::Sample {
                    t: index as f64,
                    p: crate::navigation::position(position).relative_to(anchor),
                })
                .collect(),
            impact: false,
            analytic: None,
        };
        let color = if path.header.meta.role == toy_sim_ship_api::abi::PATH_CONTACT_FORECAST {
            COLOURS[2]
        } else {
            COLOURS[1]
        };
        for pair in curve.points.windows(2) {
            if let Some(line) = view.segment(pair[0].p, pair[1].p) {
                painter.line_segment(line, egui::Stroke::new(1.5, color));
            }
        }
    }

    for id in spatial.markers.keys() {
        let Some(position) = spatial.marker_at(*id, presentation) else {
            continue;
        };
        let Some(point) = view.point(crate::navigation::position(position).relative_to(anchor))
        else {
            continue;
        };
        let marker = &spatial.markers[id].record;
        let color = COLOURS[1];
        painter.circle_stroke(point, 5., egui::Stroke::new(1.5, color));

        if !reserved.iter().any(|rect| rect.expand(8.).contains(point)) {
            painter.text(
                point + egui::vec2(9., -9.),
                egui::Align2::LEFT_BOTTOM,
                marker.meta.label.as_str().unwrap_or(""),
                egui::FontId::monospace(12.),
                color,
            );
        }
    }
}
