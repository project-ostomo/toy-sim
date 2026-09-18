use super::{ViewLayer, orbit, sky, sun_direction};
use crate::state::{
    Celestial, CelestialSystem, Contact, DisplayPose, OwnedShip, SystemSubscription,
    ViewObservation,
};
use crate::ui::{SelectedTarget, Selection};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::{
    camera::{Hdr, Viewport, visibility::RenderLayers},
    prelude::*,
};
use toy_sim_model::{GalacticPosition, Id};

pub(in crate::ui) const LOOK_AT_RANGE_M: f64 = 100_000.0;

#[derive(Component, Default)]
pub(in crate::ui) struct CameraOptions {
    pub focus: Option<SelectedTarget>,
}

#[derive(Component)]
#[require(orbit::ViewOptions, CameraOptions, sky::ViewSky, sky::ExposureSettings)]
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
}

pub(super) fn setup_views(
    mut commands: Commands,
    views: Query<(Entity, &ViewObservation, Option<&SystemSubscription>), Without<ViewCamera>>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
) {
    for (entity, observation, systems) in &views {
        let view = &observation.0;
        let direction = sun_direction(
            view.origin,
            bodies
                .iter()
                .filter(|(_, _, system)| {
                    systems.is_some_and(|systems| {
                        systems.0.iter().any(|entry| entry.system == system.0)
                    })
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
            bevy::pbr::ScreenSpaceAmbientOcclusion::default(),
            bevy::camera::Exposure::SUNLIGHT,
            Hdr,
            Camera::default(),
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
            },
            Transform::from_translation(direction * 100.).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        commands.spawn((
            ChildOf(entity),
            bevy::light::SunDisk::OFF,
            DirectionalLight {
                illuminance: 0.,
                shadow_maps_enabled: true,
                contact_shadows_enabled: true,
                ..default()
            },
            Transform::default(),
            RenderLayers::layer(1),
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
        Option<&SystemSubscription>,
    )>,
    owned: Query<(&OwnedShip, &DisplayPose)>,
    contacts: Query<(&Contact, &DisplayPose)>,
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
        if own_ship
            .is_some_and(|(ship, _)| ship.0.presence != toy_sim_model::travel::Presence::Space)
        {
            options.focus = None;
        }
        let private = own_ship
            .is_some_and(|(ship, _)| ship.0.presence != toy_sim_model::travel::Presence::Space);
        if private && !state.private {
            let rotation = own_ship
                .map(|(_, pose)| Quat::from_array(pose.0.rotation.map(|n| n as f32)))
                .unwrap_or_default();
            let direction = rotation * Vec3::new(0.3, 0.18, 1.).normalize();
            state.yaw = direction.x.atan2(direction.z);
            state.pitch = direction.y.asin();
            state.aligned_to_sun = true;
        }
        state.private = private;
        let mut followed = view.focused_ship;
        let mut radius = contacts
            .iter()
            .find(|(contact, _)| contact.0.entity == followed)
            .and_then(|(contact, _)| contact.0.radius_m)
            .unwrap_or_else(|| own_ship.map_or(1., |(ship, _)| ship.0.radius_m))
            as f32;
        if let Some(focus) = options.focus {
            let selected = match focus {
                SelectedTarget::Contact(reference) => contacts
                    .iter()
                    .find(|(contact, pose)| {
                        contact.1 == reference
                            && contact.1.group == view.group
                            && view.tracks.contains(&contact.0.id)
                            && pose.0.position.relative_to(origin).length() <= LOOK_AT_RANGE_M
                    })
                    .map(|(contact, pose)| {
                        (
                            pose.0.position,
                            reference.track,
                            contact.0.radius_m.unwrap_or(1.) as f32,
                        )
                    }),
                SelectedTarget::Beacon(id) => beacons
                    .iter()
                    .find(|(beacon, pose)| {
                        beacon.0.id == id
                            && pose.0.position.relative_to(origin).length() <= LOOK_AT_RANGE_M
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
                        systems.is_some_and(|systems| {
                            systems.0.iter().any(|entry| entry.system == system.0)
                        })
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
        state.layer = index + 1;
        state.radius = radius;
        if state.followed != followed {
            state.distance = (radius * 3.).max(30.);
            state.followed = followed;
        }
        commands
            .entity(entity)
            .insert((RenderLayers::layer(index + 1), ViewLayer(index + 1)));
    }
}

pub(super) fn camera_controls(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    windows: Query<&Window>,
    mut cameras: Query<(&Camera, &mut ViewCamera, &mut Transform, &mut CameraOptions)>,
    mut contexts: toy_sim_ui::bevy_egui::EguiContexts,
    mut selection: ResMut<Selection>,
    mut gui_drag: Local<bool>,
) {
    let cursor = windows
        .iter()
        .next()
        .and_then(Window::physical_cursor_position);
    let captured = contexts.ctx_mut().is_ok_and(|ctx| {
        ctx.egui_wants_pointer_input()
            || cursor.is_some_and(|cursor| {
                let position = toy_sim_ui::egui::pos2(
                    cursor.x / ctx.pixels_per_point(),
                    cursor.y / ctx.pixels_per_point(),
                );
                ctx.layer_id_at(position)
                    .is_some_and(|layer| layer.order >= toy_sim_ui::egui::Order::Middle)
            })
    });
    let restore_focus = contexts.ctx_mut().is_ok_and(|ctx| {
        !ctx.egui_wants_keyboard_input()
            && ctx.input(|input| input.key_pressed(toy_sim_ui::egui::Key::Escape))
    });
    if buttons.just_pressed(MouseButton::Left) || buttons.just_pressed(MouseButton::Right) {
        *gui_drag = captured;
    }
    let dragging = buttons.pressed(MouseButton::Left) || buttons.pressed(MouseButton::Right);
    if !dragging {
        *gui_drag = false;
    }
    for (camera, mut state, mut transform, mut options) in &mut cameras {
        if restore_focus && selection.view == Some(state.view) {
            options.focus = None;
        }
        let over_view = cursor.is_some_and(|cursor| {
            camera.viewport.as_ref().is_none_or(|viewport| {
                cursor.x >= viewport.physical_position.x as f32
                    && cursor.x < (viewport.physical_position.x + viewport.physical_size.x) as f32
                    && cursor.y >= viewport.physical_position.y as f32
                    && cursor.y < (viewport.physical_position.y + viewport.physical_size.y) as f32
            })
        });
        if over_view && !captured {
            if buttons.just_pressed(MouseButton::Left) || buttons.just_pressed(MouseButton::Right) {
                selection.view = Some(state.view);
            }
            if dragging && !*gui_drag {
                state.aligned_to_sun = true;
                state.yaw -= motion.delta.x * 0.0025;
                state.pitch = (state.pitch - motion.delta.y * 0.0025).clamp(-1.5, 1.5);
            }
            state.distance *= (-scroll.delta.y * 0.05).exp();
        }
        state.distance = state.distance.clamp(
            (state.radius * 1.01).max(1.),
            if state.private {
                (state.radius * 10.).max(100.)
            } else {
                1e15
            },
        );
        let rotation = Quat::from_rotation_y(state.yaw) * Quat::from_rotation_x(-state.pitch);
        *transform = Transform::from_translation(rotation * Vec3::Z * state.distance)
            .looking_at(Vec3::ZERO, Vec3::Y);
    }
}
