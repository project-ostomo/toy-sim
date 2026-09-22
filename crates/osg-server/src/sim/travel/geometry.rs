use crate::sim::{precision::GalacticPosition, spatial::SpatialIndex};
use bevy::prelude::*;
use osg_spatial_bvh::{QueryBudget, RecordKind, SpatialQuery};

/// Query the authoritative tick state. Exhaustion fails closed.
pub fn candidates(world: &World, position: GalacticPosition, radius: f64) -> Option<Vec<Entity>> {
    let index = world.get_resource::<SpatialIndex>()?;
    let mut cursor = index.service().query_dynamic(SpatialQuery::Sphere {
        centre: position.to_array(),
        radius_m: radius,
    });
    let mut result = Vec::new();
    while !cursor.is_complete() {
        let batch = cursor.advance(QueryBudget {
            max_work: 4096,
            max_results: 256,
        });
        for record in batch.objects {
            if record.kind == RecordKind::Body
                && record.distance(position.to_array()) <= radius + record.radius_m
            {
                result.push(Entity::from_bits(record.id));
                if result.len() > 4096 {
                    return None;
                }
            }
        }
        if cursor.stats().work() > 32768 {
            return None;
        }
    }
    Some(result)
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
