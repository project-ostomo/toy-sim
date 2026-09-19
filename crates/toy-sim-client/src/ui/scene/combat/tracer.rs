use super::super::{ViewCamera, ViewMember};
use crate::state::SessionInfo;
use crate::state::{
    CombatPublication, Contact, OwnedShip, RenderTime, SpatialInstance, ViewObservation,
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{NoFrustumCulling, RenderLayers},
    math::DVec3,
    mesh::PrimitiveTopology,
    prelude::*,
};
use std::collections::BTreeMap;
use toy_sim_model::{CombatEvent, CombatEventKind, GalacticPosition, Id};
use toy_sim_ship_view::tracer::{TracerMaterial, TracerPlugin};

const MAX_TRACERS: usize = 4096;

#[derive(Clone, Copy, PartialEq)]
struct HistoryKey {
    world: Option<Id>,
    generation: u64,
    focus: Option<Id>,
    spatial_instance: Option<Id>,
}

#[derive(Default)]
struct CameraHistory {
    previous: Option<(HistoryKey, u64, GalacticPosition)>,
    velocity: Option<DVec3>,
}

impl CameraHistory {
    fn sample(&mut self, key: HistoryKey, now: u64, position: GalacticPosition) -> Option<DVec3> {
        let velocity = self.previous.and_then(|(previous_key, time, previous)| {
            if previous_key != key || now < time {
                return None;
            }
            if now == time {
                return (position == previous).then_some(self.velocity).flatten();
            }
            let seconds = (now - time) as f64 * 1e-9;
            let delta = position.relative_to(previous);
            Some(delta / seconds)
        });
        self.previous = Some((key, now, position));
        self.velocity = velocity;
        velocity
    }
}

#[derive(Component, Default)]
struct ViewTracers {
    history: CameraHistory,
    mesh: Option<Handle<Mesh>>,
    entity: Option<Entity>,
}

#[derive(Resource, Default)]
struct TracerAssets(Option<Handle<TracerMaterial>>);

pub(super) fn install(app: &mut App) {
    app.add_plugins(TracerPlugin)
        .init_resource::<TracerAssets>()
        .add_systems(
            PostUpdate,
            (prepare, render)
                .chain()
                .before(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
}

fn prepare(mut commands: Commands, views: Query<Entity, (With<ViewCamera>, Without<ViewTracers>)>) {
    for entity in &views {
        commands.entity(entity).insert(ViewTracers::default());
    }
}

fn render(
    mut commands: Commands,
    clock: Res<RenderTime>,
    time: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    session: Res<SessionInfo>,
    publications: Query<&CombatPublication>,
    contacts: Query<(&Contact, Option<&SpatialInstance>)>,
    owned: Query<(&OwnedShip, Option<&SpatialInstance>)>,
    mut cameras: Query<(
        Entity,
        &ViewCamera,
        &ViewObservation,
        &Camera,
        &Transform,
        &Projection,
        &mut ViewTracers,
    )>,
    mut assets: ResMut<TracerAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TracerMaterial>>,
) {
    let now = clock.display_ns;
    let exposure_ns = (time.delta_secs_f64() / fixed.timestep().as_secs_f64()
        * clock.current_ns.saturating_sub(clock.previous_ns) as f64) as u64;
    let mut segments = BTreeMap::<u64, Vec<&CombatEvent>>::new();
    for publication in &publications {
        if let CombatEventKind::Projectile {
            id, end_time_ns, ..
        } = &publication.0.kind
        {
            if publication.0.sim_time_ns <= now && *end_time_ns >= now.saturating_sub(exposure_ns) {
                segments.entry(*id).or_default().push(&publication.0);
            }
        }
    }
    for (view_entity, view, observation, camera, transform, projection, mut state) in &mut cameras {
        let spatial_instance = view.followed.and_then(|focus| {
            owned
                .iter()
                .find(|(ship, _)| ship.0.ship == focus)
                .map(|(_, instance)| instance.map(|instance| instance.0))
                .unwrap_or_else(|| {
                    contacts
                        .iter()
                        .find(|(contact, _)| {
                            contact.1.group == observation.0.group
                                && (contact.0.id == focus || contact.0.entity == Some(focus))
                        })
                        .and_then(|(_, instance)| instance.map(|instance| instance.0))
                })
        });
        let key = HistoryKey {
            world: session.world,
            generation: session.generation,
            focus: view.followed,
            spatial_instance,
        };
        let position = view.origin.offset_by(transform.translation.as_dvec3());
        let velocity = state.history.sample(key, now, position);
        let fov = match projection {
            Projection::Perspective(projection) => projection.fov as f64,
            _ => 1.,
        };
        let height = camera
            .physical_viewport_size()
            .map_or(1080., |size| size.y.max(1) as f64);
        let mut vertices = Vec::new();
        let mut uv = Vec::new();
        if let Some(velocity) = velocity {
            for events in segments.values().take(MAX_TRACERS) {
                let Some((mut tail, head)) = exposure(events, now, exposure_ns, position, velocity)
                else {
                    continue;
                };
                if head.length() > 1e7 {
                    continue;
                }
                let width = (head.length() * (fov * 0.5).tan() * 2. / height * 1.5).max(0.005);
                if tail.distance_squared(head) < width * width {
                    tail += transform.rotation.as_dquat() * DVec3::Y * width;
                }
                let side = (head - tail)
                    .cross(-head)
                    .try_normalize()
                    .unwrap_or(transform.rotation.as_dquat() * DVec3::X)
                    * width
                    * 3.;
                for (point, tex) in [
                    (tail - side, [0., 0.]),
                    (head - side, [1., 0.]),
                    (head + side, [1., 1.]),
                    (tail - side, [0., 0.]),
                    (head + side, [1., 1.]),
                    (tail + side, [0., 1.]),
                ] {
                    vertices.push(point.as_vec3().to_array());
                    uv.push(tex);
                }
            }
        }
        if vertices.is_empty() {
            if let Some(entity) = state.entity {
                commands.entity(entity).insert(Visibility::Hidden);
            }
            continue;
        }
        let count = vertices.len();
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1_f32; 4]; count]);
        if let Some(handle) = &state.mesh {
            if let Some(mut existing) = meshes.get_mut(handle) {
                *existing = mesh;
            }
        } else {
            let handle = meshes.add(mesh);
            let material = assets
                .0
                .get_or_insert_with(|| {
                    materials.add(TracerMaterial {
                        emission: Vec4::new(1., 0.5, 0.15, 50000.),
                    })
                })
                .clone();
            state.entity = Some(
                commands
                    .spawn((
                        ViewMember(view_entity),
                        Mesh3d(handle.clone()),
                        MeshMaterial3d(material),
                        Transform::default(),
                        NoFrustumCulling,
                        RenderLayers::layer(view.layer),
                    ))
                    .id(),
            );
            state.mesh = Some(handle);
        }
        if let Some(entity) = state.entity {
            commands
                .entity(entity)
                .insert((Visibility::Inherited, RenderLayers::layer(view.layer)));
        }
    }
}

fn sample(events: &[&CombatEvent], time: u64) -> Option<GalacticPosition> {
    let event = events
        .iter()
        .filter(|event| {
            matches!(&event.kind, CombatEventKind::Projectile { end_time_ns, .. }
            if event.sim_time_ns <= time && time <= *end_time_ns)
        })
        .max_by_key(|event| event.sim_time_ns)?;
    let CombatEventKind::Projectile {
        start,
        end,
        end_time_ns,
        ..
    } = &event.kind
    else {
        return None;
    };
    let fraction = time.saturating_sub(event.sim_time_ns) as f64
        / end_time_ns.saturating_sub(event.sim_time_ns).max(1) as f64;
    Some(start.offset_by(end.relative_to(*start) * fraction))
}

fn exposure(
    events: &[&CombatEvent],
    now: u64,
    exposure_ns: u64,
    camera: GalacticPosition,
    velocity: DVec3,
) -> Option<(DVec3, DVec3)> {
    let birth = events.iter().map(|event| event.sim_time_ns).min()?;
    let begin = now.saturating_sub(exposure_ns).max(birth);
    let head = sample(events, now)?.relative_to(camera);
    let tail = sample(events, begin)?.relative_to(camera) + velocity * (now - begin) as f64 * 1e-9;
    Some((tail, head))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> HistoryKey {
        HistoryKey {
            world: Some(Id([1; 16])),
            generation: 1,
            focus: Some(Id([2; 16])),
            spatial_instance: Some(Id([5; 16])),
        }
    }

    #[test]
    fn exposure_is_galilean_invariant_across_motion_segments() {
        let anchor = GalacticPosition::new(10_i128.pow(28), -10_i128.pow(28), 0);
        let projectile_velocity = DVec3::new(80_000., 20_000., -1_000.);
        let camera_velocity = DVec3::new(7_000., 300., 0.);
        let now = 101_000_000;
        let render = |boost: DVec3| {
            let point =
                |time: u64| anchor.offset_by((projectile_velocity + boost) * (time as f64 * 1e-9));
            let make = |start: u64, end: u64| CombatEvent {
                sequence: start,
                sim_time_ns: start,
                kind: CombatEventKind::Projectile {
                    id: 7,
                    source: None,
                    start: point(start),
                    end: point(end),
                    end_time_ns: end,
                    radius_m: 0.01,
                },
            };
            let first = make(0, 100_000_000);
            let second = make(100_000_000, 200_000_000);
            let camera =
                anchor.offset_by(DVec3::Z * 100. + (camera_velocity + boost) * (now as f64 * 1e-9));
            exposure(
                &[&first, &second],
                now,
                8_333_333,
                camera,
                camera_velocity + boost,
            )
            .unwrap()
        };
        let stationary = render(DVec3::ZERO);
        let boosted = render(DVec3::new(200_000., -50_000., 10_000.));
        assert!(stationary.0.distance(boosted.0) < 1e-5);
        assert!(stationary.1.distance(boosted.1) < 1e-5);
        assert!(
            (stationary.1 - stationary.0)
                .distance((projectile_velocity - camera_velocity) * 0.008333333)
                < 1e-5
        );
    }

    #[test]
    fn camera_history_resets_for_focus_spatial_lifetime_and_clock_changes() {
        for changed in [
            HistoryKey {
                focus: Some(Id([3; 16])),
                ..key()
            },
            HistoryKey {
                spatial_instance: Some(Id([6; 16])),
                ..key()
            },
            HistoryKey {
                spatial_instance: None,
                ..key()
            },
            HistoryKey {
                generation: 2,
                ..key()
            },
            HistoryKey {
                world: Some(Id([4; 16])),
                ..key()
            },
        ] {
            let mut history = CameraHistory::default();
            assert!(history.sample(key(), 0, GalacticPosition::ZERO).is_none());
            let position = GalacticPosition::ZERO.offset_by(DVec3::X * 10.);
            assert_eq!(
                history.sample(key(), 1_000_000_000, position),
                Some(DVec3::X * 10.)
            );
            assert!(history.sample(changed, 2_000_000_000, position).is_none());
            assert_eq!(
                history.sample(changed, 3_000_000_000, position),
                Some(DVec3::ZERO)
            );
        }
        let mut history = CameraHistory::default();
        history.sample(key(), 1_000_000_000, GalacticPosition::ZERO);
        assert!(history.sample(key(), 0, GalacticPosition::ZERO).is_none());
        assert_eq!(
            history.sample(
                key(),
                1_000_000_000,
                GalacticPosition::ZERO.offset_by(DVec3::X * 1e12)
            ),
            Some(DVec3::X * 1e12),
        );
    }

    #[test]
    fn paused_camera_preserves_motion_estimate_until_the_camera_moves() {
        let mut history = CameraHistory::default();
        history.sample(key(), 0, GalacticPosition::ZERO);
        let position = GalacticPosition::ZERO.offset_by(DVec3::Y * 5.);
        let velocity = history.sample(key(), 1_000_000_000, position);
        assert_eq!(velocity, Some(DVec3::Y * 5.));
        assert_eq!(history.sample(key(), 1_000_000_000, position), velocity);
        assert!(
            history
                .sample(key(), 1_000_000_000, position.offset_by(DVec3::X))
                .is_none()
        );
    }
}
