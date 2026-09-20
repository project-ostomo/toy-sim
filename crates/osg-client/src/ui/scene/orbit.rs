mod conic;
mod curves;
mod geometry;
mod paint;
use super::projection;

use super::{ViewCamera, camera::CameraOptions};
use crate::state::{
    Celestial, CelestialSystem, Contact, DisplayPose, OwnedShip, PresentationSet, RenderTime,
    ShipDetails, ViewObservation, ViewSystems,
};
use crate::ui::celestials::SystemDefinition;
use bevy::prelude::*;
use osg_model::*;
use osg_ui::bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

#[derive(Component)]
pub(in crate::ui) struct ViewOptions {
    pub enabled: bool,
    pub instruments: Instruments,
    horizon: f64,
    curves: Vec<geometry::AnalyticCurve>,
    curve_anchor: GalacticPosition,
    encounter: Option<[glam::DVec3; 2]>,
    ship: Option<Id>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            instruments: Instruments::default(),
            horizon: 0.,
            curves: Vec::new(),
            curve_anchor: GalacticPosition::ZERO,
            encounter: None,
            ship: None,
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
    .add_systems(
        EguiPrimaryContextPass,
        (
            toggle.in_set(crate::ui::input::GameplayInput::Keyboard),
            draw_coasts,
        )
            .chain(),
    );
}

fn refresh_views(
    clock: Res<RenderTime>,
    mut views: Query<(
        &ViewObservation,
        &ViewSystems,
        &mut ViewOptions,
        &mut CameraOptions,
    )>,
    ships: Query<(&OwnedShip, Option<&ShipDetails>, Option<&DisplayPose>)>,
    contacts: Query<(&Contact, &DisplayPose)>,
    celestials: Query<(&Celestial, &CelestialSystem, &DisplayPose)>,
    definitions: Query<&SystemDefinition>,
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
            options.curves.clear();
            options.encounter = None;
            options.horizon = 0.;
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
                    && systems.0.iter().any(|reference| *reference == system.0)
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
                definitions
                    .iter()
                    .find_map(|definition| definition.body_position(body.entity, time))
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

fn toggle(mut contexts: EguiContexts, mut views: Query<&mut ViewOptions>) -> Result {
    let ctx = contexts.ctx_mut()?;
    if ctx.input(|input| input.key_pressed(egui::Key::O)) {
        for mut options in &mut views {
            options.enabled = !options.enabled;
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
    options.horizon = own
        .period()
        .filter(|period| period.is_finite())
        .unwrap_or(3600.)
        .clamp(60., 31_557_600.);
    let target = target_pose.map(fit);
    options.curves.clear();
    options.curve_anchor = anchor;
    let encounter = target.as_ref().and_then(|target| {
        closest(0., options.horizon, |seconds| {
            Some((own.state(seconds)?.0, target.state(seconds)?.0))
        })
    });
    options.encounter = encounter.map(|(_, a, b)| [a, b]);
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
    if let Some((seconds, a, b)) = encounter {
        let range = a.distance(b);
        options.instruments.markers.push(NavigationMarker {
            id: u64::MAX - 200,
            kind: 2,
            position: anchor.offset_by((a + b) * 0.5),
            sim_time_ns: clock.display_ns + (seconds * 1e9) as u64,
            label: format!("CA\n{range:.1} m separation · T+{seconds:.1} s"),
        });
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
    clock: Res<RenderTime>,
    cameras: Query<(
        &Camera,
        &Transform,
        &Projection,
        &ViewCamera,
        &ViewOptions,
        &ViewSystems,
    )>,
    bodies: Query<(&Celestial, &CelestialSystem, &DisplayPose)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let pixels_per_point = ctx.pixels_per_point();
    let reserved: Vec<_> = ctx
        .memory(|memory| memory.areas().visible_layer_ids())
        .into_iter()
        .filter(|layer| layer.order >= egui::Order::Middle || *layer == crate::ui::console::layer())
        .filter_map(|layer| {
            egui::containers::AreaState::load(ctx, layer.id).map(|area| area.rect())
        })
        .collect();
    for (camera, transform, projection, camera_state, options, systems) in &cameras {
        if camera_state.private || !options.enabled {
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
            .filter(|(_, system, _)| systems.0.iter().any(|entry| *entry == system.0))
            .map(|(body, _, pose)| {
                (
                    pose.0.position.relative_to(options.curve_anchor),
                    body.0.radius_m,
                )
            })
            .collect();
        let painter =
            osg_ui::desktop::hud_painter(ctx, egui::Id::new(("orbit_coasts", camera_state.view)))
                .with_clip_rect(rect);
        paint::draw(
            &painter,
            &view,
            options,
            &occluders,
            clock.display_ns,
            &reserved,
        );
    }
    Ok(())
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
                reference: osg_model::travel::CelestialRef {
                    system: osg_model::Id::default(),
                    body: osg_model::Id::default(),
                },
                entity: Id::default(),
                name: "Moving primary".into(),
                pose: primary_pose.clone(),
                radius_m: conic.radius,
                gravitational_parameter: conic.mu,
                luminosity_lumens: 0.,
                temperature_k: 300.,
                color: [1.; 3],
                atmosphere: None,
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
            let coast = &options.curves[0].conic;
            for time in [0., options.horizon] {
                let position = options.curve_anchor.offset_by(coast.state(time).unwrap().0);
                assert!(position.relative_to(ship_pose.position).length() < 0.01);
            }
            for index in 0..=256 {
                let time = options.horizon * index as f64 / 256.;
                let position = coast.state(time).unwrap().0;
                assert!((position.length() - conic.r.length()).abs() < 0.01);
            }
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
