//! Regional BVHs for ship centres and a separate extent BVH for occlusion.
use crate::{
    GameState,
    precision::{GalacticPosition, PreciseTransform},
};

use bevy::prelude::*;

#[derive(Component, Clone, Copy)]
pub struct SpatialBody {
    pub radius_m: f64,
    pub occludes: bool,
}

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
pub struct SpatialIndex(std::sync::Arc<SpatialData>);
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
    targets: std::sync::OnceLock<crate::spatial_tree::RegionIndex>,
    occluders: std::sync::OnceLock<(GalacticPosition, parry3d_f64::partitioning::Bvh)>,
}

impl Default for SpatialData {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            targets: Default::default(),
            occluders: Default::default(),
        }
    }
}

impl SpatialData {
    fn clear(&mut self) {
        self.objects.clear();
        self.targets.take();
        self.occluders.take();
    }

    pub fn insert(&mut self, object: SpatialObject) {
        assert!(object.radius_m.is_finite() && object.radius_m >= 0.0);
        self.objects.push(object);
        self.targets.take();
        self.occluders.take();
    }

    fn targets(&self) -> &crate::spatial_tree::RegionIndex {
        self.targets.get_or_init(|| {
            let proxies: Vec<_> = self
                .objects
                .iter()
                .enumerate()
                .map(|(id, o)| crate::spatial_tree::Proxy {
                    id: id as u32,
                    position: o.position,
                    displacement: bevy::math::DVec3::ZERO,
                    radius: 0.0,
                })
                .collect();
            crate::spatial_tree::RegionIndex::build(&proxies)
        })
    }

    fn blockers(&self) -> &(GalacticPosition, parry3d_f64::partitioning::Bvh) {
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
                    (i as u32, crate::spatial_tree::bounds(p - r, p + r))
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
        tree.intersect_aabb(&crate::spatial_tree::bounds(p - r, p + r))
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
            crate::spatial_tree::vector(p),
            crate::spatial_tree::vector(endpoint),
        );
        tree.cast_ray(&ray, 1.0, |id, _| {
            let o = self.objects[id as usize];
            (o.entity != observer
                && id as usize != target
                && crate::sensors::sphere_blocks(
                    endpoint,
                    o.position.relative_to(centre),
                    o.radius_m,
                ))
            .then_some(0.0)
        })
        .is_some()
    }

    pub fn occupied_cells(&self) -> usize {
        self.targets().regions.len()
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensorSystems {
    Index,
    Scan,
}

pub struct SpatialPlugin;
impl Plugin for SpatialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpatialIndex>()
            .configure_sets(
                FixedLast,
                (SensorSystems::Index, SensorSystems::Scan).chain(),
            )
            .add_systems(
                FixedLast,
                rebuild
                    .in_set(SensorSystems::Index)
                    .run_if(in_state(GameState::Game)),
            );
    }
}

fn rebuild(
    mut index: ResMut<SpatialIndex>,
    bodies: Query<
        (Entity, &PreciseTransform, &SpatialBody),
        Without<crate::physics::collision::Projectile>,
    >,
) {
    // Retained VM snapshots may still own the old data. Do not clone their
    // entire BVHs just to discard them while constructing this tick's snapshot.
    if std::sync::Arc::get_mut(&mut index.0).is_none() {
        *index = SpatialIndex::default();
    }
    index.clear();
    for (entity, pose, body) in &bodies {
        index.insert(SpatialObject {
            entity,
            position: pose.translation_um,
            radius_m: body.radius_m,
            occludes: body.occludes,
        });
    }
    // Account for shared construction once, outside a guest's metered scan.
    index.targets();
    index.blockers();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use rand::{RngExt, SeedableRng};

    #[test]
    fn regional_queries_match_brute_force_at_negative_and_galactic_coordinates() {
        let mut world = World::new();
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(67);
        let origin = GalacticPosition::new(1_i128 << 90, -(1_i128 << 90), -1);
        let mut index = SpatialIndex::default();
        for _ in 0..1000 {
            let delta = DVec3::new(
                rng.random_range(-1e10..1e10),
                rng.random_range(-1e10..1e10),
                rng.random_range(-1e10..1e10),
            );
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: 10_f64.powf(rng.random_range(1.0..10.0)),
                occludes: rng.random_bool(0.5),
            });
        }
        // Exact boundary and negative-cell probes.
        for delta in [
            DVec3::ZERO,
            DVec3::X * 1e6,
            -DVec3::X * 1e6,
            DVec3::splat(-0.001),
        ] {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: 1.0,
                occludes: true,
            });
        }
        for radius in [0.0, 0.001, 1e6, 1e7, 1e8, 1e9, 1e10, 1e18] {
            let mut actual = index.within_range(origin, radius);
            let expected: Vec<_> = index
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| o.position.relative_to(origin).length_squared() <= radius * radius)
                .map(|(i, _)| i)
                .collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
            let mut actual = index.occluders_in_range(origin, radius);
            let expected: Vec<_> = index
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| {
                    o.occludes
                        && o.position.relative_to(origin).length_squared()
                            <= (radius + o.radius_m).powi(2)
                })
                .map(|(i, _)| i)
                .collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
        assert_eq!(
            crate::spatial_tree::key(GalacticPosition::new(-1, -100_000_000_000, 0)),
            [-1, -1, 0]
        );
    }
}

#[cfg(test)]
mod projectile_visibility_tests {
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn projectiles_neither_occupy_sensor_slots_nor_occlude_ships() {
        let mut app = App::new();
        app.init_resource::<SpatialIndex>()
            .add_systems(Update, rebuild);
        let observer = app.world_mut().spawn_empty().id();
        let target = app
            .world_mut()
            .spawn((
                PreciseTransform {
                    translation_um: GalacticPosition::from_meters(DVec3::X * 100.0),
                    ..Default::default()
                },
                SpatialBody {
                    radius_m: 1.0,
                    occludes: false,
                },
            ))
            .id();
        app.world_mut().spawn((
            crate::physics::collision::Projectile::new(10.0, 1.0),
            PreciseTransform {
                translation_um: GalacticPosition::from_meters(DVec3::X * 50.0),
                ..Default::default()
            },
            SpatialBody {
                radius_m: 10.0,
                occludes: true,
            },
        ));
        app.update();
        let contacts = crate::sensors::detect_nearest(
            app.world().resource::<SpatialIndex>(),
            observer,
            GalacticPosition::ZERO,
            &crate::sensors::Sensor::default(),
            1,
        );
        assert_eq!(contacts.visible.len(), 1);
        assert_eq!(contacts.visible[0].entity, target);
    }
}
