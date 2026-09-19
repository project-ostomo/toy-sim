use super::{EffectOf, ViewCamera};
use crate::assets::{Appearance, ShipAppearance};
use crate::state::{CombatPublication, RenderTime};
use bevy::{
    camera::visibility::RenderLayers,
    math::{DQuat, DVec3},
    prelude::*,
};
use std::collections::HashSet;
use toy_sim_model::{CombatEvent, CombatEventKind, Pose};

const MAX_FRAGMENTS: usize = 4096;

#[derive(Component)]
struct Piece {
    offset: DVec3,
    velocity: DVec3,
    rotation: DQuat,
    angular: DVec3,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    temperature: f64,
    initial_temperature: f64,
    capacity: f64,
    area: f64,
    advanced: f64,
}

impl Piece {
    fn transform(&self, age: f64) -> Transform {
        Transform::from_translation((self.offset + self.velocity * age.max(0.)).as_vec3())
            .with_rotation(
                (DQuat::from_scaled_axis(self.angular * age.max(0.)) * self.rotation).as_quat(),
            )
    }

    fn cool(&mut self, age: f64) {
        if age < self.advanced {
            self.temperature = self.initial_temperature;
            self.advanced = 0.;
        }
        while self.advanced + 1e-9 < age {
            let dt = (age - self.advanced).min(1. / 240.);
            self.temperature =
                toy_sim_ships::thermal::cooled(self.temperature, self.capacity, self.area, dt);
            self.advanced += dt;
        }
    }
}

#[derive(Component)]
struct FragmentsReady;

#[derive(Component)]
#[relationship(relationship_target = FragmentVisuals)]
struct FragmentOf(Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = FragmentOf, linked_spawn)]
struct FragmentVisuals(Vec<Entity>);

#[derive(Component)]
struct FragmentVisual {
    camera: Entity,
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (prepare, cool, update).chain().after(super::update),
    );
}

fn prepare(
    mut commands: Commands,
    clock: Res<RenderTime>,
    designs: Res<Assets<ShipAppearance>>,
    publications: Query<(Entity, &CombatPublication, Option<&Appearance>), Without<FragmentsReady>>,
    pieces: Query<&Piece>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut count = pieces.iter().count();
    for (entity, publication, loaded) in &publications {
        let Some((pose, _)) = active_pose(&publication.0, clock.display_ns) else {
            continue;
        };
        let geometry = match &publication.0.kind {
            CombatEventKind::Destroyed {
                appearance,
                energy_j,
                mass_kg,
                radius_m,
                ..
            } => {
                let design = loaded
                    .and_then(|loaded| designs.get(&loaded.asset))
                    .map(|design| &design.0);
                if appearance.is_some() && design.is_none() {
                    continue;
                }
                let expected =
                    design.map_or(8, |design| design.parts.len().min(MAX_FRAGMENTS - 8) + 8);
                if count + expected > MAX_FRAGMENTS {
                    continue;
                }
                breakup(
                    &pose,
                    design,
                    *energy_j,
                    *mass_kg,
                    *radius_m,
                    &mut meshes,
                    &mut materials,
                )
            }
            CombatEventKind::Impact {
                energy_j, normal, ..
            } => {
                if count + 4 > MAX_FRAGMENTS {
                    continue;
                }
                impact(*energy_j, *normal, &mut meshes, &mut materials)
            }
            _ => continue,
        };

        count += geometry.len();
        for piece in geometry {
            commands.spawn((piece, EffectOf(entity)));
        }
        commands.entity(entity).insert(FragmentsReady);
    }
}

fn cool(
    mut commands: Commands,
    clock: Res<RenderTime>,
    publications: Query<&CombatPublication>,
    mut pieces: Query<(Entity, &EffectOf, &mut Piece)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, source, mut piece) in &mut pieces {
        let Ok(publication) = publications.get(source.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        let Some((_, age)) = active_pose(&publication.0, clock.display_ns) else {
            continue;
        };

        piece.cool(age.max(0.));
        if let Some(mut material) = materials.get_mut(&piece.material) {
            let rgb = toy_sim_ship_view::thermal::blackbody(piece.temperature) * 0.8;
            material.emissive = LinearRgba::new(rgb.x, rgb.y, rgb.z, 1.);
        }
    }
}

fn update(
    mut commands: Commands,
    clock: Res<RenderTime>,
    publications: Query<&CombatPublication>,
    pieces: Query<(Entity, &EffectOf, &Piece)>,
    cameras: Query<(Entity, &ViewCamera)>,
    mut visuals: Query<(Entity, &FragmentOf, &FragmentVisual, &mut Transform)>,
) {
    let mut existing = HashSet::new();
    let mut count = 0;
    for (entity, fragment, visual, mut transform) in &mut visuals {
        let next = pieces.get(fragment.0).ok().and_then(|(_, source, piece)| {
            let publication = publications.get(source.0).ok()?;
            let (_, camera) = cameras.get(visual.camera).ok()?;
            fragment_transform(&publication.0, piece, camera, clock.display_ns)
                .map(|transform| (transform, camera.layer))
        });
        let Some((next, layer)) = next else {
            commands.entity(entity).despawn();
            continue;
        };

        *transform = next;
        commands.entity(entity).insert(RenderLayers::layer(layer));
        existing.insert((fragment.0, visual.camera));
        count += 1;
    }

    for (piece_entity, source, piece) in &pieces {
        let Ok(publication) = publications.get(source.0) else {
            continue;
        };
        for (camera_entity, camera) in &cameras {
            if count >= MAX_FRAGMENTS {
                return;
            }
            if existing.contains(&(piece_entity, camera_entity)) {
                continue;
            }
            let Some(transform) =
                fragment_transform(&publication.0, piece, camera, clock.display_ns)
            else {
                continue;
            };

            commands.spawn((
                FragmentOf(piece_entity),
                FragmentVisual {
                    camera: camera_entity,
                },
                Mesh3d(piece.mesh.clone()),
                MeshMaterial3d(piece.material.clone()),
                transform,
                RenderLayers::layer(camera.layer),
            ));
            count += 1;
        }
    }
}

fn active_pose(event: &CombatEvent, now: u64) -> Option<(Pose, f64)> {
    let age = (now as f64 - event.sim_time_ns as f64) * 1e-9;
    match &event.kind {
        CombatEventKind::Destroyed { pose, .. } if (0. ..10.).contains(&age) => {
            Some((pose.clone(), age))
        }
        CombatEventKind::Impact {
            position,
            velocity_m_s,
            shield: false,
            ..
        } if (0. ..1.).contains(&age) => Some((
            Pose {
                position: *position,
                velocity: *velocity_m_s,
                ..default()
            },
            age,
        )),
        _ => None,
    }
}

fn fragment_transform(
    event: &CombatEvent,
    piece: &Piece,
    camera: &ViewCamera,
    now: u64,
) -> Option<Transform> {
    let (pose, age) = active_pose(event, now)?;
    let position = pose
        .position
        .offset_by(DVec3::from_array(pose.velocity) * age);
    let translation = position.relative_to(camera.origin).as_vec3();
    if translation.length() > 1e7 {
        return None;
    }

    let mut transform = piece.transform(age);
    transform.translation += translation;
    Some(transform)
}

fn piece(
    offset: DVec3,
    velocity: DVec3,
    rotation: DQuat,
    angular: DVec3,
    size: Vec3,
    color: [f32; 3],
    temperature: f64,
    capacity: f64,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Piece {
    Piece {
        offset,
        velocity,
        rotation,
        angular,
        mesh: meshes.add(Cuboid::from_size(size)),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb_from_array(color),
            perceptual_roughness: 0.8,
            emissive_exposure_weight: 1.,
            ..default()
        }),
        temperature,
        initial_temperature: temperature,
        capacity,
        area: 2. * (size.x * size.y + size.x * size.z + size.y * size.z) as f64,
        advanced: 0.,
    }
}

fn breakup(
    pose: &Pose,
    design: Option<&toy_sim_ships::appearance::PreparedAppearance>,
    energy: f64,
    mass: f64,
    radius: f64,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Vec<Piece> {
    let rotation = DQuat::from_array(pose.rotation);
    let angular = DVec3::from_array(pose.angular_velocity);
    let baseline = toy_sim_ships::thermal::INITIAL_K;
    let mut pieces = Vec::new();
    if let Some(design) = design {
        let offsets: Vec<_> = design
            .parts
            .iter()
            .map(|part| rotation * part.position)
            .collect();
        let masses: Vec<_> = design
            .parts
            .iter()
            .map(|part| part.definition.mass_kg)
            .collect();
        let velocities = breakup_velocities(&offsets, &masses, energy * 0.125);
        for (index, part) in design.parts.iter().take(MAX_FRAGMENTS - 8).enumerate() {
            let offset = offsets[index];
            pieces.push(piece(
                offset,
                angular.cross(offset) + velocities[index],
                rotation * DQuat::from_mat3(&part.rotation),
                angular,
                Vec3::from_array(
                    part.definition
                        .dimensions
                        .map(|dimension| dimension as f32 * toy_sim_ships::GRID as f32),
                ),
                part.definition.color,
                baseline,
                part.definition.mass_kg * 500.,
                meshes,
                materials,
            ));
        }
    }
    let chip_mass = (mass * 0.001).max(1e-6);
    let chip_heat = (energy * 0.1).min(chip_mass * 500. * (2500. - baseline));
    for index in 0..8 {
        let angle = index as f64 * std::f64::consts::TAU / 8.;
        let offset = rotation * DVec3::new(angle.cos(), angle.sin(), 0.) * radius;
        pieces.push(piece(
            offset,
            angular.cross(offset),
            rotation,
            angular,
            Vec3::splat((chip_mass / 8. / 7800.).cbrt() as f32),
            [0.3; 3],
            baseline + chip_heat / (chip_mass * 500.),
            chip_mass * 500. / 8.,
            meshes,
            materials,
        ));
    }
    pieces
}

fn impact(
    energy: f64,
    normal: [f64; 3],
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Vec<Piece> {
    let normal = DVec3::from_array(normal)
        .try_normalize()
        .unwrap_or(DVec3::Y);
    let tangent = normal.any_orthonormal_vector();
    let bitangent = normal.cross(tangent);
    let mass = (energy / 1e8).clamp(0.00001, 1.);
    let chip_mass = mass * 0.1;
    let speed = (2. * energy * 0.01 / chip_mass).sqrt();
    let temperature = (300. + energy * 0.01 / (chip_mass * 500.)).min(2500.);
    [
        normal + tangent,
        -normal - tangent,
        normal + bitangent,
        -normal - bitangent,
    ]
    .into_iter()
    .map(|direction| {
        piece(
            DVec3::ZERO,
            direction.normalize() * speed,
            DQuat::IDENTITY,
            direction,
            Vec3::splat((chip_mass / 4. / 7800.).cbrt() as f32),
            [0.35, 0.32, 0.28],
            temperature,
            chip_mass * 500. / 4.,
            meshes,
            materials,
        )
    })
    .collect()
}

pub fn breakup_velocities(offsets: &[DVec3], masses: &[f64], energy: f64) -> Vec<DVec3> {
    let total = masses.iter().sum::<f64>().max(1e-9);
    let centre = offsets
        .iter()
        .zip(masses)
        .map(|(r, m)| *r * *m)
        .sum::<DVec3>()
        / total;
    let mut velocities: Vec<_> = offsets.iter().map(|r| *r - centre).collect();
    let kinetic = velocities
        .iter()
        .zip(masses)
        .map(|(v, m)| 0.5 * m * v.length_squared())
        .sum::<f64>();
    let scale = if kinetic > 0.0 {
        (energy / kinetic).sqrt()
    } else {
        0.0
    };
    for v in &mut velocities {
        *v *= scale;
    }
    velocities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_preserves_fragment_physics_across_birth_and_expiry() {
        let mut app = App::new();
        app.init_resource::<RenderTime>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, cool);
        let event = CombatEvent {
            sequence: 1,
            sim_time_ns: 1_000_000_000,
            kind: CombatEventKind::Impact {
                normal: [0., 1., 0.],
                target: None,
                position: toy_sim_model::GalacticPosition::ZERO,
                velocity_m_s: [0.; 3],
                energy_j: 1e6,
                shield: false,
            },
        };
        let publication = app
            .world_mut()
            .spawn((CombatPublication(event.clone()), FragmentsReady))
            .id();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let fragment = || {
            piece(
                DVec3::ZERO,
                DVec3::X,
                DQuat::IDENTITY,
                DVec3::ZERO,
                Vec3::ONE,
                [1.; 3],
                2000.,
                10.,
                &mut Assets::<Mesh>::default(),
                &mut Assets::<StandardMaterial>::default(),
            )
        };
        let item = piece(
            DVec3::ZERO,
            DVec3::X,
            DQuat::IDENTITY,
            DVec3::ZERO,
            Vec3::ONE,
            [1.; 3],
            2000.,
            10.,
            &mut meshes,
            &mut materials,
        );
        let entity = app.world_mut().spawn((item, EffectOf(publication))).id();
        for now in [
            1_800_000_000,
            900_000_000,
            1_150_000_000,
            2_100_000_000,
            1_150_000_000,
        ] {
            app.world_mut().resource_mut::<RenderTime>().display_ns = now;
            app.update();
            let replay = app
                .world()
                .get::<Piece>(entity)
                .expect("publication owns fragment lifetime");
            if let Some((_, age)) = active_pose(&event, now) {
                let mut fresh = fragment();
                fresh.cool(age);
                assert_eq!(replay.temperature, fresh.temperature);
            } else {
                assert!(app.world().get::<FragmentsReady>(publication).is_some());
            }
        }
        app.world_mut().despawn(publication);
        assert!(app.world().get::<Piece>(entity).is_none());
    }

    #[test]
    fn destruction_fragments_are_invisible_before_the_event() {
        let event = CombatEvent {
            sequence: 1,
            sim_time_ns: 1_000_000_000,
            kind: CombatEventKind::Destroyed {
                target: toy_sim_model::ContactRef {
                    group: toy_sim_model::Id([1; 16]),
                    track: toy_sim_model::Id([2; 16]),
                },
                pose: Pose::default(),
                appearance: None,
                energy_j: 1e6,
                mass_kg: 100.,
                radius_m: 1.,
            },
        };
        assert!(active_pose(&event, 900_000_000).is_none());
        assert!(active_pose(&event, 1_000_000_000).is_some());
    }

    #[test]
    fn removing_publication_removes_fragment_views() {
        let mut world = World::new();
        let publication = world.spawn_empty().id();
        let piece = world.spawn(EffectOf(publication)).id();
        let first_view = world.spawn(FragmentOf(piece)).id();
        let second_view = world.spawn(FragmentOf(piece)).id();

        world.despawn(publication);

        assert!(world.get_entity(piece).is_err());
        assert!(world.get_entity(first_view).is_err());
        assert!(world.get_entity(second_view).is_err());
    }

    #[test]
    fn breakup_adds_requested_energy_without_net_linear_or_angular_momentum() {
        let offsets = [DVec3::X, DVec3::Y * 3.0, DVec3::NEG_Z * 2.0];
        let masses = [10.0, 200.0, 40.0];
        let velocities = breakup_velocities(&offsets, &masses, 1e8);
        let mut linear = DVec3::ZERO;
        let mut angular = DVec3::ZERO;
        let mut energy = 0.0;
        for ((position, mass), velocity) in offsets.iter().zip(masses).zip(velocities) {
            linear += velocity * mass;
            angular += position.cross(velocity * mass);
            energy += 0.5 * mass * velocity.length_squared();
        }
        assert!(linear.length() < 1e-8);
        assert!(angular.length() < 1e-8);
        assert!((energy - 1e8).abs() < 1e-6);
    }
}
