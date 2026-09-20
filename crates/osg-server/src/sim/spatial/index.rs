use crate::sim::precision::GalacticPosition;
use bevy::{math::DVec3, prelude::*};
use osg_spatial::{Entry, SpatialHash};
use std::ops::ControlFlow;

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
pub struct SpatialIndex(pub std::sync::Arc<SpatialData>);
impl std::ops::Deref for SpatialIndex {
    type Target = SpatialData;
    fn deref(&self) -> &SpatialData {
        &self.0
    }
}
impl std::ops::DerefMut for SpatialIndex {
    fn deref_mut(&mut self) -> &mut SpatialData {
        std::sync::Arc::make_mut(&mut self.0)
    }
}

#[derive(Clone, Default)]
pub struct SpatialData {
    pub objects: Vec<SpatialObject>,
    entities: ahash::AHashMap<Entity, usize>,
    excluded_targets: ahash::AHashSet<Entity>,
    hash: SpatialHash,
    optical_occluders: SpatialHash,
    illumination: Vec<Illumination>,
}

#[derive(Clone, Default)]
struct Illumination {
    emitted_w: f64,
    reflected: Vec<(usize, f64)>,
}

impl SpatialData {
    pub fn clear(&mut self) {
        self.objects.clear();
        self.entities.clear();
        self.excluded_targets.clear();
        self.hash.clear();
        self.optical_occluders.clear();
        self.illumination.clear();
    }

    pub fn insert(&mut self, object: SpatialObject) {
        let id = self.objects.len();
        assert!(id < u32::MAX as usize);
        self.hash.insert(
            id as u32,
            Entry {
                position: object.position,
                radius_m: object.radius_m,
                luminosity: object.optical_luminosity_w,
            },
        );
        if object.optical_occludes {
            self.optical_occluders.insert(
                id as u32,
                Entry {
                    position: object.position,
                    radius_m: object.radius_m,
                    luminosity: 0.0,
                },
            );
        }
        self.entities.insert(object.entity, id);
        self.objects.push(object);
        self.illumination.push(Illumination {
            emitted_w: object.optical_luminosity_w,
            reflected: Vec::new(),
        });
    }

    pub fn set_luminosity(&mut self, id: usize, luminosity_w: f64) {
        self.objects[id].optical_luminosity_w = luminosity_w;
        self.hash.set_luminosity(id as u32, luminosity_w);
        self.illumination[id] = Illumination {
            emitted_w: luminosity_w,
            reflected: Vec::new(),
        };
    }

    pub fn set_illumination(&mut self, id: usize, emitted_w: f64, reflected: Vec<(usize, f64)>) {
        let peak = emitted_w + reflected.iter().map(|(_, power)| power).sum::<f64>();
        self.set_luminosity(id, peak);
        self.illumination[id] = Illumination {
            emitted_w,
            reflected,
        };
    }

    pub fn observed_luminosity(&self, target: usize, observer: GalacticPosition) -> f64 {
        let illumination = &self.illumination[target];
        let position = self.objects[target].position;
        let view = observer.relative_to(position).normalize_or_zero();
        illumination.emitted_w
            + illumination
                .reflected
                .iter()
                .map(|&(source, peak)| {
                    let light = self.objects[source]
                        .position
                        .relative_to(position)
                        .normalize_or_zero();
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

    pub fn exclude_sensor_target(&mut self, entity: Entity) {
        self.excluded_targets.insert(entity);
    }

    pub fn all_in_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        self.hash
            .within_radius(centre, radius)
            .ids
            .into_iter()
            .map(|id| id as usize)
            .collect()
    }

    pub fn within_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        self.all_in_range(centre, radius)
            .into_iter()
            .filter(|&id| !self.excluded_targets.contains(&self.objects[id].entity))
            .collect()
    }

    pub fn visible(
        &self,
        centre: GalacticPosition,
        min_luminosity_over_distance2: f64,
    ) -> Vec<usize> {
        self.hash
            .visible(centre, min_luminosity_over_distance2)
            .ids
            .into_iter()
            .map(|id| id as usize)
            .collect()
    }

    pub fn nearest(
        &self,
        observer: Entity,
        centre: GalacticPosition,
        radius: f64,
        n: usize,
    ) -> Vec<usize> {
        self.hash
            .nearest_filtered(centre, radius, n, |id| {
                let object = self.objects[id as usize];
                (object.entity != observer && !self.excluded_targets.contains(&object.entity))
                    .then_some(object.entity.to_bits())
            })
            .into_iter()
            .map(|id| id as usize)
            .collect()
    }

    pub fn occluders_in_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        self.hash
            .intersecting_sphere(centre, radius)
            .ids
            .into_iter()
            .map(|id| id as usize)
            .filter(|&id| self.objects[id].occludes)
            .collect()
    }

    pub fn occluded(&self, observer: Entity, target: usize, centre: GalacticPosition) -> bool {
        let endpoint = self.objects[target].position.relative_to(centre);
        self.hash
            .visit_segment_candidates(centre, endpoint, 0.0, |id| {
                let object = self.objects[id as usize];
                if object.occludes
                    && object.entity != observer
                    && id as usize != target
                    && sphere_blocks(
                        endpoint,
                        object.position.relative_to(centre),
                        object.radius_m,
                    )
                {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .0
            .is_break()
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
        })
    }

    pub fn any_optical_blocker_on_segment(
        &self,
        origin: GalacticPosition,
        displacement: DVec3,
        mut blocks: impl FnMut(usize) -> bool,
    ) -> bool {
        self.optical_occluders
            .visit_segment_candidates(origin, displacement, 0.0, |id| {
                if blocks(id as usize) {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .0
            .is_break()
    }

    pub fn occupied_cells(&self) -> usize {
        self.hash.occupied_cells()
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
