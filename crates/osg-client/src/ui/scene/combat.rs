mod debris;
mod tracer;

use super::{ViewCamera, ViewMember};
use crate::state::{CombatPublication, Contact, DisplayPose, RenderTime};
use bevy::{camera::visibility::RenderLayers, mesh::MeshTag, prelude::*};
use osg_model::{CombatEvent, GalacticPosition, presentation::CombatEventKind};
use osg_ship_view::explosion::{ExplosionAssets, flash_power};
use std::collections::HashMap;

const MAX_SPRITES_PER_VIEW: usize = 256;

#[derive(Resource, Default)]
struct EffectClock {
    previous_ns: Option<u64>,
}

#[derive(Component)]
#[relationship(relationship_target = EventEffects)]
struct EffectOf(Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = EffectOf, linked_spawn)]
struct EventEffects(Vec<Entity>);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Phase {
    Flash,
    Puff,
}

#[derive(Component)]
struct EffectVisual {
    camera: Entity,
    phase: Phase,
}

struct Effect {
    position: GalacticPosition,
    radius: f32,
    brightness: f32,
    tile: u32,
}

fn sample(
    event: &CombatEvent,
    phase: Phase,
    previous_ns: u64,
    now: u64,
    source_velocity: glam::DVec3,
) -> Option<Effect> {
    let age = now.checked_sub(event.sim_time_ns)? as f64 * 1e-9;
    let previous_age = (previous_ns as f64 - event.sim_time_ns as f64) * 1e-9;
    let (position, energy, scale, lifetime) = match &event.kind {
        CombatEventKind::Fired {
            position, energy_j, ..
        } => {
            if phase == Phase::Puff {
                return None;
            }
            (
                position.offset_by(source_velocity * age),
                *energy_j,
                0.5,
                0.08,
            )
        }
        CombatEventKind::Impact {
            position,
            velocity_m_s,
            energy_j,
            ..
        } => (
            position.offset_by(glam::DVec3::from_array(*velocity_m_s) * age),
            *energy_j,
            (energy_j.max(0.) / 200_000.).cbrt().clamp(0.2, 4.),
            0.45,
        ),
        CombatEventKind::Destroyed {
            pose,
            energy_j,
            radius_m,
            ..
        } => (
            pose.position
                .offset_by(glam::DVec3::from_array(pose.velocity) * age),
            *energy_j,
            (radius_m * 0.7).clamp(2., 200.),
            2.,
        ),
        CombatEventKind::Projectile { .. } | CombatEventKind::Beam { .. } => return None,
    };
    if !energy.is_finite() || energy <= 0. || !scale.is_finite() {
        return None;
    }
    let (radius, brightness, tile) = match phase {
        Phase::Flash if age < 0.08 => (
            scale * 0.5,
            2_000_000. * flash_power(1., previous_age, age) * 0.01,
            0,
        ),
        Phase::Puff if age < lifetime => {
            let t = age / lifetime;
            (
                scale * (0.2 + 1.8 * t),
                180_000. * (1. - t).powi(3),
                1 + event.sequence as u32 % 3,
            )
        }
        _ => return None,
    };
    (brightness >= 64.).then_some(Effect {
        position,
        radius: radius as f32,
        brightness: brightness as f32,
        tile,
    })
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(osg_ship_view::explosion::ExplosionPlugin)
        .init_resource::<EffectClock>()
        .add_observer(crate::state::reset_resource::<EffectClock>)
        .add_systems(
            PostUpdate,
            (update, record_time)
                .chain()
                .before(bevy::transform::TransformSystems::Propagate),
        );
    debris::install(app);
    tracer::install(app);
}

fn update(
    mut commands: Commands,
    clock: Res<RenderTime>,
    history: Res<EffectClock>,
    assets: Res<ExplosionAssets>,
    publications: Query<(Entity, &CombatPublication)>,
    contacts: Query<(&Contact, &DisplayPose)>,
    cameras: Query<(Entity, &ViewCamera, &Transform, &Camera, &Projection), Without<EffectVisual>>,
    mut visuals: Query<(
        Entity,
        &EffectOf,
        &EffectVisual,
        &mut Transform,
        &mut MeshTag,
    )>,
) {
    let now = clock.display_ns;
    let previous_ns = history.previous_ns.unwrap_or(now).min(now);
    let mut existing: HashMap<_, _> = visuals
        .iter()
        .map(|(entity, source, visual, ..)| ((source.0, visual.camera, visual.phase), entity))
        .collect();
    let mut events: Vec<_> = publications.iter().collect();
    events.sort_unstable_by_key(|(_, publication)| std::cmp::Reverse(publication.0.sequence));

    for (camera_entity, view, camera_transform, camera, projection) in &cameras {
        let Projection::Perspective(projection) = projection else {
            continue;
        };
        let size = camera
            .physical_viewport_size()
            .unwrap_or(UVec2::new(1280, 720));
        let tan = (projection.fov * 0.5).tan();
        let aspect = size.x as f32 / size.y.max(1) as f32;
        let mut count = 0;

        for &(source, publication) in &events {
            let event = &publication.0;
            let velocity = if let CombatEventKind::Fired { source, .. } = &event.kind {
                contacts
                    .iter()
                    .find(|(contact, _)| contact.1 == *source)
                    .map_or(glam::DVec3::ZERO, |(_, pose)| {
                        glam::DVec3::from_array(pose.0.velocity)
                    })
            } else {
                glam::DVec3::ZERO
            };
            for phase in [Phase::Flash, Phase::Puff] {
                if count == MAX_SPRITES_PER_VIEW {
                    break;
                }
                let Some(effect) = sample(event, phase, previous_ns, now, velocity) else {
                    continue;
                };
                let center = effect.position.relative_to(view.origin).as_vec3();
                let delta = center - camera_transform.translation;
                let local = camera_transform.rotation.inverse() * delta;
                let depth = -local.z;
                if !delta.is_finite() || depth <= projection.near || depth > 1e7 {
                    continue;
                }
                let radius = effect.radius.min(depth * tan * 0.12);
                if radius / (depth * tan) * (size.y as f32) < 0.5
                    || local.x.abs() > depth * tan * aspect + radius
                    || local.y.abs() > depth * tan + radius
                {
                    continue;
                }
                let rotation = event.sequence.wrapping_mul(2654435761) as u32 as f32
                    / u32::MAX as f32
                    * std::f32::consts::TAU;
                let transform = Transform {
                    translation: center - delta.normalize() * (radius * 0.02).min(0.1),
                    rotation: camera_transform.rotation * Quat::from_rotation_z(rotation),
                    scale: Vec3::splat(radius),
                };
                let tag = MeshTag(((effect.brightness / 64.).round() as u32) << 2 | effect.tile);
                if let Some(entity) = existing.remove(&(source, camera_entity, phase)) {
                    let (_, _, _, mut old_transform, mut old_tag) =
                        visuals.get_mut(entity).unwrap();
                    old_transform.set_if_neq(transform);
                    old_tag.set_if_neq(tag);
                    commands
                        .entity(entity)
                        .insert(RenderLayers::layer(view.layer));
                } else {
                    commands.spawn((
                        EffectOf(source),
                        ViewMember(camera_entity),
                        EffectVisual {
                            camera: camera_entity,
                            phase,
                        },
                        Mesh3d(assets.mesh.clone()),
                        MeshMaterial3d(assets.material.clone()),
                        tag,
                        transform,
                        RenderLayers::layer(view.layer),
                    ));
                }
                count += 1;
            }
            if count == MAX_SPRITES_PER_VIEW {
                break;
            }
        }
    }
    for entity in existing.into_values() {
        commands.entity(entity).despawn();
    }
}

fn record_time(clock: Res<RenderTime>, mut history: ResMut<EffectClock>) {
    history.previous_ns = Some(clock.display_ns);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impact_sprites_follow_motion_and_expire_at_extreme_energies() {
        for energy_j in [0.001, 200_000., 1e30] {
            let event = CombatEvent {
                sequence: 1,
                sim_time_ns: 1_000_000_000,
                kind: CombatEventKind::Impact {
                    normal: [0., 1., 0.],
                    target: None,
                    position: GalacticPosition::ZERO,
                    velocity_m_s: [3000., 0., 0.],
                    energy_j,
                    shield: false,
                },
            };
            assert!(sample(&event, Phase::Puff, 0, 0, glam::DVec3::ZERO).is_none());
            let puff = sample(
                &event,
                Phase::Puff,
                1_100_000_000,
                1_200_000_000,
                glam::DVec3::ZERO,
            )
            .unwrap();
            assert!(puff.radius.is_finite() && puff.radius <= 8.);
            assert!((puff.position.relative_to(GalacticPosition::ZERO).x - 600.).abs() < 1e-6);
            assert!(
                sample(
                    &event,
                    Phase::Puff,
                    1_400_000_000,
                    1_500_000_000,
                    glam::DVec3::ZERO
                )
                .is_none()
            );
        }
    }

    #[test]
    fn removing_publication_removes_all_attached_effects() {
        let mut world = World::new();
        let publication = world.spawn_empty().id();
        let first_view = world.spawn(EffectOf(publication)).id();
        let second_view = world.spawn(EffectOf(publication)).id();
        world.despawn(publication);
        assert!(world.get_entity(first_view).is_err());
        assert!(world.get_entity(second_view).is_err());
    }
}
