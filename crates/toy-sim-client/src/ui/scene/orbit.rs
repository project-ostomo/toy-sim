mod conic;
mod curves;
mod geometry;
mod projection;

use super::{
    ViewCamera,
    camera::{CameraOptions, Framing},
};
use crate::state::{
    Celestial, CelestialSystem, Contact, DisplayPose, OwnedShip, PresentationSet, RenderTime,
    SessionInfo, ShipDetails, SystemSubscription, ViewObservation,
};
use crate::ui::{SelectedTarget, Selection, celestials::SystemDefinition};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use toy_sim_model::*;

#[derive(Component)]
pub(super) struct ViewOptions {
    pub enabled: bool,
    pub instruments: Instruments,
    preview: f64,
    horizon: f64,
    period: Option<f64>,
    encounter: Option<(f64, f64, GalacticPosition)>,
    own: Vec<TrajectoryVertex>,
    curves: Vec<geometry::AnalyticCurve>,
    curve_anchor: GalacticPosition,
    ship: Option<Id>,
    orbit_origin: GalacticPosition,
    orbit_distance: f32,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            instruments: Instruments::default(),
            preview: 0.,
            horizon: 0.,
            period: None,
            encounter: None,
            own: Vec::new(),
            curves: Vec::new(),
            curve_anchor: GalacticPosition::ZERO,
            ship: None,
            orbit_origin: GalacticPosition::ZERO,
            orbit_distance: 100.,
        }
    }
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        Update,
        refresh_views
            .in_set(PresentationSet::Views)
            .after(super::camera::setup_views)
            .before(super::camera::update_views),
    )
    .add_systems(EguiPrimaryContextPass, (controls, draw_coasts).chain());
}

fn refresh_views(
    clock: Res<RenderTime>,
    mut views: Query<(
        &ViewObservation,
        &SystemSubscription,
        &mut ViewOptions,
        &mut CameraOptions,
    )>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>, Option<&DisplayPose>)>,
    contacts: Query<(&Contact, &DisplayPose)>,
    celestials: Query<(&Celestial, &CelestialSystem, &DisplayPose)>,
    definitions: Query<&SystemDefinition>,
    definition_assets: Res<Assets<crate::assets::SystemDefinition>>,
) {
    for (observation, systems, mut options, mut framing) in &mut views {
        let view = &observation.0;
        if options.ship != view.focused_ship {
            *framing = CameraOptions::default();
            *options = ViewOptions {
                ship: view.focused_ship,
                ..default()
            };
        }
        let Some((_, details, Some(ship_pose))) = ships
            .iter()
            .find(|(ship, _, _)| Some(ship.0.ship) == view.focused_ship)
        else {
            options.instruments = Instruments::default();
            options.own.clear();
            options.curves.clear();
            options.horizon = 0.;
            options.encounter = None;
            continue;
        };
        let mut instruments = details
            .and_then(|details| details.0.instruments.clone())
            .unwrap_or_default();
        let target = instruments.selected_contact.and_then(|reference| {
            contacts
                .iter()
                .find(|(contact, _)| contact.1 == reference)
                .map(|(_, pose)| &pose.0)
        });
        let primary = celestials
            .iter()
            .filter(|(body, system, _)| {
                body.0.gravitational_parameter > 0.
                    && systems
                        .0
                        .iter()
                        .any(|reference| reference.system == system.0)
            })
            .max_by(|(a, _, a_pose), (b, _, b_pose)| {
                let acceleration = |body: &CelestialPresentation, pose: &Pose| {
                    body.gravitational_parameter
                        / pose
                            .position
                            .relative_to(ship_pose.0.position)
                            .length_squared()
                            .max(body.radius_m.powi(2))
                };
                acceleration(&a.0, &a_pose.0).total_cmp(&acceleration(&b.0, &b_pose.0))
            })
            .map(|(body, _, pose)| (&body.0, &pose.0));
        if let Some((body, pose)) = primary {
            reframe_instruments(&mut instruments, pose.position, |time| {
                definitions.iter().find_map(|definition| {
                    definition.body_position(body.entity, time, &definition_assets)
                })
            });
        }
        refresh(
            &mut options,
            &ship_pose.0,
            instruments,
            primary,
            target,
            &clock,
        );
    }
}

fn controls(
    mut contexts: EguiContexts,
    clock: Res<RenderTime>,
    session: Res<SessionInfo>,
    selection: Res<Selection>,
    mut views: Query<(
        &ViewObservation,
        &ViewCamera,
        &mut ViewOptions,
        &mut CameraOptions,
    )>,
) -> Result {
    if session.world.is_none() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let toggle =
        !ctx.egui_wants_keyboard_input() && ctx.input(|input| input.key_pressed(egui::Key::O));
    for (observation, camera, mut options, mut framing) in &mut views {
        let view = &observation.0;
        let options = &mut *options;
        if toggle {
            options.enabled = !options.enabled;
        }
        egui::Window::new(format!("Orbit · view {}", view.id))
            .default_open(false)
            .show(ctx, |ui| {
                ui.checkbox(&mut options.enabled, "Trajectories [O]");
                ui.horizontal(|ui| {
                    for (label, mode) in [
                        ("Ship", Framing::Ship),
                        ("Orbit", Framing::Orbit),
                        ("Encounter", Framing::Encounter),
                        ("Return", Framing::Return),
                    ] {
                        if ui
                            .selectable_label(framing.framing == mode, label)
                            .clicked()
                        {
                            if mode == Framing::Return {
                                if let Some((distance, yaw, pitch)) = framing.saved.take() {
                                    framing.distance = Some(distance);
                                    framing.restore_angles = Some((yaw, pitch));
                                }
                            } else if framing.saved.is_none() {
                                framing.saved = Some((camera.distance, camera.yaw, camera.pitch));
                            }
                            framing.framing = mode;
                            framing.focus = None;
                            options.preview = 0.;
                            if mode != Framing::Return {
                                framing.distance = Some(match mode {
                                    Framing::Orbit => options.orbit_distance,
                                    Framing::Encounter => {
                                        options.encounter.map_or(100., |(_, range, _)| {
                                            (range * 2.).max(100.) as f32
                                        })
                                    }
                                    _ => 100.,
                                });
                            }
                        }
                    }
                });
                if ui
                    .add_enabled(
                        selection.contact().is_some(),
                        egui::Button::new("Follow selected contact"),
                    )
                    .clicked()
                {
                    framing.focus = selection
                        .contact()
                        .map(|reference| SelectedTarget::Contact(reference));
                    framing.framing = Framing::Ship;
                    options.preview = 0.;
                    framing.distance = Some(100.);
                }
                if ui.button("Restore ship focus").clicked() {
                    framing.focus = None;
                    framing.framing = Framing::Ship;
                    options.preview = 0.;
                    framing.distance = Some(100.);
                }
                if let Some(period) = options.period {
                    ui.label(format!("Period {:.1} min", period / 60.));
                } else {
                    ui.label("Unbound / inertial");
                }
                if let Some((seconds, range, _)) = options.encounter {
                    ui.label(format!(
                        "Closest {:.1} km · T+{seconds:.0} s",
                        range / 1000.
                    ));
                }
                ui.add(
                    egui::Slider::new(&mut options.preview, 0.0..=options.horizon.max(0.1))
                        .text("Preview seconds"),
                );
                ui.small("Two-body estimate · external forces may change the path");
            });
        framing.origin = if framing.focus.is_some() {
            None
        } else {
            match framing.framing {
                Framing::Orbit => Some(options.orbit_origin),
                Framing::Encounter => options.encounter.map(|(_, _, position)| position),
                _ if options.preview > 0. => sample(
                    &options.own,
                    clock.display_ns + (options.preview * 1e9) as u64,
                ),
                _ => None,
            }
        };
        options
            .instruments
            .markers
            .retain(|marker| marker.id != u64::MAX - 201);
        if options.preview > 0. {
            let time = clock.display_ns + (options.preview * 1e9) as u64;
            if let Some(position) = sample(&options.own, time) {
                options.instruments.markers.push(NavigationMarker {
                    id: u64::MAX - 201,
                    kind: 0,
                    position,
                    sim_time_ns: time,
                    label: format!("PREVIEW\nT+{:.1} s", options.preview),
                });
            }
        }
    }
    Ok(())
}

fn reframe_instruments(
    instruments: &mut Instruments,
    anchor: GalacticPosition,
    primary_position: impl Fn(u64) -> Option<GalacticPosition>,
) {
    for path in &mut instruments.paths {
        if !path.timed {
            continue;
        }
        for vertex in &mut path.vertices {
            if let Some(primary) = primary_position(vertex.sim_time_ns) {
                vertex.position = anchor + (vertex.position - primary);
            }
        }
    }
    for marker in &mut instruments.markers {
        if let Some(primary) = primary_position(marker.sim_time_ns) {
            marker.position = anchor + (marker.position - primary);
        }
    }
}

fn refresh(
    options: &mut ViewOptions,
    ship_pose: &Pose,
    instruments: Instruments,
    primary: Option<(&CelestialPresentation, &Pose)>,
    target_pose: Option<&Pose>,
    clock: &RenderTime,
) {
    options.instruments = instruments;
    options.instruments.valid_until_ns = options
        .instruments
        .valid_until_ns
        .max(clock.current_ns + 200_000_000);
    let anchor = primary.map_or(ship_pose.position, |(_, pose)| pose.position);
    let velocity = primary.map_or(glam::DVec3::ZERO, |(_, pose)| {
        glam::DVec3::from_array(pose.velocity)
    });
    let fit = |pose: &Pose| conic::Conic {
        r: pose.position.relative_to(anchor),
        v: glam::DVec3::from_array(pose.velocity) - velocity,
        mu: primary.map_or(0., |(body, _)| body.gravitational_parameter),
        radius: primary.map_or(0., |(body, _)| body.radius_m),
    };
    let own = fit(ship_pose);
    options.period = own.period().filter(|period| period.is_finite());
    options.horizon = options.period.unwrap_or(3600.).clamp(60., 31_557_600.);
    let target = target_pose.map(fit);
    options.own.clear();
    options.curves.clear();
    options.curve_anchor = anchor;
    options.encounter = None;
    let mut low = glam::DVec3::splat(f64::INFINITY);
    let mut high = glam::DVec3::splat(f64::NEG_INFINITY);
    for index in 0..=256 {
        let seconds = options.horizon * index as f64 / 256.;
        let Some((position, _)) = own.state(seconds) else {
            break;
        };
        if !position.is_finite() || position.length() < own.radius {
            break;
        }
        let galactic = anchor.offset_by(position);
        let sim_time_ns = clock.display_ns + (seconds * 1e9) as u64;
        options.own.push(TrajectoryVertex {
            sim_time_ns,
            position: galactic,
        });
        low = low.min(position);
        high = high.max(position);
        if let Some((target, _)) = target.as_ref().and_then(|target| target.state(seconds)) {
            let range = target.distance(position);
            if options
                .encounter
                .is_none_or(|(_, previous, _)| range < previous)
            {
                options.encounter =
                    Some((seconds, range, anchor.offset_by((position + target) * 0.5)));
            }
        }
    }
    if let Some(target) = &target {
        if let Some((seconds, a, b)) = closest(0., options.horizon, |seconds| {
            Some((own.state(seconds)?.0, target.state(seconds)?.0))
        }) {
            options.encounter = Some((seconds, a.distance(b), anchor.offset_by((a + b) * 0.5)));
        }
    }
    options.instruments.markers.extend(coast_markers(
        &own,
        target.as_ref(),
        anchor,
        options.horizon,
        clock.display_ns,
    ));
    for path in &options.instruments.paths {
        if let Some(last) = path.vertices.last() {
            options.instruments.markers.push(NavigationMarker {
                id: path.id ^ (1 << 63),
                kind: 1,
                position: last.position,
                sim_time_ns: last.sim_time_ns,
                label: format!(
                    "PLAN END\nT+{:.1} s",
                    last.sim_time_ns.saturating_sub(clock.display_ns) as f64 * 1e-9
                ),
            });
        }
    }
    if let Some((seconds, range, position)) = options.encounter {
        options.instruments.markers.push(NavigationMarker {
            id: u64::MAX - 200,
            kind: 2,
            position,
            sim_time_ns: clock.display_ns + (seconds * 1e9) as u64,
            label: format!("CA\n{range:.1} m separation · T+{seconds:.1} s"),
        });
    }
    if low.is_finite() && high.is_finite() {
        options.orbit_origin = anchor.offset_by((low + high) * 0.5);
        options.orbit_distance = ((high - low).length() * 1.2).max(100.) as f32;
    }
    options.curves.push(geometry::AnalyticCurve {
        conic: own,
        offset: glam::DVec3::ZERO,
    });
    if let Some(target) = target {
        options.curves.push(geometry::AnalyticCurve {
            conic: target,
            offset: glam::DVec3::ZERO,
        });
    }
}

fn draw_coasts(
    mut contexts: EguiContexts,
    cameras: Query<(
        &Camera,
        &Transform,
        &Projection,
        &ViewCamera,
        &ViewOptions,
        &SystemSubscription,
    )>,
    bodies: Query<(&Celestial, &CelestialSystem, &DisplayPose)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let pixels_per_point = ctx.pixels_per_point();
    for (camera, transform, projection, camera_state, options, systems) in &cameras {
        if !options.enabled {
            continue;
        }
        let (Some(viewport), Projection::Perspective(projection)) =
            (camera.physical_viewport_rect(), projection)
        else {
            continue;
        };
        let rect = egui::Rect::from_min_max(
            egui::pos2(viewport.min.x as f32, viewport.min.y as f32) / pixels_per_point,
            egui::pos2(viewport.max.x as f32, viewport.max.y as f32) / pixels_per_point,
        );
        let sy = 1. / (projection.fov as f64 * 0.5).tan();
        let view = projection::View {
            rect,
            eye: camera_state.origin.relative_to(options.curve_anchor)
                + transform.translation.as_dvec3(),
            rotation: transform.rotation.as_dquat().inverse(),
            sx: sy / (rect.width() as f64 / rect.height() as f64),
            sy,
        };
        let occluders: Vec<_> = bodies
            .iter()
            .filter(|(_, system, _)| systems.0.iter().any(|entry| entry.system == system.0))
            .map(|(body, _, pose)| {
                (
                    pose.0.position.relative_to(options.curve_anchor),
                    body.0.radius_m,
                )
            })
            .collect();
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new(("orbit_coasts", camera_state.view)),
            ))
            .with_clip_rect(rect);
        for (index, curve) in options.curves.iter().enumerate() {
            let end = curve
                .conic
                .impact(options.horizon)
                .unwrap_or(options.horizon);
            let Some(arcs) = curve.arcs(0., end) else {
                continue;
            };
            let color = if index == 0 {
                egui::Color32::from_rgb(80, 218, 255)
            } else {
                egui::Color32::from_rgb(233, 129, 213)
            };
            for arc in curves::project(&view, &arcs, pixels_per_point, &occluders) {
                let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                    arc.points,
                    false,
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new(1.5, color),
                );
                if arc.hidden {
                    for pair in shape.flatten(Some(0.25 / pixels_per_point)).windows(2) {
                        let delta = pair[1] - pair[0];
                        let length = delta.length();
                        for segment in 0..(length / 10.).ceil().min(512.) as usize {
                            let start = segment as f32 * 10.;
                            let end = (start + 4.).min(length);
                            painter.line_segment(
                                [
                                    pair[0] + delta * (start / length.max(1e-6)),
                                    pair[0] + delta * (end / length.max(1e-6)),
                                ],
                                egui::Stroke::new(1., color.gamma_multiply(0.35)),
                            );
                        }
                    }
                } else {
                    painter.add(shape);
                }
            }
        }
    }
    Ok(())
}

fn sample(vertices: &[TrajectoryVertex], time: u64) -> Option<GalacticPosition> {
    let pair = vertices
        .windows(2)
        .find(|pair| pair[0].sim_time_ns <= time && pair[1].sim_time_ns >= time)?;
    let fraction = (time - pair[0].sim_time_ns) as f64
        / pair[1]
            .sim_time_ns
            .saturating_sub(pair[0].sim_time_ns)
            .max(1) as f64;
    Some(
        pair[0]
            .position
            .offset_by(pair[1].position.relative_to(pair[0].position) * fraction),
    )
}

fn closest(
    start: f64,
    end: f64,
    pair: impl Fn(f64) -> Option<(glam::DVec3, glam::DVec3)>,
) -> Option<(f64, glam::DVec3, glam::DVec3)> {
    if end < start {
        return None;
    }
    let cost = |t| {
        pair(t)
            .map(|(a, b)| (a - b).length_squared())
            .unwrap_or(f64::INFINITY)
    };
    let step = (end - start) / 256.;
    let index = (0..=256)
        .min_by(|a, b| cost(start + *a as f64 * step).total_cmp(&cost(start + *b as f64 * step)))?;
    let mut lo = (start + (index as f64 - 1.) * step).max(start);
    let mut hi = (lo + 2. * step).min(end);
    for _ in 0..48 {
        let a = lo + (hi - lo) / 3.;
        let b = hi - (hi - lo) / 3.;
        if cost(a) < cost(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let t = [start, end, (lo + hi) * 0.5]
        .into_iter()
        .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))?;
    let (a, b) = pair(t)?;
    Some((t, a, b))
}

fn coast_markers(
    own: &conic::Conic,
    target: Option<&conic::Conic>,
    anchor: GalacticPosition,
    horizon: f64,
    epoch: u64,
) -> Vec<NavigationMarker> {
    let mut markers = Vec::new();
    let mut add = |name: &str, curve: &conic::Conic, seconds: f64, kind| {
        if seconds < 0. || seconds > horizon {
            return;
        }
        let Some((position, _)) = curve.state(seconds) else {
            return;
        };
        markers.push(NavigationMarker {
            id: u64::MAX - 100 - markers.len() as u64,
            kind,
            position: anchor.offset_by(position),
            sim_time_ns: epoch + (seconds * 1e9) as u64,
            label: format!(
                "{name}\n{:.1} km altitude · T+{seconds:.1} s",
                (position.length() - curve.radius) / 1000.
            ),
        });
    };
    add("SHIP", own, 0., 0);
    for (name, seconds) in own.apsides() {
        add(name, own, seconds, 0);
    }
    if let Some(seconds) = own.impact(horizon) {
        add("IMPACT", own, seconds, 0);
    }
    if let Some(target) = target {
        add("TARGET", target, 0., 2);
        for (name, seconds) in target.apsides() {
            add(&format!("TARGET {name}"), target, seconds, 2);
        }
        if let Some(seconds) = target.impact(horizon) {
            add("TARGET IMPACT", target, seconds, 2);
        }
        if let (Some(a), Some(b)) = (own.normal(), target.normal()) {
            if let Some(node) = a
                .cross(b)
                .try_normalize()
                .filter(|_| a.cross(b).length() > 1e-4)
            {
                for direction in [node, -node] {
                    if let Some(seconds) = own.time_at_direction(direction) {
                        if let Some((_, velocity)) = own.state(seconds) {
                            add(
                                if velocity.dot(b) > 0. { "AN" } else { "DN" },
                                own,
                                seconds,
                                0,
                            );
                        }
                    }
                }
            }
        }
    }
    markers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_firmware_paths_remove_exact_primary_motion_but_not_spatial_geometry() {
        let anchor = GalacticPosition::new(10_i128.pow(28), 0, 0);
        let primary = |time: u64| {
            let seconds = time as f64 * 1e-9;
            Some(anchor.offset_by(glam::DVec3::new(30_000. * seconds, seconds * seconds, 0.)))
        };
        let vertices: Vec<_> = [0, 10_000_000_000, 20_000_000_000]
            .into_iter()
            .map(|time| TrajectoryVertex {
                sim_time_ns: time,
                position: primary(time).unwrap().offset_by(glam::DVec3::X * 100.),
            })
            .collect();
        let path = Trajectory {
            id: 1,
            revision: 1,
            published_at_ns: 0,
            valid_until_ns: u64::MAX,
            timed: true,
            vertices,
        };
        let spatial = Trajectory {
            id: 2,
            timed: false,
            ..path.clone()
        };
        let mut instruments = Instruments {
            paths: vec![path, spatial.clone()],
            ..default()
        };
        reframe_instruments(&mut instruments, anchor, primary);
        for vertex in &instruments.paths[0].vertices {
            assert_eq!(vertex.position, anchor.offset_by(glam::DVec3::X * 100.));
        }
        assert_eq!(instruments.paths[1], spatial);
    }

    #[test]
    fn orbit_remains_closed_in_the_moving_primary_frame_between_snapshots() {
        let anchor = GalacticPosition::new(10_i128.pow(28), -10_i128.pow(28), 0);
        let drift = glam::DVec3::new(30_000., 12_000., -4_000.);
        let conic = conic::Conic {
            r: glam::DVec3::X * 7e6,
            v: glam::DVec3::Y * (3.986004418e14_f64 / 7e6).sqrt(),
            mu: 3.986004418e14,
            radius: 6.371e6,
        };
        let mut options = ViewOptions::default();
        for seconds in [0., 0.016, 0.033, 0.09, 1.] {
            let (position, velocity) = conic.state(seconds).unwrap();
            let primary_pose = Pose {
                position: anchor.offset_by(drift * seconds),
                rotation: [0., 0., 0., 1.],
                velocity: drift.to_array(),
                angular_velocity: [0.; 3],
            };
            let ship_pose = Pose {
                position: primary_pose.position.offset_by(position),
                velocity: (drift + velocity).to_array(),
                ..primary_pose.clone()
            };
            let body = CelestialPresentation {
                entity: Id::default(),
                name: "Moving primary".into(),
                pose: primary_pose.clone(),
                radius_m: conic.radius,
                gravitational_parameter: conic.mu,
                luminosity_lumens: 0.,
                temperature_k: 300.,
                color: [1.; 3],
                atmosphere: None,
                ephemeris: None,
            };
            let clock = RenderTime {
                display_ns: (seconds * 1e9) as u64,
                current_ns: 1_000_000_000,
                ..default()
            };
            refresh(
                &mut options,
                &ship_pose,
                Instruments::default(),
                Some((&body, &primary_pose)),
                None,
                &clock,
            );
            assert!(
                options
                    .own
                    .first()
                    .unwrap()
                    .position
                    .relative_to(ship_pose.position)
                    .length()
                    < 1e-4
            );
            assert!(
                options
                    .own
                    .last()
                    .unwrap()
                    .position
                    .relative_to(ship_pose.position)
                    .length()
                    < 0.01
            );
            assert!(options.own.iter().all(|vertex| {
                (vertex.position.relative_to(primary_pose.position).length() - conic.r.length())
                    .abs()
                    < 0.01
            }));
            assert_eq!(options.curve_anchor, primary_pose.position);
            assert!(options.curves[0].conic.r.distance(position) < 1e-4);
        }
    }

    #[test]
    fn native_coast_annotations_keep_apsides_nodes_and_galactic_precision() {
        let own = conic::Conic {
            r: glam::DVec3::X * 10.,
            v: glam::DVec3::Y * 2.,
            mu: 100.,
            radius: 1.,
        };
        let target = conic::Conic {
            v: glam::DVec3::new(0., 1.5, 1.5),
            ..own.clone()
        };
        let anchor = GalacticPosition::new(10_i128.pow(28), -10_i128.pow(28), 0);
        let markers = coast_markers(
            &own,
            Some(&target),
            anchor,
            own.period().unwrap(),
            1_000_000_000,
        );
        assert!(
            markers
                .iter()
                .any(|marker| marker.label.starts_with("PE\n"))
        );
        assert!(
            markers
                .iter()
                .any(|marker| marker.label.starts_with("AP\n"))
        );
        assert!(
            markers
                .iter()
                .any(|marker| marker.label.starts_with("AN\n") || marker.label.starts_with("DN\n"))
        );
        assert!(
            markers
                .iter()
                .all(|marker| marker.position.relative_to(anchor).length() < 100.)
        );
    }
}
