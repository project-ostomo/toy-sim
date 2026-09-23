use std::hash::Hash;

use crate::{
    Nearest, Position, QueryBudget, QueryExhausted,
    neighbor_index::{NeighborIndex, Record, Records},
};

const BRIGHTNESS_STEP: usize = if cfg!(feature = "luminosity-scale-4") {
    2
} else {
    1
};
const BUCKETS: usize = 64 / BRIGHTNESS_STEP;

/// An index of sources visible under inverse-square brightness falloff.
pub struct LuminosityMap<T> {
    records: Records<T, f64>,
    // Clamp values below the first exponent and above the last bucket.
    buckets: [NeighborIndex; BUCKETS],
    // Retain the maximum seen to avoid scanning records on removal.
    top_brightness_bound: f64,
    minimum_shift: u32,
}

fn brightness_bucket(brightness: f64) -> usize {
    ((brightness.to_bits() >> 52) & 0x7ff)
        .saturating_sub(1023)
        .min(63) as usize
        / BRIGHTNESS_STEP
}

impl<T: Eq + Hash> LuminosityMap<T> {
    pub fn new() -> Self {
        Self::with_minimum_cell_shift(0)
    }

    /// Uses cells at least `2^shift` coordinate units wide, with `shift <= 63`.
    /// Stored positions and exact query filtering retain their original precision.
    /// Larger cells reduce update work at the cost of more query candidates.
    pub fn with_minimum_cell_shift(shift: u32) -> Self {
        Self {
            records: Records::new(),
            buckets: std::array::from_fn(|_| NeighborIndex::with_minimum_cell_shift(shift)),
            top_brightness_bound: 0.0,
            minimum_shift: shift,
        }
    }

    /// Inserts or updates a source's position and absolute brightness.
    /// Returns whether the stored record changed.
    /// Panics unless brightness is finite and nonnegative.
    pub fn insert(&mut self, item: T, position: Position, brightness: f64) -> bool
    where
        T: Clone,
    {
        assert!(brightness.is_finite() && brightness >= 0.0);
        let bucket = brightness_bucket(brightness);
        self.top_brightness_bound = self.top_brightness_bound.max(brightness);
        if let Some(&key) = self.records.keys.get(&item) {
            let record = &mut self.records.slots[key];
            if record.position == position && record.metadata == brightness {
                return false;
            }
            let old_bucket = brightness_bucket(record.metadata);
            let old = if bucket == old_bucket {
                Some(record.position)
            } else {
                self.buckets[old_bucket].remove(key, record.position);
                None
            };
            self.buckets[bucket].insert(key, position, old);
            record.position = position;
            record.metadata = brightness;
        } else {
            let key = self.records.slots.insert(Record {
                item: item.clone(),
                position,
                metadata: brightness,
            });
            self.records.keys.insert(item, key);
            self.buckets[bucket].insert(key, position, None);
        }
        true
    }

    pub fn len(&self) -> usize {
        self.records.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.keys.is_empty()
    }

    /// Finds the minimum eligible exact distance by expanding radius queries.
    /// `distance` returns None for ineligible items. Its result must be at least
    /// the stored centre distance minus `maximum_offset`; include object radii
    /// and combined position quantization error in that bound. All distances use
    /// index coordinate units. This supports nearest surfaces as well as centres.
    ///
    /// A successful None proves that no eligible item exists. Budget exhaustion
    /// is a separate result and must not be interpreted as empty space.
    pub fn nearest_by(
        &self,
        position: Position,
        maximum_offset: f64,
        budget: &mut QueryBudget,
        distance: impl FnMut(&T) -> Option<f64>,
    ) -> Result<Option<Nearest<'_, T>>, QueryExhausted> {
        self.nearest_within_by(position, maximum_offset, f64::INFINITY, budget, distance)
    }

    /// Nearest eligible distance within a finite limit. Proving the bounded
    /// region empty does not require scanning distant or ineligible records.
    pub fn nearest_within_by(
        &self,
        position: Position,
        maximum_offset: f64,
        maximum_distance: f64,
        budget: &mut QueryBudget,
        mut distance: impl FnMut(&T) -> Option<f64>,
    ) -> Result<Option<Nearest<'_, T>>, QueryExhausted> {
        assert!(maximum_offset.is_finite() && maximum_offset >= 0.0);
        assert!(!maximum_distance.is_nan());
        if self.is_empty() {
            return Ok(None);
        }

        let mut radius = 2_f64.powi(self.minimum_shift as i32);
        let mut best: Option<Nearest<'_, T>> = None;
        loop {
            budget.charge_probe()?;
            let all = radius >= 2_f64.powi(63);
            for bucket in &self.buckets {
                let nearby = (!all)
                    .then_some(radius as i64)
                    .into_iter()
                    .flat_map(|r| bucket.candidates(&self.records.slots, position, r, 0));
                let unbounded = bucket.all(&self.records.slots, position).take(if all {
                    usize::MAX
                } else {
                    0
                });
                for (record, squared) in nearby.chain(unbounded) {
                    budget.charge()?;
                    if !all && squared > radius * radius {
                        continue;
                    }
                    if let Some(value) = distance(&record.item) {
                        assert!(value.is_finite(), "exact query distance must be finite");
                        if value <= maximum_distance
                            && best.as_ref().is_none_or(|old| value < old.distance)
                        {
                            best = Some(Nearest {
                                item: &record.item,
                                distance: value,
                            });
                        }
                    }
                }
            }

            // Round the lower bound outward, including at tangencies.
            let rounding = 64.0 * f64::EPSILON * (radius + maximum_offset);
            let lower = (radius.next_down() - maximum_offset - rounding).next_down();
            if all
                || lower > maximum_distance
                || best.as_ref().is_some_and(|hit| lower > hit.distance)
            {
                return Ok(best);
            }
            radius *= 2.0;
        }
    }

    /// Removes a source, if present.
    pub fn remove(&mut self, item: &T) {
        if let Some(key) = self.records.keys.remove(item) {
            let record = &self.records.slots[key];
            self.buckets[brightness_bucket(record.metadata)].remove(key, record.position);
            // Every index must release the key before the slab can reuse it.
            self.records.slots.remove(key);
        }
    }

    /// Yields sources with `brightness / distance² >= threshold`.
    /// Results borrow stored items, are unordered, and are filtered individually.
    /// A positive source at distance zero is always visible; zero brightness is never visible.
    /// Panics unless the threshold is finite and positive.
    pub fn nearest_visible(&self, position: Position, threshold: f64) -> impl Iterator<Item = &T> {
        self.visible(position, threshold, 0, 0.0)
    }

    /// Geometric radius query across all brightness buckets, including dark sources.
    pub fn within_radius(
        &self,
        position: Position,
        radius: i64,
    ) -> impl Iterator<Item = (&T, &Position)> {
        self.buckets.iter().flat_map(move |bucket| {
            bucket
                .nearest(&self.records.slots, position, radius, 0)
                .map(|(record, _)| (&record.item, &record.position))
        })
    }

    /// Radius query that charges every examined cell candidate before filtering.
    /// Infinite radii enumerate the whole index under the same work budget.
    pub fn within_radius_budgeted(
        &self,
        position: Position,
        radius: f64,
        budget: &mut QueryBudget,
    ) -> Result<Vec<&T>, QueryExhausted> {
        assert!(!radius.is_nan() && radius >= 0.0);
        budget.charge_probe()?;
        let all = radius >= 2_f64.powi(63);
        let radius = radius.ceil();
        let mut found = Vec::new();
        for bucket in &self.buckets {
            let nearby = (!all)
                .then_some(radius as i64)
                .into_iter()
                .flat_map(|r| bucket.candidates(&self.records.slots, position, r, 0));
            let unbounded =
                bucket
                    .all(&self.records.slots, position)
                    .take(if all { usize::MAX } else { 0 });
            for (record, squared) in nearby.chain(unbounded) {
                budget.charge()?;
                if all || squared <= (radius * radius).next_up() {
                    found.push(&record.item);
                }
            }
        }
        Ok(found)
    }

    /// Conservative visibility candidates when stored/query positions have bounded error.
    /// `padding` is the maximum combined position error, in index coordinate units.
    /// Callers must apply their exact brightness test using authoritative positions.
    pub fn visible_candidates(
        &self,
        position: Position,
        threshold: f64,
        padding: f64,
    ) -> impl Iterator<Item = &T> {
        assert!(padding.is_finite() && padding >= 0.0);
        self.visible(position, threshold, 0, padding)
    }

    /// Uses one finer spatial level to trade more cell lookups for fewer candidates.
    /// Returns the same visible sources as `nearest_visible`, in arbitrary order.
    pub fn nearest_visible_finer(
        &self,
        position: Position,
        threshold: f64,
    ) -> impl Iterator<Item = &T> {
        self.visible(position, threshold, 1, 0.0)
    }

    fn visible(
        &self,
        position: Position,
        threshold: f64,
        finer: usize,
        padding: f64,
    ) -> impl Iterator<Item = &T> {
        assert!(threshold.is_finite() && threshold > 0.0);
        self.buckets
            .iter()
            .enumerate()
            .flat_map(move |(index, bucket)| {
                let upper = if index == BUCKETS - 1 {
                    self.top_brightness_bound
                } else {
                    2_f64.powi(((index + 1) * BRIGHTNESS_STEP) as i32)
                };
                // Round outward to retain floating-point boundary candidates.
                let radius = ((upper / threshold).next_up().sqrt().next_up() + padding)
                    .next_up()
                    .ceil();
                // Extreme coordinate separations can exceed every i64 radius.
                let all = radius >= 2_f64.powi(63);
                let nearby = (!all)
                    .then_some(radius as i64)
                    .into_iter()
                    .flat_map(move |radius| {
                        bucket.nearest(&self.records.slots, position, radius, finer)
                    });
                let unbounded = bucket.all(&self.records.slots, position).take(if all {
                    usize::MAX
                } else {
                    0
                });
                nearby
                    .chain(unbounded)
                    .filter_map(move |(record, distance)| {
                        let visible = if padding == 0.0 {
                            record.metadata / distance >= threshold
                        } else {
                            let minimum_distance = (distance.sqrt().next_down() - padding).max(0.0);
                            let denominator = minimum_distance.powi(2).next_down().max(0.0);
                            (record.metadata / denominator).next_up() >= threshold
                        };
                        (record.metadata > 0.0 && visible).then_some(&record.item)
                    })
            })
    }
}

impl<T: Eq + Hash> Default for LuminosityMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_surface_can_have_a_farther_centre() {
        let mut map = LuminosityMap::with_minimum_cell_shift(9);
        let origin = Position { x: 0, y: 0, z: 0 };
        map.insert(0, Position { x: 100, ..origin }, 0.0);
        map.insert(1, Position { x: 1000, ..origin }, 1e20);
        let surfaces = [99.0, 10.0];
        let hit = map
            .nearest_by(origin, 990.0, &mut QueryBudget::new(100), |&id| {
                Some(surfaces[id])
            })
            .unwrap()
            .unwrap();
        assert_eq!(*hit.item, 1);
        assert_eq!(hit.distance, 10.0);

        map.remove(&1);
        let hit = map
            .nearest_by(origin, 990.0, &mut QueryBudget::new(100), |&id| {
                Some(surfaces[id])
            })
            .unwrap()
            .unwrap();
        assert_eq!(*hit.item, 0);
    }

    #[test]
    fn nearest_budget_counts_rejected_cell_candidates() {
        let mut map = LuminosityMap::with_minimum_cell_shift(9);
        let origin = Position { x: 0, y: 0, z: 0 };
        // These lie in a visited cell but outside the initial radius sphere.
        for id in 0..100 {
            map.insert(
                id,
                Position {
                    x: 510,
                    y: 510,
                    z: 510,
                },
                0.0,
            );
        }
        let mut budget = QueryBudget::new(20);
        assert_eq!(
            map.nearest_by(origin, 0.0, &mut budget, |_| None)
                .unwrap_err(),
            QueryExhausted
        );
        assert_eq!(budget.used(), 20);
    }

    #[test]
    fn nearest_expansion_covers_extreme_separations_and_empty_filters() {
        let mut map = LuminosityMap::with_minimum_cell_shift(9);
        let origin = Position {
            x: i64::MIN,
            y: 0,
            z: 0,
        };
        let target = Position {
            x: i64::MAX,
            ..origin
        };
        map.insert(1, target, 0.0);
        let hit = map
            .nearest_by(origin, 0.0, &mut QueryBudget::new(1000), |_| {
                Some(origin.x.abs_diff(target.x) as f64)
            })
            .unwrap()
            .unwrap();
        assert_eq!(*hit.item, 1);
        assert!(
            map.nearest_by(origin, 0.0, &mut QueryBudget::new(1000), |_| None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn visibility_filters_exact_brightness_and_distance() {
        let mut map = LuminosityMap::new();
        let origin = Position { x: 0, y: 0, z: 0 };
        map.insert(1, Position { x: 1, ..origin }, 0.25);
        map.insert(2, Position { x: 3, y: 4, z: 0 }, 25.0);
        map.insert(3, Position { x: 3, y: 4, z: 0 }, 24.0);
        map.insert(4, origin, 0.0);
        map.insert(5, origin, 0.01);
        let mut visible: Vec<_> = map.nearest_visible(origin, 1.0).copied().collect();
        visible.sort_unstable();
        assert_eq!(visible, vec![2, 5]);
        assert!(map.nearest_visible(origin, 0.25).any(|&item| item == 1));

        // Moving between brightness buckets must remove the previous entry.
        map.insert(2, origin, 0.5);
        assert_eq!(
            map.nearest_visible(origin, 1.0)
                .filter(|&&item| item == 2)
                .count(),
            1
        );
        map.insert(2, Position { x: 2, ..origin }, 0.75);
        assert!(!map.nearest_visible(origin, 1.0).any(|&item| item == 2));
    }

    #[test]
    fn top_bucket_covers_brightness_and_distance_beyond_integer_radius() {
        let mut map = LuminosityMap::new();
        let origin = Position { x: 0, y: 0, z: 0 };
        map.insert(
            1,
            Position {
                x: 1_i64 << 40,
                ..origin
            },
            2_f64.powi(80),
        );
        assert_eq!(
            map.nearest_visible(origin, 1.0)
                .copied()
                .collect::<Vec<_>>(),
            vec![1]
        );

        let query = Position {
            x: i64::MIN,
            ..origin
        };
        map.insert(
            2,
            Position {
                x: i64::MAX,
                ..origin
            },
            f64::MAX,
        );
        assert!(map.nearest_visible(query, 1.0).any(|&item| item == 2));
    }
}
