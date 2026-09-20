use super::{Dormant, Gate};
use crate::sim::{
    precision::{GalacticPosition, PreciseTransform},
    spatial::SpatialBody,
};
use bevy::prelude::*;
use osg_spatial::{Entry, SpatialHash};
use std::collections::{HashMap, HashSet};

#[derive(Resource, Default)]
pub struct TravelGeometry {
    index: SpatialHash,
    entities: Vec<Entity>,
    slots: HashMap<Entity, u32>,
    free: Vec<u32>,
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
    let mut geometry = world
        .remove_resource::<TravelGeometry>()
        .unwrap_or_default();
    let live: HashSet<_> = entries.iter().map(|(entity, _, _)| *entity).collect();
    let removed: Vec<_> = geometry
        .slots
        .keys()
        .filter(|entity| !live.contains(entity))
        .copied()
        .collect();
    for entity in removed {
        geometry.remove(entity);
    }
    for (entity, position, radius) in entries {
        geometry.put(entity, position, radius);
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
        geometry.put(entity, position, radius);
    } else {
        geometry.remove(entity);
    }
}

impl TravelGeometry {
    fn put(&mut self, entity: Entity, position: GalacticPosition, radius_m: f64) {
        let slot = if let Some(&slot) = self.slots.get(&entity) {
            slot
        } else {
            let slot = self.free.pop().unwrap_or_else(|| {
                let slot = u32::try_from(self.entities.len()).expect("too many travel obstacles");
                self.entities.push(Entity::PLACEHOLDER);
                slot
            });
            self.entities[slot as usize] = entity;
            self.slots.insert(entity, slot);
            slot
        };
        self.index.insert(
            slot,
            Entry {
                position,
                radius_m,
                luminosity: 0.0,
            },
        );
    }

    fn remove(&mut self, entity: Entity) {
        if let Some(slot) = self.slots.remove(&entity) {
            self.index.remove(slot);
            self.entities[slot as usize] = Entity::PLACEHOLDER;
            self.free.push(slot);
        }
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
    let mut cursor = geometry.index.range_cursor(position, radius, true);
    let found = geometry.index.advance_range(&mut cursor, usize::MAX, 4097);
    if found.ids.len() > 4096 {
        return None;
    }
    Some(
        found
            .ids
            .into_iter()
            .map(|slot| geometry.entities[slot as usize])
            .collect(),
    )
}

pub fn mouth_candidates(world: &World, position: GalacticPosition, radius: f64) -> Vec<Entity> {
    let geometry = world.resource::<TravelGeometry>();
    geometry
        .index
        .intersecting_sphere(position, radius)
        .ids
        .into_iter()
        .map(|slot| geometry.entities[slot as usize])
        .filter(|&entity| world.get::<Gate>(entity).is_some_and(|gate| gate.enabled))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

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
                    paired: osg_model::Id::new(),
                    radius_m: 100.,
                    exclusion_m: 1e7,
                    enabled: true,
                },
            ))
            .id();
        refresh(&mut world);
        assert_eq!(world.resource::<TravelGeometry>().index.len(), 2);
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

    #[test]
    fn exclusion_queries_match_sphere_intersections_and_never_return_a_truncated_list() {
        let mut world = World::new();
        let origin = GalacticPosition::new(1 << 90, -(1 << 90), 1 << 86);
        let mut obstacles = Vec::new();
        for number in 0..128 {
            let position = origin.offset_by(DVec3::new(
                (number % 8) as f64 * 100.0,
                (number / 8) as f64 * 100.0,
                0.0,
            ));
            let radius_m = 1.0 + number as f64 * 3.0;
            let entity = world
                .spawn((
                    PreciseTransform {
                        translation_um: position,
                        ..Default::default()
                    },
                    Gate {
                        paired: osg_model::Id::new(),
                        radius_m: 1.0,
                        exclusion_m: radius_m,
                        enabled: true,
                    },
                ))
                .id();
            obstacles.push((entity, position, radius_m));
        }
        refresh(&mut world);
        for radius in [0.0, 50.0, 500.0, 10_000.0] {
            let mut actual = candidates(&mut world, origin, radius).unwrap();
            actual.sort_unstable();
            let mut expected: Vec<_> = obstacles
                .iter()
                .filter(|(_, position, extent)| {
                    position.relative_to(origin).length() <= radius + extent
                })
                .map(|(entity, _, _)| *entity)
                .collect();
            expected.sort_unstable();
            assert_eq!(actual, expected);
            let mut mouths = mouth_candidates(&world, origin, radius);
            mouths.sort_unstable();
            assert_eq!(mouths, expected);
        }
        for _ in 0..4097 {
            world.spawn((
                PreciseTransform::default(),
                SpatialBody {
                    radius_m: 1.0,
                    occludes: true,
                },
            ));
        }
        refresh(&mut world);
        assert!(candidates(&mut world, GalacticPosition::ZERO, 1.0).is_none());
    }
}
