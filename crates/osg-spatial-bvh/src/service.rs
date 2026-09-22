//! The current tick's shared spatial queries.

use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;
use std::ops::Deref;
use std::sync::Arc;

use crate::{Aabb, BvhEntry, LuminosityBvh, Position};

mod cursor;
mod query;
pub use cursor::{QueryBatch, QueryBudget, QueryCursor, QueryStats, SpatialQuery};

#[cfg(test)]
mod tests;

/// Input describing one object captured at the start of a tick.
/// Separate dynamic trees index `bounds` and `swept_bounds`.
#[derive(Clone, Debug)]
pub struct DynamicEntry<T> {
    pub object: T,
    /// Bounds of the object at the start of the tick.
    pub bounds: Aabb,
    /// Conservative bounds of motion predicted over the entire tick.
    /// These must include `bounds`, rotation, and any curved motion.
    pub swept_bounds: Aabb,
    /// Finite, nonnegative optical power bound in watts, including reflection.
    pub luminosity: f64,
}

/// An exact hit accepted by the caller's intersection predicate.
#[derive(Debug)]
pub struct SegmentHit<'a, T> {
    pub object: &'a T,
    /// Fraction along the query segment, in the inclusive range `[0, 1]`.
    pub fraction: f64,
}

/// A spatial service with static, instantaneous dynamic, and swept dynamic trees.
///
/// Static objects and their tree never change after construction and are shared
/// across services. Both dynamic trees are rebuilt every simulation tick (10 Hz).
/// Leaves store only `T::Id`. The dynamic trees share one object lookup table.
///
/// Instantaneous queries use start-of-tick bounds; motion and collision queries
/// use swept bounds. Callers supply the predicted bounds without timestamps.
///
/// Rebuilding replaces the current dynamic trees. Queries borrow the service,
/// preventing rebuilds during traversal. A caller needing independent state
/// can explicitly clone the service, copying its dynamic trees.
/// Queries use the start-of-tick state or motion predicted from it; changes
/// during the tick enter spatial discovery on the next rebuild.
///
/// Positions are integer micrometres, radii are metres, and flux is W/m².
/// Candidate queries are conservative and unordered. Callers apply exact
/// geometry, visibility, eligibility, and stable ordering as appropriate.
/// Multiple proxies may refer to one game object; callers identify those via T.
///
/// Invalid bounds, negative or NaN radii, invalid luminosities or flux thresholds,
/// and invalid values returned by exact-query callbacks panic. Radii may be
/// positive infinity. Swept bounds must contain the instantaneous bounds.
#[derive(Clone)]
pub struct SpatialService<T: SpatialObject> {
    static_tree: Arc<IndexedTree<T>>,
    dynamic_tree: IndexedTree<T>,
    swept_tree: IndexedTree<T>,
}

/// Identity stored in BVH leaves. IDs must be unique within each tree.
pub trait SpatialObject {
    type Id: Copy + Eq + Hash;
    fn spatial_id(&self) -> Self::Id;
}

macro_rules! identity_payload {
    ($($ty:ty),*) => {
        $(
            impl SpatialObject for $ty {
                type Id = Self;

                fn spatial_id(&self) -> Self {
                    *self
                }
            }
        )*
    };
}
identity_payload!(u64, usize, i32, i128, &'static str);

/// Query metadata is shared by the two dynamic trees, outside the BVH nodes.
#[derive(Clone)]
struct IndexedTree<T: SpatialObject> {
    tree: LuminosityBvh<T::Id>,
    records: Arc<HashMap<T::Id, T>>,
}

impl<T: SpatialObject> IndexedTree<T> {
    fn build(entries: impl IntoIterator<Item = BvhEntry<T>>) -> Self {
        let entries = entries.into_iter();
        let mut records = HashMap::with_capacity(entries.size_hint().0);
        let entries = entries.map(|entry| {
            let id = entry.object.spatial_id();
            assert!(
                records.insert(id, entry.object).is_none(),
                "duplicate spatial ID"
            );
            BvhEntry {
                object: id,
                bounds: entry.bounds,
                luminosity: entry.luminosity,
            }
        });
        let tree = LuminosityBvh::build(entries).0;
        Self {
            tree,
            records: Arc::new(records),
        }
    }
}

impl<T: SpatialObject> Deref for IndexedTree<T> {
    type Target = LuminosityBvh<T::Id>;
    fn deref(&self) -> &Self::Target {
        &self.tree
    }
}

impl<T: SpatialObject> Default for SpatialService<T> {
    fn default() -> Self {
        Self::new([])
    }
}

impl<T: SpatialObject> SpatialService<T> {
    pub fn node_count(&self) -> usize {
        self.static_tree.nodes.len() + self.dynamic_tree.nodes.len() + self.swept_tree.nodes.len()
    }

    /// Select the largest exact flux. The callback may exclude records, and
    /// must not exceed their indexed luminosity/distance flux bound.
    pub fn brightest(&self, observer: Position, flux: impl Fn(&T) -> Option<f64>) -> Option<&T> {
        let mut best = None;
        query::brightest(&self.static_tree, observer, &flux, &mut best);
        query::brightest(&self.dynamic_tree, observer, &flux, &mut best);
        best.map(|(_, object)| object)
    }

    /// Build the static tree once, with empty dynamic trees.
    /// Static bounds and luminosity bounds must remain valid for its lifetime.
    pub fn new(static_entries: impl IntoIterator<Item = BvhEntry<T>>) -> Self {
        Self {
            static_tree: Arc::new(IndexedTree::build(static_entries)),
            dynamic_tree: IndexedTree::build(std::iter::empty()),
            swept_tree: IndexedTree::build(std::iter::empty()),
        }
    }

    /// Call every simulation tick (10 Hz) to replace both dynamic trees, keeping
    /// the static tree unchanged. Fully rebuild the dynamic trees from entries
    /// captured at `t1`, even if the records have not changed. The two trees
    /// build in parallel after their shared object records have been collected.
    pub fn rebuild_dynamic(&mut self, entries: impl IntoIterator<Item = DynamicEntry<T>>)
    where
        T::Id: Send,
    {
        let profile = std::env::var_os("OSG_SPATIAL_PROFILE").is_some();
        let detail = profile && std::env::var_os("OSG_SPATIAL_PROFILE_DETAIL").is_some();
        let started = std::time::Instant::now();
        let mut input_time = std::time::Duration::ZERO;
        let mut insert_time = std::time::Duration::ZERO;
        let mut entries = entries.into_iter();
        let mut records = HashMap::with_capacity(entries.size_hint().0);
        let mut instantaneous = Vec::with_capacity(entries.size_hint().0);
        let mut swept = Vec::with_capacity(entries.size_hint().0);
        loop {
            let input_started = detail.then(std::time::Instant::now);
            let entry = entries.next();
            if let Some(started) = input_started {
                input_time += started.elapsed();
            }
            let Some(entry) = entry else { break };
            crate::luminosity_bvh::validate(entry.bounds, entry.luminosity);
            assert!(
                (0..3).all(|axis| {
                    entry.swept_bounds.min[axis] <= entry.bounds.min[axis]
                        && entry.swept_bounds.max[axis] >= entry.bounds.max[axis]
                }),
                "swept bounds must contain instantaneous bounds"
            );

            let id = entry.object.spatial_id();
            let insert_started = detail.then(std::time::Instant::now);
            assert!(
                records.insert(id, entry.object).is_none(),
                "duplicate spatial ID"
            );
            if let Some(started) = insert_started {
                insert_time += started.elapsed();
            }
            instantaneous.push(BvhEntry {
                bounds: entry.bounds,
                luminosity: entry.luminosity,
                object: id,
            });
            swept.push(BvhEntry {
                bounds: entry.swept_bounds,
                luminosity: entry.luminosity,
                object: id,
            });
        }

        let count = records.len();
        let prepare_ms = started.elapsed().as_secs_f64() * 1000.;
        let records = Arc::new(records);
        let build_started = std::time::Instant::now();
        let (instantaneous, swept) = rayon::join(
            || LuminosityBvh::build(instantaneous).0,
            || LuminosityBvh::build(swept).0,
        );
        let build_ms = build_started.elapsed().as_secs_f64() * 1000.;
        let replace_started = std::time::Instant::now();
        let dynamic_tree = IndexedTree {
            tree: instantaneous,
            records: Arc::clone(&records),
        };
        let swept_tree = IndexedTree {
            tree: swept,
            records,
        };
        self.dynamic_tree = dynamic_tree;
        self.swept_tree = swept_tree;
        if profile {
            eprintln!(
                "spatial_build entries={count} prepare_ms={prepare_ms:.3} input_ms={:.3} insert_ms={:.3} build_wall_ms={build_ms:.3} replace_ms={:.3} total_ms={:.3} detail={detail}",
                input_time.as_secs_f64() * 1000.,
                insert_time.as_secs_f64() * 1000.,
                replace_started.elapsed().as_secs_f64() * 1000.,
                started.elapsed().as_secs_f64() * 1000.,
            );
        }
    }

    /// Static and instantaneous dynamic bounds intersecting a query sphere.
    /// An exact centre-range or shape-overlap test belongs to the caller.
    pub fn sphere_candidates(&self, centre: Position, radius_m: f64) -> Vec<&T> {
        self.query(SpatialQuery::Sphere { centre, radius_m })
            .collect()
    }

    /// Static and instantaneous dynamic bounds overlapping a closed AABB.
    pub fn aabb_candidates(&self, bounds: Aabb) -> Vec<&T> {
        self.query(SpatialQuery::Aabb(bounds)).collect()
    }

    /// Sources whose flux bound meets the threshold somewhere in the observer
    /// sphere. Use an observer radius of zero for a point observation.
    /// Exact source brightness and occlusion remain caller checks.
    /// Bounds containing the observer sphere's points are retained, even for
    /// dark objects. A zero threshold includes all objects.
    pub fn visibility_candidates(
        &self,
        observer: Position,
        observer_radius_m: f64,
        min_flux_w_m2: f64,
    ) -> Vec<&T> {
        self.query(SpatialQuery::Visibility {
            observer,
            observer_radius_m,
            min_flux_w_m2,
        })
        .collect()
    }

    /// Up to `count` objects within `radius_m`, ordered by exact distance.
    /// The callback returns a finite, nonnegative distance in metres or None to
    /// exclude an object. Its distance must be at least the distance to the
    /// instantaneous AABB so pruning remains conservative. Equal-distance order
    /// is unset.
    pub fn nearest(
        &self,
        centre: Position,
        radius_m: f64,
        count: usize,
        distance: impl Fn(&T) -> Option<f64>,
    ) -> Vec<&T> {
        query::validate_radius(radius_m);
        if count == 0 {
            return Vec::new();
        }

        let mut best = BinaryHeap::new();
        query::nearest(
            &self.static_tree,
            centre,
            radius_m,
            count,
            &distance,
            &mut best,
        );
        query::nearest(
            &self.dynamic_tree,
            centre,
            radius_m,
            count,
            &distance,
            &mut best,
        );

        best.into_sorted_vec()
            .into_iter()
            .map(|hit| hit.object)
            .collect()
    }

    /// Bounds touched by a sphere swept from start to end, against objects at
    /// `t1`. A zero radius gives a line segment. This query does not predict
    /// target motion; use motion_candidates for moving targets.
    /// Traversal expands boxes by the radius, conservatively retaining corner
    /// candidates for the caller's exact swept-sphere test.
    pub fn segment_candidates(&self, start: Position, end: Position, radius_m: f64) -> Vec<&T> {
        self.query(SpatialQuery::Segment {
            start,
            end,
            radius_m,
        })
        .collect()
    }

    /// Static and predicted dynamic bounds overlapping a swept query AABB.
    /// The query must describe motion within the current tick interval.
    /// Candidates still require an exact test accounting for relative motion.
    pub fn motion_candidates(&self, swept_bounds: Aabb) -> Vec<&T> {
        self.query(SpatialQuery::Motion(swept_bounds)).collect()
    }

    /// Closest accepted hit along a segment against start-of-tick objects.
    /// The callback returns an exact entry fraction in `[0, 1]`, or None.
    /// The hit must lie within the object's indexed bounds. Equal-hit order is
    /// unset; traversal order must never decide which distance is closest.
    pub fn first_hit(
        &self,
        start: Position,
        end: Position,
        intersect: impl Fn(&T) -> Option<f64>,
    ) -> Option<SegmentHit<'_, T>> {
        let mut best = None;
        query::first_hit(&self.static_tree, start, end, &intersect, &mut best);
        query::first_hit(&self.dynamic_tree, start, end, &intersect, &mut best);
        best
    }

    /// Unordered dynamic-dynamic and dynamic-static candidate pairs for the
    /// interval. Each pair of distinct proxy records appears at most once;
    /// there are no static-static pairs. The callback excludes ineligible pairs,
    /// including multiple proxies belonging to the same physical object.
    pub fn collision_candidates(&self, accept: impl Fn(&T, &T) -> bool) -> Vec<(&T, &T)> {
        let mut pairs = self.dynamic_collision_candidates(&accept);
        query::pairs(
            &self.swept_tree,
            &self.static_tree,
            false,
            &accept,
            &mut pairs,
        );
        pairs
    }

    /// Pairs among dynamic records, excluding the static catalogue entirely.
    pub fn dynamic_collision_candidates(&self, accept: impl Fn(&T, &T) -> bool) -> Vec<(&T, &T)> {
        let mut pairs = Vec::new();
        query::pairs(
            &self.swept_tree,
            &self.swept_tree,
            true,
            &accept,
            &mut pairs,
        );
        pairs
    }

    /// Nearest dynamic records; distance has the same contract as `nearest`.
    pub fn nearest_dynamic(
        &self,
        centre: Position,
        radius_m: f64,
        count: usize,
        distance: impl Fn(&T) -> Option<f64>,
    ) -> Vec<&T> {
        query::validate_radius(radius_m);
        if count == 0 {
            return Vec::new();
        }
        let mut best = BinaryHeap::new();
        query::nearest(
            &self.dynamic_tree,
            centre,
            radius_m,
            count,
            &distance,
            &mut best,
        );
        best.into_sorted_vec()
            .into_iter()
            .map(|hit| hit.object)
            .collect()
    }

    /// Query only dynamic records, without visiting the shared static tree.
    pub fn query_dynamic(&self, query: SpatialQuery) -> QueryCursor<'_, T> {
        QueryCursor::new(self, query, false)
    }

    /// Begin a resumable candidate query with explicit work and result budgets.
    /// The cursor borrows the service and must finish before its next rebuild.
    ///
    /// ```
    /// use osg_spatial_bvh::{Aabb, BvhEntry, QueryBudget, SpatialQuery, SpatialService};
    ///
    /// let service = SpatialService::new([BvhEntry {
    ///     object: "star",
    ///     bounds: Aabb { min: [0; 3], max: [0; 3] },
    ///     luminosity: 1.0,
    /// }]);
    /// let mut cursor = service.query(SpatialQuery::Sphere {
    ///     centre: [0; 3],
    ///     radius_m: 1.0,
    /// });
    /// let mut found: Vec<&str> = Vec::new();
    /// while !cursor.is_complete() {
    ///     let batch = cursor.advance(QueryBudget { max_work: 1, max_results: 1 });
    ///     found.extend(batch.objects.into_iter().copied());
    /// }
    /// assert_eq!(found, vec!["star"]);
    /// assert_eq!(cursor.stats().work(), 2); // One node and one object test.
    /// ```
    pub fn query(&self, query: SpatialQuery) -> QueryCursor<'_, T> {
        QueryCursor::new(self, query, true)
    }
}
