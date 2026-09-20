use super::orrery::Universe;
use bevy::prelude::*;
use osg_model::{Id, Pose, travel::CelestialRef};
use std::sync::Arc;

#[derive(Resource, Clone)]
pub struct UniverseRegistry {
    pub universe: Arc<osg_universe::universe::Universe>,
}

pub fn initialize(world: &mut World) -> anyhow::Result<()> {
    let universe = world.resource::<Universe>().0.clone();
    world.insert_resource(UniverseRegistry { universe });
    Ok(())
}

pub fn universe_reference(reference: CelestialRef) -> osg_universe::universe::CelestialId {
    osg_universe::universe::CelestialId {
        system: reference.system.0,
        local: reference.body.0,
    }
}

pub fn model_reference(reference: osg_universe::universe::CelestialId) -> CelestialRef {
    CelestialRef {
        system: Id(reference.system),
        body: Id(reference.local),
    }
}

/// Identity for an instantiated celestial entity, distinct from its local body key.
pub fn celestial_identity(reference: CelestialRef) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame celestial entity v2");
    hash.update(&reference.system.0);
    hash.update(&reference.body.0);
    Id(hash.finalize().as_bytes()[..16].try_into().unwrap())
}

impl UniverseRegistry {
    pub fn pose(&self, reference: CelestialRef, epoch: hifitime::Epoch) -> Option<Pose> {
        let reference = universe_reference(reference);
        let position = self.universe.solve_position(reference, epoch)?;
        let velocity = self.universe.solve_velocity(reference, epoch)?;
        let rotation = self.universe.solve_rotation(reference, epoch)?;
        let dt = hifitime::Duration::from_seconds(0.01);
        let next = self.universe.solve_rotation(reference, epoch + dt)?;
        let angular_velocity = (next * rotation.inverse()).to_scaled_axis() / 0.01;
        Some(Pose {
            position,
            rotation: rotation.to_array(),
            velocity: velocity.to_array(),
            angular_velocity: angular_velocity.to_array(),
        })
    }
}
