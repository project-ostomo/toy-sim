use crate::sim::precision::GalacticPosition;
use bevy::{math::DVec3, prelude::*};
use osg_space::spatial::{GalacticIndex, QueryBudget, QueryError, SpatialRecord as HashRecord};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpatialKey {
    Catalogue(usize),
    Entity(Entity),
}

#[derive(Clone, Copy)]
pub struct SpatialObject {
    pub entity: Entity,
    pub position: GalacticPosition,
    pub radius_m: f64,
    pub occludes: bool,
    pub optical_occludes: bool,
    pub optical_luminosity_w: f64,
}

#[derive(Resource, Clone, Default)]
pub struct SpatialIndex {
    pub objects: Vec<SpatialObject>,
    entities: ahash::AHashMap<Entity, usize>,
    targets: Vec<usize>,
    /// Physical radius bound for segment queries over `objects`.
    maximum_object_radius_m: f64,
    pub velocities: ahash::AHashMap<Entity, DVec3>,
    pub capture_radii: ahash::AHashMap<Entity, f64>,
    collision_entities: ahash::AHashSet<Entity>,
    pub tick_seconds: f64,
    pub tick: u64,
    celestial_systems: ahash::AHashSet<usize>,
    illumination: Vec<OnceLock<Illumination>>,
    pub sky: super::lighting::Sky,
    pub hash: std::sync::Arc<std::sync::RwLock<GalacticIndex<SpatialKey>>>,
    indexed_entities: ahash::AHashSet<Entity>,
    indexed_universe: Option<std::sync::Arc<osg_universe::universe::Universe>>,
    collecting: bool,
}

#[derive(Clone, Default)]
struct Illumination {
    emitted_w: f64,
    reflected: Vec<(GalacticPosition, f64)>,
}

impl SpatialIndex {
    /// Immutable sensor metadata sharing the persistent hash, without copying
    /// catalogue records or optical caches. Consumers run between scene updates.
    pub(crate) fn sensor_snapshot(&self) -> Self {
        Self {
            objects: self.objects.clone(),
            entities: self.entities.clone(),
            targets: self.targets.clone(),
            maximum_object_radius_m: self.maximum_object_radius_m,
            hash: self.hash.clone(),
            ..Default::default()
        }
    }

    pub fn clear(&mut self) {
        let _profile = crate::sim::diagnostics::ProfileScope::new("spatial_clear");
        self.objects.clear();
        self.maximum_object_radius_m = 0.0;
        self.entities.clear();
        self.targets.clear();
        self.velocities.clear();
        self.capture_radii.clear();
        self.collision_entities.clear();
        self.celestial_systems.clear();
        self.illumination.clear();
        self.sky.local.clear();
        self.collecting = true;
    }

    pub fn insert(&mut self, object: SpatialObject) {
        self.insert_object(object, None);
    }

    pub fn insert_celestial(&mut self, object: SpatialObject, system: usize) {
        self.insert_object(object, Some(system));
    }

    fn insert_object(&mut self, object: SpatialObject, system: Option<usize>) {
        self.maximum_object_radius_m = self.maximum_object_radius_m.max(object.radius_m);
        let id = self.objects.len();
        if let Some(system) = system {
            self.celestial_systems.insert(system);
        } else {
            self.targets.push(id);
        }
        self.entities.insert(object.entity, id);
        self.objects.push(object);
        self.illumination.push(OnceLock::new());
        if !self.collecting {
            self.sync_object(id);
        }
    }

    pub fn finish_geometry(&mut self) {
        let _profile = crate::sim::diagnostics::ProfileScope::new("spatial_finish_geometry");
        self.collecting = false;
        for id in 0..self.objects.len() {
            self.sync_object(id);
        }
        let mut present: ahash::AHashSet<_> = self.entities.keys().copied().collect();
        present.extend(self.collision_entities.iter().copied());
        for &entity in self.indexed_entities.difference(&present) {
            self.hash
                .write()
                .unwrap()
                .remove(&SpatialKey::Entity(entity));
        }
        self.indexed_entities = present;
    }

    pub fn insert_collision(&mut self, entity: Entity, position: GalacticPosition, radius_m: f64) {
        self.collision_entities.insert(entity);
        self.indexed_entities.insert(entity);
        if !self.entities.contains_key(&entity) {
            self.hash
                .write()
                .unwrap()
                .insert(
                    SpatialKey::Entity(entity),
                    HashRecord {
                        position,
                        radius_m,
                        luminosity: 0.0,
                    },
                )
                .expect("collision body coordinate range");
        }
    }

    pub fn seed_catalogue(&mut self) {
        if self.indexed_universe.as_ref().map(std::sync::Arc::as_ptr)
            == self.sky.universe.as_ref().map(std::sync::Arc::as_ptr)
        {
            return;
        }
        *self.hash.write().unwrap() = GalacticIndex::new();
        self.indexed_entities.clear();
        self.indexed_universe = self.sky.universe.clone();
        if let Some(universe) = &self.indexed_universe {
            for (id, entry) in universe.index.entries.iter().enumerate() {
                self.hash
                    .write()
                    .unwrap()
                    .insert(
                        SpatialKey::Catalogue(id),
                        HashRecord {
                            position: entry.position,
                            radius_m: 0.0,
                            luminosity: entry.luminosity / super::lighting::LUMENS_PER_OPTICAL_WATT,
                        },
                    )
                    .expect("catalogue coordinate range");
            }
        }
    }

    fn sync_object(&mut self, id: usize) {
        let object = self.objects[id];
        self.hash
            .write()
            .unwrap()
            .insert(
                SpatialKey::Entity(object.entity),
                HashRecord {
                    position: object.position,
                    radius_m: object.radius_m.max(
                        self.capture_radii
                            .get(&object.entity)
                            .copied()
                            .unwrap_or(0.0),
                    ),
                    luminosity: if self.targets.binary_search(&id).is_ok() {
                        self.peak(id)
                    } else {
                        0.0
                    },
                },
            )
            .expect("body coordinate range");
    }

    fn peak(&self, id: usize) -> f64 {
        let _profile = crate::sim::diagnostics::ProfileScope::new("spatial_luminosity_bounds");
        let object = self.objects[id];
        self.illumination[id].get().map_or_else(
            || object.optical_luminosity_w + self.sky.peak(object.position, object.radius_m),
            |light| light.emitted_w + light.reflected.iter().map(|(_, power)| power).sum::<f64>(),
        )
    }

    pub fn segment_candidates(&self, origin: GalacticPosition, displacement: DVec3) -> Vec<usize> {
        let _profile = crate::sim::diagnostics::ProfileScope::new("sensor_segment_query");
        let mut budget = QueryBudget::new(usize::MAX);
        let candidates = self
            .hash
            .read()
            .unwrap()
            .segment_candidates_filtered(
                origin,
                displacement,
                0.0,
                self.maximum_object_radius_m,
                &mut budget,
                |key| self.key_object(key).map(|id| self.objects[id].radius_m),
            )
            .expect("scene query coordinates");
        #[cfg(test)]
        {
            use crate::sim::diagnostics::samples::count;
            count("segment.rays", 1);
            count("segment.probes", budget.probes());
            count(
                "segment.candidates_examined",
                budget.used() - budget.probes(),
            );
            count("segment.intersections", candidates.len());
        }
        candidates
            .into_iter()
            .filter_map(|key| self.key_object(key))
            .collect()
    }

    fn key_object(&self, key: SpatialKey) -> Option<usize> {
        match key {
            SpatialKey::Entity(entity) => self.entities.get(&entity).copied(),
            SpatialKey::Catalogue(_) => None,
        }
    }

    pub fn overlap_candidates(
        &self,
        position: GalacticPosition,
        radius: f64,
        budget: &mut QueryBudget,
    ) -> Result<Vec<Entity>, QueryError> {
        Ok(self
            .hash
            .read()
            .unwrap()
            .within_radius_budgeted(position, radius, true, budget)?
            .into_iter()
            .filter_map(|key| self.key_object(key))
            .map(|id| self.objects[id].entity)
            .collect())
    }

    #[cfg(test)]
    pub fn set_luminosity(&mut self, id: usize, luminosity_w: f64) {
        self.objects[id].optical_luminosity_w = luminosity_w;
        self.illumination[id] = OnceLock::from(Illumination {
            emitted_w: luminosity_w,
            reflected: Vec::new(),
        });
        self.sync_object(id);
    }

    #[cfg(test)]
    pub fn set_illumination(&mut self, id: usize, emitted_w: f64, reflected: Vec<(usize, f64)>) {
        let peak = emitted_w + reflected.iter().map(|(_, power)| power).sum::<f64>();
        self.set_luminosity(id, peak);
        self.illumination[id] = OnceLock::from(Illumination {
            emitted_w,
            reflected: reflected
                .into_iter()
                .map(|(source, power)| (self.objects[source].position, power))
                .collect(),
        });
    }

    pub fn observed_luminosity(&self, target: usize, observer: GalacticPosition) -> f64 {
        let illumination = self.illumination[target].get_or_init(|| Illumination {
            emitted_w: self.objects[target].optical_luminosity_w,
            reflected: super::lighting::reflection_sources(self, target, &self.sky),
        });
        let position = self.objects[target].position;
        let view = observer.relative_to(position).normalize_or_zero();
        illumination.emitted_w
            + illumination
                .reflected
                .iter()
                .map(|&(source, peak)| {
                    let light = source.relative_to(position).normalize_or_zero();
                    let cosine = light.dot(view).clamp(-1.0, 1.0);
                    let phase = cosine.acos();
                    let fraction = (phase.sin() + (std::f64::consts::PI - phase) * cosine)
                        / std::f64::consts::PI;
                    peak * fraction.clamp(0.0, 1.0)
                })
                .sum::<f64>()
    }

    pub fn object_index(&self, entity: Entity) -> Option<usize> {
        self.entities.get(&entity).copied()
    }

    pub fn within_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        self.hash
            .read()
            .unwrap()
            .within_radius(centre, radius, false)
            .expect("scene query coordinates")
            .into_iter()
            .filter_map(|key| self.key_object(key))
            .filter(|id| self.targets.binary_search(id).is_ok())
            .collect()
    }

    pub fn visible(
        &self,
        centre: GalacticPosition,
        min_luminosity_over_distance2: f64,
    ) -> Vec<usize> {
        self.hash
            .read()
            .unwrap()
            .visibility_candidates(centre, min_luminosity_over_distance2, 0.0)
            .expect("scene query coordinates")
            .into_iter()
            .filter_map(|key| self.key_object(key))
            .filter(|id| self.targets.binary_search(id).is_ok())
            .filter_map(|id| {
                let object = &self.objects[id];
                let peak = self.illumination[id].get().map_or_else(
                    || {
                        object.optical_luminosity_w
                            + self.sky.peak(object.position, object.radius_m)
                    },
                    |light| {
                        light.emitted_w
                            + light.reflected.iter().map(|(_, power)| power).sum::<f64>()
                    },
                );
                let distance2 = object.position.relative_to(centre).length_squared();
                (peak >= min_luminosity_over_distance2 * distance2).then_some(id)
            })
            .collect()
    }

    pub fn nearest(
        &self,
        observer: Entity,
        centre: GalacticPosition,
        radius: f64,
        n: usize,
    ) -> Vec<usize> {
        if radius.is_finite() {
            let mut found = {
                let _profile = crate::sim::diagnostics::ProfileScope::new("sensor_range_query");
                self.within_range(centre, radius)
            };
            #[cfg(test)]
            crate::sim::diagnostics::samples::count("sensor.range_results", found.len());
            let _profile = crate::sim::diagnostics::ProfileScope::new("sensor_sort_truncate");
            found.retain(|&id| self.objects[id].entity != observer);
            found.sort_unstable_by(|&a, &b| {
                self.objects[a]
                    .position
                    .relative_to(centre)
                    .length_squared()
                    .total_cmp(
                        &self.objects[b]
                            .position
                            .relative_to(centre)
                            .length_squared(),
                    )
            });
            found.truncate(n);
            return found;
        }
        let available = self.targets.len().saturating_sub(usize::from(
            self.object_index(observer)
                .is_some_and(|id| self.targets.binary_search(&id).is_ok()),
        ));
        self.hash
            .read()
            .unwrap()
            .nearest_many(
                centre,
                n.min(available),
                &mut QueryBudget::new(usize::MAX),
                |key| {
                    self.key_object(key).is_some_and(|id| {
                        self.targets.binary_search(&id).is_ok()
                            && self.objects[id].entity != observer
                    })
                },
            )
            .expect("scene query coordinates")
            .into_iter()
            .filter(|(_, distance)| *distance <= radius)
            .filter_map(|(key, _)| self.key_object(key))
            .collect()
    }

    pub fn occluders_in_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        self.hash
            .read()
            .unwrap()
            .within_radius(centre, radius, true)
            .expect("scene query coordinates")
            .into_iter()
            .filter_map(|key| self.key_object(key))
            .filter(|&id| {
                let object = self.objects[id];
                object.occludes
                    && object.position.relative_to(centre).length() <= radius + object.radius_m
            })
            .collect()
    }

    pub fn occluded(&self, observer: Entity, target: usize, centre: GalacticPosition) -> bool {
        let endpoint = self.objects[target].position.relative_to(centre);
        self.segment_candidates(centre, endpoint)
            .into_iter()
            .any(|id| {
                let object = self.objects[id];
                object.occludes
                    && object.entity != observer
                    && id != target
                    && sphere_blocks(
                        endpoint,
                        object.position.relative_to(centre),
                        object.radius_m,
                    )
            })
    }

    pub fn fully_occluded(
        &self,
        observer: Entity,
        target: usize,
        centre: GalacticPosition,
    ) -> bool {
        let target_object = self.objects[target];
        let endpoint = target_object.position.relative_to(centre);
        self.any_optical_blocker_on_segment(centre, endpoint, |id| {
            let object = self.objects[id];
            object.entity != observer
                && id != target
                && sphere_fully_blocks(
                    endpoint,
                    target_object.radius_m,
                    object.position.relative_to(centre),
                    object.radius_m,
                )
        }) || self
            .sky
            .uninstantiated_occlusion(self, centre, endpoint, target_object.radius_m)
    }

    pub fn has_celestial_system(&self, system: usize) -> bool {
        self.celestial_systems.contains(&system)
    }

    pub fn any_optical_blocker_on_segment(
        &self,
        origin: GalacticPosition,
        displacement: DVec3,
        mut blocks: impl FnMut(usize) -> bool,
    ) -> bool {
        self.segment_candidates(origin, displacement)
            .into_iter()
            .any(|id| self.objects[id].optical_occludes && blocks(id))
    }
}

pub fn sphere_blocks(target: DVec3, centre: DVec3, radius: f64) -> bool {
    let length2 = target.length_squared();
    if length2 <= 0.0 || radius <= 0.0 {
        return false;
    }
    let t = (centre.dot(target) / length2).clamp(0.0, 1.0);
    let closest = target * t;
    (closest - centre).length_squared() < radius * radius * (1.0 - 1e-12)
}

pub fn sphere_fully_blocks(
    target: DVec3,
    target_radius: f64,
    blocker: DVec3,
    blocker_radius: f64,
) -> bool {
    let target_distance = target.length();
    let blocker_distance = blocker.length();
    if target_distance <= target_radius
        || blocker_distance <= blocker_radius
        || blocker_radius <= 0.0
    {
        return false;
    }
    let target_angle = (target_radius / target_distance).asin();
    let blocker_angle = (blocker_radius / blocker_distance).asin();
    let separation = target.cross(blocker).length().atan2(target.dot(blocker));
    if separation + target_angle >= blocker_angle {
        return false;
    }
    let furthest_entry_distance = (blocker_distance.powi(2) - blocker_radius.powi(2)).sqrt();
    target_distance - target_radius > furthest_entry_distance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflected_phase_changes_observed_brightness_without_hiding_candidate() {
        let mut world = World::new();
        let mut index = SpatialIndex::default();
        for position in [DVec3::ZERO, DVec3::X * 1e9] {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: GalacticPosition::from_meters(position),
                radius_m: 1.0,
                occludes: false,
                optical_occludes: false,
                optical_luminosity_w: 0.0,
            });
        }
        index.set_illumination(0, 10.0, vec![(1, 1000.0)]);
        let bright = GalacticPosition::from_meters(DVec3::X * 1000.0);
        let dark = GalacticPosition::from_meters(DVec3::NEG_X * 1000.0);
        let quarter = GalacticPosition::from_meters(DVec3::Y * 1000.0);
        assert_eq!(index.observed_luminosity(0, bright), 1010.0);
        assert!((index.observed_luminosity(0, dark) - 10.0).abs() < 1e-9);
        assert!(
            (index.observed_luminosity(0, quarter) - (10.0 + 1000.0 / std::f64::consts::PI)).abs()
                < 1e-9
        );
        assert_eq!(index.visible(bright, 0.001), vec![0]);
        assert_eq!(index.visible(dark, 0.001), vec![0]);
    }

    #[test]
    fn optical_occlusion_keeps_limb_targets_and_ignores_hollow_ship_bounds() {
        let target = DVec3::X * 100.0;
        assert!(sphere_blocks(target, DVec3::new(50.0, 4.9, 0.0), 5.0));
        assert!(!sphere_fully_blocks(
            target,
            2.0,
            DVec3::new(50.0, 4.9, 0.0),
            5.0
        ));
        assert!(sphere_fully_blocks(target, 2.0, DVec3::X * 50.0, 5.0));
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        for optical_occludes in [false, true] {
            let mut index = SpatialIndex::default();
            for (position, radius) in [(target, 2.0), (DVec3::X * 50.0, 5.0)] {
                index.insert(SpatialObject {
                    entity: world.spawn_empty().id(),
                    position: GalacticPosition::from_meters(position),
                    radius_m: radius,
                    occludes: true,
                    optical_occludes,
                    optical_luminosity_w: 0.0,
                });
            }
            assert_eq!(
                index.fully_occluded(observer, 0, GalacticPosition::ZERO),
                optical_occludes
            );
            assert!(index.occluded(observer, 0, GalacticPosition::ZERO));
        }
    }

    #[test]
    fn segment_candidates_only_hide_target_after_exact_full_occlusion() {
        let mut world = World::new();
        let observer = world.spawn_empty().id();
        let origin = GalacticPosition::splat(1_i128 << 100);
        let mut index = SpatialIndex::default();
        for (position, radius_m) in [
            (DVec3::X * 100.0, 2.0),
            (DVec3::new(50.0, 4.9, 0.0), 5.0),
            (DVec3::ZERO, 20.0),
            (DVec3::X * 101.0, 5.0),
        ] {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(position),
                radius_m,
                occludes: true,
                optical_occludes: true,
                optical_luminosity_w: 0.0,
            });
        }

        assert!(!index.fully_occluded(observer, 0, origin));
        index.insert(SpatialObject {
            entity: world.spawn_empty().id(),
            position: origin.offset_by(DVec3::X * 30.0),
            radius_m: 5.0,
            occludes: true,
            optical_occludes: true,
            optical_luminosity_w: 0.0,
        });
        assert!(index.fully_occluded(observer, 0, origin));
    }
}
