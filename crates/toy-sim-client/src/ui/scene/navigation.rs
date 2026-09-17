use super::{ViewCamera, orbit::ViewOptions};
use crate::state::SessionInfo;
use crate::state::{OwnedShip, PresentationSet, RenderTime, ViewObservation};
use bevy::{
    asset::RenderAssetUsages, camera::visibility::RenderLayers, mesh::PrimitiveTopology, prelude::*,
};
use toy_sim_model::{GalacticPosition, Id, Instruments, Trajectory};

const MAX_PATHS: usize = 32;
const MAX_SEGMENTS: usize = 4096;
const MAX_MARKERS: usize = 128;

#[derive(Component, Default)]
pub(super) struct NavigationDrawing {
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    origin: GalacticPosition,
    key: Option<(Id, Id, u64, u64, u32)>,
}

#[derive(Component)]
#[relationship(relationship_target = NavigationDrawings)]
struct NavigationFor(Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = NavigationFor, linked_spawn)]
struct NavigationDrawings(Vec<Entity>);

#[derive(Resource, Default)]
struct NavigationAssets(Option<Handle<StandardMaterial>>);

pub(super) fn install(app: &mut App) {
    app.init_resource::<NavigationAssets>()
        .add_systems(Update, update.in_set(PresentationSet::Render));
}

fn update(
    mut commands: Commands,
    clock: Res<RenderTime>,
    session: Res<SessionInfo>,
    mut views: Query<(
        Entity,
        &ViewObservation,
        &ViewCamera,
        &Transform,
        &ViewOptions,
        &mut NavigationDrawing,
    )>,
    ships: Query<&OwnedShip>,
    mut assets: ResMut<NavigationAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(world) = session.world else {
        return;
    };
    let now = clock.display_ns;
    let material = assets
        .0
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                unlit: true,
                base_color: Color::WHITE,
                ..default()
            })
        })
        .clone();

    for (view_entity, observation, camera, transform, options, mut drawing) in &mut views {
        let ship = observation
            .0
            .focused_ship
            .filter(|id| ships.iter().any(|ship| ship.0.ship == *id));
        let active = options.enabled && options.instruments.valid_until_ns >= now && ship.is_some();
        if !active {
            if let Some(entity) = drawing.entity {
                commands.entity(entity).insert(Visibility::Hidden);
            }
            continue;
        }
        let ship = ship.unwrap();
        let marker_size = (transform.translation.length() * 0.003).max(1.0);
        let key = (world, ship, session.sequence, now, marker_size.to_bits());
        let rebuild =
            drawing.key != Some(key) || camera.origin.relative_to(drawing.origin).length() > 1.0e6;
        if rebuild {
            let mesh = geometry(&options.instruments, camera.origin, now, marker_size);
            if let Some(handle) = drawing.mesh.as_ref() {
                if let Some(mut existing) = meshes.get_mut(handle) {
                    *existing = mesh;
                }
            } else {
                let mesh = meshes.add(mesh);
                let entity = commands
                    .spawn((
                        NavigationFor(view_entity),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform::IDENTITY,
                        RenderLayers::layer(camera.layer),
                    ))
                    .id();
                drawing.mesh = Some(mesh);
                drawing.entity = Some(entity);
            }
            drawing.origin = camera.origin;
            drawing.key = Some(key);
        }
        if let Some(entity) = drawing.entity {
            commands.entity(entity).insert((
                Transform::from_translation(drawing.origin.relative_to(camera.origin).as_vec3()),
                RenderLayers::layer(camera.layer),
                Visibility::Inherited,
            ));
        }
    }
}

fn geometry(
    instruments: &Instruments,
    origin: GalacticPosition,
    now: u64,
    marker_size: f32,
) -> Mesh {
    let mut positions = Vec::<[f32; 3]>::new();
    let mut colors = Vec::<[f32; 4]>::new();
    let mut remaining = MAX_SEGMENTS;
    for path in instruments.paths.iter().take(MAX_PATHS) {
        if path.published_at_ns > now || path.valid_until_ns < now {
            continue;
        }
        let color = if instruments
            .navigation
            .as_ref()
            .is_some_and(|navigation| navigation.target_path == Some(path.id))
        {
            [1.0, 0.65, 0.2, 1.0]
        } else {
            [0.15, 0.85, 1.0, 1.0]
        };
        append_path(
            path,
            origin,
            now,
            color,
            &mut remaining,
            &mut positions,
            &mut colors,
        );
    }

    for marker in instruments.markers.iter().take(MAX_MARKERS) {
        let center = marker.position.relative_to(origin).as_vec3();
        if !center.is_finite() {
            continue;
        }
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            positions.extend([
                (center - axis * marker_size).to_array(),
                (center + axis * marker_size).to_array(),
            ]);
            colors.extend([[1.0, 0.9, 0.35, 1.0]; 2]);
        }
    }

    Mesh::new(
        PrimitiveTopology::LineList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

fn append_path(
    path: &Trajectory,
    origin: GalacticPosition,
    now: u64,
    color: [f32; 4],
    remaining: &mut usize,
    positions: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
) {
    for pair in path.vertices.windows(2).take(MAX_SEGMENTS) {
        if *remaining == 0 {
            break;
        }
        if path.timed && pair[1].sim_time_ns < now {
            continue;
        }
        let mut start = pair[0].position.relative_to(origin);
        let end = pair[1].position.relative_to(origin);
        if path.timed && pair[0].sim_time_ns < now && pair[1].sim_time_ns > pair[0].sim_time_ns {
            let fraction = (now - pair[0].sim_time_ns) as f64
                / (pair[1].sim_time_ns - pair[0].sim_time_ns) as f64;
            start = start.lerp(end, fraction.clamp(0.0, 1.0));
        }
        let start = start.as_vec3();
        let end = end.as_vec3();
        if start.is_finite() && end.is_finite() {
            positions.extend([start.to_array(), end.to_array()]);
            colors.extend([color; 2]);
            *remaining -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::TrajectoryVertex;

    #[test]
    fn closing_a_view_removes_its_navigation_geometry() {
        let mut world = World::new();
        let view = world.spawn(NavigationDrawing::default()).id();
        let mesh = world.spawn(NavigationFor(view)).id();

        world.despawn(view);

        assert!(world.get_entity(mesh).is_err());
    }

    #[test]
    fn timed_path_starts_at_presentation_time_and_preserves_galactic_precision() {
        let origin = GalacticPosition::new(10_i128.pow(27), 0, 0);
        let path = Trajectory {
            id: 1,
            revision: 1,
            published_at_ns: 0,
            valid_until_ns: 100,
            timed: true,
            vertices: vec![
                TrajectoryVertex {
                    sim_time_ns: 0,
                    position: origin,
                },
                TrajectoryVertex {
                    sim_time_ns: 100,
                    position: origin.offset_by(Vec3::X.as_dvec3() * 10.0),
                },
            ],
        };
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let mut remaining = 1;
        append_path(
            &path,
            origin,
            50,
            [1.0; 4],
            &mut remaining,
            &mut positions,
            &mut colors,
        );
        assert_eq!(positions, [[5.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
        assert_eq!(remaining, 0);
        append_path(
            &path,
            origin,
            50,
            [1.0; 4],
            &mut remaining,
            &mut positions,
            &mut colors,
        );
        assert_eq!(positions.len(), 2);
    }
}
