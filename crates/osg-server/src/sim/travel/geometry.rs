use crate::sim::{precision::GalacticPosition, spatial::SpatialIndex};
use bevy::prelude::*;
use osg_space::spatial::QueryBudget;

/// Query the authoritative tick state. Exhaustion fails closed.
pub fn candidates(world: &World, position: GalacticPosition, radius: f64) -> Option<Vec<Entity>> {
    let index = world.get_resource::<SpatialIndex>()?;
    let result = index
        .overlap_candidates(position, radius, &mut QueryBudget::new(32768))
        .ok()?;
    (result.len() <= 4096).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{
        precision::PreciseTransform,
        spatial::{self, SpatialBody},
        travel::Dormant,
    };
    use bevy::math::DVec3;

    #[test]
    fn geometry_retains_tick_state_until_rebuild() {
        let mut world = World::new();
        let origin = GalacticPosition::splat(1_i128 << 100);
        let entity = world
            .spawn((
                PreciseTransform {
                    translation_um: origin,
                    ..default()
                },
                SpatialBody {
                    radius_m: 1.,
                    occludes: true,
                },
            ))
            .id();
        spatial::rebuild(&mut world);
        assert_eq!(candidates(&world, origin, 0.).unwrap(), vec![entity]);
        world
            .get_mut::<PreciseTransform>(entity)
            .unwrap()
            .translation_um = origin.offset_by(DVec3::X * 100.);
        assert_eq!(candidates(&world, origin, 0.).unwrap(), vec![entity]);
        spatial::rebuild(&mut world);
        assert!(candidates(&world, origin, 0.).unwrap().is_empty());
        world.entity_mut(entity).insert(Dormant);
        spatial::rebuild(&mut world);
        assert!(candidates(&world, origin, 1000.).unwrap().is_empty());
    }
}
