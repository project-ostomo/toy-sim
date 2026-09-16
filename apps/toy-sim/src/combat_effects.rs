//! Presentation of authoritative combat events. These entities never collide.
use crate::{
    camera::{CameraFocus, MainCamera},
    physics::collision::{Destruction, ImpactEvent, MotionSegment, Report},
    precision::{
        GalacticPosition, PreciseTransform, PrecisionSystems, PresentationPose, presentation_time,
    },
    vessel::{ShipDesign, ShipHardware},
};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use std::collections::{HashMap, VecDeque};
use toy_sim_ship_view::{
    explosion::{ExplosionMaterial, flash_power},
    thermal::blackbody,
    tracer::TracerMaterial,
};

#[derive(Clone)]
struct Trace {
    birth: f64,
    death: Option<f64>,
}

#[derive(Resource, Default)]
pub struct CombatHistory {
    motion: HashMap<Entity, VecDeque<MotionSegment>>,
    traces: HashMap<Entity, Trace>,
    impacts: Vec<ImpactEvent>,
    previous_camera: Option<(f64, GalacticPosition)>,
    previous_frame: f64,
}

#[derive(Component)]
pub struct Explosion {
    pub velocity: DVec3,
    pub age: f64,
    pub lifetime: f64,
    pub birth_s: f64,
    pub origin: GalacticPosition,
    pub source: Entity,
    pub rotation: DQuat,
}

#[derive(Component)]
struct Cloud {
    birth: f64,
    radius: f64,
    initial_radius: f64,
    expansion: f64,
    temperature: f64,
    capacity: f64,
    advanced: f64,
    flash_j: f64,
    material: Handle<ExplosionMaterial>,
}

#[derive(Component)]
struct Fragment {
    birth: f64,
    offset: DVec3,
    velocity: DVec3,
    rotation: DQuat,
    angular: DVec3,
    temperature: f64,
    capacity: f64,
    area: f64,
    advanced: f64,
    material: Handle<StandardMaterial>,
}

#[derive(Component)]
struct IntactPart {
    offset: DVec3,
    rotation: DQuat,
}

#[derive(Resource)]
struct TraceMesh {
    mesh: Handle<Mesh>,
    entity: Entity,
}

pub fn install(app: &mut App) {
    app.init_resource::<CombatHistory>()
        .add_systems(
            PostUpdate,
            (sample_motion, animate_explosions)
                .chain()
                .after(PrecisionSystems::Interpolate)
                .before(PrecisionSystems::Camera),
        )
        .add_systems(
            PostUpdate,
            render_effects
                .after(PrecisionSystems::Camera)
                .before(PrecisionSystems::Rebase),
        );
}

pub fn ingest(world: &mut World, report: &Report, epoch: f64) {
    world.init_resource::<CombatHistory>();
    let mut history = world.resource_mut::<CombatHistory>();
    // Bound history even in headless runs or when rendering is paused.
    for segments in history.motion.values_mut() {
        while segments.front().is_some_and(|s| s.end < epoch - 0.5) {
            segments.pop_front();
        }
    }
    history.motion.retain(|_, segments| !segments.is_empty());
    history.traces.retain(|_, trace| {
        epoch < trace.birth + 4.5 && trace.death.is_none_or(|death| epoch < death + 0.5)
    });
    history.impacts.retain(|impact| impact.time >= epoch - 0.5);

    for segment in &report.motion {
        let mut segment = segment.clone();
        segment.start += epoch;
        segment.end += epoch;
        history
            .motion
            .entry(segment.entity)
            .or_default()
            .push_back(segment);
    }
    for shot in &report.shots {
        history.traces.insert(
            shot.projectile,
            Trace {
                birth: epoch + shot.time,
                death: None,
            },
        );
    }
    for death in &report.destroyed {
        if let Some(trace) = history.traces.get_mut(&death.entity) {
            trace.death = Some(epoch + death.time);
        }
    }
    for impact in &report.impact_events {
        // A ship breakup already includes the deposited collision energy.
        if report.destroyed.iter().any(|death| {
            !death.projectile
                && impact.entities.contains(&death.entity)
                && (impact.time - death.time).abs() < 1e-6
        }) {
            continue;
        }
        let mut impact = impact.clone();
        impact.time += epoch;
        history.impacts.push(impact);
    }
}

fn sample(segments: &VecDeque<MotionSegment>, now: f64) -> Option<PreciseTransform> {
    let segment = segments
        .iter()
        .rev()
        .find(|s| now >= s.start - 1e-9 && now <= s.end + 1e-9)?;
    let dt = (now - segment.start).clamp(0.0, segment.end - segment.start);
    Some(PreciseTransform {
        translation_um: segment.position.offset_by(segment.velocity * dt),
        rotation: crate::physics::rotation::drift(
            segment.rotation,
            segment.momentum,
            segment.inertia_inv,
            dt,
        )
        .0,
    })
}

fn sample_motion(
    time: Res<Time<Fixed>>,
    history: Res<CombatHistory>,
    mut ships: Query<(Entity, &mut PresentationPose)>,
) {
    let now = presentation_time(&time);
    for (entity, mut pose) in &mut ships {
        if let Some(value) = history.motion.get(&entity).and_then(|s| sample(s, now)) {
            pose.0 = value;
        }
    }
}

pub fn animate_explosions(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    history: Res<CombatHistory>,
    mut objects: Query<(Entity, &mut Explosion, &mut PreciseTransform)>,
) {
    let now = presentation_time(&time);
    for (entity, mut explosion, mut pose) in &mut objects {
        explosion.age = now - explosion.birth_s;
        if explosion.age >= explosion.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        pose.translation_um = explosion
            .origin
            .offset_by(explosion.velocity * explosion.age);
        pose.rotation = DQuat::IDENTITY;
        if explosion.age < 0.0 {
            pose.rotation = explosion.rotation;
            if let Some(before) = history
                .motion
                .get(&explosion.source)
                .and_then(|s| sample(s, now))
            {
                pose.translation_um = before.translation_um;
                pose.rotation = before.rotation;
            }
        }
    }
}

pub fn destroy(world: &mut World, death: &Destruction, epoch: f64) {
    let followed = world.get::<CameraFocus>(death.entity).is_some();
    if death.projectile {
        world.despawn(death.entity);
        return;
    }
    let design = world.get::<ShipDesign>(death.entity).map(|d| d.0.clone());
    let electrical = world
        .get::<ShipHardware>(death.entity)
        .map_or(0.0, |hardware| hardware.0.inventory.energy_j);
    let baseline = toy_sim_ships::thermal::INITIAL_K;
    let energy = death.thermal.hull_energy_j
        + death.thermal.shield_energy_j
        + death.thermal.pending_waste_heat_j
        + electrical * 0.25;
    let parent = world
        .spawn((
            Explosion {
                velocity: death.velocity,
                age: -death.time,
                lifetime: 10.0,
                birth_s: epoch + death.time,
                origin: death.position,
                source: death.entity,
                rotation: death.rotation,
            },
            PreciseTransform {
                translation_um: death.position,
                rotation: DQuat::IDENTITY,
            },
            Visibility::default(),
        ))
        .id();
    if followed {
        world.entity_mut(parent).insert(CameraFocus);
        for mut camera in world
            .query_filtered::<&mut crate::camera::CameraParams, With<MainCamera>>()
            .iter_mut(world)
        {
            camera.return_from_navigation();
        }
    }
    if world.contains_resource::<Assets<Mesh>>()
        && world.contains_resource::<Assets<ExplosionMaterial>>()
    {
        let birth = epoch + death.time;
        let gas_mass = (death.mass * 0.01).max(1e-6);
        let chip_mass = (death.mass * 0.001).max(1e-6);
        let chip_heat = (energy * 0.10).min(chip_mass * 500.0 * (2500.0 - baseline));
        let gas_heat = energy * 0.64 + energy * 0.10 - chip_heat;
        spawn_cloud(
            world,
            parent,
            birth,
            death.radius * 0.1,
            gas_mass,
            baseline + gas_heat / (gas_mass * 1000.0),
            energy * 0.125,
            energy * 0.01,
        );
        if let Some(design) = &design {
            let mut offsets = Vec::new();
            let mut masses = Vec::new();
            for part in &design.parts {
                offsets.push(death.rotation * (part.centre - design.centre));
                masses.push(part.definition.mass_kg);
            }
            let velocities = breakup_velocities(&offsets, &masses, energy * 0.125);
            for (index, part) in design.parts.iter().enumerate() {
                let size = part
                    .definition
                    .dimensions
                    .map(|d| d as f32 * toy_sim_ships::GRID as f32);
                let offset = offsets[index];
                let fragment = spawn_fragment(
                    world,
                    parent,
                    birth,
                    offset,
                    death.angular_velocity.cross(offset) + velocities[index],
                    death.rotation * DQuat::from_mat3(&part.rotation),
                    death.angular_velocity,
                    Vec3::from_array(size),
                    part.definition.color,
                    baseline,
                    part.definition.mass_kg * 500.0,
                );
                world.entity_mut(fragment).insert(IntactPart {
                    offset: part.centre - design.centre,
                    rotation: DQuat::from_mat3(&part.rotation),
                });
            }
        }
        for index in 0..8 {
            let angle = index as f64 * std::f64::consts::TAU / 8.0;
            let offset = death.rotation * DVec3::new(angle.cos(), angle.sin(), 0.0) * death.radius;
            spawn_fragment(
                world,
                parent,
                birth,
                offset,
                death.angular_velocity.cross(offset),
                death.rotation,
                death.angular_velocity,
                Vec3::splat((chip_mass / 8.0 / 7800.0).cbrt() as f32),
                [0.3, 0.3, 0.3],
                baseline + chip_heat / (chip_mass * 500.0),
                chip_mass * 500.0 / 8.0,
            );
        }
    }
    world.despawn(death.entity);
}

/// Added radial velocities carry no net linear or angular momentum.
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

fn spawn_fragment(
    world: &mut World,
    parent: Entity,
    birth: f64,
    offset: DVec3,
    velocity: DVec3,
    rotation: DQuat,
    angular: DVec3,
    size: Vec3,
    color: [f32; 3],
    temperature: f64,
    capacity: f64,
) -> Entity {
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::from_size(size));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(color[0], color[1], color[2]),
            perceptual_roughness: 0.8,
            emissive_exposure_weight: 1.0,
            ..Default::default()
        });
    let area = 2.0 * (size.x * size.y + size.x * size.z + size.y * size.z) as f64;
    world
        .spawn((
            ChildOf(parent),
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(offset.as_vec3()).with_rotation(rotation.as_quat()),
            Visibility::default(),
            Fragment {
                birth,
                offset,
                velocity,
                rotation,
                angular,
                temperature,
                capacity,
                area,
                advanced: 0.0,
                material,
            },
        ))
        .id()
}

fn spawn_cloud(
    world: &mut World,
    parent: Entity,
    birth: f64,
    radius: f64,
    mass: f64,
    temperature: f64,
    kinetic: f64,
    flash_j: f64,
) {
    let radius = radius.max(0.02);
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Sphere::new(1.0).mesh().uv(24, 16));
    let material = world
        .resource_mut::<Assets<ExplosionMaterial>>()
        .add(ExplosionMaterial {
            radiance: Vec4::ZERO,
            optical_depth: Vec4::ZERO,
            flash_radiance: Vec4::ZERO,
        });
    world.spawn((
        ChildOf(parent),
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        Transform::from_scale(Vec3::splat(radius as f32)),
        Visibility::Hidden,
        Cloud {
            birth,
            radius,
            initial_radius: radius,
            expansion: (2.0 * kinetic.max(0.0) / mass).sqrt(),
            temperature,
            capacity: mass * 1000.0,
            advanced: 0.0,
            flash_j,
            material,
        },
    ));
}

fn render_effects(world: &mut World) {
    if !world.contains_resource::<Assets<ExplosionMaterial>>() {
        return;
    }
    let now = presentation_time(world.resource::<Time<Fixed>>());
    let previous = world.resource::<CombatHistory>().previous_frame;
    let impacts = std::mem::take(&mut world.resource_mut::<CombatHistory>().impacts);
    let mut pending = Vec::new();
    for impact in impacts {
        if impact.time > now {
            pending.push(impact);
            continue;
        }
        let anchor = world
            .spawn((
                Explosion {
                    velocity: impact.velocity,
                    age: now - impact.time,
                    lifetime: 1.0,
                    birth_s: impact.time,
                    origin: impact.position,
                    source: impact.entities[0],
                    rotation: DQuat::IDENTITY,
                },
                PreciseTransform {
                    translation_um: impact.position,
                    rotation: DQuat::IDENTITY,
                },
                Visibility::default(),
            ))
            .id();
        // Small impacts are localized flashes and ejecta, never ship-sized spheres.
        let mass = (impact.energy_j / 1e8).clamp(0.00001, 1.0);
        spawn_cloud(
            world,
            anchor,
            impact.time,
            0.05,
            mass,
            300.0 + impact.energy_j * 0.1 / (mass * 1000.0),
            impact.energy_j * 0.01,
            impact.energy_j * 0.01,
        );
        if !impact.shields.iter().any(|shield| *shield) {
            let normal = impact.normal.normalize_or_zero();
            let tangent = normal.any_orthonormal_vector();
            let bitangent = normal.cross(tangent);
            let chip_mass = mass * 0.1;
            let speed = (2.0 * impact.energy_j * 0.01 / chip_mass).sqrt();
            let temperature = (300.0 + impact.energy_j * 0.01 / (chip_mass * 500.0)).min(2500.0);
            for direction in [
                normal + tangent,
                -normal - tangent,
                normal + bitangent,
                -normal - bitangent,
            ] {
                spawn_fragment(
                    world,
                    anchor,
                    impact.time,
                    DVec3::ZERO,
                    direction.normalize() * speed,
                    DQuat::IDENTITY,
                    direction,
                    Vec3::splat((chip_mass / 4.0 / 7800.0).cbrt() as f32),
                    [0.35, 0.32, 0.28],
                    temperature,
                    chip_mass * 500.0 / 4.0,
                );
            }
        }
    }
    world.resource_mut::<CombatHistory>().impacts = pending;

    let mut clouds = world.query::<(&mut Cloud, &mut Transform, &mut Visibility)>();
    let mut cloud_updates = Vec::new();
    for (mut cloud, mut transform, mut visibility) in clouds.iter_mut(world) {
        let age = now - cloud.birth;
        *visibility = if age >= 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if age < 0.0 {
            continue;
        }
        while cloud.advanced + 1e-9 < age {
            let dt = (age - cloud.advanced).min(1.0 / 240.0);
            let old_radius = cloud.radius;
            cloud.radius += cloud.expansion * dt;
            cloud.temperature =
                3.0 + (cloud.temperature - 3.0) * (old_radius / cloud.radius).powi(2);
            let tau = 3.0 * (cloud.initial_radius / cloud.radius).powi(2);
            cloud.temperature = toy_sim_ships::thermal::cooled(
                cloud.temperature,
                cloud.capacity,
                4.0 * std::f64::consts::PI * cloud.radius.powi(2) * (1.0 - (-tau).exp()),
                dt,
            );
            cloud.advanced += dt;
        }
        transform.scale = Vec3::splat(cloud.radius as f32);
        let power = flash_power(cloud.flash_j, previous - cloud.birth, age);
        let flash_temperature: f64 = 10000.0;
        let normalization = power
            / (4.0
                * std::f64::consts::PI
                * cloud.initial_radius.powi(2)
                * toy_sim_ships::thermal::SIGMA
                * flash_temperature.powi(4));
        let rgb = blackbody(cloud.temperature) * 0.8;
        let flash_rgb = blackbody(flash_temperature) * normalization as f32;
        cloud_updates.push((
            cloud.material.clone(),
            rgb,
            3.0 * (cloud.initial_radius / cloud.radius).powi(2),
            flash_rgb,
            cloud.initial_radius / cloud.radius,
        ));
    }
    for (handle, rgb, tau, flash_rgb, relative_flash_radius) in cloud_updates {
        if let Some(mut material) = world
            .resource_mut::<Assets<ExplosionMaterial>>()
            .get_mut(&handle)
        {
            material.radiance = rgb.extend(0.0);
            material.optical_depth = Vec4::new(tau as f32, 0.0, relative_flash_radius as f32, 0.0);
            material.flash_radiance = flash_rgb.extend(0.0);
        }
    }
    let mut updates = Vec::new();
    for (mut fragment, mut transform, mut visibility, intact) in world
        .query::<(
            &mut Fragment,
            &mut Transform,
            &mut Visibility,
            Option<&IntactPart>,
        )>()
        .iter_mut(world)
    {
        let age = now - fragment.birth;
        *visibility = if age >= 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if age < 0.0 {
            if let Some(intact) = intact {
                *visibility = Visibility::Inherited;
                transform.translation = intact.offset.as_vec3();
                transform.rotation = intact.rotation.as_quat();
            }
            continue;
        }
        while fragment.advanced + 1e-9 < age {
            let dt = (age - fragment.advanced).min(1.0 / 240.0);
            fragment.temperature = toy_sim_ships::thermal::cooled(
                fragment.temperature,
                fragment.capacity,
                fragment.area,
                dt,
            );
            fragment.advanced += dt;
        }
        transform.translation = (fragment.offset + fragment.velocity * age).as_vec3();
        transform.rotation =
            (DQuat::from_scaled_axis(fragment.angular * age) * fragment.rotation).as_quat();
        updates.push((
            fragment.material.clone(),
            blackbody(fragment.temperature) * 0.8,
        ));
    }
    for (handle, rgb) in updates {
        if let Some(mut material) = world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&handle)
        {
            material.emissive = LinearRgba::new(rgb.x, rgb.y, rgb.z, 1.0);
        }
    }
    render_traces(world, now);
    let mut history = world.resource_mut::<CombatHistory>();
    history.previous_frame = now;
    for segments in history.motion.values_mut() {
        while segments.front().is_some_and(|s| s.end < now - 0.25) {
            segments.pop_front();
        }
    }
    history.motion.retain(|_, segments| !segments.is_empty());
    history
        .traces
        .retain(|_, trace| now < trace.birth + 4.0 && trace.death.is_none_or(|death| now < death));
}

fn render_traces(world: &mut World, now: f64) {
    use bevy::{asset::RenderAssetUsages, render::render_resource::PrimitiveTopology};
    let Some((camera, projection)) = world
        .query_filtered::<(&PreciseTransform, &Projection), With<MainCamera>>()
        .iter(world)
        .next()
        .map(|(p, projection)| (*p, projection.clone()))
    else {
        return;
    };
    let fov = match projection {
        Projection::Perspective(p) => p.fov as f64,
        _ => 1.0,
    };
    let height = world
        .query::<&Window>()
        .iter(world)
        .next()
        .map_or(1080.0, |w| w.height() as f64);
    let mut vertices = Vec::new();
    let mut uv = Vec::new();
    let mut colors = Vec::new();
    let mut history = world.resource_mut::<CombatHistory>();
    let camera_velocity = history
        .previous_camera
        .and_then(|(time, position)| {
            (now > time + 1e-8).then(|| camera.translation_um.relative_to(position) / (now - time))
        })
        .unwrap_or(DVec3::ZERO);
    history.previous_camera = Some((now, camera.translation_um));
    for (&entity, trace) in &history.traces {
        if now < trace.birth
            || now > trace.birth + 4.0
            || trace.death.is_some_and(|death| now >= death)
        {
            continue;
        }
        let end = trace.death.map_or(now, |death| now.min(death));
        let begin = (end - 0.002).max(trace.birth);
        let Some(segments) = history.motion.get(&entity) else {
            continue;
        };
        let Some(head) = sample(segments, end) else {
            continue;
        };
        let Some(tail) = sample(segments, begin) else {
            continue;
        };
        let mut a = tail.translation_um.relative_to(
            camera
                .translation_um
                .offset_by(-camera_velocity * (end - begin)),
        );
        let b = head.translation_um.relative_to(camera.translation_um);
        let width = (b.length() * (fov * 0.5).tan() * 2.0 / height * 1.5).max(0.005);
        if a.distance_squared(b) < width * width {
            a += camera.rotation * DVec3::Y * width;
        }
        let side = (b - a)
            .cross(-b)
            .try_normalize()
            .unwrap_or(camera.rotation * DVec3::X)
            * width
            * 3.0;
        let fade = 1.0;
        for (position, tex) in [
            (a - side, [0.0, 0.0]),
            (b - side, [1.0, 0.0]),
            (b + side, [1.0, 1.0]),
            (a - side, [0.0, 0.0]),
            (b + side, [1.0, 1.0]),
            (a + side, [0.0, 1.0]),
        ] {
            vertices.push(position.as_vec3().to_array());
            uv.push(tex);
            colors.push([1.0_f32, 1.0, 1.0, fade as f32]);
        }
    }
    drop(history);
    if vertices.is_empty() {
        if let Some(beam) = world.get_resource::<TraceMesh>() {
            let entity = beam.entity;
            world.entity_mut(entity).insert(Visibility::Hidden);
        }
        return;
    }
    if !world.contains_resource::<TraceMesh>() {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        ));
        let material = world
            .resource_mut::<Assets<TracerMaterial>>()
            .add(TracerMaterial {
                emission: Vec4::new(1.0, 0.5, 0.15, 50000.0),
            });
        let entity = world
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material),
                Transform::default(),
                Visibility::Hidden,
                bevy::camera::visibility::NoFrustumCulling,
            ))
            .id();
        world.insert_resource(TraceMesh { mesh, entity });
    }
    let beam = world.resource::<TraceMesh>();
    let (handle, entity) = (beam.mesh.clone(), beam.entity);
    world.entity_mut(entity).insert(if vertices.is_empty() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    });
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let mut mesh = meshes.get_mut(&handle).unwrap();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DMat3;

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

    #[test]
    fn short_lived_projectile_keeps_birth_to_impact_motion_at_large_coordinates() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let origin = GalacticPosition::from_meters(DVec3::splat(1e20));
        let mut report = Report::default();
        report.motion.push(MotionSegment {
            entity,
            start: 0.02,
            end: 0.04,
            position: origin,
            velocity: DVec3::X * 5000.0,
            rotation: DQuat::IDENTITY,
            momentum: DVec3::ZERO,
            inertia_inv: DMat3::IDENTITY,
        });
        ingest(&mut world, &report, 10.0);
        let history = world.resource::<CombatHistory>();
        let segments = &history.motion[&entity];
        assert!(sample(segments, 10.01).is_none());
        let middle = sample(segments, 10.03).unwrap();
        assert!((middle.translation_um.relative_to(origin).x - 50.0).abs() < 1e-5);
        assert!(sample(segments, 10.05).is_none());
    }

    #[test]
    fn explosion_anchor_follows_inertia_for_ten_seconds_independently_of_light() {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::from_hz(10.0))
            .init_resource::<CombatHistory>()
            .add_systems(Update, animate_explosions);
        let origin = GalacticPosition::from_meters(DVec3::splat(1e18));
        let source = app.world_mut().spawn_empty().id();
        let anchor = app
            .world_mut()
            .spawn((
                Explosion {
                    velocity: DVec3::X * 1e6,
                    age: 0.0,
                    lifetime: 10.0,
                    birth_s: 0.0,
                    origin,
                    source,
                    rotation: DQuat::IDENTITY,
                },
                PreciseTransform {
                    translation_um: origin,
                    rotation: DQuat::IDENTITY,
                },
            ))
            .id();
        for _ in 0..100 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_millis(100));
            app.update();
        }
        let pose = app.world().get::<PreciseTransform>(anchor).unwrap();
        assert!((pose.translation_um.relative_to(origin).x - 9.9e6).abs() < 1e-5);
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_millis(100));
        app.update();
        assert!(app.world().get_entity(anchor).is_err());
    }
}
