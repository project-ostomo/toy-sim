use std::hash::Hash;

use crate::{
    Position,
    neighbor_index::{NeighborIndex, Record, Records},
};

/// A spatial index that can efficiently answer "what is within a given radius" queries.
pub struct NeighborMap<T> {
    records: Records<T, ()>,
    index: NeighborIndex,
}

impl<T: Eq + Hash> NeighborMap<T> {
    pub fn new() -> Self {
        Self {
            records: Records::new(),
            index: NeighborIndex::new(),
        }
    }

    /// Inserts an item, updating only resolutions where its cell has changed.
    pub fn insert(&mut self, item: T, position: Position)
    where
        T: Clone,
    {
        if let Some(&key) = self.records.keys.get(&item) {
            let record = &mut self.records.slots[key];
            self.index.insert(key, position, Some(record.position));
            record.position = position;
        } else {
            let key = self.records.slots.insert(Record {
                item: item.clone(),
                position,
                metadata: (),
            });
            self.records.keys.insert(item, key);
            self.index.insert(key, position, None);
        }
    }

    /// Removes an item from every resolution, if present.
    pub fn remove(&mut self, item: &T) {
        if let Some(key) = self.records.keys.remove(item) {
            self.index.remove(key, self.records.slots[key].position);
            // Every index must release the key before the slab can reuse it.
            self.records.slots.remove(key);
        }
    }

    /// Yields items within `radius`, using f64 squared Euclidean distances.
    /// Results are unordered. Negative radii yield no items.
    /// Visits at most 27 cells; total cost also depends on the candidates examined.
    pub fn nearest(
        &self,
        position: Position,
        radius: i64,
    ) -> impl Iterator<Item = (&T, &Position)> {
        self.index
            .nearest(&self.records.slots, position, radius, 0)
            .map(|(r, _)| (&r.item, &r.position))
    }

    /// Returns the same items using one finer spatial level, visiting at most 729 cells.
    /// With `spatial-scale-2`, the finer level visits at most 125 cells.
    pub fn nearest_finer(
        &self,
        position: Position,
        radius: i64,
    ) -> impl Iterator<Item = (&T, &Position)> {
        self.index
            .nearest(&self.records.slots, position, radius, 1)
            .map(|(r, _)| (&r.item, &r.position))
    }
}

impl<T: Eq + Hash> Default for NeighborMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_filters_distance_at_both_resolutions() {
        let mut map = NeighborMap::new();
        let origin = Position { x: 0, y: 0, z: 0 };
        map.insert(1, Position { x: 3, y: 0, z: 0 });
        map.insert(2, Position { x: 15, y: 0, z: 0 });
        assert_eq!(map.nearest(origin, 1).count(), 0);
        assert_eq!(map.nearest(origin, 2).count(), 0);
        for radius in [3, 4] {
            assert_eq!(
                map.nearest(origin, radius)
                    .map(|(item, _)| *item)
                    .collect::<Vec<_>>(),
                vec![1]
            );
        }
        assert_eq!(map.nearest(origin, 5).count(), 1);
        assert_eq!(map.nearest(origin, -1).count(), 0);
        assert_eq!(map.nearest_finer(origin, 2).count(), 0);
        assert_eq!(
            map.nearest_finer(origin, 3)
                .map(|(item, _)| *item)
                .collect::<Vec<_>>(),
            vec![1]
        );
        map.insert(3, Position { x: 3, y: 4, z: 0 });
        map.insert(4, Position { x: 4, y: 4, z: 0 });
        let mut precise: Vec<_> = map.nearest(origin, 5).map(|(&id, _)| id).collect();
        precise.sort_unstable();
        assert_eq!(precise, vec![1, 3]);
    }

    #[test]
    fn moving_items_updates_every_resolution() {
        let mut map = NeighborMap::new();
        let origin = Position { x: 0, y: 0, z: 0 };
        let destination = Position {
            x: i64::MAX,
            y: -7,
            z: 9,
        };
        map.insert(1, origin);
        for position in [
            Position { x: 1, y: 0, z: 0 }, // Only the finest cell changes.
            Position { x: 1, y: 0, z: 0 }, // No cells change.
            destination, // Crosses cells at every resolution, including the sign boundary.
        ] {
            map.insert(1, position);
            assert_eq!(map.nearest(origin, 0).count(), 0);
            let radii = std::iter::once(0)
                .chain((0..32).map(|level| 1_i64 << (level * 2)))
                .chain(std::iter::once(i64::MAX));
            for radius in radii {
                assert_eq!(
                    map.nearest(position, radius).collect::<Vec<_>>(),
                    vec![(&1, &position)]
                );
                assert_eq!(
                    map.nearest_finer(position, radius).collect::<Vec<_>>(),
                    vec![(&1, &position)]
                );
            }
        }
    }
}
