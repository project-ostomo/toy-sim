use super::{BodyClass, Celestial, Star, Universe, orrery_cfg::Body};
use crate::sim::{
    physics::{Velocity, sim_time},
    precision::PreciseTransform,
    vessel::Vessel,
};
use bevy::{math::DVec3, prelude::*};
use smol_str::SmolStr;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Component, Clone)]
pub struct CelestialState {
    pub body: Body,
    pub system: usize,
    pub anchor: crate::sim::precision::GalacticPosition,
    pub influence: f64,
    pub velocity: DVec3,
}
#[derive(Resource, Default)]
pub struct ActiveSystems {
    pub entities: BTreeMap<usize, Vec<Entity>>,
    pub ship_systems: BTreeSet<usize>,
}
#[derive(Resource, Default)]
pub struct UniverseDebug {
    pub inspect: Option<SmolStr>,
}

pub fn activate(
    mut commands: Commands,
    universe: Res<Universe>,
    mut active: ResMut<ActiveSystems>,
    debug: Res<UniverseDebug>,
    time: Res<Time<Fixed>>,
    ships: Query<(&PreciseTransform, &Velocity), With<Vessel>>,
) {
    let mut needed = BTreeSet::new();
    for (p, v) in &ships {
        needed.extend(
            universe
                .tree
                .containing_segment(p.translation_um, v.0 * time.timestep().as_secs_f64()),
        );
    }
    active.ship_systems = needed.clone();
    if let Some(name) = &debug.inspect {
        if let Some(id) = universe.system_for(name) {
            needed.insert(id);
        }
    }
    let remove: Vec<_> = active
        .entities
        .keys()
        .filter(|id| !needed.contains(id))
        .copied()
        .collect();
    for id in remove {
        for entity in active.entities.remove(&id).unwrap() {
            commands.entity(entity).despawn();
        }
    }
    let epoch = sim_time(&time) - hifitime::Duration::from_seconds(time.timestep().as_secs_f64());
    for id in needed {
        if active.entities.contains_key(&id) {
            continue;
        }
        let system = &universe.systems[id];
        let mut entities = Vec::new();
        for b in system.solver.iter() {
            let mut root = commands.spawn((
                Celestial(b.name.clone()),
                CelestialState {
                    body: b.clone(),
                    system: id,
                    anchor: system.solver.anchor,
                    influence: system.influence,
                    velocity: system.solver.solve_velocity(&b.name, epoch).unwrap(),
                },
                crate::sim::spatial::SpatialBody {
                    radius_m: b.radius,
                    occludes: true,
                },
                PreciseTransform {
                    translation_um: system.solver.solve_position(&b.name, epoch).unwrap(),
                    rotation: system.solver.solve_rotation(&b.name, epoch).unwrap(),
                },
            ));
            if let BodyClass::Star { lumens } = b.class_params {
                root.insert(Star {
                    lumens,
                    color_temp: 5000.0,
                });
            }
            entities.push(root.id());
        }
        active.entities.insert(id, entities);
    }
}

pub fn arrival(
    universe: &Universe,
    name: &str,
    epoch: hifitime::Epoch,
) -> (PreciseTransform, DVec3) {
    let body = universe.get_body(name).unwrap();
    let altitude = 1_000_000f64.max(body.atmosphere.as_ref().map_or(0.0, |a| a.height) + 100_000.0);
    let radius = body.radius + altitude;
    let velocity =
        DVec3::X * (crate::sim::physics::GRAVITATIONAL_CONSTANT * body.mass / radius).sqrt();
    let mut pose = PreciseTransform {
        translation_um: universe
            .solve_position(name, epoch)
            .unwrap()
            .offset_by(DVec3::Z * radius),
        ..default()
    };
    pose.look_to(velocity.normalize(), DVec3::Y);
    (
        pose,
        universe.solve_velocity(name, epoch).unwrap() + velocity,
    )
}
