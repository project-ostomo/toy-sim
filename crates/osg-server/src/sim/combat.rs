use super::{
    hardware::ShipInventory,
    identity::{Appearance, Identity},
    physics::{Velocity, collision::Projectile},
    session::Events,
};
use bevy::prelude::*;
use osg_model::{CombatEvent, CombatEventKind, Event, GalacticPosition, Id, Pose};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Component, Clone, Copy)]
pub(crate) struct TraceId(u64);

#[derive(Resource, Default)]
pub struct CombatHistory(VecDeque<Recorded>);

struct Recorded {
    sequence: u64,
    time_ns: u64,
    kind: RecordedKind,
}

enum RecordedKind {
    Beam {
        source: Id,
        start: GalacticPosition,
        end: GalacticPosition,
        velocity: [f64; 3],
        end_time_ns: u64,
    },
    Fired {
        source: Id,
        position: GalacticPosition,
        energy: f64,
    },
    Projectile {
        id: u64,
        source: Id,
        start: GalacticPosition,
        end: GalacticPosition,
        end_time_ns: u64,
        radius: f64,
    },
    Impact {
        targets: [Option<Id>; 2],
        positions: [GalacticPosition; 2],
        velocity: [f64; 3],
        normal: [f64; 3],
        energy: f64,
        shields: [bool; 2],
    },
    Destroyed {
        target: Id,
        pose: Pose,
        appearance: Option<[u8; 32]>,
        energy: f64,
        mass: f64,
        radius: f64,
    },
}

fn nanoseconds(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1e9).round() as u64
}

pub fn ingest(
    mut commands: Commands,
    report: Res<super::physics::collision::CollisionReport>,
    identities: Query<&Identity>,
    velocities: Query<&Velocity>,
    projectiles: Query<&Projectile>,
    traces: Query<&TraceId>,
    inventories: Query<&ShipInventory>,
    appearances: Query<&Appearance>,
    mut events: ResMut<Events>,
    mut history: ResMut<CombatHistory>,
) {
    let epoch = report.epoch;
    let report = &report.report;
    let mut allocated = BTreeMap::new();
    let mut pending = Vec::new();
    for shot in &report.shots {
        let Some(source) = identities.get(shot.owner).ok().map(|id| id.0) else {
            continue;
        };
        let velocity = velocities
            .get(shot.owner)
            .ok()
            .map_or(bevy::math::DVec3::ZERO, |v| v.0);
        pending.push((
            nanoseconds(epoch + shot.time),
            RecordedKind::Fired {
                source,
                position: shot.position,
                energy: 0.5 * shot.mass_kg * (shot.velocity - velocity).length_squared(),
            },
        ));
    }
    for beam in &report.beam_traces {
        let Some(source) = identities.get(beam.owner).ok().map(|id| id.0) else {
            continue;
        };
        pending.push((
            nanoseconds(epoch + beam.time),
            RecordedKind::Beam {
                source,
                start: beam.start,
                end: beam.end,
                velocity: beam.velocity.to_array(),
                end_time_ns: nanoseconds(epoch + beam.time + beam.duration_s),
            },
        ));
    }
    for segment in &report.motion {
        let Some((owner, radius)) = projectiles
            .get(segment.entity)
            .ok()
            .map(|projectile| (projectile.launch_owner, projectile.radius_m))
        else {
            continue;
        };
        let Some(source) = owner
            .and_then(|owner| identities.get(owner).ok())
            .map(|id| id.0)
        else {
            continue;
        };
        let id = traces
            .get(segment.entity)
            .map(|id| id.0)
            .unwrap_or_else(|_| {
                *allocated.entry(segment.entity).or_insert_with(|| {
                    let id = rand::random();
                    commands.entity(segment.entity).insert(TraceId(id));
                    id
                })
            });
        pending.push((
            nanoseconds(epoch + segment.start),
            RecordedKind::Projectile {
                id,
                source,
                start: segment.position,
                end: segment
                    .position
                    .offset_by(segment.velocity * (segment.end - segment.start)),
                end_time_ns: nanoseconds(epoch + segment.end),
                radius,
            },
        ));
    }
    for impact in &report.impact_events {
        if report.destroyed.iter().any(|death| {
            !death.projectile
                && impact.entities.contains(&death.entity)
                && (impact.time - death.time).abs() < 1e-6
        }) {
            continue;
        }
        let targets = impact
            .entities
            .map(|entity| identities.get(entity).ok().map(|id| id.0));
        if targets.iter().all(Option::is_none) {
            continue;
        }
        pending.push((
            nanoseconds(epoch + impact.time),
            RecordedKind::Impact {
                targets,
                positions: impact.surface_positions,
                velocity: impact.velocity.to_array(),
                normal: impact.normal.to_array(),
                energy: impact.energy_j,
                shields: impact.shields,
            },
        ));
    }
    for death in &report.destroyed {
        if death.projectile {
            continue;
        }
        let Some(target) = identities.get(death.entity).ok().map(|id| id.0) else {
            continue;
        };
        let electrical = inventories
            .get(death.entity)
            .ok()
            .map_or(0.0, |inventory| inventory.0.energy_j as f64);
        pending.push((
            nanoseconds(epoch + death.time),
            RecordedKind::Destroyed {
                target,
                pose: Pose {
                    position: death.position,
                    rotation: death.rotation.to_array(),
                    velocity: death.velocity.to_array(),
                    angular_velocity: death.angular_velocity.to_array(),
                },
                appearance: appearances
                    .get(death.entity)
                    .ok()
                    .map(|appearance| appearance.0),
                energy: death.thermal.hull_energy_j
                    + death.thermal.shield_energy_j
                    + death.thermal.pending_waste_heat_j
                    + electrical * 0.25,
                mass: death.mass,
                radius: death.radius,
            },
        ));
    }
    pending.sort_by_key(|(time, _)| *time);
    for (time_ns, kind) in pending {
        let sequence = events.0.back().map_or(1, |event| event.sequence + 1);
        events.0.push_back(Event {
            sequence,
            tick: time_ns.div_ceil(osg_model::TICK_NS),
            subject: None,
            kind: "combat".into(),
            position: None,
        });
        history.0.push_back(Recorded {
            sequence,
            time_ns,
            kind,
        });
    }
}

pub fn flush_travel(
    pending: Option<ResMut<super::travel::TravelEvents>>,
    mut events: ResMut<Events>,
    clock: Res<super::simulation::SimulationCounters>,
    identities: Query<&Identity>,
) {
    let Some(mut pending) = pending else {
        return;
    };
    for (entity, kind, _) in pending.0.drain(..) {
        let subject = identities.get(entity).ok().map(|id| id.0);
        let sequence = events.0.back().map_or(1, |event| event.sequence + 1);
        events.0.push_back(Event {
            sequence,
            tick: clock.ticks,
            subject,
            kind: kind.into(),
            position: None,
        });
    }
}

pub fn prune(world: &mut World, published: u64) {
    if let Some(mut history) = world.get_resource_mut::<CombatHistory>() {
        while history
            .0
            .front()
            .is_some_and(|event| event.sequence <= published)
        {
            history.0.pop_front();
        }
    }
}

pub fn for_session(
    world: &World,
    references: &BTreeMap<Id, Id>,
    optically_visible: &BTreeSet<Id>,
    previously_visible: &BTreeSet<Id>,
    after_sequence: u64,
) -> Vec<CombatEvent> {
    let Some(history) = world.get_resource::<CombatHistory>() else {
        return Vec::new();
    };
    history
        .0
        .iter()
        .filter(|event| event.sequence > after_sequence)
        .filter_map(|event| {
            let contact = |id| references.get(&id).copied();
            let kind = match &event.kind {
                RecordedKind::Beam {
                    source,
                    start,
                    end,
                    velocity,
                    end_time_ns,
                } => CombatEventKind::Beam {
                    source: optically_visible
                        .contains(source)
                        .then(|| contact(*source))
                        .flatten()?,
                    start: *start,
                    end: *end,
                    velocity_m_s: *velocity,
                    end_time_ns: *end_time_ns,
                },
                RecordedKind::Fired {
                    source,
                    position,
                    energy,
                } => CombatEventKind::Fired {
                    source: optically_visible
                        .contains(source)
                        .then(|| contact(*source))
                        .flatten()?,
                    position: *position,
                    energy_j: *energy,
                },
                RecordedKind::Projectile {
                    id,
                    source,
                    start,
                    end,
                    end_time_ns,
                    radius,
                } => CombatEventKind::Projectile {
                    id: *id,
                    source: Some(
                        optically_visible
                            .contains(source)
                            .then(|| contact(*source))
                            .flatten()?,
                    ),
                    start: *start,
                    end: *end,
                    end_time_ns: *end_time_ns,
                    radius_m: *radius,
                },
                RecordedKind::Impact {
                    targets,
                    positions,
                    velocity,
                    normal,
                    energy,
                    shields,
                } => {
                    let (index, target) = targets.iter().enumerate().find_map(|(index, id)| {
                        id.filter(|id| optically_visible.contains(id))
                            .and_then(contact)
                            .map(|target| (index, target))
                    })?;
                    CombatEventKind::Impact {
                        target: Some(target),
                        position: positions[index],
                        velocity_m_s: *velocity,
                        normal: *normal,
                        energy_j: *energy,
                        shield: shields[index],
                    }
                }
                RecordedKind::Destroyed {
                    target,
                    pose,
                    appearance,
                    energy,
                    mass,
                    radius,
                } => CombatEventKind::Destroyed {
                    target: (optically_visible.contains(target)
                        || previously_visible.contains(target))
                    .then(|| contact(*target))
                    .flatten()?,
                    pose: pose.clone(),
                    appearance: *appearance,
                    energy_j: *energy,
                    mass_kg: *mass,
                    radius_m: *radius,
                },
            };
            Some(CombatEvent {
                sequence: event.sequence,
                sim_time_ns: event.time_ns,
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::collision::Report;
    use bevy::math::{DQuat, DVec3};

    fn observed(id: Id, _unused: f64) -> BTreeMap<Id, Id> {
        BTreeMap::from([(id, Id::new())])
    }

    #[test]
    fn destruction_keeps_energy_and_metadata_after_entity_removal_without_disclosing_hidden_contacts()
     {
        let mut world = World::new();
        let id = Id::new();
        let appearance = [7; 32];
        let ship = world
            .spawn((
                Identity(id),
                Appearance(appearance),
                ShipInventory(osg_ships::Inventory {
                    packaged_parts: Default::default(),
                    reservations: Default::default(),
                    custody: Default::default(),
                    tank_capacities_m3: Vec::new(),
                    quantities: Vec::new(),
                    cargo: Vec::new(),
                    energy_j: 80,
                }),
            ))
            .id();
        let mut report = Report::default();
        let position = GalacticPosition::splat(1_000_000_000_000_000_000_000_000);
        report
            .destroyed
            .push(super::super::physics::collision::Destruction {
                entity: ship,
                projectile: false,
                position,
                velocity: DVec3::X,
                rotation: DQuat::IDENTITY,
                angular_velocity: DVec3::ZERO,
                mass: 100.0,
                radius: 10.0,
                time: 0.05,
                thermal: osg_ships::thermal::ThermalState {
                    hull_energy_j: 10.0,
                    shield_energy_j: 20.0,
                    pending_waste_heat_j: 30.0,
                    ..Default::default()
                },
            });
        world.init_resource::<Events>();
        world.init_resource::<CombatHistory>();
        world.insert_resource(super::super::physics::collision::CollisionReport {
            epoch: 10.0,
            report,
        });
        world.run_system_cached(ingest).unwrap();
        world.despawn(ship);
        assert!(
            for_session(
                &world,
                &BTreeMap::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                0
            )
            .is_empty()
        );
        assert!(
            for_session(
                &world,
                &observed(id, 10.0),
                &BTreeSet::new(),
                &BTreeSet::new(),
                0
            )
            .is_empty()
        );
        let events = for_session(
            &world,
            &observed(id, 0.0),
            &BTreeSet::new(),
            &BTreeSet::from([id]),
            0,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sim_time_ns, 10_050_000_000);
        let CombatEventKind::Destroyed {
            pose,
            energy_j,
            appearance: image,
            ..
        } = &events[0].kind
        else {
            panic!("expected destruction");
        };
        assert_eq!(pose.position, position);
        assert_eq!(*energy_j, 80.0);
        assert_eq!(*image, Some(appearance));
        assert!(
            for_session(
                &world,
                &observed(id, 0.0),
                &BTreeSet::new(),
                &BTreeSet::from([id]),
                events[0].sequence
            )
            .is_empty()
        );
        assert!(
            world
                .resource::<Events>()
                .0
                .iter()
                .all(|event| event.subject.is_none() && event.position.is_none())
        );
    }

    #[test]
    fn shared_iff_and_previous_visibility_do_not_authorize_current_weapon_effects() {
        let mut world = World::new();
        let id = Id::new();
        world.insert_resource(CombatHistory(VecDeque::from([Recorded {
            sequence: 1,
            time_ns: osg_model::TICK_NS,
            kind: RecordedKind::Fired {
                source: id,
                position: GalacticPosition::ZERO,
                energy: 1000.,
            },
        }])));
        let tracks = observed(id, 0.);
        let none = BTreeSet::new();
        let visible = BTreeSet::from([id]);
        assert!(for_session(&world, &tracks, &none, &none, 0).is_empty());
        assert!(for_session(&world, &tracks, &none, &visible, 0).is_empty());
        assert_eq!(for_session(&world, &tracks, &visible, &none, 0).len(), 1);
    }

    #[test]
    fn travel_events_carry_gameplay_information_without_rendering_events() {
        let mut world = World::new();
        let id = Id::new();
        let ship = world.spawn(Identity(id)).id();
        world.init_resource::<super::super::simulation::SimulationCounters>();
        world.insert_resource(super::super::travel::TravelEvents(vec![(
            ship,
            "gate-transferred",
            None,
        )]));
        world.init_resource::<Events>();
        world.run_system_cached(flush_travel).unwrap();
        assert!(
            world
                .resource::<super::super::travel::TravelEvents>()
                .0
                .is_empty()
        );
        let event = world.resource::<Events>().0.back().unwrap();
        assert_eq!(event.kind, "gate-transferred");
        assert_eq!(event.subject, Some(id));
        assert!(event.position.is_none());
        assert!(
            for_session(
                &world,
                &observed(id, 0.0),
                &BTreeSet::new(),
                &BTreeSet::from([id]),
                0
            )
            .is_empty()
        );
    }

    #[test]
    fn combat_events_survive_large_batches_until_published() {
        let mut world = World::new();
        let ship = world.spawn(Identity(Id::new())).id();
        let projectile = world.spawn_empty().id();
        let mut report = Report::default();
        report.shots = (0..8197)
            .map(|_| super::super::physics::collision::weapons::ShotEvent {
                projectile,
                owner: ship,
                device: 1,
                time: 0.0,
                position: GalacticPosition::ZERO,
                velocity: DVec3::X,
                mass_kg: 1.0,
                radius_m: 0.1,
            })
            .collect();
        world.init_resource::<Events>();
        world.init_resource::<CombatHistory>();
        world.insert_resource(super::super::physics::collision::CollisionReport {
            epoch: 0.0,
            report,
        });
        world.run_system_cached(ingest).unwrap();
        let history = world.resource::<CombatHistory>();
        let markers = world.resource::<Events>();
        assert_eq!(history.0.len(), 8197);
        assert_eq!(markers.0.len(), 8197);
        assert_eq!(
            history.0.front().unwrap().sequence,
            markers.0.front().unwrap().sequence
        );
        assert_eq!(
            history.0.back().unwrap().sequence,
            markers.0.back().unwrap().sequence
        );
        prune(&mut world, 8196);
        assert_eq!(world.resource::<CombatHistory>().0.len(), 1);
        assert_eq!(
            world
                .resource::<CombatHistory>()
                .0
                .front()
                .unwrap()
                .sequence,
            8197
        );
        prune(&mut world, 8197);
        assert!(world.resource::<CombatHistory>().0.is_empty());
    }
}
