use super::{
    hardware::ShipInventory,
    identity::{Appearance, Identity},
    physics::{
        Velocity,
        collision::{Projectile, Report},
    },
    session::Events,
};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use toy_sim_model::{
    CombatEvent, CombatEventKind, ContactRef, Event, GalacticPosition, GroupId, Id, Pose, Track,
    TrackId,
};

#[derive(Component, Clone, Copy)]
struct TraceId(u64);

#[derive(Resource, Default)]
pub struct CombatHistory(VecDeque<Recorded>);

struct Recorded {
    sequence: u64,
    time_ns: u64,
    kind: RecordedKind,
}

enum RecordedKind {
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
        position: GalacticPosition,
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

pub fn ingest(world: &mut World, report: &Report, epoch: f64) {
    let mut pending = Vec::new();
    for shot in &report.shots {
        let Some(source) = world.get::<Identity>(shot.owner).map(|id| id.0) else {
            continue;
        };
        let velocity = world
            .get::<Velocity>(shot.owner)
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
    for segment in &report.motion {
        let Some((owner, radius)) = world
            .get::<Projectile>(segment.entity)
            .map(|projectile| (projectile.launch_owner, projectile.radius_m))
        else {
            continue;
        };
        let Some(source) = owner
            .and_then(|owner| world.get::<Identity>(owner))
            .map(|id| id.0)
        else {
            continue;
        };
        let id = if let Some(id) = world.get::<TraceId>(segment.entity) {
            id.0
        } else {
            let id = rand::random();
            world.entity_mut(segment.entity).insert(TraceId(id));
            id
        };
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
            .map(|entity| world.get::<Identity>(entity).map(|id| id.0));
        if targets.iter().all(Option::is_none) {
            continue;
        }
        pending.push((
            nanoseconds(epoch + impact.time),
            RecordedKind::Impact {
                targets,
                position: impact.position,
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
        let Some(target) = world.get::<Identity>(death.entity).map(|id| id.0) else {
            continue;
        };
        let electrical = world
            .get::<ShipInventory>(death.entity)
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
                appearance: world
                    .get::<Appearance>(death.entity)
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
    append(world, pending, "combat", None);
}

fn append(
    world: &mut World,
    pending: Vec<(u64, RecordedKind)>,
    event_kind: &str,
    subject: Option<Id>,
) {
    world.init_resource::<Events>();
    world.init_resource::<CombatHistory>();
    world.resource_scope(|world, mut history: Mut<CombatHistory>| {
        let mut events = world.resource_mut::<Events>();
        for (time_ns, kind) in pending {
            let sequence = events.0.back().map_or(1, |event| event.sequence + 1);
            events.0.push_back(Event {
                sequence,
                tick: time_ns.div_ceil(100_000_000),
                subject,
                kind: event_kind.into(),
                position: None,
            });
            history.0.push_back(Recorded {
                sequence,
                time_ns,
                kind,
            });
        }
    });
}

pub fn flush_travel(world: &mut World) {
    let Some(mut events) = world.get_resource_mut::<super::travel::TravelEvents>() else {
        return;
    };
    let pending = std::mem::take(&mut events.0);
    world.init_resource::<Events>();
    for (entity, kind, _) in pending {
        let subject = world.get::<Identity>(entity).map(|id| id.0);
        let tick = world
            .resource::<super::simulation::SimulationCounters>()
            .ticks;
        let mut events = world.resource_mut::<Events>();
        let sequence = events.0.back().map_or(1, |event| event.sequence + 1);
        events.0.push_back(Event {
            sequence,
            tick,
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
    tracks: &BTreeMap<GroupId, BTreeMap<TrackId, Track>>,
    optically_visible: &BTreeSet<Id>,
    previously_visible: &BTreeSet<Id>,
    after_sequence: u64,
) -> Vec<CombatEvent> {
    let Some(history) = world.get_resource::<CombatHistory>() else {
        return Vec::new();
    };
    let mut contacts: BTreeMap<Id, Vec<(ContactRef, &Track)>> = BTreeMap::new();
    for (&group, tracks) in tracks {
        for (&id, track) in tracks {
            if let Some(entity) = track.entity {
                contacts
                    .entry(entity)
                    .or_default()
                    .push((ContactRef { group, track: id }, track));
            }
        }
    }
    history
        .0
        .iter()
        .filter(|event| event.sequence > after_sequence)
        .filter_map(|event| {
            let tick = event.time_ns.div_ceil(100_000_000);
            let contact = |id| {
                contacts.get(&id)?.iter().find_map(|(contact, track)| {
                    (track.position_sigma_m == 0.0
                        || (track.velocity_sigma_m_s == 0.0
                            && track.observed_tick.saturating_add(1) >= tick))
                        .then_some(*contact)
                })
            };
            let kind = match &event.kind {
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
                    position,
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
                        position: *position,
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
    use bevy::math::{DQuat, DVec3};
    use toy_sim_model::Provenance;

    fn observed(id: Id, sigma: f64) -> BTreeMap<GroupId, BTreeMap<TrackId, Track>> {
        let group = Id::new();
        let track = Id::new();
        BTreeMap::from([(
            group,
            BTreeMap::from([(
                track,
                Track {
                    spatial_instance: Id::new(),
                    id: track,
                    entity: Some(id),
                    pose: Pose::default(),
                    position_sigma_m: sigma,
                    velocity_sigma_m_s: sigma,
                    observed_tick: 100,
                    estimate_tick: 101,
                    tags: Default::default(),
                    provenance: Provenance::Transponder,
                    radius_m: Some(10.0),
                    appearance: None,
                },
            )]),
        )])
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
                ShipInventory(toy_sim_ships::Inventory {
                    packaged_parts: Default::default(),
                    reservations: Default::default(),
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
                thermal: toy_sim_ships::thermal::ThermalState {
                    hull_energy_j: 10.0,
                    shield_energy_j: 20.0,
                    pending_waste_heat_j: 30.0,
                    ..Default::default()
                },
            });
        ingest(&mut world, &report, 10.0);
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
                &BTreeSet::from([id]),
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
            time_ns: 100_000_000,
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
        flush_travel(&mut world);
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
        ingest(&mut world, &report, 0.0);
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
