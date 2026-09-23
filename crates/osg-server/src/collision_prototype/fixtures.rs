use super::*;
use crate::sim::{
    physics::{self, collision::CollisionBody},
    precision::PreciseTransform,
    spatial::SpatialBody,
};

pub(super) struct Fixture {
    pub bodies: Vec<Body>,
    pub sources: Vec<Source>,
    pub catalogue: Vec<Source>,
}

pub(super) fn production(players: usize, include_catalogue: bool) -> Result<Fixture> {
    let accounts: Vec<_> = (0..players)
        .map(|i| osg_model::Id((i as u128 + 1).to_le_bytes()))
        .collect();
    let mut app = crate::scenario(&accounts, accounts.first().copied(), None)?;
    for _ in 0..70 {
        app.update();
    }
    let world = app.world_mut();
    let active: BTreeSet<_> = world
        .resource::<crate::sim::orrery::activity::ActiveSystems>()
        .entities
        .keys()
        .copied()
        .collect();
    let universe = world.resource::<crate::sim::orrery::Universe>().clone();
    let catalogue: Vec<_> = universe
        .index
        .entries
        .iter()
        .enumerate()
        .filter(|_| include_catalogue)
        .map(|(i, s)| Source {
            id: (1_u64 << 63) | i as u64,
            position: s.position,
            velocity: DVec3::ZERO,
            brightness: if active.contains(&i) {
                0.0
            } else {
                s.luminosity / 2_f64.powi(64)
            },
        })
        .collect();
    let mut geometry = HashMap::new();
    let mut bodies = Vec::new();
    let mut query = world.query_filtered::<(
        Entity,
        &PreciseTransform,
        &physics::Velocity,
        &physics::AngularVelocity,
        &physics::MassProps,
        &crate::sim::vessel::ShipDesign,
    ), With<CollisionBody>>();
    for (entity, pose, velocity, angular, mass, design) in query.iter(world) {
        let shape = geometry
            .entry(Arc::as_ptr(&design.0) as usize)
            .or_insert_with(|| Arc::new(Shape::Boxes(osg_ships::collision::voxel_boxes(&design.0))))
            .clone();
        let Shape::Boxes(boxes) = &*shape else {
            unreachable!()
        };
        let radius = boxes
            .iter()
            .map(|(c, h)| (c.abs() + h).length())
            .fold(design.0.radius, f64::max);
        let mut body = Body::ball(entity.to_bits(), DVec3::ZERO, radius);
        body.position = pose.translation_um;
        body.rotation = pose.rotation;
        body.velocity = velocity.0;
        body.angular = angular.0;
        body.mass = mass.mass;
        body.inertia = mass.inertia;
        body.shape = shape;
        let systems = world.resource::<crate::sim::orrery::activity::ActiveSystems>();
        body.attractors = Arc::new(
            systems
                .systems_for_object(&universe, entity, pose.translation_um)
                .iter()
                .flat_map(|system| systems.entities.get(system).into_iter().flatten())
                .filter_map(|&entity| {
                    let state =
                        world.get::<crate::sim::orrery::activity::CelestialState>(entity)?;
                    let position = world.get::<PreciseTransform>(entity)?.translation_um;
                    Some(Attractor {
                        position,
                        mass: state.body.mass,
                        velocity: state.velocity,
                    })
                })
                .collect(),
        );
        bodies.push(body);
    }
    let mut sources = Vec::new();
    let mut query = world.query_filtered::<(
        Entity,
        &PreciseTransform,
        &SpatialBody,
        &crate::sim::orrery::activity::CelestialState,
        Option<&crate::sim::orrery::Star>,
    ), Without<CollisionBody>>();
    for (entity, pose, _, state, star) in query.iter(world) {
        sources.push(Source {
            id: entity.to_bits(),
            position: pose.translation_um,
            velocity: state.velocity,
            brightness: star.map_or(0.0, |s| s.lumens / 2_f64.powi(64)),
        });
    }
    ensure!(!bodies.is_empty(), "scenario has no collision bodies");
    Ok(Fixture {
        bodies,
        sources,
        catalogue,
    })
}

pub(super) fn scripted() -> Vec<Body> {
    let mut bodies = Vec::new();
    for group in 0..32 {
        let origin = DVec3::new(group as f64 * 1_000_000.0, 0.0, 0.0);
        let mut hull = Body::ball(group * 3, origin, 4.0);
        hull.mass = 10_000.0;
        hull.inertia = DMat3::IDENTITY * 100_000.0;
        hull.shape = Arc::new(Shape::Boxes(vec![(
            DVec3::ZERO,
            DVec3::new(0.05, 2.0, 2.0),
        )]));
        let mut projectile = Body::ball(group * 3 + 1, origin - DVec3::X * 1000.0, 0.005);
        projectile.velocity = DVec3::X * 20_000.0;
        projectile.mass = 0.001;
        projectile.inertia *= projectile.mass;
        bodies.extend([hull, projectile]);
    }
    bodies
}
