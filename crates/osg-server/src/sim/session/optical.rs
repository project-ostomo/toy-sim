use super::*;
use crate::sim::{identity::SpatialInstance, spatial::SpatialIndex, vessel::ShipDesign};
use osg_model::optical::{
    MAX_OPTICAL_OBSERVATIONS, MIN_OPTICAL_FLUX_W_M2, OpticalObservation, flux_w_m2,
};
use std::collections::{BinaryHeap, HashMap};

const IDENTITY_GRACE_TICKS: u64 = 100;
const MAX_OPTICAL_BYTES: usize = 4 * 1024 * 1024;

struct ViewBudget {
    remaining_bytes: usize,
    remaining_count: usize,
}

impl ViewBudget {
    fn new(view_count: usize) -> Self {
        Self {
            remaining_bytes: (MAX_OPTICAL_BYTES - 10) / view_count.max(1),
            remaining_count: MAX_OPTICAL_OBSERVATIONS / view_count.max(1),
        }
    }

    fn admit(&mut self, observation: &OpticalObservation) -> bool {
        if self.remaining_count == 0 {
            return false;
        }
        let bytes = postcard::experimental::serialized_size(observation)
            .expect("optical observation must serialize");
        if bytes > self.remaining_bytes {
            return false;
        }
        self.remaining_bytes -= bytes;
        self.remaining_count -= 1;
        true
    }
}

#[derive(Clone, Copy, Debug)]
struct OpticalCandidate {
    index: usize,
    luminosity_w: f64,
    flux: f64,
}

impl Ord for OpticalCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.flux
            .total_cmp(&other.flux)
            .then_with(|| other.index.cmp(&self.index))
    }
}

impl PartialOrd for OpticalCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for OpticalCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for OpticalCandidate {}

struct OpticalIdentity {
    physical: Id,
    id: Id,
    last_seen: u64,
}

#[derive(Default)]
pub(super) struct OpticalSession {
    identities: HashMap<Entity, OpticalIdentity>,
    pub(super) previous_entities: BTreeSet<Id>,
    previous_views: Vec<(u64, Option<Id>, Option<Id>)>,
}

impl OpticalSession {
    pub(super) fn references(&self) -> BTreeMap<Id, Id> {
        self.identities
            .values()
            .map(|value| (value.physical, value.id))
            .collect()
    }

    pub(super) fn observe(
        &mut self,
        world: &World,
        account: AccountId,
        views: &[ViewState],
        contacts: &BTreeMap<EntityId, BTreeMap<u64, SensorObservation>>,
    ) -> (Vec<OpticalObservation>, BTreeSet<Id>) {
        let _profile = crate::sim::diagnostics::ProfileScope::new("optical_observe");
        let tick = world.resource::<SimulationCounters>().ticks;
        let current_views: Vec<_> = views
            .iter()
            .map(|view| {
                let instance = view
                    .focused_ship
                    .and_then(|id| identity::lookup(world, id).ok())
                    .and_then(|entity| world.get::<SpatialInstance>(entity))
                    .map(|instance| instance.0);
                (view.id, view.focused_ship, instance)
            })
            .collect();
        if current_views != self.previous_views {
            self.previous_entities.clear();
            self.previous_views = current_views;
        }
        let mut visual_states = HashMap::<Entity, (Pose, ShipVisual, usize)>::new();
        let mut observations = Vec::new();
        let mut visible_entities = BTreeSet::new();
        for view in views {
            let Some(observer) = view
                .focused_ship
                .and_then(|id| observe(world, account, id).ok())
            else {
                continue;
            };
            if world
                .get::<super::super::travel::PresenceState>(observer)
                .is_some_and(|state| {
                    matches!(
                        state.0,
                        travel::Presence::Destroyed | travel::Presence::StoredInWreck(_)
                    )
                })
            {
                continue;
            }
            let mut budget = ViewBudget::new(views.len());
            let origin = world
                .get_resource::<SpatialIndex>()
                .and_then(|index| {
                    let slot = index.object_index(observer)?;
                    let pose = ship_pose(world, observer)?;
                    Some(
                        view.origin
                            .offset_by(index.objects()[slot].position.relative_to(pose.position)),
                    )
                })
                .unwrap_or(view.origin);
            let mut append = |entity: Entity, radius_m: f64, luminosity_w: f64| {
                if budget.remaining_count == 0 {
                    return true;
                }
                let Some(identity) = world.get::<Identity>(entity).map(|id| id.0) else {
                    return false;
                };
                let (pose, visual, visual_bytes) = match visual_states.entry(entity) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let Some(pose) = ship_pose(world, entity) else {
                            return false;
                        };
                        let Some(visual) = super::super::presentation::visual(world, entity) else {
                            return false;
                        };
                        let visual_bytes = postcard::experimental::serialized_size(&visual)
                            .expect("ship visual must serialize");
                        entry.insert((pose, visual, visual_bytes))
                    }
                };
                if *visual_bytes > budget.remaining_bytes {
                    return false;
                }
                let Some(instance) = world.get::<SpatialInstance>(entity) else {
                    return false;
                };
                let id = self
                    .identities
                    .get(&entity)
                    .map_or_else(Id::new, |identity| identity.id);
                let iff = world
                    .get::<super::super::identity::Transponder>(entity)
                    .filter(|value| value.0.enabled)
                    .map(|value| value.0.clone());
                let known_entity = (entity == observer
                    || iff.is_some()
                    || super::super::ownership::can_access(
                        world,
                        account,
                        entity,
                        ownership::Permission::Control,
                    ))
                .then_some(identity);
                let observation = OpticalObservation {
                    view: view.id,
                    id,
                    spatial_instance: identity::observation_spatial_instance(id, instance.0),
                    known_entity,
                    iff,
                    contact: view.focused_ship.and_then(|observer_id| {
                        let handle = world
                            .get::<super::super::sensors::Observations>(observer)?
                            .0
                            .targets
                            .get(&identity)?;
                        contacts.get(&observer_id)?.get(handle)?;
                        Some(ContactRef {
                            observer: observer_id,
                            contact: *handle,
                        })
                    }),
                    pose: pose.clone(),
                    radius_m,
                    luminosity_w,
                    appearance: world
                        .get::<identity::Appearance>(entity)
                        .map(|value| value.0),
                    visual: visual.clone(),
                };
                if budget.admit(&observation) {
                    self.identities.insert(
                        entity,
                        OpticalIdentity {
                            physical: identity,
                            id,
                            last_seen: tick,
                        },
                    );
                    visible_entities.insert(identity);
                    observations.push(observation);
                } else {
                    assert_ne!(entity, observer, "focused ship exceeds optical byte budget");
                }
                budget.remaining_count == 0
            };
            let index = world.get_resource::<SpatialIndex>();
            if let Some(design) = world.get::<ShipDesign>(observer) {
                let luminosity_w = index
                    .and_then(|index| {
                        index
                            .object_index(observer)
                            .map(|id| index.observed_luminosity(id, origin))
                    })
                    .unwrap_or(0.);
                append(observer, design.0.radius, luminosity_w);
            }
            if world
                .get::<super::super::travel::Dormant>(observer)
                .is_some()
            {
                continue;
            }
            let Some(index) = index else {
                continue;
            };
            let candidates: Vec<_> = index
                .visible(origin, 4. * std::f64::consts::PI * MIN_OPTICAL_FLUX_W_M2)
                .into_iter()
                .filter_map(|candidate| {
                    let object = &index.objects()[candidate];
                    if object.entity == observer
                        || world.get::<ShipDesign>(object.entity).is_none()
                        || world
                            .get::<super::super::travel::Dormant>(object.entity)
                            .is_some()
                    {
                        return None;
                    }
                    let luminosity_w = index.observed_luminosity(candidate, origin);
                    let flux = flux_w_m2(
                        luminosity_w,
                        object.position.relative_to(origin).length(),
                        object.radius_m,
                    );
                    (flux >= MIN_OPTICAL_FLUX_W_M2).then_some(OpticalCandidate {
                        index: candidate,
                        luminosity_w,
                        flux,
                    })
                })
                .collect();
            let mut candidates = BinaryHeap::from(candidates);
            while let Some(candidate) = candidates.pop() {
                if index.fully_occluded(observer, candidate.index, origin) {
                    continue;
                }
                let object = &index.objects()[candidate.index];
                if append(object.entity, object.radius_m, candidate.luminosity_w) {
                    break;
                }
            }
        }
        self.identities
            .retain(|_, identity| tick.saturating_sub(identity.last_seen) <= IDENTITY_GRACE_TICKS);
        observations.sort_unstable_by_key(|object| (object.view, object.id));
        (observations, visible_entities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{
        hardware::SensorRange, identity::Transponder, precision::PreciseTransform,
        spatial::SpatialObject,
    };
    use bevy::math::DVec3;

    struct Fixture {
        app: App,
        account: Id,
        observer: Entity,
        target: Entity,
        own_id: Id,
        target_id: Id,
        view: ViewState,
    }

    impl Fixture {
        fn new() -> Self {
            let account = Id::new();
            let mut app =
                crate::sim::bootstrap::provision_combat_fixture(&[account], None, None).unwrap();
            let world = app.world_mut();
            let observer = world
                .query_filtered::<Entity, With<crate::sim::vessel::ControlledVessel>>()
                .single(world)
                .unwrap();
            let target = world
                .query::<(Entity, &Transponder)>()
                .iter(world)
                .find(|(_, iff)| iff.0.labels.contains("Hostile patrol"))
                .unwrap()
                .0;
            let own_id = world.get::<Identity>(observer).unwrap().0;
            let target_id = world.get::<Identity>(target).unwrap().0;
            for (entity, distance) in [(observer, 0.), (target, 1000.)] {
                world
                    .get_mut::<PreciseTransform>(entity)
                    .unwrap()
                    .translation_um = GalacticPosition::ZERO.offset_by(DVec3::X * distance);
            }
            world.get_mut::<SensorRange>(observer).unwrap().0 = 0.;
            let mut index = SpatialIndex::default();
            for (entity, distance, luminosity) in [(observer, 0., 0.), (target, 1000., 100.)] {
                index.insert(SpatialObject {
                    entity,
                    position: GalacticPosition::ZERO.offset_by(DVec3::X * distance),
                    radius_m: 10.,
                    occludes: false,
                    optical_occludes: false,
                    optical_luminosity_w: luminosity,
                });
            }
            index.finish_geometry();
            world.insert_resource(index);
            Self {
                app,
                account,
                observer,
                target,
                own_id,
                target_id,
                view: ViewState {
                    id: 7,
                    revision: 1,
                    focused_ship: Some(own_id),
                    origin: GalacticPosition::ZERO,
                },
            }
        }
    }

    #[test]
    fn optical_access_depends_on_light_and_line_of_sight_not_shared_iff_or_sensors() {
        let mut fixture = Fixture::new();
        let mut optical = OpticalSession::default();
        let views = [fixture.view.clone()];
        fixture
            .app
            .world_mut()
            .get_mut::<Transponder>(fixture.target)
            .unwrap()
            .0
            .enabled = false;
        let world = fixture.app.world();
        let (observed, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert_eq!(world.get::<SensorRange>(fixture.observer).unwrap().0, 0.);
        assert!(
            observed
                .iter()
                .any(|object| object.known_entity == Some(fixture.own_id))
        );
        let unknown = observed
            .iter()
            .find(|object| object.known_entity.is_none())
            .unwrap();
        assert_ne!(unknown.id, fixture.target_id);
        assert_eq!(unknown.contact, None);
        assert!(unknown.appearance.is_some());
        let opaque_id = unknown.id;
        let world = fixture.app.world_mut();
        world
            .get_mut::<Transponder>(fixture.target)
            .unwrap()
            .0
            .enabled = true;
        let (observed, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        let identified = observed
            .iter()
            .find(|object| object.known_entity == Some(fixture.target_id))
            .unwrap();
        assert_eq!(identified.id, opaque_id);
        assert!(identified.iff.is_some());
        assert!(identified.contact.is_none());
        world
            .get_mut::<Transponder>(fixture.target)
            .unwrap()
            .0
            .enabled = false;
        let (unidentified, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        let anonymous = unidentified
            .iter()
            .find(|object| object.id == opaque_id)
            .unwrap();
        assert!(anonymous.iff.is_none());
        assert!(anonymous.known_entity.is_none());

        let world = fixture.app.world_mut();
        let target_index = world
            .resource::<SpatialIndex>()
            .object_index(fixture.target)
            .unwrap();
        world
            .resource_mut::<SpatialIndex>()
            .set_luminosity(target_index, 0.);
        let (dark, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert_eq!(dark.len(), 1);
        world.resource_mut::<SimulationCounters>().ticks += 5;
        world
            .resource_mut::<SpatialIndex>()
            .set_luminosity(target_index, 100.);
        let (lit_again, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert!(lit_again.iter().any(|object| object.id == opaque_id));

        let blocker = world.spawn_empty().id();
        world.resource_mut::<SpatialIndex>().insert(SpatialObject {
            entity: blocker,
            position: GalacticPosition::ZERO.offset_by(DVec3::X * 500.),
            radius_m: 100.,
            occludes: true,
            optical_occludes: true,
            optical_luminosity_w: 0.,
        });
        world.resource_mut::<SpatialIndex>().finish_geometry();
        let (occluded, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert_eq!(occluded.len(), 1);
        assert_eq!(occluded[0].known_entity, Some(fixture.own_id));
    }

    #[test]
    fn dormant_focus_receives_only_its_hangar_or_slip_scene() {
        let mut fixture = Fixture::new();
        let world = fixture.app.world_mut();
        world
            .entity_mut(fixture.observer)
            .insert(crate::sim::travel::Dormant);
        world
            .entity_mut(fixture.target)
            .insert(crate::sim::travel::DockingBays(vec![
                crate::sim::travel::Bay {
                    centre_m: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    radius_m: 100.0,
                    mass_capacity_kg: 1e9,
                    public: true,
                    allowed: Default::default(),
                    reservation: None,
                },
            ]));
        let mut optical = OpticalSession::default();
        for presence in [
            travel::Presence::Docked {
                host: fixture.target_id,
                bay: 0,
            },
            travel::Presence::SlipTransit(Id::new()),
        ] {
            world
                .entity_mut(fixture.observer)
                .insert(crate::sim::travel::PresenceState(presence));
            let (objects, visible) = optical.observe(
                world,
                fixture.account,
                &[fixture.view.clone()],
                &BTreeMap::new(),
            );
            assert_eq!(objects.len(), 1);
            assert_eq!(objects[0].known_entity, Some(fixture.own_id));
            assert_eq!(visible, BTreeSet::from([fixture.own_id]));
            assert!(objects[0].appearance.is_some());
        }
    }

    #[test]
    fn destroyed_or_wreck_stored_focus_has_no_optical_mesh_or_remote_scene() {
        let mut fixture = Fixture::new();
        let world = fixture.app.world_mut();
        let mut optical = OpticalSession::default();
        let views = [fixture.view.clone()];
        let (before, _) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert!(
            before
                .iter()
                .any(|object| object.known_entity == Some(fixture.own_id))
        );
        for presence in [
            travel::Presence::Destroyed,
            travel::Presence::StoredInWreck(fixture.target_id),
        ] {
            world.entity_mut(fixture.observer).insert((
                crate::sim::travel::Dormant,
                crate::sim::travel::PresenceState(presence),
            ));
            let (objects, visible) =
                optical.observe(world, fixture.account, &views, &BTreeMap::new());
            assert!(objects.is_empty());
            assert!(visible.is_empty());
        }
    }

    #[test]
    fn multiple_views_scope_visibility_and_transits_clear_previous_effect_access() {
        let mut fixture = Fixture::new();
        let world = fixture.app.world_mut();
        let mut second_view = fixture.view.clone();
        second_view.id = 8;
        second_view.origin = GalacticPosition::ZERO.offset_by(DVec3::Y * 1e12);
        let mut optical = OpticalSession::default();
        let views = [fixture.view.clone(), second_view];
        let (objects, visible) = optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert_eq!(objects.iter().filter(|object| object.view == 7).count(), 2);
        assert_eq!(objects.iter().filter(|object| object.view == 8).count(), 1);
        let own: Vec<_> = objects
            .iter()
            .filter(|object| object.known_entity == Some(fixture.own_id))
            .collect();
        assert_eq!(own[0].id, own[1].id);
        optical.previous_entities = visible;
        optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert!(!optical.previous_entities.is_empty());
        world
            .get_mut::<SpatialInstance>(fixture.observer)
            .unwrap()
            .0 = Id::new();
        optical.observe(world, fixture.account, &views, &BTreeMap::new());
        assert!(optical.previous_entities.is_empty());
    }

    #[test]
    fn heap_selection_matches_sorted_publication_with_occlusion_and_variable_byte_costs() {
        let observations: Vec<_> = (0..200_usize)
            .map(|index| OpticalObservation {
                view: 128,
                id: Id((index as u128).to_le_bytes()),
                spatial_instance: Id::new(),
                iff: None,
                known_entity: (index % 2 == 0).then(Id::new),
                contact: None,
                pose: Pose::default(),
                radius_m: 10.0,
                luminosity_w: 100.0,
                appearance: (index % 3 == 0).then_some([7; 32]),
                visual: ShipVisual {
                    slip_readiness: 0.0,
                    engines: (0..(index % 7) * 128)
                        .map(|part| EngineVisual {
                            part: part as u64,
                            thrust_n: [0.0, 0.0, 1000.0],
                            thrust_fraction: 0.5,
                        })
                        .collect(),
                    turrets: Vec::new(),
                    shield: None,
                },
            })
            .collect();
        let candidates: Vec<_> = (0..observations.len())
            .rev()
            .map(|index| OpticalCandidate {
                index,
                luminosity_w: 100.0,
                flux: (index * 17 % 11) as f64,
            })
            .collect();
        let mut sorted = candidates.clone();
        sorted.sort_unstable_by(|a, b| b.flux.total_cmp(&a.flux).then(a.index.cmp(&b.index)));
        let encoded_sizes: Vec<_> = observations
            .iter()
            .map(|observation| postcard::to_allocvec(observation).unwrap().len())
            .collect();

        let mut rejected_large = false;
        let mut accepted_after_rejection = false;
        for byte_limit in [0, 200, 4096, 100_000, MAX_OPTICAL_BYTES] {
            for count_limit in [1, 4, observations.len()] {
                let mut remaining_bytes = byte_limit;
                let mut expected = Vec::new();
                for candidate in &sorted {
                    if candidate.index % 5 == 0 {
                        continue;
                    }
                    let size = encoded_sizes[candidate.index];
                    if size > remaining_bytes {
                        continue;
                    }
                    remaining_bytes -= size;
                    expected.push(candidate.index);
                    if expected.len() == count_limit {
                        break;
                    }
                }

                let mut budget = ViewBudget {
                    remaining_bytes: byte_limit,
                    remaining_count: count_limit,
                };
                let mut heap = BinaryHeap::from(candidates.clone());
                let mut actual = Vec::new();
                let mut rejected = false;
                while let Some(candidate) = heap.pop() {
                    if candidate.index % 5 == 0 {
                        continue;
                    }
                    if budget.admit(&observations[candidate.index]) {
                        accepted_after_rejection |= rejected;
                        actual.push(candidate.index);
                        if budget.remaining_count == 0 {
                            break;
                        }
                    } else {
                        rejected = true;
                        rejected_large = true;
                    }
                }

                assert_eq!(actual, expected);
                assert_eq!(budget.remaining_bytes, remaining_bytes);
                assert_eq!(budget.remaining_count, count_limit - actual.len());
            }
        }
        assert!(rejected_large && accepted_after_rejection);
    }

    #[test]
    fn dense_complex_ships_respect_serialized_budget_and_keep_each_views_focus() {
        let focus = Id::new();
        let heavy = OpticalObservation {
            view: 0,
            id: Id::new(),
            spatial_instance: Id::new(),
            iff: None,
            known_entity: Some(focus),
            contact: None,
            pose: Pose::default(),
            radius_m: 100.,
            luminosity_w: 1e6,
            appearance: Some([7; 32]),
            visual: ShipVisual {
                slip_readiness: 0.0,
                engines: (0..4096)
                    .map(|part| EngineVisual {
                        part,
                        thrust_n: [0., 0., 1e6],
                        thrust_fraction: 1.,
                    })
                    .collect(),
                turrets: (0..4096)
                    .map(|part| TurretVisual {
                        part,
                        yaw_rad: 1.,
                        pitch_rad: 1.,
                    })
                    .collect(),
                shield: None,
            },
        };
        let mut published = Vec::new();
        let mut counts = Vec::new();
        for view in 0..8 {
            let mut budget = ViewBudget::new(8);
            let mut own = heavy.clone();
            own.view = view;
            assert!(budget.admit(&own));
            published.push(own);
            let mut rejected = false;
            for _ in 0..64 {
                let mut candidate = heavy.clone();
                candidate.view = view;
                candidate.id = Id::new();
                candidate.known_entity = None;
                if budget.admit(&candidate) {
                    published.push(candidate);
                } else {
                    rejected = true;
                }
            }
            assert!(rejected);
            let mut small = heavy.clone();
            small.view = view;
            small.id = Id::new();
            small.known_entity = None;
            small.visual.engines.clear();
            small.visual.turrets.clear();
            assert!(budget.admit(&small));
            published.push(small);
            let count = published
                .iter()
                .filter(|object| object.view == view)
                .count();
            counts.push(count);
            assert_eq!(
                published
                    .iter()
                    .filter(|object| { object.view == view && object.known_entity == Some(focus) })
                    .count(),
                1
            );
        }
        assert!(counts.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(published.len() <= MAX_OPTICAL_OBSERVATIONS);
        assert!(postcard::to_allocvec(&published).unwrap().len() <= MAX_OPTICAL_BYTES);
    }
}
