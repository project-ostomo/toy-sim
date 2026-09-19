use super::{Dormant, Gate};
use crate::sim::{
    precision::{GalacticPosition, PreciseTransform},
    spatial::SpatialBody,
    spatial_tree::bounds,
};
use bevy::{math::DVec3, prelude::*};
use parry3d_f64::{
    bounding_volume::Aabb,
    partitioning::{Bvh, BvhBuildStrategy},
};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub struct TravelGeometry {
    tree: Bvh,
    anchor: GalacticPosition,
    entities: Vec<Entity>,
    slots: HashMap<Entity, u32>,
}

fn aabb(anchor: GalacticPosition, position: GalacticPosition, radius: f64) -> Aabb {
    let center = position.relative_to(anchor);
    let padding = center.abs().max_element() * f64::EPSILON * 8. + 1e-4;
    let extent = DVec3::splat(radius + padding);
    bounds(center - extent, center + extent)
}

pub fn refresh(world: &mut World) {
    let mut query = world.query_filtered::<(
        Entity,
        &PreciseTransform,
        Option<&SpatialBody>,
        Option<&Gate>,
    ), Without<Dormant>>();
    let entries: Vec<_> = query
        .iter(world)
        .filter_map(|(entity, transform, body, gate)| {
            let radius = extent(body, gate)?;
            Some((entity, transform.translation_um, radius))
        })
        .collect();
    let anchor = entries
        .first()
        .map_or(GalacticPosition::ZERO, |entry| entry.1);
    let mut geometry = TravelGeometry {
        anchor,
        ..Default::default()
    };
    let leaves: Vec<_> = entries
        .into_iter()
        .enumerate()
        .map(|(slot, (entity, position, radius))| {
            geometry.entities.push(entity);
            geometry.slots.insert(entity, slot as u32);
            (slot, aabb(anchor, position, radius))
        })
        .collect();
    if leaves.len() <= 2 {
        for (slot, bounds) in leaves {
            geometry.tree.insert(bounds, slot as u32);
        }
    } else {
        geometry.tree = Bvh::from_iter(BvhBuildStrategy::Binned, leaves);
    }
    world.insert_resource(geometry);
}

fn extent(body: Option<&SpatialBody>, gate: Option<&Gate>) -> Option<f64> {
    let physical = body.map(|body| body.radius_m);
    let exclusion = gate
        .filter(|gate| gate.enabled)
        .map(|gate| gate.exclusion_m);
    match (physical, exclusion) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(radius), None) | (None, Some(radius)) => Some(radius),
        (None, None) => None,
    }
}

pub fn update(world: &mut World, entity: Entity) {
    if !world.contains_resource::<TravelGeometry>() {
        return;
    }
    let proxy = if world.get::<Dormant>(entity).is_some() {
        None
    } else {
        world.get::<PreciseTransform>(entity).and_then(|transform| {
            extent(world.get::<SpatialBody>(entity), world.get::<Gate>(entity))
                .map(|radius| (transform.translation_um, radius))
        })
    };
    let mut geometry = world.resource_mut::<TravelGeometry>();
    if let Some((position, radius)) = proxy {
        let slot = if let Some(&slot) = geometry.slots.get(&entity) {
            slot
        } else {
            let slot = geometry.entities.len() as u32;
            geometry.entities.push(entity);
            geometry.slots.insert(entity, slot);
            slot
        };
        let bounds = aabb(geometry.anchor, position, radius);
        geometry.tree.insert(bounds, slot);
    } else if let Some(&slot) = geometry.slots.get(&entity) {
        geometry.tree.remove(slot);
    }
}

pub fn candidates(
    world: &mut World,
    position: GalacticPosition,
    radius: f64,
) -> Option<Vec<Entity>> {
    if !world.contains_resource::<TravelGeometry>() {
        refresh(world);
    }
    let geometry = world.resource::<TravelGeometry>();
    let bounds = aabb(geometry.anchor, position, radius);
    let mut candidates = Vec::new();
    for slot in geometry.tree.intersect_aabb(&bounds) {
        if candidates.len() >= 4096 {
            return None;
        }
        candidates.push(geometry.entities[slot as usize]);
    }
    Some(candidates)
}

pub fn mouth_candidates(world: &World, position: GalacticPosition, radius: f64) -> Vec<Entity> {
    let geometry = world.resource::<TravelGeometry>();
    let bounds = aabb(geometry.anchor, position, radius);
    geometry
        .tree
        .intersect_aabb(&bounds)
        .map(|slot| geometry.entities[slot as usize])
        .filter(|&entity| world.get::<Gate>(entity).is_some_and(|gate| gate.enabled))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_extents_have_one_leaf_each_and_updates_move_queries_immediately() {
        let mut world = World::new();
        let star = world
            .spawn((
                PreciseTransform::default(),
                SpatialBody {
                    radius_m: 1e12,
                    occludes: true,
                },
            ))
            .id();
        let gate = world
            .spawn((
                PreciseTransform::default(),
                Gate {
                    paired: toy_sim_model::Id::new(),
                    radius_m: 100.,
                    exclusion_m: 1e7,
                    enabled: true,
                    public: true,
                    allowed: Default::default(),
                },
            ))
            .id();
        refresh(&mut world);
        assert_eq!(world.resource::<TravelGeometry>().tree.leaf_count(), 2);
        assert_eq!(
            candidates(&mut world, GalacticPosition::ZERO, 1.)
                .unwrap()
                .len(),
            2
        );
        world.entity_mut(star).insert(Dormant);
        update(&mut world, star);
        let far = GalacticPosition::ZERO.offset_by(DVec3::X * 1e15);
        world
            .get_mut::<PreciseTransform>(gate)
            .unwrap()
            .translation_um = far;
        update(&mut world, gate);
        assert!(
            candidates(&mut world, GalacticPosition::ZERO, 1.)
                .unwrap()
                .is_empty()
        );
        assert_eq!(candidates(&mut world, far, 1.).unwrap(), vec![gate]);
    }

    #[test]
    fn galactic_roundoff_does_not_hide_a_small_nearby_obstacle() {
        let mut world = World::new();
        world.spawn((
            PreciseTransform::default(),
            SpatialBody {
                radius_m: 1.,
                occludes: true,
            },
        ));
        let position = GalacticPosition::ZERO.offset_by(DVec3::X * 1e20);
        let obstacle = world
            .spawn((
                PreciseTransform {
                    translation_um: position,
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 1.,
                    occludes: true,
                },
            ))
            .id();
        refresh(&mut world);
        assert!(
            candidates(&mut world, position.offset_by(DVec3::X * 0.5), 0.1)
                .unwrap()
                .contains(&obstacle)
        );
    }
}
