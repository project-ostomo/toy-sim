use super::{ViewLayer, exposure, orbit, sky, sun_direction};
use crate::state::{
    Celestial, CelestialSystem, DisplayPose, Optical, OwnedShip, ViewObservation, ViewSystems,
};
use crate::ui::{SelectedTarget, Selection};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::{
    camera::{Hdr, Viewport, visibility::RenderLayers},
    prelude::*,
};
use osg_model::{GalacticPosition, Id};
use osg_ui::egui;

pub(in crate::ui) const LOOK_AT_RANGE_M: f64 = 100_000.0;

#[derive(Component, Default)]
pub(in crate::ui) struct CameraOptions {
    pub focus: Option<SelectedTarget>,
}

#[derive(Component)]
#[require(
    orbit::ViewOptions,
    CameraOptions,
    sky::ViewSky,
    exposure::ExposureSettings
)]
pub(in crate::ui) struct ViewCamera {
    pub view: u64,
    pub origin: GalacticPosition,
    pub layer: usize,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) radius: f32,
    pub(super) private: bool,
    pub(super) followed: Option<Id>,
    pub(super) aligned_to_sun: bool,
    smoothed_angles: Option<Vec2>,
}

impl ViewCamera {
    fn smooth_angles(&mut self, dt: f32) -> Vec2 {
        smooth_angles(
            &mut self.smoothed_angles,
            Vec2::new(self.yaw, self.pitch),
            dt,
        )
    }
}

pub(super) fn regression_camera(layer: usize) -> ViewCamera {
    ViewCamera {
        view: layer as u64,
        origin: GalacticPosition::default(),
        layer,
        yaw: 0.0,
        pitch: 0.0,
        distance: 100.0,
        radius: 10.0,
        private: false,
        followed: None,
        aligned_to_sun: false,
        smoothed_angles: None,
    }
}

fn smooth_angles(current: &mut Option<Vec2>, target: Vec2, dt: f32) -> Vec2 {
    let current = current.get_or_insert(target);
    let blend = -(-dt / 0.04).exp_m1();
    *current = current.lerp(target, blend);
    *current
}

pub(super) fn setup_views(
    mut commands: Commands,
    views: Query<(Entity, &ViewObservation, Option<&ViewSystems>), Without<ViewCamera>>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
) {
    for (entity, observation, systems) in &views {
        let view = &observation.0;
        let direction = sun_direction(
            view.origin,
            bodies
                .iter()
                .filter(|(_, _, system)| {
                    systems.is_some_and(|systems| systems.0.iter().any(|entry| *entry == system.0))
                })
                .map(|(body, pose, _)| (body.0.luminosity_lumens, pose.0.position)),
        );
        let aligned_to_sun = direction.is_some();
        let direction = direction.unwrap_or_else(|| Vec3::new(0.5, 0.35, 0.8).normalize());
        let yaw = direction.x.atan2(direction.z);
        let pitch = direction.y.clamp(-0.98, 0.98).asin();
        commands.entity(entity).insert((
            Camera3d::default(),
            Msaa::Off,
            bevy::anti_alias::fxaa::Fxaa::default(),
            bevy::pbr::ContactShadows::default(),
            bevy::camera::Exposure::SUNLIGHT,
            Hdr,
            // Space is black; stars are sprites drawn over the clear colour.
            Camera {
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                near: 0.1,
                far: 1e15,
                ..default()
            }),
            bevy::post_process::bloom::Bloom::NATURAL,
            ViewLayer(1),
            RenderLayers::layer(1),
            ViewCamera {
                view: view.id,
                origin: view.origin,
                layer: 1,
                yaw,
                pitch,
                distance: 100.,
                radius: 1.,
                private: false,
                followed: None,
                aligned_to_sun,
                smoothed_angles: None,
            },
            Transform::from_translation(direction * 100.).looking_at(Vec3::ZERO, Vec3::Y),
        ));
    }
}

pub(super) fn update_views(
    mut commands: Commands,
    mut cameras: Query<(
        Entity,
        &ViewObservation,
        &mut Camera,
        &mut ViewCamera,
        &mut CameraOptions,
        Option<&ViewSystems>,
    )>,
    owned: Query<(&OwnedShip, &DisplayPose)>,
    optical: Query<(&Optical, &DisplayPose)>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    windows: Query<&Window>,
    beacons: Query<(&crate::state::NavigationObject, &DisplayPose)>,
) {
    let size = windows
        .iter()
        .next()
        .map_or(UVec2::new(1280, 720), |window| {
            UVec2::new(window.physical_width(), window.physical_height())
        });
    let mut order: Vec<_> = cameras
        .iter()
        .map(|(entity, observation, ..)| (observation.0.id, entity))
        .collect();
    order.sort_unstable_by_key(|(id, _)| *id);
    let count = order.len().clamp(1, 8);
    for (index, (_, entity)) in order.into_iter().take(8).enumerate() {
        let Ok((entity, observation, mut camera, mut state, mut options, systems)) =
            cameras.get_mut(entity)
        else {
            continue;
        };
        let view = &observation.0;
        let mut origin = view
            .focused_ship
            .and_then(|id| {
                owned
                    .iter()
                    .find(|(ship, _)| ship.0.ship == id)
                    .map(|(_, pose)| pose.0.position)
            })
            .unwrap_or(view.origin);
        let own_ship = owned
            .iter()
            .find(|(ship, _)| Some(ship.0.ship) == view.focused_ship);
        if own_ship.is_some_and(|(ship, _)| ship.0.presence != osg_model::travel::Presence::Space) {
            options.focus = None;
        }
        let private = own_ship.is_some_and(|(ship, _)| {
            matches!(
                ship.0.presence,
                osg_model::travel::Presence::Docked { .. }
                    | osg_model::travel::Presence::StoredInWreck(_)
                    | osg_model::travel::Presence::Destroyed
            )
        });
        if private
            && !state.private
            && own_ship.is_some_and(|(ship, _)| {
                matches!(ship.0.presence, osg_model::travel::Presence::Docked { .. })
            })
        {
            let rotation = own_ship
                .map(|(_, pose)| Quat::from_array(pose.0.rotation.map(|n| n as f32)))
                .unwrap_or_default();
            let direction = rotation * Vec3::new(0.3, 0.18, 1.).normalize();
            state.yaw = direction.x.atan2(direction.z);
            state.pitch = direction.y.asin();
            state.aligned_to_sun = true;
            state.smoothed_angles = None;
        }
        state.private = private;
        let mut followed = view.focused_ship;
        let mut radius = own_ship.map_or(1., |(ship, _)| ship.0.radius_m) as f32;
        if let Some(focus) = options.focus {
            let selected = match focus {
                SelectedTarget::Contact(reference) => optical
                    .iter()
                    .find(|(object, pose)| {
                        object.0.contact == Some(reference)
                            && object.0.view == view.id
                            && pose.0.position.relative_to(origin).length() <= LOOK_AT_RANGE_M
                    })
                    .map(|(object, pose)| (pose.0.position, object.0.id, object.0.radius_m as f32)),
                SelectedTarget::Beacon(id) => beacons
                    .iter()
                    .find(|(beacon, pose)| {
                        beacon.0.id == id
                            && pose.0.position.relative_to(origin).length() <= LOOK_AT_RANGE_M
                            && optical.iter().any(|(object, _)| {
                                object.0.view == view.id && object.0.known_entity == Some(id)
                            })
                    })
                    .map(|(beacon, pose)| (pose.0.position, id, beacon.0.radius_m as f32)),
                SelectedTarget::Celestial(id) => bodies
                    .iter()
                    .find(|(body, _, _)| body.0.entity == id)
                    .map(|(body, pose, _)| (pose.0.position, id, body.0.radius_m as f32)),
            };
            if let Some((position, target, size)) = selected {
                origin = position;
                followed = Some(target);
                radius = size;
            } else {
                options.focus = None;
            }
        }
        if !state.aligned_to_sun {
            if let Some(direction) = sun_direction(
                origin,
                bodies
                    .iter()
                    .filter(|(_, _, system)| {
                        systems
                            .is_some_and(|systems| systems.0.iter().any(|entry| *entry == system.0))
                    })
                    .map(|(body, pose, _)| (body.0.luminosity_lumens, pose.0.position)),
            ) {
                state.yaw = direction.x.atan2(direction.z);
                state.pitch = direction.y.clamp(-0.98, 0.98).asin();
                state.aligned_to_sun = true;
            }
        }
        camera.viewport = Some(Viewport {
            physical_position: UVec2::new(size.x * index as u32 / count as u32, 0),
            physical_size: UVec2::new((size.x / count as u32).max(1), size.y.max(1)),
            ..default()
        });
        camera.order = index as isize;
        state.origin = origin;
        let relayered = state.layer != index + 1;
        state.layer = index + 1;
        state.radius = radius;
        if state.followed != followed {
            state.distance = (radius * 3.).max(30.);
            state.followed = followed;
        }
        if relayered {
            commands
                .entity(entity)
                .insert((RenderLayers::layer(index + 1), ViewLayer(index + 1)));
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct CameraDrag(bool);

pub(super) fn track_camera_drag(
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Res<crate::ui::input::InputCapture>,
    mut drag: ResMut<CameraDrag>,
) {
    if buttons.just_pressed(MouseButton::Right) {
        drag.0 = capture.mouse_available;
    } else if !buttons.pressed(MouseButton::Right) {
        drag.0 = false;
    }
}

pub(super) fn camera_controls(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    windows: Query<&Window>,
    mut cameras: Query<(&Camera, &mut ViewCamera)>,
    mut selection: ResMut<Selection>,
    drag: Res<CameraDrag>,
) {
    let cursor = windows
        .iter()
        .next()
        .and_then(Window::physical_cursor_position);
    for (camera, mut state) in &mut cameras {
        let over_view = cursor.is_some_and(|cursor| {
            camera.viewport.as_ref().is_none_or(|viewport| {
                cursor.x >= viewport.physical_position.x as f32
                    && cursor.x < (viewport.physical_position.x + viewport.physical_size.x) as f32
                    && cursor.y >= viewport.physical_position.y as f32
                    && cursor.y < (viewport.physical_position.y + viewport.physical_size.y) as f32
            })
        });
        if !over_view {
            continue;
        }
        if buttons.just_pressed(MouseButton::Left) || buttons.just_pressed(MouseButton::Right) {
            selection.view = Some(state.view);
        }
        if drag.0 && buttons.pressed(MouseButton::Right) {
            state.aligned_to_sun = true;
            state.yaw -= motion.delta.x * 0.0025;
            state.pitch = (state.pitch - motion.delta.y * 0.0025).clamp(-1.5, 1.5);
        }
        state.distance *= (-scroll.delta.y * 0.05).exp();
    }
}

pub(super) fn reset_focus(
    mut contexts: osg_ui::bevy_egui::EguiContexts,
    selection: Res<Selection>,
    mut cameras: Query<(&ViewCamera, &mut CameraOptions)>,
) -> Result {
    if contexts
        .ctx_mut()?
        .input(|input| input.key_pressed(osg_ui::egui::Key::Escape))
    {
        for (view, mut options) in &mut cameras {
            if selection.view == Some(view.view) {
                options.focus = None;
            }
        }
    }
    Ok(())
}

pub(super) fn animate_camera(
    mut cameras: Query<(&mut ViewCamera, &mut Transform)>,
    time: Res<Time<Real>>,
) {
    for (mut state, mut transform) in &mut cameras {
        state.distance = state.distance.clamp(
            (state.radius * 1.01).max(1.),
            if state.private {
                (state.radius * 10.).max(100.)
            } else {
                1e15
            },
        );
        let angles = state.smooth_angles(time.delta_secs());
        let rotation = Quat::from_rotation_y(angles.x) * Quat::from_rotation_x(-angles.y);
        *transform = Transform::from_translation(rotation * Vec3::Z * state.distance)
            .looking_at(Vec3::ZERO, Vec3::Y);
    }
}

pub(super) fn align_on_double_click(
    mut contexts: osg_ui::bevy_egui::EguiContexts,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform, &ViewCamera, &ViewObservation)>,
    ships: Query<&OwnedShip>,
    mut outgoing: ResMut<crate::state::Outgoing>,
    mut selection: ResMut<Selection>,
) -> Result {
    use osg_model::{FlightCommand, ShipCommand, travel};
    let ctx = contexts.ctx_mut()?;
    let pointer = ctx.input(|input| {
        (input
            .pointer
            .button_double_clicked(egui::PointerButton::Primary)
            && !input.pointer.secondary_down())
        .then(|| input.pointer.interact_pos())
        .flatten()
    });
    let Some(pointer) = pointer else {
        return Ok(());
    };
    let Ok(window) = windows.single() else {
        return Ok(());
    };
    let cursor = Vec2::new(pointer.x, pointer.y) * ctx.pixels_per_point() / window.scale_factor();
    for (camera, transform, view_camera, observation) in &cameras {
        if view_camera.private
            || !camera
                .logical_viewport_rect()
                .is_some_and(|rect| rect.contains(cursor))
        {
            continue;
        }
        let Some(ship) = ships
            .iter()
            .find(|ship| Some(ship.0.ship) == observation.0.focused_ship)
        else {
            continue;
        };
        if ship.0.presence != travel::Presence::Space {
            continue;
        }
        let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
            continue;
        };
        outgoing.ship(
            &ship.0,
            ShipCommand::Flight(FlightCommand::AimDirection(
                ray.direction.as_dvec3().to_array(),
            )),
        );
        selection.view = Some(view_camera.view);
        selection.ship = Some(ship.0.ship);
        break;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_preserves_initial_view_and_is_frame_rate_independent() {
        let initial = Vec2::new(0.7, 0.2);
        let target = Vec2::new(1.7, -0.3);
        let mut slow = None;
        assert_eq!(smooth_angles(&mut slow, initial, 0.), initial);
        let mut fast = slow;
        for _ in 0..3 {
            smooth_angles(&mut slow, target, 1. / 30.);
        }
        for _ in 0..12 {
            smooth_angles(&mut fast, target, 1. / 120.);
        }
        assert!(slow.unwrap().abs_diff_eq(fast.unwrap(), 1e-6));
        assert!(smooth_angles(&mut slow, target, 1.).abs_diff_eq(target, 1e-6));
    }
}
