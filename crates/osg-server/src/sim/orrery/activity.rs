use super::{BodyClass, Celestial, Star, Universe, orrery_cfg::Body};
use crate::sim::{
    physics::RigidBody,
    physics::{Velocity, sim_time},
    precision::PreciseTransform,
    spatial::SpatialBody,
};
use bevy::{math::DVec3, prelude::*};
use std::collections::{BTreeMap, BTreeSet};
const CELL_UM: i128 = 1_i128 << 64;
#[derive(Component, Clone)]
pub struct CelestialState {
    pub reference: osg_model::travel::CelestialRef,
    pub body: Body,
    pub system: usize,
    pub anchor: crate::sim::precision::GalacticPosition,
    pub influence: f64,
    pub velocity: DVec3,
}
#[derive(Resource, Default)]
pub struct ActiveSystems {
    pub entities: BTreeMap<usize, Vec<Entity>>,
    pub definitions: BTreeMap<usize, std::sync::Arc<osg_universe::universe::SystemDefinition>>,
    pub occupied_systems: BTreeSet<usize>,
    pub object_systems: std::collections::HashMap<Entity, Vec<usize>>,
    regions: std::collections::HashMap<[i128; 3], Vec<usize>>,
}

impl ActiveSystems {
    pub fn systems_at(
        &self,
        universe: &osg_universe::universe::Universe,
        position: crate::sim::precision::GalacticPosition,
    ) -> Vec<usize> {
        let key = [
            position.x.div_euclid(CELL_UM),
            position.y.div_euclid(CELL_UM),
            position.z.div_euclid(CELL_UM),
        ];
        let Some(candidates) = self.regions.get(&key) else {
            return universe.containing_segment(position, DVec3::ZERO);
        };
        candidates
            .iter()
            .copied()
            .filter(|id| {
                let definition =
                    self.definitions.get(id).cloned().unwrap_or_else(|| {
                        universe.resolve_index(*id).expect("valid nearby system")
                    });
                definition
                    .solver
                    .anchor
                    .relative_to(position)
                    .length_squared()
                    <= definition.influence.powi(2)
            })
            .collect()
    }

    pub fn systems_for_object<'a>(
        &'a self,
        universe: &Universe,
        entity: Entity,
        position: crate::sim::precision::GalacticPosition,
    ) -> std::borrow::Cow<'a, [usize]> {
        match self.object_systems.get(&entity) {
            Some(systems) => std::borrow::Cow::Borrowed(systems),
            // Objects can be created after the activation phase of this tick.
            None => {
                std::borrow::Cow::Owned(universe.index.containing_segment(position, DVec3::ZERO))
            }
        }
    }
}
#[derive(Resource, Default)]
pub struct UniverseDebug {
    pub inspect: Option<osg_model::travel::CelestialRef>,
}

pub fn activate(
    mut commands: Commands,
    universe: Res<Universe>,
    mut active: ResMut<ActiveSystems>,
    time: Res<Time<Fixed>>,
    objects: Query<
        (
            Entity,
            &PreciseTransform,
            Option<&Velocity>,
            Option<&crate::sim::travel::PresenceState>,
            Option<&crate::sim::travel::DormantMotion>,
            Has<crate::sim::travel::Dormant>,
        ),
        (
            Without<Celestial>,
            Or<(
                With<SpatialBody>,
                With<RigidBody>,
                With<crate::sim::travel::DormantMotion>,
            )>,
        ),
    >,
) {
    let mut needed = BTreeSet::new();
    let _profile = crate::sim::diagnostics::ProfileScope::new("activate_systems");
    active.object_systems.clear();
    for (entity, p, v, presence, motion, dormant) in &objects {
        match presence.map(|presence| &presence.0) {
            Some(osg_model::travel::Presence::Destroyed) => {
                if !motion.is_some_and(|motion| motion.has_physical_body()) {
                    continue;
                }
            }
            None | Some(osg_model::travel::Presence::Space) if !dormant => {}
            _ => continue,
        }
        let displacement = v.map_or(DVec3::ZERO, |v| v.0) * time.timestep().as_secs_f64();
        let mut overlapping = Vec::new();
        let position = p.translation_um;
        let candidates = if displacement.length() <= CELL_UM as f64 / 2e6 {
            let key = [
                position.x.div_euclid(CELL_UM),
                position.y.div_euclid(CELL_UM),
                position.z.div_euclid(CELL_UM),
            ];
            if active.regions.len() >= 8192 {
                active.regions.clear();
            }
            active
                .regions
                .entry(key)
                .or_insert_with(|| {
                    let centre = crate::sim::precision::GalacticPosition::new(
                        key[0] * CELL_UM + CELL_UM / 2,
                        key[1] * CELL_UM + CELL_UM / 2,
                        key[2] * CELL_UM + CELL_UM / 2,
                    );
                    universe
                        .index
                        .intersecting_sphere(centre, (3_f64.sqrt() + 1.) * CELL_UM as f64 / 2e6)
                })
                .clone()
        } else {
            universe.index.containing_segment(position, displacement)
        };
        for id in candidates {
            let definition = active
                .definitions
                .get(&id)
                .cloned()
                .unwrap_or_else(|| universe.resolve_index(id).expect("valid nearby system"));
            let relative = definition.solver.anchor.relative_to(p.translation_um);
            let fraction = if displacement.length_squared() > 0.0 {
                (relative.dot(displacement) / displacement.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if (relative - fraction * displacement).length_squared() <= definition.influence.powi(2)
            {
                needed.insert(id);
                overlapping.push(id);
            }
        }
        active.object_systems.insert(entity, overlapping);
    }
    active.occupied_systems = needed.clone();
    let remove: Vec<_> = active
        .entities
        .keys()
        .filter(|id| !needed.contains(id))
        .copied()
        .collect();
    for id in remove {
        active.definitions.remove(&id);
        for entity in active.entities.remove(&id).unwrap() {
            commands.entity(entity).despawn();
        }
    }
    let epoch = sim_time(&time) - hifitime::Duration::from_seconds(time.timestep().as_secs_f64());
    for id in needed {
        if active.entities.contains_key(&id) {
            continue;
        }
        let system = universe.resolve_index(id).expect("valid occupied system");
        let mut entities = Vec::new();
        for b in system.solver.iter() {
            if matches!(b.class_params, BodyClass::Barycenter) {
                continue;
            }
            let mut root = commands.spawn((
                Celestial(b.name.clone()),
                CelestialState {
                    reference: crate::sim::registry::model_reference(
                        system.body_id(&b.name).expect("body identity"),
                    ),
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
                    color_temp: b.stellar.as_ref().map_or_else(
                        || {
                            ((lumens / 93.0)
                                / (4.0 * std::f64::consts::PI * b.radius.powi(2) * 5.670374419e-8))
                                .powf(0.25)
                        },
                        |stellar| stellar.effective_temperature_k,
                    ),
                });
            }
            entities.push(root.id());
        }
        active.entities.insert(id, entities);
        active.definitions.insert(id, system);
    }
}

pub fn arrival(
    universe: &Universe,
    reference: osg_model::travel::CelestialRef,
    epoch: hifitime::Epoch,
) -> (PreciseTransform, DVec3) {
    let reference = crate::sim::registry::universe_reference(reference);
    let body = universe.body(reference).unwrap();
    let altitude = 1_000_000f64.max(body.atmosphere.as_ref().map_or(0.0, |a| a.height) + 100_000.0);
    let radius = body.radius + altitude;
    let velocity =
        DVec3::X * (crate::sim::physics::GRAVITATIONAL_CONSTANT * body.mass / radius).sqrt();
    let mut pose = PreciseTransform {
        translation_um: universe
            .solve_position(reference, epoch)
            .unwrap()
            .offset_by(DVec3::Z * radius),
        ..default()
    };
    pose.look_to(velocity.normalize(), DVec3::Y);
    (
        pose,
        universe.solve_velocity(reference, epoch).unwrap() + velocity,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_objects_control_activation_and_inspection_does_not() {
        let universe = Universe::init(osg_universe::example_config()).unwrap();
        let inspected = universe.authored_body("Helion").unwrap();
        let mut app = App::new();
        app.insert_resource(universe)
            .init_resource::<Time<Fixed>>()
            .init_resource::<ActiveSystems>()
            .insert_resource(UniverseDebug {
                inspect: Some(crate::sim::registry::model_reference(inspected)),
            })
            .add_systems(Update, activate);
        app.update();
        assert!(app.world().resource::<ActiveSystems>().entities.is_empty());

        // A stationary object needs neither vessel software nor velocity to keep
        // its environment simulated.
        let object = app
            .world_mut()
            .spawn((
                PreciseTransform::default(),
                SpatialBody {
                    radius_m: 10.0,
                    occludes: true,
                },
            ))
            .id();
        app.update();
        assert_eq!(app.world().resource::<ActiveSystems>().entities.len(), 1);

        app.world_mut()
            .entity_mut(object)
            .insert(crate::sim::travel::Dormant);
        app.update();
        assert!(app.world().resource::<ActiveSystems>().entities.is_empty());
        app.world_mut()
            .entity_mut(object)
            .remove::<crate::sim::travel::Dormant>();
        app.update();
        assert_eq!(app.world().resource::<ActiveSystems>().entities.len(), 1);

        app.world_mut().despawn(object);
        app.update();
        assert!(app.world().resource::<ActiveSystems>().entities.is_empty());

        let wreck = app
            .world_mut()
            .spawn((
                PreciseTransform::default(),
                crate::sim::travel::Dormant,
                crate::sim::travel::PresenceState(osg_model::travel::Presence::Destroyed),
                crate::sim::travel::DormantMotion {
                    velocity: DVec3::ZERO,
                    angular_velocity: DVec3::ZERO,
                    spatial: Some(SpatialBody {
                        radius_m: 10.0,
                        occludes: true,
                    }),
                    rigid_body: false,
                    collision_body: false,
                },
            ))
            .id();
        app.update();
        assert_eq!(app.world().resource::<ActiveSystems>().entities.len(), 1);

        // A ship lost in slip has no physical wreck at its last position.
        app.world_mut()
            .get_mut::<crate::sim::travel::DormantMotion>(wreck)
            .unwrap()
            .spatial = None;
        app.update();
        assert!(app.world().resource::<ActiveSystems>().entities.is_empty());
    }
}
