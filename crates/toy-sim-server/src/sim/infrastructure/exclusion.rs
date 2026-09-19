use super::{GateOrbit, travel};
use crate::sim::{orrery::Universe, precision::PreciseTransform};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Resource, Default)]
pub(crate) struct CertifiedExclusion {
    pub members: BTreeSet<Entity>,
}

pub(crate) fn configuration_changed(
    certified: Option<Res<CertifiedExclusion>>,
    changed: Query<(), Or<(Changed<GateOrbit>, Changed<travel::Gate>)>>,
    mut removed: RemovedComponents<GateOrbit>,
) -> bool {
    let removed = removed.read().next().is_some();
    certified.is_none() || !changed.is_empty() || removed
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RigidOrbit {
    reference: String,
    axis: [u64; 3],
    rate: u64,
    epoch: u64,
}

pub(crate) fn certify(world: &mut World) {
    world.remove_resource::<CertifiedExclusion>();
    super::enforce_exclusion(world);

    let universe = world.resource::<Universe>().clone();
    let mut groups = BTreeMap::<RigidOrbit, Vec<(Entity, GateOrbit, f64)>>::new();
    for (entity, orbit, gate) in world
        .query::<(Entity, &GateOrbit, &travel::Gate)>()
        .iter(world)
    {
        if !gate.enabled || !orbit.valid(&universe) {
            continue;
        }
        groups
            .entry(RigidOrbit {
                reference: orbit.body.clone(),
                axis: orbit.axis.to_array().map(f64::to_bits),
                rate: orbit.rate.to_bits(),
                epoch: orbit.epoch_seconds.to_bits(),
            })
            .or_default()
            .push((entity, orbit.clone(), gate.exclusion_m));
    }
    let groups: Vec<_> = groups.into_values().collect();
    let mut envelopes = toy_sim_spatial::SpatialHash::default();
    for (index, group) in groups.iter().enumerate() {
        let Some((centre, radius)) = group[0].1.envelope(&universe) else {
            continue;
        };
        let outer = group
            .iter()
            .filter_map(|(_, orbit, exclusion)| {
                orbit
                    .envelope(&universe)
                    .map(|(_, radius)| radius + exclusion)
            })
            .fold(radius, f64::max);
        envelopes.insert(
            index as u32,
            toy_sim_spatial::Entry {
                position: centre,
                radius_m: outer,
                luminosity: 0.0,
            },
        );
    }
    let mut certified = CertifiedExclusion::default();
    for (index, group) in groups.iter().enumerate() {
        let Some((centre, radius)) = group[0].1.envelope(&universe) else {
            continue;
        };
        let outer = group
            .iter()
            .filter_map(|(_, orbit, exclusion)| {
                orbit
                    .envelope(&universe)
                    .map(|(_, radius)| radius + exclusion)
            })
            .fold(radius, f64::max);
        let overlaps = envelopes
            .intersecting_sphere(centre, outer)
            .ids
            .into_iter()
            .any(|other| other != index as u32);
        if !overlaps {
            certified
                .members
                .extend(group.iter().map(|(entity, _, _)| *entity));
        }
    }
    world.insert_resource(certified);
}

pub(crate) fn candidates(
    world: &mut World,
) -> Vec<(Entity, crate::sim::precision::GalacticPosition, f64)> {
    let mut query = world.query::<(Entity, &PreciseTransform, &travel::Gate)>();
    let certified = world.get_resource::<CertifiedExclusion>();
    query
        .iter(world)
        .filter(|(entity, _, gate)| {
            gate.enabled && !certified.is_some_and(|certified| certified.members.contains(entity))
        })
        .map(|(entity, pose, gate)| (entity, pose.translation_um, gate.exclusion_m))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{orrery, precision::GalacticPosition, spatial::SpatialBody};
    use bevy::math::DVec3;
    use toy_sim_model::Id;

    fn scene() -> (World, [Entity; 2]) {
        let mut world = World::new();
        let universe = orrery::Universe::init(orrery::orrery_cfg::OrreryCfg {
            name: "Test system".into(),
            position_um: GalacticPosition::ZERO,
            bodies: vec![orrery::orrery_cfg::Body {
                name: "Test star".into(),
                mass: 1e30,
                radius: 1e8,
                class_params: orrery::BodyClass::Star { lumens: 1e26 },
                ..Default::default()
            }],
        })
        .unwrap();
        world.insert_resource(universe);
        let gates = [DVec3::X, DVec3::Y].map(|direction| {
            world
                .spawn((
                    GateOrbit {
                        system: 0,
                        body: "Test star".into(),
                        offset: direction * 1e11,
                        axis: DVec3::Z,
                        rate: 0.001,
                        epoch_seconds: 0.0,
                    },
                    travel::Gate {
                        paired: Id::new(),
                        radius_m: 1.0,
                        exclusion_m: 1000.0,
                        enabled: true,
                    },
                    PreciseTransform {
                        translation_um: GalacticPosition::from_meters(direction * 1e11),
                        ..Default::default()
                    },
                    SpatialBody {
                        radius_m: 2.0,
                        occludes: false,
                    },
                ))
                .id()
        });
        travel::geometry::refresh(&mut world);
        (world, gates)
    }

    #[test]
    fn mismatched_orbital_rates_keep_runtime_exclusion_checks() {
        let (mut world, gates) = scene();
        certify(&mut world);
        assert_eq!(world.resource::<CertifiedExclusion>().members.len(), 2);
        world.get_mut::<GateOrbit>(gates[1]).unwrap().rate *= 2.0;
        certify(&mut world);
        assert!(world.resource::<CertifiedExclusion>().members.is_empty());
        let position = world
            .get::<PreciseTransform>(gates[0])
            .unwrap()
            .translation_um;
        world
            .get_mut::<PreciseTransform>(gates[1])
            .unwrap()
            .translation_um = position;
        travel::geometry::refresh(&mut world);
        super::super::enforce_exclusion(&mut world);
        assert!(
            gates
                .iter()
                .all(|&entity| !world.get::<travel::Gate>(entity).unwrap().enabled)
        );
    }

    #[test]
    fn uncertified_intruder_invalidates_a_certified_mouth_too() {
        let (mut world, gates) = scene();
        certify(&mut world);
        let position = world
            .get::<PreciseTransform>(gates[0])
            .unwrap()
            .translation_um;
        let intruder = world
            .spawn((
                travel::Gate {
                    paired: Id::new(),
                    radius_m: 1.0,
                    exclusion_m: 1000.0,
                    enabled: true,
                },
                PreciseTransform {
                    translation_um: position,
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 2.0,
                    occludes: false,
                },
            ))
            .id();
        travel::geometry::refresh(&mut world);
        super::super::enforce_exclusion(&mut world);
        assert!(!world.get::<travel::Gate>(gates[0]).unwrap().enabled);
        assert!(!world.get::<travel::Gate>(intruder).unwrap().enabled);
        assert!(world.get::<travel::Gate>(gates[1]).unwrap().enabled);
    }
}
