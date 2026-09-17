mod debris;
mod tracer;

use super::ViewCamera;
use crate::state::{CombatPublication, RenderTime};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use std::collections::HashSet;
use toy_sim_model::presentation::CombatEventKind;
use toy_sim_ship_view::explosion::{ExplosionMaterial, flash_power};

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

#[derive(Component)]
struct EffectVisual {
    camera: Entity,
}

#[derive(Component)]
struct CloudMaterial(Handle<ExplosionMaterial>);

#[derive(Component, Clone, Copy)]
struct CloudPhysics {
    radius: f64,
    initial_radius: f64,
    temperature: f64,
    initial_temperature: f64,
    capacity: f64,
    expansion: f64,
    flash_j: f64,
    advanced: f64,
}

impl CloudPhysics {
    fn new(kind: &CombatEventKind) -> Option<Self> {
        let (radius, mass, temperature, kinetic, flash_j) = match kind {
            CombatEventKind::Destroyed {
                energy_j,
                mass_kg,
                radius_m,
                ..
            } => {
                let baseline = toy_sim_ships::thermal::INITIAL_K;
                let gas_mass = (mass_kg * 0.01).max(1e-6);
                let chip_mass = (mass_kg * 0.001).max(1e-6);
                let chip_heat = (energy_j * 0.10).min(chip_mass * 500. * (2500. - baseline));
                let gas_heat = energy_j * 0.74 - chip_heat;
                (
                    radius_m * 0.1,
                    gas_mass,
                    baseline + gas_heat / (gas_mass * 1000.),
                    energy_j * 0.125,
                    energy_j * 0.01,
                )
            }
            CombatEventKind::Impact { energy_j, .. } | CombatEventKind::Fired { energy_j, .. } => {
                let mass = (energy_j / 1e8).clamp(0.00001, 1.);
                (
                    0.05,
                    mass,
                    300. + energy_j * 0.1 / (mass * 1000.),
                    energy_j * 0.01,
                    energy_j * 0.01,
                )
            }
            _ => return None,
        };
        Some(Self {
            radius: radius.max(0.02),
            initial_radius: radius.max(0.02),
            temperature,
            initial_temperature: temperature,
            capacity: mass * 1000.,
            expansion: (2. * kinetic.max(0.) / mass).sqrt(),
            flash_j,
            advanced: 0.,
        })
    }

    fn advance(&mut self, age: f64) {
        if age < self.advanced {
            self.radius = self.initial_radius;
            self.temperature = self.initial_temperature;
            self.advanced = 0.;
        }
        while self.advanced + 1e-9 < age {
            let dt = (age - self.advanced).min(1. / 240.);
            let previous_radius = self.radius;
            self.radius += self.expansion * dt;
            self.temperature =
                3. + (self.temperature - 3.) * (previous_radius / self.radius).powi(2);
            let tau = 3. * (self.initial_radius / self.radius).powi(2);
            self.temperature = toy_sim_ships::thermal::cooled(
                self.temperature,
                self.capacity,
                4. * std::f64::consts::PI * self.radius.powi(2) * (1. - (-tau).exp()),
                dt,
            );
            self.advanced += dt;
        }
    }

    fn material(&self, previous_age: f64, age: f64) -> ExplosionMaterial {
        let power = flash_power(self.flash_j, previous_age, age);
        let flash_temperature: f64 = 10000.;
        let normalization = power
            / (4.
                * std::f64::consts::PI
                * self.initial_radius.powi(2)
                * toy_sim_ships::thermal::SIGMA
                * flash_temperature.powi(4));
        let rgb = toy_sim_ship_view::thermal::blackbody(self.temperature) * 0.8;
        let flash_rgb =
            toy_sim_ship_view::thermal::blackbody(flash_temperature) * normalization as f32;
        ExplosionMaterial {
            radiance: rgb.extend(0.),
            optical_depth: Vec4::new(
                (3. * (self.initial_radius / self.radius).powi(2)) as f32,
                0.,
                (self.initial_radius / self.radius) as f32,
                0.,
            ),
            flash_radiance: flash_rgb.extend(0.),
        }
    }
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(toy_sim_ship_view::explosion::ExplosionPlugin)
        .init_resource::<EffectClock>()
        .add_observer(crate::state::reset_resource::<EffectClock>)
        .add_systems(PostUpdate, (prepare, update, record_time).chain());
    debris::install(app);
    tracer::install(app);
}

fn prepare(
    mut commands: Commands,
    clock: Res<RenderTime>,
    publications: Query<(Entity, &CombatPublication), Without<CloudPhysics>>,
) {
    for (entity, publication) in &publications {
        if publication.0.sim_time_ns <= clock.display_ns
            && let Some(physics) = CloudPhysics::new(&publication.0.kind)
        {
            commands.entity(entity).insert(physics);
        }
    }
}

fn update(
    mut commands: Commands,
    clock: Res<RenderTime>,
    history: Res<EffectClock>,
    mut publications: Query<(Entity, &CombatPublication, Option<&mut CloudPhysics>)>,
    cameras: Query<(Entity, &ViewCamera)>,
    mut visuals: Query<(
        Entity,
        &EffectOf,
        &EffectVisual,
        &mut Transform,
        Option<&CloudMaterial>,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut clouds: ResMut<Assets<ExplosionMaterial>>,
) {
    let now = clock.display_ns;
    let previous_ns = history.previous_ns.unwrap_or(now);

    for (_, publication, physics) in &mut publications {
        if let Some(mut physics) = physics {
            let age = now.saturating_sub(publication.0.sim_time_ns) as f64 * 1e-9;
            physics.advance(age);
        }
    }

    let mut existing = HashSet::new();
    for (entity, source, visual, mut transform, material) in &mut visuals {
        let Ok((_, publication, physics)) = publications.get(source.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        let Ok((_, camera)) = cameras.get(visual.camera) else {
            commands.entity(entity).despawn();
            continue;
        };
        let Some(next) = visual_transform(&publication.0, camera, physics, now) else {
            commands.entity(entity).despawn();
            continue;
        };

        *transform = next;
        commands
            .entity(entity)
            .insert(RenderLayers::layer(camera.layer));
        if let Some(physics) = physics
            && let Some(mut material) = material.and_then(|handle| clouds.get_mut(&handle.0))
        {
            let previous_age = (previous_ns as f64 - publication.0.sim_time_ns as f64) * 1e-9;
            let age = (now - publication.0.sim_time_ns) as f64 * 1e-9;
            *material = physics.material(previous_age, age);
        }
        existing.insert((source.0, visual.camera));
    }

    for (source, publication, physics) in &publications {
        for (camera_entity, camera) in &cameras {
            if existing.contains(&(source, camera_entity)) {
                continue;
            }
            let Some(transform) = visual_transform(&publication.0, camera, physics, now) else {
                continue;
            };

            let mut entity = commands.spawn((
                EffectOf(source),
                EffectVisual {
                    camera: camera_entity,
                },
                transform,
                RenderLayers::layer(camera.layer),
            ));
            if let Some(physics) = physics {
                let previous_age = (previous_ns as f64 - publication.0.sim_time_ns as f64) * 1e-9;
                let age = (now - publication.0.sim_time_ns) as f64 * 1e-9;
                let material = clouds.add(physics.material(previous_age, age));
                entity.insert((
                    Mesh3d(meshes.add(Sphere::new(1.).mesh().uv(16, 8))),
                    MeshMaterial3d(material.clone()),
                    CloudMaterial(material),
                ));
            }
        }
    }
}

fn visual_transform(
    event: &toy_sim_model::CombatEvent,
    camera: &ViewCamera,
    physics: Option<&CloudPhysics>,
    now: u64,
) -> Option<Transform> {
    if event.sim_time_ns > now {
        return None;
    }
    let age = (now - event.sim_time_ns) as f64 * 1e-9;
    let (mut transform, lifetime) = match &event.kind {
        CombatEventKind::Projectile { .. } => return None,
        CombatEventKind::Fired { position, .. } => (
            Transform::from_translation(position.relative_to(camera.origin).as_vec3()),
            0.1,
        ),
        CombatEventKind::Impact {
            position,
            velocity_m_s,
            ..
        } => (
            Transform::from_translation(
                position
                    .offset_by(glam::DVec3::from_array(*velocity_m_s) * age)
                    .relative_to(camera.origin)
                    .as_vec3(),
            ),
            1.,
        ),
        CombatEventKind::Destroyed { pose, .. } => (
            Transform::from_translation(
                pose.position
                    .offset_by(glam::DVec3::from_array(pose.velocity) * age)
                    .relative_to(camera.origin)
                    .as_vec3(),
            ),
            10.,
        ),
    };
    if let Some(physics) = physics {
        transform.scale = Vec3::splat(physics.radius as f32);
    }
    (age <= lifetime && transform.translation.length() <= 1e7).then_some(transform)
}

fn record_time(clock: Res<RenderTime>, mut history: ResMut<EffectClock>) {
    history.previous_ns = Some(clock.display_ns);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_rewind_restores_the_requested_thermal_and_expansion_state() {
        let kind = CombatEventKind::Impact {
            normal: [0., 1., 0.],
            target: None,
            position: toy_sim_model::GalacticPosition::ZERO,
            velocity_m_s: [0.; 3],
            energy_j: 1e6,
            shield: false,
        };
        let mut replay = CloudPhysics::new(&kind).unwrap();
        replay.advance(0.8);
        replay.advance(0.15);
        let mut fresh = CloudPhysics::new(&kind).unwrap();
        fresh.advance(0.15);
        assert_eq!(replay.radius, fresh.radius);
        assert_eq!(replay.temperature, fresh.temperature);
        replay.advance(0.);
        assert_eq!(replay.radius, replay.initial_radius);
        assert_eq!(replay.temperature, replay.initial_temperature);
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
