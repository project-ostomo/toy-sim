use super::{ViewCamera, ViewLayer, ViewMember};
use crate::state::{
    CombatPublication, DisplayPose, OwnedShip, PresentationSet, RenderTime, ShipDetails,
    ViewObservation,
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
use osg_model::{CombatEventKind, Id, Pose, travel::Presence};
use std::collections::HashSet;

#[cfg(test)]
mod tests;

const TRANSITION_S: f32 = 1.25;

#[derive(Component, Default)]
pub(super) struct SlipView {
    ship: Option<Id>,
    transit: Option<Id>,
    previous_ns: u64,
    pub coverage: f32,
    pub entering: bool,
    pub anchor: Option<Pose>,
    direction: Vec3,
    speed: f64,
    flow: f64,
    flow_step: f32,
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
        let Some((ship, pose, details)) = ships
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
            let mut initialized = false;
            if let Some(telemetry) = details.and_then(|details| details.0.slip_transit.as_ref()) {
                state.direction =
                    Vec3::from_array(telemetry.direction.map(|v| v as f32)).normalize_or_zero();
                state.speed = telemetry.speed_ly_s;
                if state.transit != Some(transit) {
                    let age = clock.display_ns.saturating_sub(telemetry.departed_ns) as f32 * 1e-9;
                    state.coverage = (age / TRANSITION_S).clamp(0.0, 1.0);
                    initialized = true;
                }
            }
            state.transit = Some(transit);
            if !initialized {
                state.coverage = (state.coverage + dt as f32 / TRANSITION_S).min(1.0);
            }
            state.entering = state.coverage < 1.0 && state.anchor.is_some();
        } else {
            state.coverage = (state.coverage - dt as f32 / TRANSITION_S).max(0.0);
            state.entering = false;
            if state.coverage == 0.0 {
                state.anchor = Some(pose.0.clone());
                state.transit = None;
            }
        }
        let flow_step = dt * state.speed / 0.01;
        state.flow += flow_step;
        state.flow_step = flow_step as f32;
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct SlipMaterial {
    // Flow phase, coverage / event age, mode (tunnel, sheath, trail, flash), frame motion.
    #[uniform(0)]
    parameters: Vec4,
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
    trail: Handle<Mesh>,
}

#[derive(Component)]
struct Effect {
    mode: u8,
    event: Option<u64>,
    material: Handle<SlipMaterial>,
}

#[derive(Component)]
struct SlipLight(f32);

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "slip.wgsl");
    app.add_plugins(MaterialPlugin::<SlipMaterial>::default())
        .add_systems(Startup, setup)
        .add_systems(Update, draw.in_set(PresentationSet::Render));
}

fn setup(mut commands: Commands, mut meshes: ResMut<bevy::asset::Assets<Mesh>>) {
    commands.insert_resource(Assets {
        sphere: meshes.add(Sphere::new(1.0).mesh().uv(48, 32)),
        trail: meshes.add(
            Cylinder::new(1.0, 2.0)
                .mesh()
                .resolution(24)
                .build()
                .rotated_by(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        ),
    });
}

fn draw(
    mut commands: Commands,
    clock: Res<RenderTime>,
    assets: Res<Assets>,
    views: Query<(Entity, &ViewCamera, &ViewObservation, &SlipView)>,
    ships: Query<(&OwnedShip, &DisplayPose)>,
    events: Query<&CombatPublication>,
    mut effects: Query<(Entity, &ViewMember, &Effect, &mut Transform)>,
    mut materials: ResMut<bevy::asset::Assets<SlipMaterial>>,
    mut lights: Query<(&ChildOf, &SlipLight, &mut DirectionalLight)>,
) {
    let mut retained = HashSet::new();
    for (view, camera, observation, state) in &views {
        let mut requested = Vec::new();
        if state.coverage > 0.0 {
            if let Some((ship, pose)) = ships
                .iter()
                .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship)
            {
                let pose = if state.entering {
                    state.anchor.as_ref().unwrap_or(&pose.0)
                } else {
                    &pose.0
                };
                let rotation = Quat::from_rotation_arc(
                    Vec3::NEG_Z,
                    state.direction.try_normalize().unwrap_or(Vec3::NEG_Z),
                );
                let center = pose.position.relative_to(camera.origin).as_vec3();
                for (mode, scale) in [(0, 10_000.0), (1, ship.0.radius_m as f32 * 1.4)] {
                    requested.push((
                        mode,
                        None,
                        Transform::from_translation(center)
                            .with_rotation(rotation)
                            .with_scale(Vec3::splat(scale)),
                        Vec4::new(
                            state.flow as f32,
                            state.coverage,
                            mode as f32,
                            state.flow_step,
                        ),
                    ));
                }
            }
        }
        if !camera.private {
            for event in events
                .iter()
                .filter(|event| {
                    matches!(event.0.kind, CombatEventKind::Slip { .. })
                        && clock.display_ns >= event.0.sim_time_ns
                        && clock.display_ns - event.0.sim_time_ns <= 3_000_000_000
                })
                .take(64)
            {
                let CombatEventKind::Slip {
                    position,
                    direction,
                    radius_m,
                    arriving,
                } = event.0.kind
                else {
                    continue;
                };
                let age = clock.display_ns.saturating_sub(event.0.sim_time_ns) as f32 * 1e-9;
                if clock.display_ns < event.0.sim_time_ns || age > 3.0 {
                    continue;
                }
                let axis = Vec3::from_array(direction.map(|v| v as f32)).normalize();
                let origin = position.relative_to(camera.origin).as_vec3();
                let radius = (radius_m as f32).max(8.0);
                let length = 512.0 * (age / 0.2).clamp(0.05, 1.0);
                let center = origin + axis * length * if arriving { -0.5 } else { 0.5 };
                requested.push((
                    2,
                    Some(event.0.sequence),
                    Transform::from_translation(center)
                        .with_rotation(Quat::from_rotation_arc(Vec3::Z, axis))
                        .with_scale(Vec3::new(radius * 0.4, radius * 0.4, length * 0.5)),
                    Vec4::new(age, age, 2.0, 1.0),
                ));
                if age < 0.35 {
                    requested.push((
                        3,
                        Some(event.0.sequence),
                        Transform::from_translation(origin)
                            .with_scale(Vec3::splat(radius * (1.0 + age * 15.0))),
                        Vec4::new(age, age, 3.0, 1.0),
                    ));
                }
            }
        }
        for (mode, event, transform, parameters) in requested {
            if let Some((entity, _, effect, mut current)) =
                effects.iter_mut().find(|(_, member, effect, _)| {
                    member.0 == view && effect.mode == mode && effect.event == event
                })
            {
                *current = transform;
                if let Some(mut material) = materials.get_mut(&effect.material) {
                    material.parameters = parameters;
                }
                retained.insert(entity);
            } else {
                let material = materials.add(SlipMaterial { parameters });
                let entity = commands
                    .spawn((
                        Effect {
                            mode,
                            event,
                            material: material.clone(),
                        },
                        ViewMember(view),
                        ViewLayer(camera.layer),
                        RenderLayers::layer(camera.layer),
                        Mesh3d(if mode == 2 {
                            assets.trail.clone()
                        } else {
                            assets.sphere.clone()
                        }),
                        MeshMaterial3d(material),
                        transform,
                        Visibility::default(),
                    ))
                    .id();
                if mode == 0 {
                    commands.entity(entity).with_children(|parent| {
                        for (direction, strength) in [
                            (Vec3::new(1.0, 2.0, 3.0), 1_200_000.0),
                            (Vec3::new(-1.0, -1.0, -2.0), 450_000.0),
                        ] {
                            parent.spawn((
                                SlipLight(strength),
                                RenderLayers::layer(camera.layer),
                                bevy::light::SunDisk::OFF,
                                DirectionalLight {
                                    color: Color::srgb(0.4, 0.7, 1.0),
                                    illuminance: strength * state.coverage,
                                    shadow_maps_enabled: false,
                                    ..default()
                                },
                                Transform::from_translation(direction)
                                    .looking_at(Vec3::ZERO, Vec3::Y),
                            ));
                        }
                    });
                }
                retained.insert(entity);
            }
        }
    }
    for (parent, light, mut directional) in &mut lights {
        if let Ok((_, member, _, _)) = effects.get(parent.parent()) {
            if let Ok((_, _, _, state)) = views.get(member.0) {
                directional.illuminance = light.0 * state.coverage;
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
