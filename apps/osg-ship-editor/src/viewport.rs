use super::*;
use bevy::{camera::Viewport, math::DVec3};
use osg_ui::egui;
#[derive(Component)]
pub struct EditorCamera;
#[derive(Component)]
pub struct EditorPart;
pub fn setup(mut commands: Commands) {
    // Keep the GUI camera full-window while the scene camera uses the centre panel.
    commands.spawn((
        osg_ui::bevy_egui::PrimaryEguiContext,
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
    assets: Res<osg_ship_view::PartVisualAssets>,
    loader: Res<AssetServer>,
    parts: Query<Entity, With<EditorPart>>,
    mut seen: Local<u64>,
    mut gizmos: Gizmos,
    mut plumes: Query<&mut osg_ship_view::plume::EnginePlume>,
) {
    for mut plume in &mut plumes {
        plume.output = if e.devices_mode { 0. } else { e.preview_thrust };
    }
    if *seen != e.revision {
        *seen = e.revision;
        for entity in parts {
            commands.entity(entity).despawn();
        }
        for p in &e.layout {
            let tf = Transform::from_translation(p.centre.as_vec3())
                .with_rotation(Quat::from_mat3(&p.rotation.as_mat3()));
            let part = commands.spawn((EditorPart, tf, Visibility::default())).id();
            osg_ship_view::attach_part_body(&mut commands, part, &p.definition, &assets, &loader);
            if let Some(plume) = assets.plumes.get(&p.placed.prototype) {
                osg_ship_view::plume::spawn_plume(&mut commands, part, plume);
            }
        }
    }
    if e.devices_mode {
        return;
    }
    let highlight = |gizmos: &mut Gizmos, p: &PreparedPart, color: Color| {
        let (lo, hi) = p.bounds();
        gizmos.cube(
            Transform::from_translation(p.centre.as_vec3())
                .with_scale((hi - lo).as_vec3() + Vec3::splat(0.015)),
            color,
        );
    };
    if e.place.is_none() {
        if let Some(part) = e.layout.iter().find(|p| Some(p.placed.id) == e.selected) {
            highlight(&mut gizmos, part, Color::srgb(1., 0.8, 0.2));
        }
    } else {
        let connector = e
            .place
            .as_ref()
            .and_then(|id| e.catalogue.part(id))
            .and_then(|def| def.attachment_nodes().get(e.plug).cloned());
        for part in &e.layout {
            for node in part.definition.attachment_nodes() {
                if connector
                    .as_ref()
                    .is_none_or(|c| c.connector != node.connector)
                    || socket_used(&e.ship, part.placed.id, &node.name)
                {
                    continue;
                }
                let point =
                    (part.centre + part.rotation * DVec3::from_array(node.position_m)).as_vec3();
                let normal = (part.rotation * DVec3::from_array(node.normal)).as_vec3();
                let size = (e.camera_distance * 0.004).clamp(0.08, 20.0);
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    gizmos.line(
                        point - axis * size,
                        point + axis * size,
                        Color::srgb(0.3, 0.85, 1.0),
                    );
                }
                gizmos.line(
                    point,
                    point + normal * size * 3.,
                    Color::srgb(0.3, 0.85, 1.0),
                );
            }
        }
    }
    if e.ghost.is_some() {
        if let Some(p) = &e.ghost_pose {
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
}

fn socket_used(ship: &ShipBlueprint, id: u64, socket: &str) -> bool {
    ship.parts.iter().any(|p| {
        p.attachment.as_ref().is_some_and(|a| {
            (a.parent == id && a.socket == socket) || (p.id == id && a.plug == socket)
        })
    })
}

#[derive(Component)]
pub struct PlacementGhost;

pub fn ghost(
    mut commands: Commands,
    editor: Res<Editor>,
    assets: Res<osg_ship_view::PartVisualAssets>,
    loader: Res<AssetServer>,
    mut existing: Query<(&mut Transform, &mut Visibility), With<PlacementGhost>>,
    mut current: Local<Option<(String, Entity)>>,
) {
    let candidate = editor.ghost.as_ref().filter(|_| !editor.devices_mode);
    let Some(part) = candidate else {
        if let Some((_, entity)) = current.as_ref() {
            if let Ok((_, mut visibility)) = existing.get_mut(*entity) {
                *visibility = Visibility::Hidden;
            }
        }
        return;
    };
    let Some(definition) = editor.catalogue.part(&part.prototype) else {
        return;
    };
    let Some(prepared) = &editor.ghost_pose else {
        return;
    };
    let transform = Transform::from_translation(prepared.centre.as_vec3())
        .with_rotation(Quat::from_mat3(&prepared.rotation.as_mat3()));
    if let Some((prototype, entity)) = current.as_ref() {
        if prototype == &part.prototype {
            if let Ok((mut existing, mut visibility)) = existing.get_mut(*entity) {
                *existing = transform;
                *visibility = Visibility::Inherited;
            }
            return;
        }
        commands.entity(*entity).despawn();
    }
    let entity = commands
        .spawn((PlacementGhost, transform, Visibility::Inherited))
        .id();
    osg_ship_view::attach_part_body(&mut commands, entity, definition, &assets, &loader);
    *current = Some((part.prototype.clone(), entity));
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
            e.cancel_placement();
        }
        if ui.input(|i| i.key_pressed(egui::Key::R)) {
            e.orientation = (e.orientation + 1) % 4;
        }
        if e.place.is_none() && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(id) = e.selected {
                e.delete_part(id);
            }
        }
    }
    let Some(pointer) = response.hover_pos() else {
        e.ghost = None;
        return;
    };
    let logical = Vec2::new(pointer.x, pointer.y) * (e.pixels_per_point / window.scale_factor());
    let Ok(ray) = camera.viewport_to_world(global, logical) else {
        return;
    };
    let origin = ray.origin.as_dvec3();
    let direction = ray.direction.as_dvec3();
    let mut nearest = None;
    for part in &e.layout {
        let (lo, hi) = part.bounds();
        if let Some((t, _)) = hit(origin, direction, lo, hi) {
            if nearest.is_none_or(|(_, old)| t < old) {
                nearest = Some((part.placed.id, t));
            }
        }
    }
    if let Some(proto) = e.place.clone() {
        let Some(def) = e.catalogue.part(&proto) else {
            e.cancel_placement();
            return;
        };
        let nodes = def.attachment_nodes();
        let Some(plug) = nodes.get(e.plug) else {
            e.ghost = None;
            return;
        };
        let mut selected: Option<(f64, Attachment)> = None;
        for part in &e.layout {
            for socket in part.definition.attachment_nodes() {
                if socket.connector != plug.connector
                    || socket_used(&e.ship, part.placed.id, &socket.name)
                {
                    continue;
                }
                let point = part.centre + part.rotation * DVec3::from_array(socket.position_m);
                let along = (point - origin).dot(direction);
                if along <= 0.0 {
                    continue;
                }
                let miss = (point - origin - direction * along).length() / along;
                if miss < 0.025 && selected.as_ref().is_none_or(|(old, _)| miss < *old) {
                    selected = Some((
                        miss,
                        Attachment {
                            parent: part.placed.id,
                            socket: socket.name,
                            plug: plug.name.clone(),
                            roll: e.orientation,
                        },
                    ));
                }
            }
        }
        if !e.ship.parts.is_empty() && selected.is_none() {
            e.ghost = None;
            return;
        }
        let part = PlacedPart {
            id: e.ship.parts.iter().map(|p| p.id).max().unwrap_or(0) + 1,
            prototype: proto,
            tanks: e.placement_tanks.clone(),
            name: String::new(),
            alias: String::new(),
            groups: vec![],
            attachment: selected.map(|(_, mount)| mount),
        };
        if e.ghost.as_ref() != Some(&part) {
            let mut candidate = e.ship.clone();
            candidate.parts.push(part.clone());
            e.ghost_pose = candidate
                .layout(&e.catalogue)
                .ok()
                .and_then(|mut parts| parts.pop());
            e.ghost_valid = candidate.compile(&e.catalogue).is_ok();
            e.ghost = Some(part.clone());
        }
        if response.clicked() && e.ghost_valid {
            e.add_part(part);
        }
    } else if response.clicked() {
        e.selected = nearest.map(|n| n.0);
        e.inspector = crate::ui::InspectorTab::Part;
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
