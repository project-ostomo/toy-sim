use crate::sim::{precision::PreciseTransform, spatial::SpatialIndex};
use bevy::prelude::*;
use osg_model::{Id, SensorObservation};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Default)]
pub struct ObservationSnapshot {
    pub tick: u64,
    pub observer_instance: Id,
    pub contacts: BTreeMap<u64, SensorObservation>,
    pub targets: BTreeMap<Id, u64>,
    instances: BTreeMap<Id, Id>,
}

#[derive(Component, Default)]
pub struct Observations(pub Arc<ObservationSnapshot>);

pub(crate) fn invalidate_observation(
    observer: Entity,
    observations: &mut Observations,
    entity: Entity,
    id: Option<Id>,
) {
    if observer == entity {
        observations.0 = Arc::default();
    } else if let Some(id) = id.filter(|id| observations.0.targets.contains_key(id)) {
        let snapshot = Arc::make_mut(&mut observations.0);
        if let Some(handle) = snapshot.targets.remove(&id) {
            snapshot.contacts.remove(&handle);
        }
        snapshot.instances.remove(&id);
    }
}

pub fn invalidate(world: &mut World, entity: Entity) {
    let id = world
        .get::<super::identity::Identity>(entity)
        .map(|id| id.0);
    if let Some(service) = world.get_resource::<SensorService>() {
        service.invalidate(entity, id);
    }
    for (observer, mut observations) in world.query::<(Entity, &mut Observations)>().iter_mut(world)
    {
        invalidate_observation(observer, &mut observations, entity, id);
    }
}

pub fn refresh_iff(world: &mut World, entity: Entity) {
    let Some(id) = world
        .get::<super::identity::Identity>(entity)
        .map(|id| id.0)
    else {
        return;
    };
    let iff = world
        .get::<super::identity::Transponder>(entity)
        .filter(|value| value.0.enabled)
        .map(|value| value.0.clone());
    if let Some(service) = world.get_resource::<SensorService>() {
        let mut scene = service.0.lock().unwrap();
        if let Some(target) = scene.targets.get_mut(&entity) {
            target.iff = iff.clone();
        }
        for (_, snapshot) in scene.cached.values_mut() {
            let snapshot = Arc::make_mut(snapshot);
            if let Some(handle) = snapshot.targets.get(&id).copied()
                && let Some(contact) = snapshot.contacts.get_mut(&handle)
            {
                contact.entity = iff.as_ref().map(|_| id);
                contact.iff = iff.clone();
            }
        }
    }
    for mut observations in world.query::<&mut Observations>().iter_mut(world) {
        if let Some(handle) = observations.0.targets.get(&id).copied() {
            let snapshot = Arc::make_mut(&mut observations.0);
            if let Some(contact) = snapshot.contacts.get_mut(&handle) {
                contact.entity = iff.as_ref().map(|_| id);
                contact.iff = iff.clone();
            }
        }
    }
}

/// Publish only current measurements. Physical identities stay on the server.
#[derive(bevy::ecs::query::QueryData)]
pub struct Observer {
    entity: Entity,
    instance: Option<&'static super::identity::SpatialInstance>,
    observations: Option<&'static Observations>,
    dormant: Has<super::travel::Dormant>,
    pose: Option<&'static PreciseTransform>,
    range: Option<&'static super::hardware::SensorRange>,
    override_settings: Option<&'static super::hardware::SensorOverride>,
}

#[derive(bevy::ecs::query::QueryData)]
pub struct Target {
    entity: Entity,
    id: &'static super::identity::Identity,
    instance: &'static super::identity::SpatialInstance,
    pose: &'static PreciseTransform,
    design: &'static super::vessel::ShipDesign,
    transponder: Option<&'static super::identity::Transponder>,
    velocity: Option<&'static super::physics::Velocity>,
    angular: Option<&'static super::physics::AngularVelocity>,
}

#[derive(Clone)]
struct ObserverState {
    instance: Id,
    origin: crate::sim::precision::GalacticPosition,
    sensor: Sensor,
}

#[derive(Clone)]
struct TargetState {
    id: Id,
    instance: Id,
    pose: osg_model::Pose,
    radius_m: f64,
    iff: Option<osg_model::IffIdentity>,
}

#[derive(Default)]
struct SensorScene {
    revision: u64,
    tick: u64,
    index: SpatialIndex,
    observers: ahash::AHashMap<Entity, ObserverState>,
    targets: ahash::AHashMap<Entity, TargetState>,
    cached: ahash::AHashMap<Entity, (u64, Arc<ObservationSnapshot>)>,
}

/// Shared with computer host calls; no query is performed until observe is called.
#[derive(Resource, Clone, Default)]
pub struct SensorService(Arc<std::sync::Mutex<SensorScene>>);

impl SensorService {
    pub(crate) fn invalidate(&self, entity: Entity, id: Option<Id>) {
        let mut scene = self.0.lock().unwrap();
        scene.observers.remove(&entity);
        scene.targets.remove(&entity);
        for (&observer, (_, snapshot)) in &mut scene.cached {
            let mut value = Observations(snapshot.clone());
            invalidate_observation(observer, &mut value, entity, id);
            *snapshot = value.0;
        }
    }

    pub fn observe(&self, entity: Entity) -> Arc<ObservationSnapshot> {
        let mut scene = self.0.lock().unwrap();
        if let Some((revision, snapshot)) = scene.cached.get(&entity)
            && *revision == scene.revision
        {
            return snapshot.clone();
        }
        let old = scene
            .cached
            .get(&entity)
            .map(|(_, snapshot)| snapshot.clone())
            .unwrap_or_default();
        let _profile = super::diagnostics::ProfileScope::new("sensor_detection");
        let mut snapshot = ObservationSnapshot {
            tick: scene.tick,
            ..Default::default()
        };
        if let Some(observer) = scene.observers.get(&entity) {
            snapshot.observer_instance = observer.instance;
            let detections =
                detect_nearest(&scene.index, entity, observer.origin, &observer.sensor, 256);
            for detection in detections.visible {
                let Some(target) = scene.targets.get(&detection.entity) else {
                    continue;
                };
                let handle = old
                    .targets
                    .get(&target.id)
                    .copied()
                    .filter(|_| {
                        old.observer_instance == observer.instance
                            && old.instances.get(&target.id) == Some(&target.instance)
                    })
                    .unwrap_or_else(|| {
                        loop {
                            let handle = rand::random::<u64>();
                            if handle != 0
                                && !snapshot.contacts.contains_key(&handle)
                                && !old.contacts.contains_key(&handle)
                            {
                                break handle;
                            }
                        }
                    });
                let mut opaque = [0; 16];
                opaque[..8].copy_from_slice(&handle.to_le_bytes());
                snapshot.contacts.insert(
                    handle,
                    SensorObservation {
                        id: handle,
                        spatial_instance: super::identity::observation_spatial_instance(
                            Id(opaque),
                            target.instance,
                        ),
                        entity: target.iff.as_ref().map(|_| target.id),
                        iff: target.iff.clone(),
                        pose: target.pose.clone(),
                        radius_m: target.radius_m,
                    },
                );
                snapshot.targets.insert(target.id, handle);
                snapshot.instances.insert(target.id, target.instance);
            }
        }
        let snapshot = Arc::new(snapshot);
        let revision = scene.revision;
        scene.cached.insert(entity, (revision, snapshot.clone()));
        snapshot
    }

    pub(crate) fn cached(&self, entity: Entity) -> Option<Arc<ObservationSnapshot>> {
        self.0
            .lock()
            .unwrap()
            .cached
            .get(&entity)
            .map(|(_, value)| value.clone())
    }
}

/// Refresh query inputs without scanning. Old handles survive until the next
/// requested observation checks whether their targets are still visible.
pub fn publish(
    mut commands: Commands,
    clock: Res<super::simulation::SimulationCounters>,
    index: Res<SpatialIndex>,
    service: Option<Res<SensorService>>,
    observers: Query<Observer, With<super::identity::Identity>>,
    targets: Query<Target>,
) {
    let _profile = super::diagnostics::ProfileScope::new("sensor_publish");
    let service = service.map(|service| service.clone()).unwrap_or_default();
    {
        let mut scene = service.0.lock().unwrap();
        scene.revision += 1;
        scene.tick = clock.ticks;
        scene.index = index.sensor_snapshot();
        scene.observers.clear();
        for observer in &observers {
            if let Some(old) = observer.observations {
                scene
                    .cached
                    .entry(observer.entity)
                    .or_insert((0, old.0.clone()));
            }
            if !observer.dormant
                && let (Some(instance), Some(pose), Some(range)) =
                    (observer.instance, observer.pose, observer.range)
                && range.0 > 0.0
            {
                scene.observers.insert(
                    observer.entity,
                    ObserverState {
                        instance: instance.0,
                        origin: pose.translation_um,
                        sensor: Sensor {
                            range_m: range.0,
                            occlusion: observer
                                .override_settings
                                .is_none_or(|value| value.occlusion),
                        },
                    },
                );
            }
        }
        scene.targets = targets
            .iter()
            .map(|target| {
                (
                    target.entity,
                    TargetState {
                        id: target.id.0,
                        instance: target.instance.0,
                        pose: super::identity::pose(target.pose, target.velocity, target.angular),
                        radius_m: target.design.0.radius,
                        iff: target
                            .transponder
                            .filter(|value| value.0.enabled)
                            .map(|value| value.0.clone()),
                    },
                )
            })
            .collect();
        let present: ahash::AHashSet<_> =
            observers.iter().map(|observer| observer.entity).collect();
        scene.cached.retain(|entity, _| present.contains(entity));
    }
    commands.insert_resource(service);
}

/// Make computer-requested handles available to command processing. This copies
/// only already-computed snapshots and never initiates a scan.
pub fn flush(mut commands: Commands, service: Option<Res<SensorService>>) {
    let Some(service) = service else {
        return;
    };
    let scene = service.0.lock().unwrap();
    for (&entity, (_, snapshot)) in &scene.cached {
        commands
            .entity(entity)
            .insert(Observations(snapshot.clone()));
    }
}

pub fn observe(world: &mut World, entity: Entity) -> Arc<ObservationSnapshot> {
    let snapshot = world
        .get_resource::<SensorService>()
        .filter(|_| world.get::<super::travel::Dormant>(entity).is_none())
        .map(|service| service.observe(entity))
        .unwrap_or_default();
    world
        .entity_mut(entity)
        .insert(Observations(snapshot.clone()));
    snapshot
}

#[derive(Component, Clone, Copy)]
pub struct Sensor {
    pub range_m: f64,
    pub occlusion: bool,
}
impl Default for Sensor {
    fn default() -> Self {
        Self {
            range_m: 100_000_000.0,
            occlusion: true,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Contact {
    pub entity: Entity,
    pub distance_m: f64,
}

#[derive(Default)]
pub struct SensorContacts {
    pub visible: Vec<Contact>,
    pub candidates: usize,
    pub blocked: usize,
    pub occluders: usize,
}

pub struct SensorsPlugin;
impl Plugin for SensorsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SensorService>();
    }
}

/// True when the open sightline enters a sphere. Tangency is not blocking.
pub use super::spatial::sphere_blocks;

pub fn detect(
    index: &SpatialIndex,
    observer: Entity,
    origin: crate::sim::precision::GalacticPosition,
    sensor: &Sensor,
) -> SensorContacts {
    let origin = index
        .object_index(observer)
        .map_or(origin, |id| index.objects[id].position);
    if sensor.range_m <= 0. || !sensor.range_m.is_finite() {
        return SensorContacts::default();
    }
    let candidates = index.within_range(origin, sensor.range_m);
    // Gather and transform blockers once per sensor, not once per target.
    let mut blockers: Vec<_> = if sensor.occlusion {
        index
            .occluders_in_range(origin, sensor.range_m)
            .into_iter()
            .map(|id| index.objects[id])
            .filter(|o| o.entity != observer)
            .map(|o| (o.entity, o.position.relative_to(origin), o.radius_m))
            .collect()
    } else {
        vec![]
    };
    blockers.sort_unstable_by(|a, b| a.1.length_squared().total_cmp(&b.1.length_squared()));
    let mut result = SensorContacts {
        occluders: blockers.len(),
        ..default()
    };
    for id in candidates {
        let target = index.objects[id];
        if target.entity == observer {
            continue;
        }
        result.candidates += 1;
        let displacement = target.position.relative_to(origin);
        if blockers.iter().any(|&(entity, centre, radius)| {
            entity != target.entity && sphere_blocks(displacement, centre, radius)
        }) {
            result.blocked += 1;
        } else {
            result.visible.push(Contact {
                entity: target.entity,
                distance_m: displacement.length(),
            });
        }
    }
    result.visible.sort_unstable_by(|a, b| {
        a.distance_m
            .total_cmp(&b.distance_m)
            .then_with(|| a.entity.cmp(&b.entity))
    });
    result
}

/// Nearest N geometrical candidates first, then occlusion. Hidden candidates
/// are never replaced with more distant contacts. Selection avoids a full sort.
pub fn detect_nearest(
    index: &SpatialIndex,
    observer: Entity,
    origin: crate::sim::precision::GalacticPosition,
    sensor: &Sensor,
    n: usize,
) -> SensorContacts {
    let origin = index
        .object_index(observer)
        .map_or(origin, |id| index.objects[id].position);
    if n == 0 || sensor.range_m <= 0. || !sensor.range_m.is_finite() {
        return SensorContacts::default();
    }
    let candidates = {
        let _profile = super::diagnostics::ProfileScope::new("sensor_contact_selection");
        index.nearest(observer, origin, sensor.range_m, n)
    };
    #[cfg(test)]
    {
        super::diagnostics::samples::count("sensor.scans", 1);
        super::diagnostics::samples::count("sensor.selected_contacts", candidates.len());
    }
    let _profile = super::diagnostics::ProfileScope::new("sensor_occlusion_and_results");
    let mut result = SensorContacts {
        candidates: candidates.len(),
        ..default()
    };
    for id in candidates {
        let object = index.objects[id];
        if sensor.occlusion && index.occluded(observer, id, origin) {
            result.blocked += 1;
        } else {
            result.visible.push(Contact {
                entity: object.entity,
                distance_m: object.position.relative_to(origin).length(),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{precision::GalacticPosition, spatial::SpatialObject};
    use bevy::math::DVec3;

    #[test]
    fn observations_are_exact_private_and_disappear_with_detection() {
        use crate::sim::{
            hardware::SensorRange,
            identity::{Identity, SpatialInstance, Transponder},
            physics::Velocity,
            vessel::ControlledVessel,
        };

        let mut app =
            crate::sim::bootstrap::provision_combat_fixture(&[Id::new()], None, None).unwrap();
        let world = app.world_mut();
        let observer = world
            .query_filtered::<Entity, With<ControlledVessel>>()
            .single(world)
            .unwrap();
        let target = world
            .query::<(Entity, &Transponder)>()
            .iter(world)
            .find(|(_, iff)| iff.0.labels.contains("Hostile patrol"))
            .unwrap()
            .0;
        let target_id = world.get::<Identity>(target).unwrap().0;
        let position = GalacticPosition::ZERO.offset_by(DVec3::X * 1000.);
        world
            .get_mut::<PreciseTransform>(observer)
            .unwrap()
            .translation_um = GalacticPosition::ZERO;
        world
            .get_mut::<PreciseTransform>(target)
            .unwrap()
            .translation_um = position;
        world.get_mut::<Velocity>(target).unwrap().0 = DVec3::new(17., -3., 2.);
        world.get_mut::<SensorRange>(observer).unwrap().0 = 2000.;
        let other = world
            .spawn((
                Identity(Id::new()),
                SpatialInstance(Id::new()),
                PreciseTransform::default(),
                SensorRange(2000.),
            ))
            .id();
        let mut index = SpatialIndex::default();
        index.insert(SpatialObject {
            entity: target,
            position,
            radius_m: 10.,
            occludes: false,
            optical_occludes: false,
            optical_luminosity_w: 0.,
        });
        world.insert_resource(index);

        world.run_system_cached(publish).unwrap();
        let service = world.resource::<SensorService>().clone();
        let source = crate::sim::services::current_source(world, observer).unwrap();
        assert_eq!(
            source.scan(2000.0, 256).len(),
            1,
            "computer scan requests must compute observations immediately"
        );
        assert!(
            service.cached(other).is_none(),
            "publishing inputs must not scan"
        );
        let first = service.observe(other);
        assert!(
            Arc::ptr_eq(&first, &service.observe(other)),
            "repeated requests must reuse the same scene observation"
        );
        observe(world, observer);
        observe(world, other);
        let handle = world.get::<Observations>(observer).unwrap().0.targets[&target_id];
        let other_handle = world.get::<Observations>(other).unwrap().0.targets[&target_id];
        assert_ne!(handle, other_handle);
        let contact = &world.get::<Observations>(observer).unwrap().0.contacts[&handle];
        assert_eq!(contact.pose.position, position);
        assert_eq!(contact.pose.velocity, [17., -3., 2.]);
        assert_eq!(contact.entity, Some(target_id));
        assert!(contact.iff.is_some());

        world.get_mut::<Transponder>(target).unwrap().0.enabled = false;
        refresh_iff(world, target);
        let contact = &world.get::<Observations>(observer).unwrap().0.contacts[&handle];
        assert!(contact.iff.is_none());
        assert!(contact.entity.is_none());
        world.get_mut::<SensorRange>(observer).unwrap().0 = 0.;
        world.run_system_cached(publish).unwrap();
        observe(world, observer);
        observe(world, other);
        assert!(
            world
                .get::<Observations>(observer)
                .unwrap()
                .0
                .contacts
                .is_empty()
        );
        assert!(
            world
                .get::<Observations>(other)
                .unwrap()
                .0
                .contacts
                .contains_key(&other_handle)
        );

        world.get_mut::<SensorRange>(observer).unwrap().0 = 2000.;
        world.run_system_cached(publish).unwrap();
        observe(world, observer);
        observe(world, other);
        let reacquired = world.get::<Observations>(observer).unwrap().0.targets[&target_id];
        assert_ne!(handle, reacquired);
        world.get_mut::<SpatialInstance>(target).unwrap().0 = Id::new();
        world.run_system_cached(publish).unwrap();
        observe(world, observer);
        observe(world, other);
        assert_ne!(
            world.get::<Observations>(observer).unwrap().0.targets[&target_id],
            reacquired
        );
        world.despawn(target);
        world.run_system_cached(publish).unwrap();
        observe(world, observer);
        observe(world, other);
        assert!(
            world
                .get::<Observations>(observer)
                .unwrap()
                .0
                .contacts
                .is_empty()
        );
        assert!(
            world
                .get::<Observations>(other)
                .unwrap()
                .0
                .contacts
                .is_empty()
        );
    }

    #[test]
    fn nearest_limit_is_applied_before_occlusion_without_backfill() {
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let hidden = world.spawn_empty().id();
        let blocker = world.spawn_empty().id();
        let farther = world.spawn_empty().id();
        let mut index = SpatialIndex::default();
        for (entity, position, radius_m, occludes) in [
            (hidden, DVec3::X * 10., 1., false),
            (blocker, DVec3::X * 12., 3., true),
            (farther, DVec3::Y * 15., 1., false),
        ] {
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity,
                position: GalacticPosition::ZERO.offset_by(position),
                radius_m,
                occludes,
                optical_occludes: occludes,
            });
        }
        let sensor = Sensor {
            range_m: 100.,
            occlusion: true,
        };
        let result = detect_nearest(&index, observer, GalacticPosition::ZERO, &sensor, 1);
        assert_eq!(result.candidates, 1);
        assert_eq!(result.blocked, 1);
        assert!(result.visible.is_empty());
        let result = detect_nearest(
            &index,
            observer,
            GalacticPosition::ZERO,
            &Sensor {
                occlusion: false,
                ..sensor
            },
            1,
        );
        assert_eq!(result.visible.len(), 1);
        assert_eq!(result.visible[0].entity, hidden);
        assert!(
            detect_nearest(&index, observer, GalacticPosition::ZERO, &sensor, 0)
                .visible
                .is_empty()
        );
    }

    #[test]
    fn sphere_sightlines_handle_tangency_surface_and_blocker_depth() {
        let target = DVec3::X * 10.0;
        assert!(sphere_blocks(target, DVec3::X * 5.0, 1.0));
        assert!(!sphere_blocks(target, DVec3::X * 12.0, 1.0));
        assert!(!sphere_blocks(target, DVec3::new(5.0, 1.0, 0.0), 1.0));
        assert!(!sphere_blocks(target, -DVec3::X, 1.0)); // On surface, looking away.
        assert!(sphere_blocks(target, DVec3::X, 1.0)); // On surface, looking inward.
        assert!(!sphere_blocks(DVec3::ZERO, DVec3::ZERO, 1.0));
    }

    #[test]
    fn huge_off_range_occluder_is_found_and_target_does_not_occlude_itself() {
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let hidden = world.spawn_empty().id();
        let star = world.spawn_empty().id();
        let clear = world.spawn_empty().id();
        let origin = GalacticPosition::splat(1_i128 << 90);
        let mut index = SpatialIndex::default();
        for (entity, delta, radius_m, occludes) in [
            (observer, DVec3::ZERO, 10.0, false),
            (hidden, DVec3::X * 15e6, 10.0, false),
            (star, DVec3::X * 1.01e9, 1e9, true),
            (clear, -DVec3::X * 15e6, 1e6, true),
        ] {
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity,
                position: origin.offset_by(delta),
                radius_m,
                occludes,
                optical_occludes: occludes,
            });
        }
        let mut sensor = Sensor {
            range_m: 20e6,
            occlusion: true,
        };
        let result = detect(&index, observer, origin, &sensor);
        assert_eq!(result.candidates, 2);
        assert_eq!(result.blocked, 1);
        assert_eq!(result.visible.len(), 1);
        assert_eq!(result.visible[0].entity, clear);
        sensor.occlusion = false;
        assert_eq!(detect(&index, observer, origin, &sensor).visible.len(), 2);
    }

    #[test]
    fn indexed_visibility_matches_all_objects_brute_force() {
        use rand::{RngExt, SeedableRng};
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(14);
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let origin = GalacticPosition::splat(-(1_i128 << 90));
        let mut index = SpatialIndex::default();
        for i in 0..500 {
            let delta = DVec3::new(
                rng.random_range(-1e8..1e8),
                rng.random_range(-1e8..1e8),
                rng.random_range(-1e8..1e8),
            );
            index.insert(SpatialObject {
                optical_luminosity_w: 0.0,
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: if i % 7 == 0 {
                    rng.random_range(1e6..4e7)
                } else {
                    10.0
                },
                occludes: i % 7 == 0,
                optical_occludes: i % 7 == 0,
            });
        }
        for range_m in [1e7, 5e7, 1e8, 2e8] {
            let actual = detect(
                &index,
                observer,
                origin,
                &Sensor {
                    range_m,
                    occlusion: true,
                },
            );
            let mut actual: Vec<_> = actual.visible.iter().map(|c| c.entity).collect();
            let mut expected: Vec<_> = index
                .objects
                .iter()
                .filter(|target| {
                    let d = target.position.relative_to(origin);
                    d.length() <= range_m
                        && !index.objects.iter().any(|b| {
                            b.occludes
                                && b.entity != target.entity
                                && sphere_blocks(d, b.position.relative_to(origin), b.radius_m)
                        })
                })
                .map(|o| o.entity)
                .collect();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected);
        }
    }
}
