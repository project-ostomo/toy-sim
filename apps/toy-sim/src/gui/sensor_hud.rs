use crate::{
    camera::MainCamera,
    precision::{PreciseTransform, PresentationPose},
    sensors::{Sensor, SensorContacts},
    spatial::SpatialIndex,
    vessel::{ControlledVessel, ShipDesign, ShipHardware, ShipSoftware, Vessel},
};
use bevy::{math::DVec3, prelude::*, window::PrimaryWindow};
use bevy_egui::{EguiContexts, egui};

#[derive(Resource)]
pub struct SensorHud {
    enabled: bool,
    labels: bool,
    ships: bool,
    celestials: bool,
    min_size: f32,
}
impl Default for SensorHud {
    fn default() -> Self {
        Self {
            enabled: true,
            labels: true,
            ships: true,
            celestials: true,
            min_size: 12.0,
        }
    }
}

pub fn controls(
    mut contexts: EguiContexts,
    mut hud: ResMut<SensorHud>,
    index: Res<SpatialIndex>,
    mut observer: Single<(&Vessel, &mut Sensor, &SensorContacts), With<ControlledVessel>>,
) -> Result {
    let (ship, sensor, contacts) = &mut *observer;
    egui::Window::new("Sensor debug")
        .default_pos(egui::pos2(590.0, 10.0))
        .default_width(270.0)
        .show(contexts.ctx_mut()?, |ui| {
            ui.label(format!("Observer: {}", ship.vessel_name));
            let mut range_km = sensor.range_m / 1000.0;
            if ui
                .add(
                    egui::Slider::new(&mut range_km, 1.0..=10_000_000.0)
                        .logarithmic(true)
                        .text("Range (km)"),
                )
                .changed()
            {
                sensor.range_m = range_km * 1000.0;
            }
            ui.checkbox(&mut sensor.occlusion, "Planet / moon / star occlusion");
            ui.label("Omnidirectional; target-centre range and visibility");
            ui.separator();
            ui.checkbox(&mut hud.enabled, "HUD markers");
            ui.checkbox(&mut hud.labels, "Names and distances");
            ui.horizontal(|ui| {
                ui.checkbox(&mut hud.ships, "Ships");
                ui.checkbox(&mut hud.celestials, "Celestial bodies");
            });
            ui.add(egui::Slider::new(&mut hud.min_size, 6.0..=40.0).text("Minimum square (px)"));
            ui.separator();
            ui.label(format!(
                "{} in range · {} blocked · {} visible",
                contacts.candidates,
                contacts.blocked,
                contacts.visible.len()
            ));
            ui.label(format!("{} possible occluders", contacts.occluders));
            ui.label(format!(
                "{} indexed objects · {} target cells",
                index.objects.len(),
                index.occupied_cells()
            ));
            ui.label(format!("Last scan: {:.1} s · 10 Hz", contacts.scan_time_s));
            ui.small(
                "Distances are from the sensor ship. Camera selection does not move the sensor.",
            );
        });
    Ok(())
}

/// Runs inside the egui pass, explicitly ordered after current transform propagation.
/// Painting has no interaction area and does not capture camera drags.
pub fn overlay(
    mut contexts: EguiContexts,
    hud: Res<SensorHud>,
    camera: Single<(&Camera, &PreciseTransform), With<MainCamera>>,
    time: Res<Time<Fixed>>,
    window: Single<&Window, With<PrimaryWindow>>,
    observer: Single<
        (&ShipSoftware, &ShipHardware, &ShipDesign, &PresentationPose),
        With<ControlledVessel>,
    >,
) -> Result {
    if !hud.enabled {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let (camera, camera_tf) = camera.into_inner();
    let Some(viewport) = camera.logical_viewport_rect() else {
        return Ok(());
    };
    let scale = window.scale_factor() / ctx.pixels_per_point();
    let clip = egui::Rect::from_min_max(
        egui::pos2(viewport.min.x * scale, viewport.min.y * scale),
        egui::pos2(viewport.max.x * scale, viewport.max.y * scale),
    );
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("sensor_hud"),
        ))
        .with_clip_rect(clip);
    let (software, hardware, design, pose) = *observer;
    if software.controller.is_booting() || !hardware.0.computer_running(&design.0) {
        return Ok(());
    }
    let now = crate::precision::presentation_time(&time);
    let right = camera_tf.rotation * DVec3::X;
    let up = camera_tf.rotation * DVec3::Y;
    let forward = camera_tf.rotation * DVec3::NEG_Z;
    for contact in software.controller.state.contact_list.iter() {
        let ship = contact.kind == toy_sim_ship_api::abi::CONTACT_SHIP;
        let celestial = contact.kind == toy_sim_ship_api::abi::CONTACT_CELESTIAL;
        if (ship && !hud.ships) || (celestial && !hud.celestials) {
            continue;
        }
        let Some(estimate) = software
            .controller
            .state
            .spatial
            .tracks
            .get(&contact.id)
            .and_then(|track| track.at(now))
        else {
            continue;
        };
        let position = crate::navigation::position(estimate.position);
        let relative = position.relative_to(camera_tf.translation_um);
        let view_centre = DVec3::new(relative.dot(right), relative.dot(up), relative.dot(forward));
        let Some((lower, upper)) = sphere_slope_bounds(view_centre, contact.radius_m) else {
            continue;
        };
        // Tangent slopes are x/depth and y/depth. Project at unit depth so
        // astronomical distances and near-plane clipping do not affect the outline.
        let project = |slope: Vec2| {
            let clip = camera.clip_from_view() * slope.extend(-1.0).extend(1.0);
            let ndc = clip.truncate().truncate() / clip.w;
            viewport.min + (Vec2::new(ndc.x, -ndc.y) + Vec2::ONE) * 0.5 * viewport.size()
        };
        let a = project(lower);
        let b = project(upper);
        if !a.is_finite() || !b.is_finite() {
            continue;
        }
        let min = a.min(b);
        let max = a.max(b);
        let side = ((max - min).max_element() * scale).max(hud.min_size);
        let middle = (min + max) * (0.5 * scale);
        let square =
            egui::Rect::from_center_size(egui::pos2(middle.x, middle.y), egui::vec2(side, side));
        if !square.intersects(clip) {
            continue;
        }
        let colour = if celestial {
            egui::Color32::from_rgb(255, 210, 125)
        } else {
            egui::Color32::from_rgb(125, 220, 255)
        };
        painter.rect_stroke(
            square,
            0.0,
            egui::Stroke::new(1.0, colour),
            egui::StrokeKind::Inside,
        );
        if hud.labels {
            let name = &contact.name;
            let distance =
                crate::navigation::distance(position.relative_to(pose.0.translation_um).length());
            let pos = egui::pos2(
                square.left().max(clip.left() + 2.0),
                square.bottom().min(clip.bottom() - 30.0),
            );
            let text = format!("{name}\n{distance}");
            painter.text(
                pos + egui::vec2(1.0, 1.0),
                egui::Align2::LEFT_TOP,
                &text,
                egui::FontId::proportional(11.0),
                egui::Color32::BLACK,
            );
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(11.0),
                colour,
            );
        }
    }
    Ok(())
}

/// Exact axis-aligned perspective bounds of a sphere, in image-plane slopes.
/// The centre uses camera right/up/forward coordinates (positive z is forward).
fn sphere_slope_bounds(centre: DVec3, radius: f64) -> Option<(Vec2, Vec2)> {
    if !centre.is_finite() || !radius.is_finite() || radius < 0.0 || centre.z <= radius {
        // A sphere touching/crossing the eye plane has no finite enclosing
        // perspective rectangle. Omit its marker, including the observer's hull.
        return None;
    }
    // Normalize by depth to avoid large squared world distances. Each extremum
    // is a ray tangent to the circle in the corresponding axis/depth plane.
    let r = radius / centre.z;
    let denominator = (1.0 - r) * (1.0 + r);
    let bounds = |axis: f64| {
        let a = axis / centre.z;
        let extent = r * (a * a + denominator).sqrt();
        ((a - extent) / denominator, (a + extent) / denominator)
    };
    let (left, right) = bounds(centre.x);
    let (bottom, top) = bounds(centre.y);
    Some((
        Vec2::new(left as f32, bottom as f32),
        Vec2::new(right as f32, top as f32),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_sphere_has_exact_angular_radius() {
        let (min, max) = sphere_slope_bounds(DVec3::new(0.0, 0.0, 5.0), 3.0).unwrap();
        assert_eq!(min, Vec2::splat(-0.75));
        assert_eq!(max, Vec2::splat(0.75));
    }

    #[test]
    fn off_axis_bounds_match_sampled_silhouette_at_any_distance() {
        for scale in [1.0, 1e9] {
            let centre = DVec3::new(8.0, -5.0, 10.0) * scale;
            let radius = 2.0 * scale;
            let (min, max) = sphere_slope_bounds(centre, radius).unwrap();
            // Independently sample the sphere's silhouette circle: its plane
            // is perpendicular to the eye-to-centre direction.
            let distance = centre.length();
            let normal = centre / distance;
            let u = normal.any_orthonormal_vector();
            let v = normal.cross(u);
            let ring_centre = centre * (1.0 - (radius / distance).powi(2));
            let ring_radius = radius * (1.0 - (radius / distance).powi(2)).sqrt();
            let mut sampled_min = Vec2::splat(f32::INFINITY);
            let mut sampled_max = Vec2::splat(f32::NEG_INFINITY);
            for i in 0..16384 {
                let angle = std::f64::consts::TAU * i as f64 / 16384.0;
                let point = ring_centre + ring_radius * (u * angle.cos() + v * angle.sin());
                let slope = (point.truncate() / point.z).as_vec2();
                sampled_min = sampled_min.min(slope);
                sampled_max = sampled_max.max(slope);
                assert!((slope.cmpge(min - Vec2::splat(1e-6))).all());
                assert!((slope.cmple(max + Vec2::splat(1e-6))).all());
            }
            assert!((sampled_min - min).abs().max_element() < 1e-6);
            assert!((sampled_max - max).abs().max_element() < 1e-6);
        }
    }

    #[test]
    fn eye_plane_and_invalid_spheres_have_no_finite_marker() {
        for centre in [DVec3::ZERO, DVec3::Z, -DVec3::Z, DVec3::NAN] {
            assert!(sphere_slope_bounds(centre, 1.0).is_none());
        }
        assert!(sphere_slope_bounds(DVec3::Z * 5.0, -1.0).is_none());
        assert!(sphere_slope_bounds(DVec3::Z * 5.0, f64::NAN).is_none());
        let (min, max) = sphere_slope_bounds(DVec3::new(2.0, 3.0, 5.0), 0.0).unwrap();
        assert_eq!(min, max);
        assert_eq!(min, Vec2::new(0.4, 0.6));
    }
}
