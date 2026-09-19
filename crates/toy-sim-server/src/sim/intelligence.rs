use super::{
    hardware::SensorRange,
    identity::*,
    physics::{AngularVelocity, Velocity},
    precision::PreciseTransform,
    simulation::SimulationCounters,
    spatial::SpatialIndex,
    vessel::ShipDesign,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
use toy_sim_intel::{Measurement, Snapshot};
use toy_sim_model::{Id, InfoGroupKey, PUBLIC_GROUP, Pose, Provenance, Tag, Track};

#[derive(Component)]
pub struct Group {
    pub id: Id,
    pub key: Option<InfoGroupKey>,
    pub snapshot: Arc<Snapshot>,
}

#[derive(Component, Default)]
pub struct Measurements(pub Vec<Measurement>);

#[derive(Component)]
pub struct TrackEstimate(pub Track);

#[derive(Component)]
#[relationship(relationship_target = GroupTracks)]
pub struct TrackGroup(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = TrackGroup, linked_spawn)]
pub struct GroupTracks(Vec<Entity>);

#[derive(Component)]
pub struct TrackAssociation(pub Id);

#[derive(Resource, Default)]
pub struct AssociationIndex(pub HashMap<(Entity, Id), Entity>);

pub fn join(world: &mut World, key: InfoGroupKey) -> Entity {
    if let Some(entity) = world.resource::<GroupIndex>().0.get(&key) {
        return *entity;
    }
    let id = Id::new();
    let entity = world
        .spawn((
            Group {
                id,
                key: Some(key),
                snapshot: Arc::default(),
            },
            Measurements::default(),
            GroupTracks::default(),
        ))
        .id();
    world.resource_mut::<GroupIndex>().0.insert(key, entity);
    register(world, entity, id);
    entity
}

pub fn initialize(world: &mut World) {
    world.init_resource::<GroupIndex>();
    world.init_resource::<IdentityIndex>();
    world.init_resource::<AssociationIndex>();
    let public = world
        .spawn((
            Group {
                id: PUBLIC_GROUP,
                key: None,
                snapshot: Arc::default(),
            },
            Measurements::default(),
            GroupTracks::default(),
        ))
        .id();
    register(world, public, PUBLIC_GROUP);
}

pub fn pose(
    transform: &PreciseTransform,
    velocity: Option<&Velocity>,
    angular: Option<&AngularVelocity>,
) -> Pose {
    Pose {
        position: transform.translation_um,
        rotation: transform.rotation.to_array(),
        velocity: velocity.map_or([0.0; 3], |velocity| velocity.0.to_array()),
        angular_velocity: angular.map_or([0.0; 3], |angular| angular.0.to_array()),
    }
}

pub fn acquire(
    clock: Res<SimulationCounters>,
    seed: Res<SensorSeed>,
    scene: Res<SpatialIndex>,
    overrides: Query<&super::hardware::SensorOverride>,
    ships: Query<(
        Entity,
        &Identity,
        &PreciseTransform,
        &Velocity,
        &AngularVelocity,
        &Membership,
        &Transponder,
        &ShipDesign,
        &SensorRange,
        &Appearance,
        Has<BeaconEmitter>,
        Has<super::missiles::Missile>,
    )>,
    mut groups: Query<(Entity, &Group, Option<&GroupShips>, &mut Measurements)>,
) {
    groups
        .par_iter_mut()
        .for_each(|(group_entity, group, members, mut measurements)| {
            measurements.0.clear();
            if group.id == PUBLIC_GROUP {
                for (
                    _,
                    identity,
                    transform,
                    velocity,
                    angular,
                    _,
                    iff,
                    design,
                    _,
                    appearance,
                    beacon,
                    missile,
                ) in &ships
                {
                    if !beacon {
                        continue;
                    }
                    let mut sample = Measurement::authenticated(
                        identity.0,
                        identity.0,
                        pose(transform, Some(velocity), Some(angular)),
                        &iff.0,
                        Provenance::Beacon,
                    );
                    sample.tags.insert(Tag::Kind("beacon".into()));
                    if missile {
                        sample.tags.insert(Tag::Kind("missile".into()));
                    }
                    sample.radius_m = Some(design.0.radius);
                    sample.appearance = Some(appearance.0);
                    measurements.0.push(sample);
                }
                return;
            }
            let Some(members) = members else {
                return;
            };
            for member in members.iter() {
                let Ok((
                    entity,
                    identity,
                    transform,
                    velocity,
                    angular,
                    _,
                    iff,
                    design,
                    hardware,
                    appearance,
                    _,
                    missile,
                )) = ships.get(member)
                else {
                    continue;
                };
                let mut own = Measurement::authenticated(
                    identity.0,
                    identity.0,
                    pose(transform, Some(velocity), Some(angular)),
                    &iff.0,
                    Provenance::GroupMember,
                );
                if missile {
                    own.tags.insert(Tag::Kind("missile".into()));
                }
                own.radius_m = Some(design.0.radius);
                own.appearance = Some(appearance.0);
                measurements.0.push(own);
                if hardware.0 <= 0.0 {
                    continue;
                }
                let sensor = super::sensors::Sensor {
                    range_m: hardware.0,
                    occlusion: overrides
                        .get(entity)
                        .map_or(true, |sensor| sensor.occlusion),
                };
                let contacts = super::sensors::detect_nearest(
                    &scene,
                    entity,
                    transform.translation_um,
                    &sensor,
                    256,
                );
                for contact in contacts.visible {
                    let Ok((
                        _,
                        target,
                        target_transform,
                        target_velocity,
                        target_angular,
                        target_group,
                        target_iff,
                        target_design,
                        _,
                        target_appearance,
                        _,
                        target_missile,
                    )) = ships.get(contact.entity)
                    else {
                        continue;
                    };
                    if target_group.0 == group_entity {
                        continue;
                    }
                    let distance = target_transform
                        .translation_um
                        .relative_to(transform.translation_um)
                        .length_squared();
                    let target_pose = pose(
                        target_transform,
                        Some(target_velocity),
                        Some(target_angular),
                    );
                    let mut sample =
                        if target_iff.0.enabled && distance <= target_iff.0.range_m.powi(2) {
                            Measurement::authenticated(
                                target.0,
                                identity.0,
                                target_pose,
                                &target_iff.0,
                                Provenance::Transponder,
                            )
                        } else {
                            Measurement::sensor(
                                &seed.0,
                                identity.0,
                                target.0,
                                transform.translation_um,
                                &target_pose,
                                clock.ticks,
                            )
                        };
                    if target_missile {
                        sample.tags.insert(Tag::Kind("missile".into()));
                    }
                    sample.radius_m = Some(target_design.0.radius);
                    sample.appearance = Some(target_appearance.0);
                    measurements.0.push(sample);
                }
            }
        });
}

pub fn coast(
    clock: Res<SimulationCounters>,
    mut commands: Commands,
    mut index: ResMut<AssociationIndex>,
    mut tracks: Query<(Entity, &TrackGroup, &TrackAssociation, &mut TrackEstimate)>,
) {
    for (entity, group, association, mut estimate) in &mut tracks {
        let track = &mut estimate.0;
        let age = clock.ticks.saturating_sub(track.observed_tick);
        if age > 600 {
            commands.entity(entity).despawn();
            if index.0.get(&(group.0, association.0)) == Some(&entity) {
                index.0.remove(&(group.0, association.0));
            }
            continue;
        }
        let elapsed = clock.ticks.saturating_sub(track.estimate_tick);
        if elapsed == 0 {
            continue;
        }
        let dt = elapsed as f64 * 0.1;
        track.pose.position = track
            .pose
            .position
            .offset_by(DVec3::from_array(track.pose.velocity) * dt);
        let acceleration =
            0.5 * ((age as f64 * 0.1).powi(2) - (age.saturating_sub(elapsed) as f64 * 0.1).powi(2));
        track.position_sigma_m = (track.position_sigma_m.powi(2)
            + (dt * track.velocity_sigma_m_s).powi(2)
            + acceleration.powi(2))
        .sqrt();
        track.estimate_tick = clock.ticks;
        track.provenance = Provenance::Extrapolated;
    }
}

pub fn fuse(
    clock: Res<SimulationCounters>,
    mut commands: Commands,
    mut index: ResMut<AssociationIndex>,
    mut groups: Query<(Entity, &mut Measurements)>,
    mut tracks: Query<&mut TrackEstimate>,
    identities: Res<IdentityIndex>,
    instances: Query<&SpatialInstance>,
) {
    for (group, mut measurements) in &mut groups {
        measurements.0.sort_by(|a, b| {
            a.physical
                .cmp(&b.physical)
                .then(a.sigma_m.total_cmp(&b.sigma_m))
                .then(a.platform.cmp(&b.platform))
        });
        for samples in measurements.0.chunk_by(|a, b| a.physical == b.physical) {
            let best = &samples[0];
            let previous = index.0.get(&(group, best.physical)).copied();
            let existing = previous.and_then(|entity| tracks.get(entity).ok());
            let continuous = existing.filter(|estimate| {
                let track = &estimate.0;
                best.entity.is_some() && best.entity == track.entity
                    || track.pose.position.relative_to(best.pose.position).length()
                        <= 3.0 * (track.position_sigma_m + best.sigma_m)
            });
            let track_id = continuous.map_or_else(Id::new, |estimate| estimate.0.id);
            let instance = identities
                .0
                .get(&best.physical)
                .and_then(|entity| instances.get(*entity).ok())
                .map_or(Id([0; 16]), |instance| instance.0);
            let mut track = Track {
                spatial_instance: track_spatial_instance(track_id, instance),
                id: track_id,
                entity: best
                    .entity
                    .or_else(|| continuous.and_then(|estimate| estimate.0.entity)),
                pose: best.pose.clone(),
                position_sigma_m: best.sigma_m,
                velocity_sigma_m_s: best.sigma_velocity,
                observed_tick: clock.ticks,
                estimate_tick: clock.ticks,
                tags: best.tags.clone(),
                provenance: best.provenance,
                radius_m: best.radius_m,
                appearance: best.appearance,
            };
            if best.entity.is_none()
                && let Some(previous) = continuous
            {
                track.tags.extend(
                    previous
                        .0
                        .tags
                        .iter()
                        .filter(|tag| matches!(tag, Tag::IffOwner(_) | Tag::IffFaction(_)))
                        .cloned(),
                );
            }
            if best.sigma_m > 0.0 {
                let mut seen = BTreeSet::new();
                let mut position = DVec3::ZERO;
                let mut velocity = DVec3::ZERO;
                let mut total = 0.0;
                for sample in samples {
                    if !seen.insert(sample.platform) {
                        continue;
                    }
                    let weight = sample.sigma_m.max(1e-6).powi(2).recip();
                    position += sample.pose.position.relative_to(best.pose.position) * weight;
                    velocity += DVec3::from_array(sample.pose.velocity) * weight;
                    total += weight;
                }
                track.pose.position = best.pose.position.offset_by(position / total);
                track.pose.velocity = (velocity / total).to_array();
                track.position_sigma_m = total.recip().sqrt().max(1.0).max(best.sigma_m / 4.0);
                track.velocity_sigma_m_s = track.position_sigma_m;
            }
            if continuous.is_some() {
                tracks.get_mut(previous.unwrap()).unwrap().0 = track;
            } else {
                let entity = commands
                    .spawn((
                        TrackEstimate(track),
                        TrackGroup(group),
                        TrackAssociation(best.physical),
                    ))
                    .id();
                index.0.insert((group, best.physical), entity);
            }
        }
    }
}

pub fn publish(
    clock: Res<SimulationCounters>,
    mut groups: Query<(&GroupTracks, &mut Group)>,
    tracks: Query<&TrackEstimate>,
) {
    groups.par_iter_mut().for_each(|(members, mut group)| {
        let mut snapshot = Snapshot::default();
        snapshot.tick = clock.ticks;
        for entity in members.iter() {
            if let Ok(track) = tracks.get(entity) {
                snapshot.put(track.0.clone());
            }
        }
        group.snapshot = Arc::new(snapshot);
    });
}

pub fn collect_unused_groups(world: &mut World) {
    if world.resource::<SimulationCounters>().ticks % 100 != 0 {
        return;
    }
    let mut retained = BTreeSet::from([PUBLIC_GROUP]);
    for session in world.query::<&super::session::Session>().iter(world) {
        retained.extend(session.groups.iter().copied());
    }
    for account in world.query::<&Account>().iter(world) {
        if let Some(group) = world.get::<Group>(account.group) {
            retained.insert(group.id);
        }
    }
    for (group, ships) in world.query::<(&Group, &GroupShips)>().iter(world) {
        if !ships.is_empty() {
            retained.insert(group.id);
        }
    }
    let unused = world
        .query::<(Entity, &Group)>()
        .iter(world)
        .filter(|(_, group)| !retained.contains(&group.id))
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    for entity in unused {
        world.despawn(entity);
    }
    let alive_groups = world
        .query_filtered::<Entity, With<Group>>()
        .iter(world)
        .collect::<BTreeSet<_>>();
    world
        .resource_mut::<AssociationIndex>()
        .0
        .retain(|(group, _), _| alive_groups.contains(group));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreferenced_secrets_expire_without_removing_account_or_view_groups() {
        let mut world = World::new();
        let account = Id::new();
        super::super::identity::initialize(&mut world, &[account]);
        world.init_resource::<SimulationCounters>();
        world.resource_mut::<SimulationCounters>().ticks = 100;
        world.init_resource::<super::super::session::Events>();
        let session = super::super::session::connect(&mut world, account).unwrap();
        let viewed = join(&mut world, InfoGroupKey([7; 32]));
        let viewed_id = world.get::<Group>(viewed).unwrap().id;
        world
            .get_mut::<super::super::session::Session>(session)
            .unwrap()
            .groups
            .insert(viewed_id);
        let unused = join(&mut world, InfoGroupKey([8; 32]));
        let account_entity = lookup(&world, account).unwrap();
        let own = world.get::<Account>(account_entity).unwrap().group;

        collect_unused_groups(&mut world);

        assert!(world.get_entity(unused).is_err());
        assert!(world.get::<Group>(viewed).is_some());
        assert!(world.get::<Group>(own).is_some());
        assert!(lookup(&world, PUBLIC_GROUP).is_ok());
    }
}

#[cfg(test)]
mod celestial_acquisition_tests {
    use super::*;

    #[test]
    fn celestial_ephemerides_do_not_create_public_sensor_measurements() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SimulationCounters>()
            .init_resource::<SpatialIndex>()
            .insert_resource(SensorSeed([0; 32]))
            .add_systems(Update, acquire);
        initialize(app.world_mut());
        let body = toy_sim_universe::example_config().bodies.remove(0);
        app.world_mut().spawn((
            Identity(Id::new()),
            PreciseTransform::default(),
            super::super::orrery::activity::CelestialState {
                body,
                system: 0,
                anchor: Default::default(),
                influence: 1e12,
                velocity: DVec3::ZERO,
            },
        ));
        app.update();
        let public = lookup(app.world(), PUBLIC_GROUP).unwrap();
        assert!(
            app.world()
                .get::<Measurements>(public)
                .unwrap()
                .0
                .is_empty()
        );
    }
}
