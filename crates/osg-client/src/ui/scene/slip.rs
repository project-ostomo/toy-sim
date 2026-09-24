use super::{ViewCamera, ViewLayer, ViewMember};
use crate::state::{
    DisplayPose, OwnedShip, PresentationSet, RenderTime, ShipDetails, SlipEffects, ViewObservation,
};
use bevy::{
    asset::embedded_asset,
    camera::visibility::RenderLayers,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};
use osg_model::{Id, slip_visual::*, travel::Presence};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod tests;

const TRANSITION_S: f32 = 1.75;

#[derive(Component, Default)]
pub(super) struct SlipView {
    ship: Option<Id>,
    transit: Option<Id>,
    previous_ns: u64,
    progress: f32,
    departure_ns: Option<u64>,
    flash_age: Option<f32>,
    pub coverage: f32,
    pub entering: bool,
    direction: Vec3,
    flow: f64,
}

pub(super) fn prepare(
    mut commands: Commands,
    clock: Res<RenderTime>,
    views: Query<(Entity, &ViewObservation)>,
    mut states: Query<&mut SlipView>,
    ships: Query<(&OwnedShip, &DisplayPose, Option<&ShipDetails>)>,
) {
    for (entity, view) in &views {
        let Ok(mut state) = states.get_mut(entity) else {
            commands.entity(entity).insert(SlipView::default());
            continue;
        };
        let Some((ship, _pose, details)) = ships
            .iter()
            .find(|(ship, _, _)| Some(ship.0.ship) == view.0.focused_ship)
        else {
            *state = SlipView::default();
            continue;
        };
        if state.ship != Some(ship.0.ship) {
            *state = SlipView {
                ship: Some(ship.0.ship),
                previous_ns: clock.display_ns,
                ..default()
            };
        }
        if matches!(
            ship.0.presence,
            Presence::Destroyed | Presence::StoredInWreck(_) | Presence::Docked { .. }
        ) {
            *state = SlipView::default();
            continue;
        }
        let dt = clock.display_ns.saturating_sub(state.previous_ns) as f64 * 1e-9;
        state.previous_ns = clock.display_ns;
        let transit = match ship.0.presence {
            Presence::SlipTransit(id) => Some(id),
            _ => None,
        };
        if let Some(transit) = transit {
            if state.transit != Some(transit) {
                state.departure_ns = Some(
                    details
                        .and_then(|details| details.0.slip_transit.as_ref())
                        .map_or(clock.display_ns, |telemetry| telemetry.departed_ns),
                );
                state.flow = 0.0;
            }
            if let Some(telemetry) = details.and_then(|details| details.0.slip_transit.as_ref()) {
                state.direction =
                    Vec3::from_array(telemetry.direction.map(|v| v as f32)).normalize_or_zero();
            }
            state.transit = Some(transit);
            let departure = state.departure_ns.unwrap_or(clock.display_ns);
            let age = (i128::from(clock.display_ns) - i128::from(departure)) as f32 * 1e-9;
            state.progress = (age / TRANSITION_S).clamp(0.0, 1.0);
            state.flash_age = (0.0..TRANSITION_LIFETIME_S as f32)
                .contains(&age)
                .then_some(age);
            state.entering = state.progress < 1.0;
        } else {
            state.progress = (state.progress - dt as f32 / TRANSITION_S).max(0.0);
            state.flash_age = None;
            state.entering = false;
            if state.progress == 0.0 {
                state.transit = None;
            }
        }
        let previous_coverage = state.coverage;
        state.coverage = state.progress * state.progress * (3.0 - 2.0 * state.progress);
        let flow_step = dt * 3.0 * f64::from((previous_coverage + state.coverage) * 0.5);
        state.flow += flow_step;
    }
}

mod distortion;
mod streaks;
mod wakes;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Default)]
struct SlipMaterial {
    // Phase, coverage / oldest age, mode, frame motion / youngest age.
    #[uniform(0)]
    parameters: Vec4,
    // Seed, length, longitudinal noise offset, physical radius.
    #[uniform(0)]
    detail: Vec4,
}

impl Material for SlipMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://osg_client/ui/scene/slip.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        Self::vertex_shader()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_shadows() -> bool {
        false
    }

    fn enable_prepass() -> bool {
        false
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Resource)]
struct Assets {
    sphere: Handle<Mesh>,
}

#[derive(Component)]
struct Effect {
    mode: u8,
    event: Option<Id>,
    piece: (u64, u16),
    material: Handle<SlipMaterial>,
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "slip.wgsl");
    app.init_resource::<SlipEffects>()
        .add_plugins(MaterialPlugin::<SlipMaterial>::default())
        .add_systems(Startup, setup)
        .add_systems(Update, draw.in_set(PresentationSet::Render));
    distortion::install(app);
}

pub(super) fn install_regression(app: &mut App) {
    app.init_resource::<RenderTime>();
    install(app);
    app.add_systems(
        PostUpdate,
        distortion::regression.after(distortion::prepare),
    );
}

fn setup(mut commands: Commands, mut meshes: ResMut<bevy::asset::Assets<Mesh>>) {
    commands.insert_resource(Assets {
        sphere: meshes.add(Sphere::new(1.0).mesh().uv(48, 32)),
    });
}

struct Requested {
    mode: u8,
    event: Option<Id>,
    piece: (u64, u16),
    transform: Transform,
    parameters: Vec4,
    detail: Vec4,
}

fn draw(
    mut commands: Commands,
    clock: Res<RenderTime>,
    assets: Res<Assets>,
    views: Query<(Entity, &ViewCamera, &ViewObservation, &SlipView, &Transform), Without<Effect>>,
    ships: Query<(&OwnedShip, &DisplayPose)>,
    history: Res<SlipEffects>,
    rings: Query<(Entity, &osg_ship_view::slip::SlipRing, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    members: Query<&ViewMember>,
    mut effects: Query<(Entity, &ViewMember, &Effect, &mut Transform)>,
    mut materials: ResMut<bevy::asset::Assets<SlipMaterial>>,
) {
    let mut retained = HashSet::new();
    let existing: HashMap<_, _> = effects
        .iter()
        .map(|(entity, member, effect, _)| {
            ((member.0, effect.mode, effect.event, effect.piece), entity)
        })
        .collect();
    for (view, camera, observation, state, camera_transform) in &views {
        let mut requested = Vec::new();
        for (entity, ring, transform) in &rings {
            if ring.readiness <= 0.0
                || !parents
                    .iter_ancestors(entity)
                    .any(|parent| members.get(parent).is_ok_and(|member| member.0 == view))
            {
                continue;
            }
            let mut key = [0u8; 16];
            key[..8].copy_from_slice(&entity.to_bits().to_le_bytes());
            requested.push(Requested {
                mode: 4,
                event: Some(Id(key)),
                piece: (0, 0),
                transform: Transform::from_translation(transform.translation())
                    .with_rotation(transform.rotation())
                    .with_scale(Vec3::new(8.8, 8.8, 5.0)),
                parameters: Vec4::new(
                    (clock.display_ns as f64 * 1e-9 % 4096.0) as f32,
                    ring.readiness,
                    4.0,
                    0.0,
                ),
                detail: Vec4::ZERO,
            });
        }
        if !camera.private {
            let own_ship = ships
                .iter()
                .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship);
            if let (Some(age), Some((ship, pose))) = (state.flash_age, own_ship) {
                let radius = ship.0.radius_m.max(8.0) as f32;
                requested.push(Requested {
                    mode: 3,
                    event: Some(ship.0.ship),
                    piece: (0, 0),
                    transform: Transform::from_translation(
                        pose.0.position.relative_to(camera.origin).as_vec3(),
                    )
                    .with_rotation(Quat::from_rotation_arc(
                        Vec3::Z,
                        state.direction.try_normalize().unwrap_or(Vec3::Z),
                    ))
                    .with_scale(Vec3::new(20.0, 16.0, 32.0) * radius),
                    parameters: Vec4::new(age, age, 3.0, 1.0),
                    detail: Vec4::new(451.0, 1.0, 0.0, radius),
                });
            }
            for event in history
                .0
                .transitions
                .iter()
                .filter(|e| e.view == camera.view)
                .take(4)
            {
                let age = (i128::from(clock.display_ns) - i128::from(event.time_ns)) as f64 * 1e-9;
                if !(0.0..TRANSITION_LIFETIME_S).contains(&age) {
                    continue;
                }
                let position = event
                    .position
                    .offset_by(bevy::math::DVec3::from_array(event.drift_m_s) * age);
                let radius = event.radius_m.max(8.0) as f32;
                let physical_radius = radius * 20.0;
                let distance = (position.relative_to(camera.origin)
                    - camera_transform.translation.as_dvec3())
                .length() as f32;
                let visual_radius = physical_radius.max(distance * 0.003);
                let axis = Vec3::from_array(event.direction.map(|v| v as f32)).normalize_or_zero();
                requested.push(Requested {
                    mode: 3,
                    event: Some(event.id),
                    piece: (0, 0),
                    transform: Transform::from_translation(
                        position.relative_to(camera.origin).as_vec3(),
                    )
                    .with_rotation(Quat::from_rotation_arc(Vec3::Z, axis))
                    .with_scale(Vec3::new(
                        visual_radius,
                        visual_radius * 0.8,
                        visual_radius * 1.6,
                    )),
                    parameters: Vec4::new(
                        age as f32,
                        age as f32,
                        3.0,
                        if event.arriving { -1.0 } else { 1.0 },
                    ),
                    detail: Vec4::new(
                        (event.seed % 65536) as f32,
                        physical_radius / visual_radius,
                        0.0,
                        radius,
                    ),
                });
            }
        }
        for request in requested {
            let key = (view, request.mode, request.event, request.piece);
            if let Some(&entity) = existing.get(&key) {
                let (_, _, effect, mut transform) = effects.get_mut(entity).unwrap();
                *transform = request.transform;
                if let Some(mut material) = materials.get_mut(&effect.material) {
                    material.parameters = request.parameters;
                    material.detail = request.detail;
                }
                retained.insert(entity);
            } else {
                let material = materials.add(SlipMaterial {
                    parameters: request.parameters,
                    detail: request.detail,
                });
                let entity = commands
                    .spawn((
                        Effect {
                            mode: request.mode,
                            event: request.event,
                            piece: request.piece,
                            material: material.clone(),
                        },
                        ViewMember(view),
                        ViewLayer(camera.layer),
                        RenderLayers::layer(camera.layer),
                        Mesh3d(assets.sphere.clone()),
                        MeshMaterial3d(material),
                        request.transform,
                        Visibility::default(),
                        bevy::camera::visibility::NoFrustumCulling,
                    ))
                    .id();
                retained.insert(entity);
            }
        }
    }
    for (entity, _, effect, _) in &effects {
        if !retained.contains(&entity) {
            materials.remove(effect.material.id());
            commands.entity(entity).despawn();
        }
    }
}
