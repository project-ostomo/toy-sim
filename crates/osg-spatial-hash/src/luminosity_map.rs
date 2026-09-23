use std::hash::Hash;

use crate::{
    Position,
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
