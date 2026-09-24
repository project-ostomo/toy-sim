//! Spatial queries over exact galactic positions, using a BVH or spatial hash.
//!
//! Both backends share records, query semantics, and work budgets. BVH mutations
//! are staged until an explicit `rebuild` or `replace`; queries never build trees.
//! Hashes update in place. Probe counters count BVH nodes or hash probes.

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

use glam::DVec3;
use osg_spatial_hash::{LuminosityMap, Position};
pub use osg_spatial_hash::{QueryBudget, QueryExhausted};

use osg_space::GalacticPosition;
use osg_spatial_bvh::{
    Aabb, BvhEntry, DynamicLuminosityBvh, DynamicTreeStats, LeafHandle, LuminosityBvh,
};

enum Backend<K> {
    Bvh(LuminosityBvh<(K, SpatialRecord)>),
    Dynamic(DynamicBackend<K>),
    Hash(LuminosityMap<K>),
}

#[derive(Clone)]
struct DynamicBackend<K> {
    tree: DynamicLuminosityBvh<(K, SpatialRecord)>,
    handles: HashMap<K, LeafHandle>,
    present: HashSet<K>,
    unchanged: usize,
    updates: Vec<(LeafHandle, BvhEntry<(K, SpatialRecord)>)>,
}

impl<K: Copy + Eq + Hash> DynamicBackend<K> {
    fn synchronize(&mut self, entries: impl IntoIterator<Item = (K, SpatialRecord)>) {
        self.present.clear();
        if self.handles.is_empty() {
            let mut ids = Vec::new();
            let (tree, handles) =
                DynamicLuminosityBvh::build(entries.into_iter().map(|(id, record)| {
                    assert!(self.present.insert(id), "duplicate spatial key");
                    ids.push(id);
                    bvh_entry(id, record)
                }));
            self.tree = tree;
            self.handles.extend(ids.into_iter().zip(handles));
            return;
        }

        for (id, record) in entries {
            assert!(self.present.insert(id), "duplicate spatial key");
            if let Some(&handle) = self.handles.get(&id) {
                if self.tree.get(handle).unwrap().1 == record {
                    self.unchanged += 1;
                } else {
                    self.updates.push((handle, bvh_entry(id, record)));
                }
            } else {
                let handle = self.tree.insert(bvh_entry(id, record));
                self.handles.insert(id, handle);
            }
        }
        self.tree.update_batch(&mut self.updates).unwrap();
        self.handles.retain(|id, handle| {
            if self.present.contains(id) {
                true
            } else {
                self.tree.remove(*handle).unwrap();
                false
            }
        });
    }
}

fn bvh_entry<K>(id: K, record: SpatialRecord) -> BvhEntry<(K, SpatialRecord)> {
    BvhEntry {
        object: (id, record),
        bounds: Aabb::sphere(coords(record.position), record.radius_m),
        luminosity: record.luminosity,
    }
}

enum TreeRef<'a, K> {
    Static(&'a LuminosityBvh<(K, SpatialRecord)>),
    Dynamic(&'a DynamicLuminosityBvh<(K, SpatialRecord)>),
}

impl<K> TreeRef<'_, K> {
    fn try_visit<E>(
        &self,
        origin: [i128; 3],
        visit: impl FnMut(Aabb, f64, Option<&(K, SpatialRecord)>) -> Result<bool, E>,
    ) -> Result<(), E> {
        match self {
            Self::Static(tree) => tree.try_visit(origin, visit),
            Self::Dynamic(tree) => tree.try_visit(origin, visit),
        }
    }
}

pub const OPTICAL_LUMENS_PER_WATT: f64 = 220.0;
pub const MINIMUM_CELL_SHIFT: u32 = 9;
const KM_UM: i128 = 1_000_000_000;
// Two floored three-dimensional kilometre positions differ from their exact
// displacement by less than 2 sqrt(3) km, including translated query anchors.
const POSITION_PADDING_KM: f64 = 4.0;

fn coords(position: GalacticPosition) -> [i128; 3] {
    [position.x, position.y, position.z]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialRecord {
    pub position: GalacticPosition,
    pub radius_m: f64,
    /// Intrinsic luminosity; query thresholds use luminosity / distance².
    pub luminosity: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryError {
    CoordinatesOutOfRange,
    Exhausted,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CoordinatesOutOfRange => "position exceeds spatial index coordinate range",
            Self::Exhausted => "spatial query work budget exhausted",
        })
    }
}

impl std::error::Error for QueryError {}

impl From<QueryExhausted> for QueryError {
    fn from(_: QueryExhausted) -> Self {
        Self::Exhausted
    }
}

/// An index with a private backend selected at construction.
/// Radii bound all geometry roles passed to segment queries.
pub struct GalacticIndex<K> {
    origin: Option<GalacticPosition>,
    records: HashMap<K, SpatialRecord>,
    backend: Backend<K>,
    maximum_radius_m: f64,
}

impl<K: Copy + Eq + Hash> Clone for GalacticIndex<K> {
    fn clone(&self) -> Self {
        let mut copy = match self.backend {
            Backend::Bvh(_) => Self::bvh(),
            Backend::Dynamic(_) => Self::dynamic_bvh(),
            Backend::Hash(_) => Self::spatial_hash(),
        };
        copy.origin = self.origin;
        for (&id, &record) in &self.records {
            copy.insert(id, record)
                .expect("existing index coordinate range");
        }
        copy.maximum_radius_m = self.maximum_radius_m;
        copy.backend = match &self.backend {
            Backend::Bvh(tree) => Backend::Bvh(tree.clone()),
            Backend::Dynamic(backend) => Backend::Dynamic(backend.clone()),
            Backend::Hash(_) => copy.backend,
        };
        copy
    }
}

impl<K: Copy + Eq + Hash> Default for GalacticIndex<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Eq + Hash> GalacticIndex<K> {
    pub fn new() -> Self {
        Self::bvh()
    }

    pub fn bvh() -> Self {
        Self {
            origin: None,
            records: HashMap::new(),
            backend: Backend::Bvh(LuminosityBvh::build([]).0),
            maximum_radius_m: 0.0,
        }
    }

    pub fn spatial_hash() -> Self {
        Self {
            backend: Backend::Hash(LuminosityMap::with_minimum_cell_shift(MINIMUM_CELL_SHIFT)),
            ..Self::bvh()
        }
    }

    /// Incrementally maintained BVH, published only by `rebuild` or `replace`.
    /// Static catalogues should use `bvh` to retain compact immutable storage.
    pub fn dynamic_bvh() -> Self {
        Self {
            backend: Backend::Dynamic(DynamicBackend {
                tree: DynamicLuminosityBvh::new(),
                handles: HashMap::new(),
                present: HashSet::new(),
                unchanged: 0,
                updates: Vec::new(),
            }),
            ..Self::bvh()
        }
    }

    /// Mutation counters and tree storage for profiling the dynamic backend.
    /// Storage excludes record/handle maps and allocations owned by payloads.
    pub fn take_dynamic_stats(&mut self) -> Option<(DynamicTreeStats, usize, usize, u32)> {
        match &mut self.backend {
            Backend::Dynamic(backend) => Some((
                backend.tree.take_stats(),
                std::mem::take(&mut backend.unchanged),
                backend.tree.allocated_bytes(),
                backend.tree.height(),
            )),
            _ => None,
        }
    }

    /// Allocated tree vectors only, excluding record maps and payload-owned data.
    pub fn bvh_storage_bytes(&self) -> Option<usize> {
        match &self.backend {
            Backend::Bvh(tree) => Some(tree.allocated_bytes()),
            Backend::Dynamic(backend) => Some(backend.tree.allocated_bytes()),
            Backend::Hash(_) => None,
        }
    }

    fn tree(&self) -> Option<TreeRef<'_, K>> {
        match &self.backend {
            Backend::Bvh(tree) => Some(TreeRef::Static(tree)),
            Backend::Dynamic(backend) => Some(TreeRef::Dynamic(&backend.tree)),
            Backend::Hash(_) => None,
        }
    }

    fn build_tree(&self) -> LuminosityBvh<(K, SpatialRecord)> {
        LuminosityBvh::build(self.records.iter().map(|(&object, record)| BvhEntry {
            object: (object, *record),
            bounds: Aabb::sphere(coords(record.position), record.radius_m),
            luminosity: record.luminosity,
        }))
        .0
    }

    /// Publish staged records. Immutable trees rebuild; dynamic trees reconcile
    /// changes incrementally. Hash updates are already visible in place.
    pub fn rebuild(&mut self) {
        if matches!(self.backend, Backend::Bvh(_)) {
            let tree = self.build_tree();
            self.backend = Backend::Bvh(tree);
        } else if let Backend::Dynamic(backend) = &mut self.backend {
            backend.synchronize(self.records.iter().map(|(&id, &record)| (id, record)));
            self.maximum_radius_m = self
                .records
                .values()
                .map(|record| record.radius_m)
                .fold(0.0, f64::max);
        }
    }

    /// Publish a complete scene. Immutable BVHs rebuild; dynamic BVHs and hashes
    /// update retained entries in place and remove absent keys.
    pub fn replace(
        &mut self,
        entries: impl IntoIterator<Item = (K, SpatialRecord)>,
    ) -> Result<(), QueryError> {
        if matches!(self.backend, Backend::Bvh(_)) {
            let mut records = HashMap::new();
            let mut maximum_radius_m = 0.0_f64;
            let tree = LuminosityBvh::build(entries.into_iter().map(|(object, record)| {
                assert!(
                    records.insert(object, record).is_none(),
                    "duplicate spatial key"
                );
                maximum_radius_m = maximum_radius_m.max(record.radius_m);
                BvhEntry {
                    object: (object, record),
                    bounds: Aabb::sphere(coords(record.position), record.radius_m),
                    luminosity: record.luminosity,
                }
            }))
            .0;
            self.maximum_radius_m = maximum_radius_m;
            self.records = records;
            self.backend = Backend::Bvh(tree);
        } else if let Backend::Dynamic(backend) = &mut self.backend {
            let mut maximum_radius_m = 0.0_f64;
            backend.synchronize(entries.into_iter().map(|(id, record)| {
                assert!(record.radius_m.is_finite() && record.radius_m >= 0.0);
                assert!(record.luminosity.is_finite() && record.luminosity >= 0.0);
                self.records.insert(id, record);
                maximum_radius_m = maximum_radius_m.max(record.radius_m);
                (id, record)
            }));
            self.records.retain(|id, _| backend.present.contains(id));
            self.maximum_radius_m = maximum_radius_m;
        } else {
            let mut present = HashSet::new();
            for (id, record) in entries {
                self.insert(id, record)?;
                present.insert(id);
            }
            let absent: Vec<_> = self
                .records
                .keys()
                .filter(|id| !present.contains(id))
                .copied()
                .collect();
            for id in absent {
                self.remove(&id);
            }
        }
        Ok(())
    }

    fn map(&self) -> &LuminosityMap<K> {
        match &self.backend {
            Backend::Hash(map) => map,
            Backend::Bvh(_) | Backend::Dynamic(_) => {
                unreachable!("BVH query handled before hash traversal")
            }
        }
    }

    fn coordinates(&self, position: GalacticPosition) -> Result<Position, QueryError> {
        let origin = self.origin.unwrap_or(position);
        let axis = |value: i128, base: i128| {
            value
                .checked_sub(base)
                .and_then(|v| i64::try_from(v.div_euclid(KM_UM)).ok())
                .ok_or(QueryError::CoordinatesOutOfRange)
        };
        Ok(Position {
            x: axis(position.x, origin.x)?,
            y: axis(position.y, origin.y)?,
            z: axis(position.z, origin.z)?,
        })
    }

    pub fn insert(&mut self, id: K, record: SpatialRecord) -> Result<bool, QueryError> {
        assert!(record.radius_m.is_finite() && record.radius_m >= 0.0);
        assert!(record.luminosity.is_finite() && record.luminosity >= 0.0);
        let position = if matches!(self.backend, Backend::Hash(_)) {
            Some(self.coordinates(record.position)?)
        } else {
            None
        };
        self.origin.get_or_insert(record.position);
        self.maximum_radius_m = self.maximum_radius_m.max(record.radius_m);
        let changed = self.records.get(&id) != Some(&record);
        if !changed {
            return Ok(false);
        }
        match &mut self.backend {
            Backend::Hash(map) => {
                map.insert(id, position.unwrap(), record.luminosity / 1e6);
            }
            Backend::Bvh(_) | Backend::Dynamic(_) => {}
        }
        self.records.insert(id, record);
        Ok(changed)
    }

    pub fn remove(&mut self, id: &K) {
        match &mut self.backend {
            Backend::Hash(map) => {
                map.remove(id);
            }
            Backend::Bvh(_) | Backend::Dynamic(_) => {}
        }
        self.records.remove(id);
    }

    pub fn get(&self, id: &K) -> Option<&SpatialRecord> {
        self.records.get(id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &SpatialRecord)> {
        self.records.iter()
    }

    /// Centre or sphere overlap candidates, filtered using exact positions.
    pub fn within_radius(
        &self,
        position: GalacticPosition,
        radius_m: f64,
        include_radii: bool,
    ) -> Result<Vec<K>, QueryError> {
        self.within_radius_budgeted(
            position,
            radius_m,
            include_radii,
            &mut QueryBudget::new(usize::MAX),
        )
    }

    pub fn within_radius_budgeted(
        &self,
        position: GalacticPosition,
        radius_m: f64,
        include_radii: bool,
        budget: &mut QueryBudget,
    ) -> Result<Vec<K>, QueryError> {
        assert!(!radius_m.is_nan() && radius_m >= 0.0);
        if let Some(tree) = self.tree() {
            let mut found = Vec::new();
            tree.try_visit(coords(position), |bounds, _, leaf| {
                budget.charge_probe()?;
                if bounds.distance_squared(coords(position)) > radius_m.next_up().powi(2) {
                    return Ok::<_, QueryError>(false);
                }
                if let Some(&(id, record)) = leaf {
                    budget.charge()?;
                    let radius = radius_m + if include_radii { record.radius_m } else { 0.0 };
                    if record.position.relative_to(position).length() <= radius.next_up() {
                        found.push(id);
                    }
                }
                Ok(true)
            })?;
            return Ok(found);
        }
        let coordinates = self.coordinates(position)?;
        let bound = if include_radii {
            self.maximum_radius_m
        } else {
            0.0
        };
        let search = (radius_m + bound) / 1000.0;
        let search = (search + POSITION_PADDING_KM + search * 64.0 * f64::EPSILON).ceil();
        let accepts = |record: &SpatialRecord| {
            let radius = radius_m + if include_radii { record.radius_m } else { 0.0 };
            record.position.relative_to(position).length() <= radius.next_up()
        };
        Ok(self
            .map()
            .within_radius_budgeted(coordinates, search, budget)?
            .into_iter()
            .filter_map(|&id| accepts(&self.records[&id]).then_some(id))
            .collect())
    }

    pub fn nearest(
        &self,
        position: GalacticPosition,
        surface: bool,
        budget: &mut QueryBudget,
        mut accept: impl FnMut(K) -> bool,
    ) -> Result<Option<(K, f64)>, QueryError> {
        if let Some(tree) = self.tree() {
            let mut best = None;
            let mut distance = f64::INFINITY;
            tree.try_visit(coords(position), |bounds, _, leaf| {
                budget.charge_probe()?;
                if bounds.distance_squared(coords(position)) > distance.max(0.0).next_up().powi(2) {
                    return Ok::<_, QueryError>(false);
                }
                if let Some(&(id, record)) = leaf {
                    budget.charge()?;
                    if accept(id) {
                        let candidate = record.position.relative_to(position).length()
                            - if surface { record.radius_m } else { 0.0 };
                        if candidate < distance {
                            distance = candidate;
                            best = Some((id, candidate));
                        }
                    }
                }
                Ok(true)
            })?;
            return Ok(best);
        }
        let coordinates = self.coordinates(position)?;
        let bound = if surface { self.maximum_radius_m } else { 0.0 };
        let hit = self.map().nearest_by(
            coordinates,
            bound / 1000.0 + POSITION_PADDING_KM,
            budget,
            |&id| {
                if !accept(id) {
                    return None;
                }
                let record = &self.records[&id];
                let radius = if surface { record.radius_m } else { 0.0 };
                Some((record.position.relative_to(position).length() - radius) / 1000.0)
            },
        )?;
        Ok(hit.map(|hit| (*hit.item, hit.distance * 1000.0)))
    }

    pub fn nearest_many(
        &self,
        position: GalacticPosition,
        count: usize,
        budget: &mut QueryBudget,
        mut accept: impl FnMut(K) -> bool,
    ) -> Result<Vec<(K, f64)>, QueryError> {
        if count == 0 || (self.tree().is_none() && self.is_empty()) {
            return Ok(Vec::new());
        }
        if let Some(tree) = self.tree() {
            let mut found: Vec<(K, f64)> = Vec::new();
            tree.try_visit(coords(position), |bounds, _, leaf| {
                budget.charge_probe()?;
                if found.len() == count
                    && bounds.distance_squared(coords(position))
                        > found.last().unwrap().1.next_up().powi(2)
                {
                    return Ok::<_, QueryError>(false);
                }
                if let Some(&(id, record)) = leaf {
                    budget.charge()?;
                    if accept(id) {
                        let distance = record.position.relative_to(position).length();
                        let at = found.partition_point(|entry| entry.1 <= distance);
                        found.insert(at, (id, distance));
                        found.truncate(count);
                    }
                }
                Ok(true)
            })?;
            return Ok(found);
        }
        let coordinates = self.coordinates(position)?;
        let mut radius = 512.0_f64;
        loop {
            let mut found: Vec<_> = self
                .map()
                .within_radius_budgeted(coordinates, radius, budget)?
                .into_iter()
                .filter_map(|&id| {
                    accept(id).then(|| {
                        (
                            id,
                            self.records[&id].position.relative_to(position).length(),
                        )
                    })
                })
                .collect();
            if found.len() > count {
                found.select_nth_unstable_by(count, |a, b| a.1.total_cmp(&b.1));
                found.truncate(count);
            }
            found.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
            if radius >= 2_f64.powi(63)
                || (found.len() >= count
                    && found.last().unwrap().1 < (radius - POSITION_PADDING_KM) * 1000.0)
            {
                return Ok(found);
            }
            radius *= 2.0;
        }
    }

    /// Conservative luminosity candidates. `padding_m` includes the query extent;
    /// stored object radii are added here. Refine brightness with exact positions.
    pub fn visibility_candidates(
        &self,
        position: GalacticPosition,
        threshold: f64,
        padding_m: f64,
    ) -> Result<Vec<K>, QueryError> {
        assert!(threshold.is_finite() && threshold >= 0.0);
        assert!(padding_m.is_finite() && padding_m >= 0.0);
        if threshold == 0.0 && self.tree().is_none() {
            return Ok(self.records.keys().copied().collect());
        }
        if let Some(tree) = self.tree() {
            let mut found = Vec::new();
            tree.try_visit(coords(position), |bounds, luminosity, leaf| {
                let distance =
                    (bounds.distance_squared(coords(position)).sqrt() - padding_m).max(0.0);
                if luminosity < threshold * distance * distance {
                    return Ok::<_, QueryError>(false);
                }
                if let Some(&(id, _)) = leaf {
                    found.push(id);
                }
                Ok(true)
            })?;
            return Ok(found);
        }
        Ok(self
            .map()
            .visible_candidates(
                self.coordinates(position)?,
                threshold,
                (padding_m + self.maximum_radius_m) / 1000.0 + POSITION_PADDING_KM,
            )
            .copied()
            .collect())
    }

    /// Finds every bounding sphere intersecting a finite swept sphere segment.
    /// Uses the selected backend's traversal and exact sphere intersections.
    pub fn segment_candidates(
        &self,
        origin: GalacticPosition,
        displacement: DVec3,
        radius_m: f64,
        budget: &mut QueryBudget,
    ) -> Result<Vec<K>, QueryError> {
        if let Some(tree) = self.tree() {
            assert!(displacement.is_finite() && radius_m.is_finite() && radius_m >= 0.0);
            let mut hits = Vec::new();
            let start = coords(origin);
            let end = coords(origin.offset_by(displacement));
            tree.try_visit(start, |bounds, _, leaf| {
                budget.charge_probe()?;
                if bounds.segment_entry(start, end, radius_m).is_none() {
                    return Ok::<_, QueryError>(false);
                }
                if let Some(&(id, record)) = leaf {
                    budget.charge()?;
                    if segment_sphere_entry(
                        record.position.relative_to(origin),
                        displacement,
                        record.radius_m + radius_m,
                    )
                    .is_some()
                    {
                        hits.push(id);
                    }
                }
                Ok(true)
            })?;
            return Ok(hits);
        }
        self.segment_candidates_filtered(
            origin,
            displacement,
            radius_m,
            self.maximum_radius_m,
            budget,
            |id| Some(self.records[&id].radius_m),
        )
    }

    /// Selects a geometry role without letting unrelated large objects determine
    /// the search bound. Every accepted radius must fit both `maximum_radius_m`
    /// and its stored record radius.
    pub fn segment_candidates_filtered(
        &self,
        origin: GalacticPosition,
        displacement: DVec3,
        radius_m: f64,
        maximum_radius_m: f64,
        budget: &mut QueryBudget,
        object_radius: impl Fn(K) -> Option<f64>,
    ) -> Result<Vec<K>, QueryError> {
        assert!(displacement.is_finite());
        assert!(radius_m.is_finite() && radius_m >= 0.0);
        assert!(maximum_radius_m.is_finite() && maximum_radius_m >= 0.0);
        if let Some(tree) = self.tree() {
            let mut hits = Vec::new();
            let start = coords(origin);
            let end = coords(origin.offset_by(displacement));
            tree.try_visit(start, |bounds, _, leaf| {
                budget.charge_probe()?;
                if bounds.segment_entry(start, end, radius_m).is_none() {
                    return Ok::<_, QueryError>(false);
                }
                if let Some(&(id, record)) = leaf {
                    budget.charge()?;
                    if let Some(radius) = object_radius(id) {
                        assert!(
                            radius.is_finite()
                                && radius >= 0.0
                                && radius <= maximum_radius_m
                                && radius <= record.radius_m
                        );
                        if segment_sphere_entry(
                            record.position.relative_to(origin),
                            displacement,
                            radius + radius_m,
                        )
                        .is_some()
                        {
                            hits.push(id);
                        }
                    }
                }
                Ok(true)
            })?;
            return Ok(hits);
        }
        let length = displacement.length();
        let direction = displacement.try_normalize().unwrap_or(DVec3::ZERO);
        let mut distance = 0.0;
        let mut examined = HashSet::new();
        let mut hits = Vec::new();

        loop {
            let point = origin.offset_by(direction * distance);
            let limit_padding =
                1e-5 + 64.0 * f64::EPSILON * (2.0 * length + maximum_radius_m + radius_m);
            let nearest = self.map().nearest_within_by(
                self.coordinates(point)?,
                maximum_radius_m / 1000.0 + POSITION_PADDING_KM,
                (length - distance + radius_m + limit_padding) / 1000.0,
                budget,
                |&id| {
                    if examined.contains(&id) {
                        return None;
                    }
                    let radius = object_radius(id)?;
                    assert!(radius.is_finite() && radius >= 0.0 && radius <= maximum_radius_m);
                    Some((self.records[&id].position.relative_to(point).length() - radius) / 1000.0)
                },
            )?;
            let Some(nearest) = nearest else {
                return Ok(hits);
            };
            let clearance = nearest.distance * 1000.0;
            // Include absolute-position rounding and floating-point distance error.
            let tolerance =
                1e-5 + 64.0 * f64::EPSILON * (clearance.abs() + maximum_radius_m + length);
            let safe = clearance - radius_m - tolerance;
            if safe > length - distance {
                return Ok(hits);
            }
            if safe > 512_000.0 && distance + safe > distance {
                distance = (distance + safe).min(length);
                continue;
            }

            let probe = (length - distance).min(512_000.0);
            let radius =
                (probe + radius_m + maximum_radius_m + tolerance) / 1000.0 + POSITION_PADDING_KM;
            // Refine a whole local batch so dense overlapping volumes do not
            // require one full nearest query for every individual object.
            for &id in
                self.map()
                    .within_radius_budgeted(self.coordinates(point)?, radius, budget)?
            {
                if !examined.insert(id) {
                    continue;
                }
                let Some(radius) = object_radius(id) else {
                    continue;
                };
                assert!(radius.is_finite() && radius >= 0.0 && radius <= maximum_radius_m);
                let record = &self.records[&id];
                if segment_sphere_entry(
                    record.position.relative_to(origin),
                    displacement,
                    radius + radius_m,
                )
                .is_some()
                {
                    hits.push(id);
                }
            }
            if probe >= length - distance {
                return Ok(hits);
            }
            distance += probe;
        }
    }
}

/// First fraction along a segment entering a sphere, including starts inside.
/// Subtract galactic positions before passing the sphere centre to this helper.
pub fn segment_sphere_entry(centre: DVec3, displacement: DVec3, radius: f64) -> Option<f64> {
    let distance = centre.length();
    if distance <= radius {
        return Some(0.0);
    }
    let length = displacement.length();
    if length == 0.0 {
        return None;
    }
    let direction = displacement / length;
    let along = centre.dot(direction);
    let perpendicular = (centre - direction * along).length_squared();
    let radius_squared = radius * radius;
    let tolerance = 64.0 * f64::EPSILON * radius_squared.max(perpendicular);
    if perpendicular > radius_squared + tolerance {
        return None;
    }
    let half = (radius_squared - perpendicular).max(0.0).sqrt();
    let entry = along - half;
    let exit = along + half;
    (exit >= 0.0 && entry <= length).then_some((entry / length).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_publication_removal_and_clone_are_independent() {
        let mut index = GalacticIndex::dynamic_bvh();
        let record = SpatialRecord {
            position: GalacticPosition::ZERO,
            radius_m: 1.0,
            luminosity: 10.0,
        };
        index.replace([(1, record), (2, record)]).unwrap();
        let mut retained = index.clone();
        let moved = SpatialRecord {
            position: record.position.offset_by(DVec3::X * 1000.0),
            luminosity: 0.0,
            ..record
        };
        index.insert(1, moved).unwrap();
        index.remove(&2);
        index.insert(3, record).unwrap();
        let mut before = index.within_radius(record.position, 2.0, false).unwrap();
        before.sort_unstable();
        assert_eq!(before, [1, 2]);
        index.rebuild();
        assert_eq!(
            index.within_radius(record.position, 2.0, false).unwrap(),
            [3]
        );
        assert_eq!(
            index.within_radius(moved.position, 2.0, false).unwrap(),
            [1]
        );
        assert!(
            index
                .visibility_candidates(moved.position.offset_by(DVec3::Y * 10.0), 1.0, 0.0)
                .unwrap()
                .is_empty()
        );
        let mut old = retained.within_radius(record.position, 2.0, false).unwrap();
        old.sort_unstable();
        assert_eq!(old, [1, 2]);
        retained.replace([(2, moved)]).unwrap();
        assert_eq!(index.get(&1), Some(&moved));
        index.replace([]).unwrap();
        assert!(
            index
                .within_radius(record.position, f64::INFINITY, true)
                .unwrap()
                .is_empty()
        );
        index.replace([(9, record)]).unwrap();
        assert_eq!(
            index.within_radius(record.position, 2.0, false).unwrap(),
            [9]
        );
        index.take_dynamic_stats();
        index.replace([(9, record)]).unwrap();
        let (stats, unchanged, _, _) = index.take_dynamic_stats().unwrap();
        assert_eq!(unchanged, 1);
        assert_eq!(stats.bounds_updates + stats.payload_updates, 0);
    }

    #[test]
    #[should_panic(expected = "duplicate spatial key")]
    fn dynamic_rejects_duplicate_batch_ids() {
        let mut index = GalacticIndex::dynamic_bvh();
        let record = SpatialRecord {
            position: GalacticPosition::ZERO,
            radius_m: 0.0,
            luminosity: 0.0,
        };
        index.replace([(1, record), (1, record)]).unwrap();
    }

    #[test]
    fn bvh_queries_keep_the_published_tree_until_explicit_rebuild() {
        let mut index = GalacticIndex::bvh();
        let record = SpatialRecord {
            position: GalacticPosition::ZERO,
            radius_m: 1.0,
            luminosity: 1.0,
        };
        index.insert(1, record).unwrap();
        assert!(
            index
                .within_radius(record.position, 2.0, false)
                .unwrap()
                .is_empty()
        );
        index.rebuild();
        index.remove(&1);
        index.insert(2, record).unwrap();
        assert_eq!(
            index.within_radius(record.position, 2.0, false).unwrap(),
            [1]
        );
        assert_eq!(
            index
                .visibility_candidates(record.position, 0.0, 0.0)
                .unwrap(),
            [1]
        );
        assert_eq!(
            index
                .segment_candidates(record.position, DVec3::X, 0.0, &mut QueryBudget::new(100))
                .unwrap(),
            [1]
        );
        index.rebuild();
        assert_eq!(
            index.within_radius(record.position, 2.0, false).unwrap(),
            [2]
        );
    }

    #[test]
    fn translated_queries_match_exhaustive_checks_after_updates() {
        for (origin, backend) in [
            (GalacticPosition::ZERO, 0),
            (GalacticPosition::splat(1_i128 << 100), 0),
            (GalacticPosition::ZERO, 1),
            (GalacticPosition::splat(1_i128 << 100), 1),
            (GalacticPosition::ZERO, 2),
            (GalacticPosition::splat(1_i128 << 100), 2),
        ] {
            let mut index = match backend {
                0 => GalacticIndex::bvh(),
                1 => GalacticIndex::spatial_hash(),
                _ => GalacticIndex::dynamic_bvh(),
            };
            for id in 0..300 {
                index
                    .insert(
                        id,
                        SpatialRecord {
                            position: origin.offset_by(
                                DVec3::new(
                                    (id * 7919 % 10001) as f64 - 5000.0,
                                    (id * 3571 % 10001) as f64 - 5000.0,
                                    (id * 101 % 10001) as f64 - 5000.0,
                                ) * 1000.0,
                            ),
                            radius_m: 100_000.0 + (id % 17) as f64 * 20_000.0,
                            luminosity: (id % 3) as f64 * 1e15,
                        },
                    )
                    .unwrap();
            }
            index.remove(&7);
            index
                .insert(
                    3,
                    SpatialRecord {
                        position: origin,
                        radius_m: 10.0,
                        luminosity: 1e18,
                    },
                )
                .unwrap();

            index.rebuild();
            for displacement in [
                DVec3::ZERO,
                DVec3::new(10_000_000.0, 0.0, 0.0),
                DVec3::new(10_000_000.0, 5_000_000.0, -5_000_000.0),
                DVec3::new(1e12, 0.0, 0.0),
            ] {
                let start = origin.offset_by(DVec3::new(-5_000_000.0, 0.0, 0.0));
                let mut expected: Vec<_> = index
                    .iter()
                    .filter_map(|(&id, record)| {
                        segment_sphere_entry(
                            record.position.relative_to(start),
                            displacement,
                            record.radius_m + 20.0,
                        )
                        .map(|_| id)
                    })
                    .collect();
                let mut actual = index
                    .segment_candidates(
                        start,
                        displacement,
                        20.0,
                        &mut QueryBudget::new(10_000_000),
                    )
                    .unwrap();
                expected.sort_unstable();
                actual.sort_unstable();
                assert_eq!(actual, expected);
            }
            let nearest = index
                .nearest(origin, true, &mut QueryBudget::new(10000), |_| true)
                .unwrap()
                .unwrap();
            let expected = index
                .iter()
                .map(|(&id, record)| {
                    (
                        id,
                        record.position.relative_to(origin).length() - record.radius_m,
                    )
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert_eq!(nearest.0, expected.0);

            let candidates = index.visibility_candidates(origin, 10.0, 0.0).unwrap();
            for (&id, record) in index.iter() {
                let minimum =
                    (record.position.relative_to(origin).length() - record.radius_m).max(0.0);
                if record.luminosity > 0.0 && record.luminosity >= 10.0 * minimum * minimum {
                    assert!(candidates.contains(&id));
                }
            }
        }
    }

    #[test]
    fn filtered_ray_uses_role_specific_radii() {
        let mut index = GalacticIndex::new();
        for (id, position, radius_m) in [
            (0, DVec3::ZERO, 1e16),
            (1, DVec3::X * 1e9, 100.0),
            (2, DVec3::new(1e9, 1000.0, 0.0), 1e15),
        ] {
            index
                .insert(
                    id,
                    SpatialRecord {
                        position: GalacticPosition::from_meters(position),
                        radius_m,
                        luminosity: 0.0,
                    },
                )
                .unwrap();
        }
        index.rebuild();
        let hits = index
            .segment_candidates_filtered(
                GalacticPosition::ZERO,
                DVec3::X * 2e9,
                0.0,
                100.0,
                &mut QueryBudget::new(2000),
                |id| (id != 0).then_some(100.0),
            )
            .unwrap();
        assert_eq!(hits, [1]);
    }

    #[test]
    fn segment_handles_tangency_overlaps_and_exhaustion() {
        let mut index = GalacticIndex::new();
        for (id, y, radius) in [(0, 1.0, 1.0), (1, 0.0, 10.0), (2, 2.0, 1.0)] {
            index
                .insert(
                    id,
                    SpatialRecord {
                        position: GalacticPosition::from_meters(DVec3::new(0.0, y, 0.0)),
                        radius_m: radius,
                        luminosity: 0.0,
                    },
                )
                .unwrap();
        }
        index.rebuild();
        let origin = GalacticPosition::from_meters(-DVec3::X * 5.0);
        let mut hits = index
            .segment_candidates(origin, DVec3::X * 10.0, 0.0, &mut QueryBudget::new(1000))
            .unwrap();
        hits.sort_unstable();
        assert_eq!(hits, [0, 1]);
        assert_eq!(
            index.segment_candidates(origin, DVec3::X * 10.0, 0.0, &mut QueryBudget::new(1)),
            Err(QueryError::Exhausted)
        );
    }
}
