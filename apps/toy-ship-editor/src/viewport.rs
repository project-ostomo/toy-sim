use super::*;
use bevy::{camera::Viewport, math::DVec3};
use bevy_egui::egui;
#[derive(Component)]
pub struct EditorCamera;
#[derive(Component)]
pub struct EditorPart;
pub fn setup(mut commands: Commands) {
    // Keep the GUI camera full-window while the scene camera uses the centre panel.
    commands.spawn((
        bevy_egui::PrimaryEguiContext,
        Camera2d,
        bevy::camera::visibility::RenderLayers::none(),
        Camera {
            order: 1,
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: Some(bevy::render::render_resource::BlendState::ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
    ));
    commands.spawn((
        EditorCamera,
        Camera3d::default(),
        bevy::camera::Hdr,
        bevy::post_process::bloom::Bloom::default(),
        Transform::from_xyz(10., 8., 15.).looking_at(Vec3::ZERO, Vec3::Y),
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            far: 100000.,
            ..default()
        }),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 20000.,
            ..default()
        },
        Transform::from_xyz(5., 10., 8.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 500.,
        ..default()
    });
}
pub fn camera(
    e: Res<Editor>,
    mut camera: Single<(&mut Camera, &mut Transform), With<EditorCamera>>,
    window: Single<&Window>,
) {
    let (c, t) = &mut *camera;
    c.is_active = !e.devices_mode && e.viewport.width() > 1. && e.viewport.height() > 1.;
    if !c.is_active {
        return;
    }
    let scale = e.pixels_per_point;
    let min = e.viewport.min;
    let max = e.viewport.max;
    let pos = UVec2::new(
        (min.x * scale).max(0.) as u32,
        (min.y * scale).max(0.) as u32,
    );
    let end = UVec2::new((max.x * scale) as u32, (max.y * scale) as u32).min(UVec2::new(
        window.physical_width(),
        window.physical_height(),
    ));
    if end.x <= pos.x || end.y <= pos.y {
        c.is_active = false;
        return;
    }
    c.viewport = Some(Viewport {
        physical_position: pos,
        physical_size: end - pos,
        ..default()
    });
    let offset = Vec3::new(
        e.camera_yaw.sin() * e.camera_pitch.cos(),
        e.camera_pitch.sin(),
        e.camera_yaw.cos() * e.camera_pitch.cos(),
    ) * e.camera_distance;
    **t =
        Transform::from_translation(e.camera_target + offset).looking_at(e.camera_target, Vec3::Y);
}
pub fn visuals(
    mut commands: Commands,
    e: Res<Editor>,
    assets: Res<toy_sim_ship_view::PartVisualAssets>,
    loader: Res<AssetServer>,
    parts: Query<Entity, With<EditorPart>>,
    mut seen: Local<u64>,
    mut gizmos: Gizmos,
    mut plumes: Query<&mut toy_sim_ship_view::plume::EnginePlume>,
) {
    for mut plume in &mut plumes {
        plume.output = if e.devices_mode { 0. } else { e.preview_thrust };
    }
    if *seen != e.revision {
        *seen = e.revision;
        for entity in parts {
            commands.entity(entity).despawn();
        }
        for p in &e.ship.parts {
            if let Some(def) = e.catalogue.part(&p.prototype) {
                let (lo, hi) = occupied(p, def);
                let centre =
                    Vec3::from_array(std::array::from_fn(|i| (lo[i] + hi[i]) as f32 * 0.05));
                let tf = Transform::from_translation(centre)
                    .with_rotation(Quat::from_mat3(&orientation(p.orientation).as_mat3()));
                let mut entity = commands.spawn((EditorPart, tf, Visibility::default()));
                if let Some(model) = &def.model {
                    entity.insert(WorldAssetRoot(loader.load(format!("{model}#Scene0"))));
                } else {
                    entity.insert((
                        Mesh3d(assets.meshes[&p.prototype].clone()),
                        MeshMaterial3d(assets.materials[&p.prototype].clone()),
                    ));
                }
                let part = entity.id();
                if let Some(plume) = assets.plumes.get(&p.prototype) {
                    toy_sim_ship_view::plume::spawn_plume(&mut commands, part, plume);
                }
            }
        }
    }
    if e.devices_mode {
        return;
    }
    for x in -20..=20 {
        let v = x as f32;
        gizmos.line(
            Vec3::new(v, -0.01, -20.),
            Vec3::new(v, -0.01, 20.),
            Color::srgba(0.25, 0.3, 0.35, 0.3),
        );
        gizmos.line(
            Vec3::new(-20., -0.01, v),
            Vec3::new(20., -0.01, v),
            Color::srgba(0.25, 0.3, 0.35, 0.3),
        );
    }
    let highlight = |gizmos: &mut Gizmos, p: &PlacedPart, color: Color| {
        if let Some(def) = e.catalogue.part(&p.prototype) {
            let (lo, hi) = occupied(p, def);
            let centre = Vec3::from_array(std::array::from_fn(|i| (lo[i] + hi[i]) as f32 * 0.05));
            let size = Vec3::from_array(std::array::from_fn(|i| {
                (hi[i] - lo[i]) as f32 * 0.1 + 0.015
            }));
            gizmos.cube(Transform::from_translation(centre).with_scale(size), color);
        }
    };
    if let Some(id) = e.selected {
        if let Some(p) = e.ship.parts.iter().find(|p| p.id == id) {
            highlight(&mut gizmos, p, Color::srgb(1., 0.8, 0.2));
        }
    }
    if let Some(p) = &e.ghost {
        highlight(
            &mut gizmos,
            p,
            if e.ghost_valid {
                Color::srgb(0.2, 1., 0.5)
            } else {
                Color::srgb(1., 0.2, 0.2)
            },
        );
    }
}
/// Slab ray/AABB hit with the normal of the entering face.
fn hit(origin: DVec3, direction: DVec3, lo: DVec3, hi: DVec3) -> Option<(f64, DVec3)> {
    let mut near = 0f64;
    let mut far = f64::INFINITY;
    let mut normal = DVec3::Y;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-12 {
            if origin[axis] < lo[axis] || origin[axis] > hi[axis] {
                return None;
            }
            continue;
        }
        let mut a = (lo[axis] - origin[axis]) / direction[axis];
        let mut b = (hi[axis] - origin[axis]) / direction[axis];
        let mut n = DVec3::ZERO;
        n[axis] = -direction[axis].signum();
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if a > near {
            near = a;
            normal = n;
        }
        far = far.min(b);
        if near > far {
            return None;
        }
    }
    Some((near, normal))
}
pub fn interact(
    ui: &mut egui::Ui,
    e: &mut Editor,
    camera: &Camera,
    global: &GlobalTransform,
    window: &Window,
) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    let delta = ui.input(|i| i.pointer.delta());
    if response.dragged_by(egui::PointerButton::Secondary) {
        e.camera_yaw -= delta.x * 0.007;
        e.camera_pitch = (e.camera_pitch + delta.y * 0.007).clamp(-1.5, 1.5);
    }
    if response.dragged_by(egui::PointerButton::Middle) {
        let right = global.right();
        let up = global.up();
        e.camera_target += (-*right * delta.x + *up * delta.y) * e.camera_distance * 0.0015;
    }
    if response.hovered() {
        e.camera_distance = (e.camera_distance
            * (-ui.input(|i| i.smooth_scroll_delta.y) * 0.002).exp())
        .clamp(0.3, 100000.);
    }
    if !ui.ctx().egui_wants_keyboard_input() {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            e.place = None;
            e.ghost = None;
        }
        if ui.input(|i| i.key_pressed(egui::Key::R)) {
            e.orientation = (e.orientation + 1) % 24;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(id) = e.selected {
                e.delete_part(id);
            }
        }
    }
    e.ghost = None;
    let Some(pointer) = response.hover_pos() else {
        return;
    };
    let logical = Vec2::new(pointer.x, pointer.y) * (e.pixels_per_point / window.scale_factor());
    let Ok(ray) = camera.viewport_to_world(global, logical) else {
        return;
    };
    let origin = ray.origin.as_dvec3();
    let direction = ray.direction.as_dvec3();
    let mut nearest = None;
    for p in &e.ship.parts {
        if let Some(def) = e.catalogue.part(&p.prototype) {
            let (lo, hi) = occupied(p, def);
            let lo = DVec3::from_array(lo.map(|v| v as f64 * GRID));
            let hi = DVec3::from_array(hi.map(|v| v as f64 * GRID));
            if let Some((t, n)) = hit(origin, direction, lo, hi) {
                if nearest.as_ref().is_none_or(|(_, old, _, _, _)| t < *old) {
                    nearest = Some((p.id, t, n, lo, hi));
                }
            }
        }
    }
    if let Some(proto) = e.place.clone() {
        let def = e.catalogue.part(&proto).unwrap();
        let size =
            orientation(e.orientation).abs() * DVec3::from_array(def.dimensions.map(|d| d as f64));
        let position = if let Some((_, t, n, lo, hi)) = nearest {
            let point = (origin + direction * t) / GRID;
            let mut pos = (point - size / 2.).round();
            let axis = n.abs().max_element();
            for k in 0..3 {
                if n[k].abs() == axis {
                    pos[k] = if n[k] > 0. {
                        hi[k] / GRID
                    } else {
                        lo[k] / GRID - size[k]
                    };
                }
            }
            pos
        } else if direction.y.abs() > 1e-8 {
            let t = -origin.y / direction.y;
            if t < 0. {
                return;
            }
            let p = (origin + direction * t) / GRID;
            DVec3::new((p.x - size.x / 2.).round(), 0., (p.z - size.z / 2.).round())
        } else {
            return;
        };
        let part = PlacedPart {
            name: String::new(),
            alias: String::new(),
            groups: vec![],
            id: 0,
            prototype: proto,
            position: position.to_array().map(|v| v as i32),
            orientation: e.orientation,
        };
        let (lo, hi) = occupied(&part, def);
        let valid = !e.ship.parts.iter().any(|p| {
            let Some(def) = e.catalogue.part(&p.prototype) else {
                return false;
            };
            let (a, b) = occupied(p, def);
            (0..3).all(|i| lo[i] < b[i] && hi[i] > a[i])
        });
        e.ghost = Some(part.clone());
        e.ghost_valid = valid;
        if response.clicked() && valid {
            e.add_part(part);
        }
    } else if response.clicked() {
        e.selected = nearest.map(|n| n.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picking_returns_nearest_face_and_rejects_parallel_misses() {
        let hit = hit(
            DVec3::new(0.5, 0.5, 5.),
            DVec3::NEG_Z,
            DVec3::ZERO,
            DVec3::ONE,
        )
        .unwrap();
        assert_eq!(hit, (4., DVec3::Z));
        assert!(
            super::hit(
                DVec3::new(2., 0.5, 5.),
                DVec3::NEG_Z,
                DVec3::ZERO,
                DVec3::ONE
            )
            .is_none()
        );
    }
}
