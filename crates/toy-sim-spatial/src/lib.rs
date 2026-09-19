use ahash::AHashMap;
use glam::DVec3;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::atomic::{AtomicU64, Ordering};
use toy_sim_space::GalacticPosition;

// Sixty-four roots cover the complete signed i128 coordinate space. Compressed
// occupied cells skip empty scales, so a local metre query does not enumerate
// every possible cell between ships and stars.
const ROOT_LEVEL: u8 = 126;
const MIN_LEVEL: u8 = 20;
const LEAF_CAPACITY: usize = 16;

/// A source's instantaneous optical luminosity coefficient and physical extent.
/// Apparent brightness is `luminosity / distance_m.powi(2)`. Callers choose one
/// consistent unit convention, including any geometric factors in luminosity.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub position: GalacticPosition,
    pub radius_m: f64,
    pub luminosity: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct QueryStats {
    pub cells_visited: usize,
    pub candidates: usize,
    pub buckets_searched: usize,
}

#[derive(Clone, Debug, Default)]
pub struct QueryResult {
    pub ids: Vec<u32>,
    pub stats: QueryStats,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct CellKey {
    coordinate: [i128; 3],
    level: u8,
}

impl CellKey {
    fn at(position: GalacticPosition, level: u8) -> Self {
        Self {
            coordinate: [
                position.x >> level,
                position.y >> level,
                position.z >> level,
            ],
            level,
        }
    }

    fn origin(self) -> GalacticPosition {
        GalacticPosition::new(
            self.coordinate[0] << self.level,
            self.coordinate[1] << self.level,
            self.coordinate[2] << self.level,
        )
    }

    fn contains(self, position: GalacticPosition) -> bool {
        Self::at(position, self.level) == self
    }

    fn common(a: GalacticPosition, b: GalacticPosition) -> Self {
        let difference = (a.x ^ b.x) as u128 | (a.y ^ b.y) as u128 | (a.z ^ b.z) as u128;
        let level = (128 - difference.leading_zeros()).max(MIN_LEVEL as u32) as u8;
        Self::at(a, level)
    }

    fn distance_squared(self, position: GalacticPosition) -> f64 {
        let side = 1_i128 << self.level;
        [position.x, position.y, position.z]
            .into_iter()
            .enumerate()
            .map(|(axis, value)| {
                let lower = self.coordinate[axis] * side;
                let upper = lower.saturating_add(side);
                let delta = if value < lower {
                    lower.abs_diff(value)
                } else if value > upper {
                    value.abs_diff(upper)
                } else {
                    0
                };
                (delta as f64 / 1_000_000.0).powi(2)
            })
            .sum()
    }

    fn intersects_segment(self, start: GalacticPosition, displacement: DVec3, radius: f64) -> bool {
        let side = 1_i128 << self.level;
        let mut enter = 0.0_f64;
        let mut exit = 1.0_f64;
        for axis in 0..3 {
            let origin = [start.x, start.y, start.z][axis];
            let lower = self.coordinate[axis] * side;
            let upper = lower.saturating_add(side);
            let near = coordinate_difference(lower, origin);
            let far = coordinate_difference(upper, origin);
            let padding = padded_radius(radius) + near.abs().max(far.abs()) * 32.0 * f64::EPSILON;
            let near = near - padding;
            let far = far + padding;
            let direction = displacement[axis];
            if direction == 0.0 {
                if near > 0.0 || far < 0.0 {
                    return false;
                }
            } else {
                let a = near / direction;
                let b = far / direction;
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
                if enter > exit {
                    return false;
                }
            }
        }
        true
    }
}

#[derive(Clone, Default)]
struct Cell {
    occupants: Vec<u32>,
    children: Vec<CellKey>,
    max_radius_m: f64,
}

#[derive(Clone, Default)]
struct Cells {
    nodes: AHashMap<CellKey, Cell>,
    roots: Vec<CellKey>,
}

impl Cells {
    fn insert(&mut self, id: u32, entries: &AHashMap<u32, Entry>) {
        let entry = entries[&id];
        let root = CellKey::at(entry.position, ROOT_LEVEL);
        if !self.nodes.contains_key(&root) {
            self.roots.push(root);
            self.roots.sort_unstable();
        }
        self.insert_into(root, id, entries);
    }

    fn insert_into(&mut self, key: CellKey, id: u32, entries: &AHashMap<u32, Entry>) {
        let entry = entries[&id];
        let cell = self.nodes.entry(key).or_default();
        cell.max_radius_m = cell.max_radius_m.max(entry.radius_m);
        if !cell.children.is_empty() {
            let octant = CellKey::at(entry.position, key.level - 1);
            let child_slot = cell
                .children
                .iter()
                .position(|child| CellKey::at(child.origin(), key.level - 1) == octant);
            let child = if let Some(slot) = child_slot {
                let previous = cell.children[slot];
                if previous.contains(entry.position) {
                    previous
                } else {
                    let branch = CellKey::common(previous.origin(), entry.position);
                    cell.children[slot] = branch;
                    let radius = self.nodes[&previous].max_radius_m;
                    self.nodes.insert(
                        branch,
                        Cell {
                            children: vec![previous],
                            max_radius_m: radius,
                            ..Default::default()
                        },
                    );
                    branch
                }
            } else {
                cell.children.push(octant);
                octant
            };
            self.insert_into(child, id, entries);
            return;
        }
        cell.occupants.push(id);
        if cell.occupants.len() <= LEAF_CAPACITY || key.level <= MIN_LEVEL {
            return;
        }
        let occupants = std::mem::take(&mut cell.occupants);
        let first = entries[&occupants[0]].position;
        let split_level = occupants
            .iter()
            .map(|id| CellKey::common(first, entries[id].position).level)
            .max()
            .unwrap();
        let branch = if split_level < key.level {
            let branch = CellKey::at(first, split_level);
            self.nodes.get_mut(&key).unwrap().children.push(branch);
            self.nodes.insert(branch, Cell::default());
            branch
        } else {
            key
        };
        for id in occupants {
            let position = entries[&id].position;
            let radius = entries[&id].radius_m;
            let parent = self.nodes.get_mut(&branch).unwrap();
            parent.max_radius_m = parent.max_radius_m.max(radius);
            if branch.level == MIN_LEVEL {
                parent.occupants.push(id);
            } else {
                let child = CellKey::at(position, branch.level - 1);
                if !parent.children.contains(&child) {
                    parent.children.push(child);
                }
                self.insert_into(child, id, entries);
            }
        }
    }

    fn update_contained(
        &mut self,
        id: u32,
        old: Entry,
        next: Entry,
        entries: &AHashMap<u32, Entry>,
    ) -> bool {
        self.update_cell(
            CellKey::at(old.position, ROOT_LEVEL),
            id,
            old,
            next,
            entries,
        )
    }

    fn update_cell(
        &mut self,
        key: CellKey,
        id: u32,
        old: Entry,
        next: Entry,
        entries: &AHashMap<u32, Entry>,
    ) -> bool {
        let cell = &self.nodes[&key];
        if cell.children.is_empty() {
            if !key.contains(next.position) {
                return false;
            }
            if old.radius_m == next.radius_m {
                return true;
            }
            let radius = if next.radius_m >= cell.max_radius_m {
                next.radius_m
            } else if old.radius_m == cell.max_radius_m {
                cell.occupants
                    .iter()
                    .map(|&candidate| {
                        if candidate == id {
                            next.radius_m
                        } else {
                            entries[&candidate].radius_m
                        }
                    })
                    .fold(0.0, f64::max)
            } else {
                cell.max_radius_m
            };
            self.nodes.get_mut(&key).unwrap().max_radius_m = radius;
            return true;
        }
        let child = cell
            .children
            .iter()
            .copied()
            .find(|child| child.contains(old.position))
            .unwrap();
        if !self.update_cell(child, id, old, next, entries) {
            return false;
        }
        if old.radius_m != next.radius_m {
            let cell = &self.nodes[&key];
            let radius = cell
                .children
                .iter()
                .map(|key| self.nodes[key].max_radius_m)
                .fold(0.0, f64::max);
            self.nodes.get_mut(&key).unwrap().max_radius_m = radius;
        }
        true
    }

    fn remove(&mut self, id: u32, entries: &AHashMap<u32, Entry>) {
        let position = entries[&id].position;
        let root = CellKey::at(position, ROOT_LEVEL);
        self.remove_from(root, id, position, entries);
        if !self.nodes.contains_key(&root) {
            self.roots.retain(|&key| key != root);
        }
    }

    fn remove_from(
        &mut self,
        key: CellKey,
        id: u32,
        position: GalacticPosition,
        entries: &AHashMap<u32, Entry>,
    ) {
        if self.nodes[&key].children.is_empty() {
            let cell = self.nodes.get_mut(&key).unwrap();
            cell.occupants.retain(|&candidate| candidate != id);
            if cell.occupants.is_empty() {
                self.nodes.remove(&key);
            } else {
                cell.max_radius_m = cell
                    .occupants
                    .iter()
                    .map(|id| entries[id].radius_m)
                    .fold(0.0, f64::max);
            }
            return;
        }
        let child = self.nodes[&key]
            .children
            .iter()
            .copied()
            .find(|child| child.contains(position))
            .unwrap();
        self.remove_from(child, id, position, entries);
        if !self.nodes.contains_key(&child) {
            self.nodes
                .get_mut(&key)
                .unwrap()
                .children
                .retain(|&key| key != child);
        }
        let cell = &self.nodes[&key];
        if cell.children.is_empty() {
            self.nodes.remove(&key);
        } else {
            let radius = cell
                .children
                .iter()
                .map(|key| self.nodes[key].max_radius_m)
                .fold(0.0, f64::max);
            self.nodes.get_mut(&key).unwrap().max_radius_m = radius;
        }
    }

    fn query(
        &self,
        centre: GalacticPosition,
        radius: f64,
        extents: bool,
        stats: &mut QueryStats,
        visit: &mut impl FnMut(u32),
    ) {
        let mut pending = self.roots.clone();
        while let Some(key) = pending.pop() {
            let cell = &self.nodes[&key];
            stats.cells_visited += 1;
            let radius = radius + if extents { cell.max_radius_m } else { 0.0 };
            let padded = padded_radius(radius);
            if key.distance_squared(centre) > padded * padded {
                continue;
            }
            if cell.children.is_empty() {
                stats.candidates += cell.occupants.len();
                for &id in &cell.occupants {
                    visit(id);
                }
            } else {
                pending.extend(cell.children.iter().copied());
            }
        }
    }
}

fn luminosity_bucket(luminosity: f64) -> Option<i16> {
    if luminosity == 0.0 {
        return None;
    }
    let bits = luminosity.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i16;
    if exponent != 0 {
        Some(exponent - 1023)
    } else {
        Some((63 - bits.leading_zeros()) as i16 - 1074)
    }
}

fn luminosity_upper(bucket: i16) -> f64 {
    let exponent = bucket as i32 + 1;
    if exponent > 1023 {
        f64::INFINITY
    } else if exponent >= -1022 {
        f64::from_bits(((exponent + 1023) as u64) << 52)
    } else {
        f64::from_bits(1_u64 << (exponent + 1074))
    }
}

/// A sparse hierarchy of hash-addressed spatial cells, partitioned additionally
/// by powers of two in luminosity. Position and brightness changes update only
/// the affected paths; empty cells and buckets disappear immediately.
pub struct SpatialHash {
    identity: u64,
    generation: u64,
    entries: AHashMap<u32, Entry>,
    positions: Cells,
    luminous: BTreeMap<i16, Cells>,
}

static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

impl Default for SpatialHash {
    fn default() -> Self {
        Self {
            identity: NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            generation: 0,
            entries: AHashMap::default(),
            positions: Cells::default(),
            luminous: BTreeMap::default(),
        }
    }
}

impl Clone for SpatialHash {
    fn clone(&self) -> Self {
        Self {
            identity: NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            generation: self.generation,
            entries: self.entries.clone(),
            positions: self.positions.clone(),
            luminous: self.luminous.clone(),
        }
    }
}

impl SpatialHash {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn occupied_cells(&self) -> usize {
        self.positions.nodes.len()
    }

    pub fn bucket_count(&self) -> usize {
        self.luminous.len()
    }

    pub fn get(&self, id: u32) -> Option<&Entry> {
        self.entries.get(&id)
    }

    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
        self.positions = Cells::default();
        self.luminous.clear();
    }

    pub fn insert(&mut self, id: u32, entry: Entry) {
        assert!(entry.radius_m.is_finite() && entry.radius_m >= 0.0);
        assert!(entry.luminosity.is_finite() && entry.luminosity >= 0.0);
        if self
            .entries
            .get(&id)
            .is_some_and(|old| old.position == entry.position && old.radius_m == entry.radius_m)
        {
            self.set_luminosity(id, entry.luminosity);
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let old = self.entries.get(&id).copied();
        let next_bucket = luminosity_bucket(entry.luminosity);
        let mut position_retained = false;
        let mut luminosity_retained = false;
        if let Some(old) = old {
            position_retained = self
                .positions
                .update_contained(id, old, entry, &self.entries);
            if !position_retained {
                self.positions.remove(id, &self.entries);
            }
            if let Some(bucket) = luminosity_bucket(old.luminosity) {
                let cells = self.luminous.get_mut(&bucket).unwrap();
                luminosity_retained = Some(bucket) == next_bucket
                    && cells.update_contained(id, old, entry, &self.entries);
                if !luminosity_retained {
                    cells.remove(id, &self.entries);
                    if cells.nodes.is_empty() {
                        self.luminous.remove(&bucket);
                    }
                }
            }
        }
        self.entries.insert(id, entry);
        if !position_retained {
            self.positions.insert(id, &self.entries);
        }
        if !luminosity_retained {
            if let Some(bucket) = next_bucket {
                self.luminous
                    .entry(bucket)
                    .or_default()
                    .insert(id, &self.entries);
            }
        }
    }

    pub fn remove(&mut self, id: u32) -> Option<Entry> {
        let entry = *self.entries.get(&id)?;
        self.generation = self.generation.wrapping_add(1);
        self.positions.remove(id, &self.entries);
        if let Some(bucket) = luminosity_bucket(entry.luminosity) {
            let cells = self.luminous.get_mut(&bucket).unwrap();
            cells.remove(id, &self.entries);
            if cells.nodes.is_empty() {
                self.luminous.remove(&bucket);
            }
        }
        self.entries.remove(&id)
    }

    pub fn within_radius(&self, centre: GalacticPosition, radius_m: f64) -> QueryResult {
        self.range(centre, radius_m, false)
    }

    pub fn intersecting_sphere(&self, centre: GalacticPosition, radius_m: f64) -> QueryResult {
        self.range(centre, radius_m, true)
    }

    fn range(&self, centre: GalacticPosition, radius_m: f64, extents: bool) -> QueryResult {
        let mut result = QueryResult::default();
        if radius_m.is_nan() || radius_m < 0.0 {
            return result;
        }
        self.positions
            .query(centre, radius_m, extents, &mut result.stats, &mut |id| {
                let entry = self.entries[&id];
                let radius = radius_m + if extents { entry.radius_m } else { 0.0 };
                if distance_squared(entry.position, centre) <= radius * radius {
                    result.ids.push(id);
                }
            });
        result.ids.sort_unstable();
        result
    }

    pub fn visible(&self, centre: GalacticPosition, min_brightness: f64) -> QueryResult {
        self.visible_from_sphere(centre, 0.0, min_brightness)
    }

    pub fn visible_from_sphere(
        &self,
        centre: GalacticPosition,
        observer_radius_m: f64,
        min_brightness: f64,
    ) -> QueryResult {
        let mut result = QueryResult::default();
        if !min_brightness.is_finite()
            || min_brightness < 0.0
            || !observer_radius_m.is_finite()
            || observer_radius_m < 0.0
        {
            return result;
        }
        for (&bucket, cells) in &self.luminous {
            result.stats.buckets_searched += 1;
            let radius = (luminosity_upper(bucket) / min_brightness).sqrt() + observer_radius_m;
            cells.query(centre, radius, false, &mut result.stats, &mut |id| {
                let entry = self.entries[&id];
                let d2 = if observer_radius_m == 0.0 {
                    distance_squared(entry.position, centre)
                } else {
                    (distance_squared(entry.position, centre).sqrt() - observer_radius_m)
                        .max(0.0)
                        .powi(2)
                };
                if d2 == 0.0 || entry.luminosity / d2 >= min_brightness {
                    result.ids.push(id);
                }
            });
        }
        result.ids.sort_unstable();
        result
    }

    pub fn nearest(&self, centre: GalacticPosition, radius_m: f64, count: usize) -> Vec<u32> {
        self.nearest_filtered(centre, radius_m, count, |id| Some(id as u64))
    }

    pub fn nearest_filtered(
        &self,
        centre: GalacticPosition,
        radius_m: f64,
        count: usize,
        accept: impl Fn(u32) -> Option<u64>,
    ) -> Vec<u32> {
        if count == 0 || radius_m.is_nan() || radius_m < 0.0 {
            return Vec::new();
        }
        let mut pending = BinaryHeap::new();
        let mut best = BinaryHeap::<Ranked>::new();
        for &key in &self.positions.roots {
            pending.push(PendingCell {
                distance: key.distance_squared(centre),
                key,
            });
        }
        while let Some(next) = pending.pop() {
            let limit = if best.len() == count {
                best.peek().unwrap().distance.min(radius_m * radius_m)
            } else {
                radius_m * radius_m
            };
            if next.distance > limit * (1.0 + 64.0 * f64::EPSILON) + 1e-12 {
                break;
            }
            let cell = &self.positions.nodes[&next.key];
            if !cell.children.is_empty() {
                for &key in &cell.children {
                    pending.push(PendingCell {
                        distance: key.distance_squared(centre),
                        key,
                    });
                }
            } else {
                for &id in &cell.occupants {
                    let Some(tie) = accept(id) else { continue };
                    let distance = distance_squared(self.entries[&id].position, centre);
                    if distance <= limit {
                        best.push(Ranked { distance, tie, id });
                        if best.len() > count {
                            best.pop();
                        }
                    }
                }
            }
        }
        best.into_sorted_vec()
            .into_iter()
            .map(|entry| entry.id)
            .collect()
    }

    pub fn brightest(&self, centre: GalacticPosition) -> Option<u32> {
        let mut best = None;
        let mut brightness: f64 = 0.0;
        for (&bucket, cells) in self.luminous.iter().rev() {
            let radius = (luminosity_upper(bucket) / brightness).sqrt();
            cells.query(
                centre,
                radius,
                false,
                &mut QueryStats::default(),
                &mut |id| {
                    let entry = self.entries[&id];
                    let value =
                        entry.luminosity / distance_squared(entry.position, centre).max(1.0);
                    if value > brightness
                        || (value == brightness && best.is_none_or(|old| id < old))
                    {
                        brightness = value;
                        best = Some(id);
                    }
                },
            );
        }
        best
    }

    pub fn set_luminosity(&mut self, id: u32, luminosity: f64) {
        assert!(luminosity.is_finite() && luminosity >= 0.0);
        let old = self.entries[&id].luminosity;
        if old == luminosity {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        if luminosity_bucket(old) == luminosity_bucket(luminosity) {
            self.entries.get_mut(&id).unwrap().luminosity = luminosity;
            return;
        }
        if let Some(bucket) = luminosity_bucket(old) {
            let cells = self.luminous.get_mut(&bucket).unwrap();
            cells.remove(id, &self.entries);
            if cells.nodes.is_empty() {
                self.luminous.remove(&bucket);
            }
        }
        self.entries.get_mut(&id).unwrap().luminosity = luminosity;
        if let Some(bucket) = luminosity_bucket(luminosity) {
            self.luminous
                .entry(bucket)
                .or_default()
                .insert(id, &self.entries);
        }
    }

    /// Returns all spheres touched by the swept query sphere. Cell rejection
    /// uses an expanded box; exact segment/sphere distance rejects false hits.
    pub fn segment_candidates(
        &self,
        start: GalacticPosition,
        displacement: DVec3,
        radius_m: f64,
    ) -> QueryResult {
        let mut result = QueryResult::default();
        if !displacement.is_finite() || radius_m.is_nan() || radius_m < 0.0 {
            return result;
        }
        let scale = displacement.abs().max_element();
        let (direction, length) = if scale == 0.0 {
            (DVec3::ZERO, 0.0)
        } else {
            let scaled = displacement / scale;
            (scaled.normalize(), scaled.length() * scale)
        };
        let mut pending = self.positions.roots.clone();
        while let Some(key) = pending.pop() {
            let cell = &self.positions.nodes[&key];
            result.stats.cells_visited += 1;
            if !key.intersects_segment(start, displacement, radius_m + cell.max_radius_m) {
                continue;
            }
            if !cell.children.is_empty() {
                pending.extend(cell.children.iter().copied());
                continue;
            }
            for &id in &cell.occupants {
                result.stats.candidates += 1;
                let entry = self.entries[&id];
                let offset = relative_position(entry.position, start);
                let along = offset.dot(direction).clamp(0.0, length);
                let error = offset.abs().max_element().max(along.abs()) * 64.0 * f64::EPSILON;
                let limit = padded_radius(radius_m + entry.radius_m) + error;
                if (offset - direction * along).length_squared() <= limit * limit {
                    result.ids.push(id);
                }
            }
        }
        result.ids.sort_unstable();
        result
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PendingCell {
    distance: f64,
    key: CellKey,
}
impl Eq for PendingCell {}
impl Ord for PendingCell {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| self.key.cmp(&other.key))
    }
}
impl PartialOrd for PendingCell {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Ranked {
    distance: f64,
    tie: u64,
    id: u32,
}
impl Eq for Ranked {}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.distance
            .total_cmp(&other.distance)
            .then_with(|| self.tie.cmp(&other.tie))
            .then_with(|| self.id.cmp(&other.id))
    }
}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub fn distance_squared(a: GalacticPosition, b: GalacticPosition) -> f64 {
    let dx = a.x.abs_diff(b.x) as f64 / 1_000_000.0;
    let dy = a.y.abs_diff(b.y) as f64 / 1_000_000.0;
    let dz = a.z.abs_diff(b.z) as f64 / 1_000_000.0;
    dx * dx + dy * dy + dz * dz
}

fn padded_radius(radius: f64) -> f64 {
    radius * (1.0 + 32.0 * f64::EPSILON) + 1e-6
}

fn coordinate_difference(a: i128, b: i128) -> f64 {
    let magnitude = a.abs_diff(b) as f64 / 1_000_000.0;
    if a >= b { magnitude } else { -magnitude }
}

fn relative_position(a: GalacticPosition, b: GalacticPosition) -> DVec3 {
    DVec3::new(
        coordinate_difference(a.x, b.x),
        coordinate_difference(a.y, b.y),
        coordinate_difference(a.z, b.z),
    )
}

mod cursor;
pub use cursor::{RangeBatch, RangeCursor};

#[cfg(test)]
mod tests;
