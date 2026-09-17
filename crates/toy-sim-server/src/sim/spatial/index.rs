use crate::sim::precision::GalacticPosition;
use bevy::prelude::*;
#[derive(Clone, Copy)]
pub struct SpatialObject {
    pub entity: Entity,
    pub position: GalacticPosition,
    pub radius_m: f64,
    pub occludes: bool,
}

/// Snapshots share the immutable index during controller execution. Rebuilds mutate
/// in place once the previous tick's synchronous callbacks have released it.
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

#[derive(Clone)]
pub struct SpatialData {
    pub objects: Vec<SpatialObject>,
    excluded_targets: std::collections::HashSet<Entity>,
    targets: std::sync::OnceLock<crate::sim::spatial_tree::RegionIndex>,
    occluders: std::sync::OnceLock<(GalacticPosition, parry3d_f64::partitioning::Bvh)>,
}

impl Default for SpatialData {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            excluded_targets: Default::default(),
            targets: Default::default(),
            occluders: Default::default(),
        }
    }
}

impl SpatialData {
    pub fn clear(&mut self) {
        self.objects.clear();
        self.excluded_targets.clear();
        self.targets.take();
        self.occluders.take();
    }

    pub fn insert(&mut self, object: SpatialObject) {
        assert!(object.radius_m.is_finite() && object.radius_m >= 0.0);
        self.objects.push(object);
        self.targets.take();
        self.occluders.take();
    }

    pub fn exclude_sensor_target(&mut self, entity: Entity) {
        self.excluded_targets.insert(entity);
        self.targets.take();
    }

    pub fn targets(&self) -> &crate::sim::spatial_tree::RegionIndex {
        self.targets.get_or_init(|| {
            let proxies: Vec<_> = self
                .objects
                .iter()
                .enumerate()
                .filter(|(_, object)| !self.excluded_targets.contains(&object.entity))
                .map(|(id, o)| crate::sim::spatial_tree::Proxy {
                    id: id as u32,
                    position: o.position,
                    displacement: bevy::math::DVec3::ZERO,
                    radius: 0.0,
                })
                .collect();
            crate::sim::spatial_tree::RegionIndex::build(&proxies)
        })
    }

    pub fn blockers(&self) -> &(GalacticPosition, parry3d_f64::partitioning::Bvh) {
        self.occluders.get_or_init(|| {
            let anchor = self
                .objects
                .first()
                .map_or(GalacticPosition::ZERO, |o| o.position);
            use parry3d_f64::partitioning::{Bvh, BvhBuildStrategy};
            let leaves: Vec<_> = self
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| o.occludes)
                .map(|(i, o)| {
                    let p = o.position.relative_to(anchor);
                    let r = bevy::math::DVec3::splat(o.radius_m);
                    (i as u32, crate::sim::spatial_tree::bounds(p - r, p + r))
                })
                .collect();
            let tree = if leaves.len() <= 2 {
                // Parry's one/two-leaf bulk constructors assume dense IDs.
                let mut tree = Bvh::new();
                for &(id, bounds) in &leaves {
                    tree.insert(bounds, id);
                }
                tree
            } else {
                Bvh::from_iter(
                    BvhBuildStrategy::Binned,
                    leaves.into_iter().map(|(id, aabb)| (id as usize, aabb)),
                )
            };
            (anchor, tree)
        })
    }

    pub fn within_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        if !radius.is_finite() || radius < 0.0 {
            return Vec::new();
        }
        self.targets()
            .range(centre, radius)
            .into_iter()
            .map(|id| id as usize)
            .filter(|&id| {
                self.objects[id]
                    .position
                    .relative_to(centre)
                    .length_squared()
                    <= radius * radius
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
        if n == 0 {
            return Vec::new();
        }
        self.targets()
            .nearest(centre, radius, n, |id| {
                let o = self.objects[id as usize];
                (o.entity != observer).then(|| {
                    (
                        o.position.relative_to(centre).length_squared(),
                        o.entity.to_bits(),
                    )
                })
            })
            .into_iter()
            .map(|id| id as usize)
            .collect()
    }

    pub fn occluders_in_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        let (anchor, tree) = self.blockers();
        let p = centre.relative_to(*anchor);
        let r = bevy::math::DVec3::splat(radius);
        tree.intersect_aabb(&crate::sim::spatial_tree::bounds(p - r, p + r))
            .map(|i| i as usize)
            .filter(|&i| {
                let o = self.objects[i];
                o.position.relative_to(centre).length_squared() <= (radius + o.radius_m).powi(2)
            })
            .collect()
    }

    pub fn occluded(&self, observer: Entity, target: usize, centre: GalacticPosition) -> bool {
        let (anchor, tree) = self.blockers();
        let p = centre.relative_to(*anchor);
        let endpoint = self.objects[target].position.relative_to(centre);
        let ray = parry3d_f64::query::Ray::new(
            crate::sim::spatial_tree::vector(p),
            crate::sim::spatial_tree::vector(endpoint),
        );
        tree.cast_ray(&ray, 1.0, |id, _| {
            let o = self.objects[id as usize];
            (o.entity != observer
                && id as usize != target
                && sphere_blocks(endpoint, o.position.relative_to(centre), o.radius_m))
            .then_some(0.0)
        })
        .is_some()
    }

    pub fn occupied_cells(&self) -> usize {
        self.targets().regions.len()
    }
}

pub fn sphere_blocks(target: bevy::math::DVec3, centre: bevy::math::DVec3, radius: f64) -> bool {
    let length2 = target.length_squared();
    if length2 <= 0.0 || radius <= 0.0 {
        return false;
    }
    let t = (centre.dot(target) / length2).clamp(0.0, 1.0);
    let closest = target * t;
    (closest - centre).length_squared() < radius * radius * (1.0 - 1e-12)
}
