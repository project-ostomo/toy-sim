use std::hash::Hash;

use ahash::AHashMap;

use crate::{Position, spatial_cell::Cell};

/// A single-level spatial hash.
pub struct SpatialHash<T: Eq + Hash> {
    map: AHashMap<(i64, i64, i64), Cell<T>>,
    shift: i64,
}

impl<T: Eq + Hash> SpatialHash<T> {
    /// Creates a spatial hash with cells of width `2^shift`.
    /// Panics if `shift` is outside `0..64`.
    pub fn new(shift: i64) -> Self {
        assert!((0..64).contains(&shift), "shift must be in 0..64");
        Self {
            map: AHashMap::new(),
            shift,
        }
    }

    pub(crate) fn cell_key(&self, posn: Position) -> (i64, i64, i64) {
        (
            posn.x >> self.shift,
            posn.y >> self.shift,
            posn.z >> self.shift,
        )
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.map.values().flat_map(|cell| cell.iter())
    }

    /// Inserts an item. The caller must ensure it is not already present.
    pub fn insert(&mut self, item: T, posn: Position) {
        let key = self.cell_key(posn);
        self.map.entry(key).or_default().insert(item);
    }

    /// Removes an item from its known previous cell.
    pub fn remove(&mut self, item: &T, posn: Position) {
        let key = self.cell_key(posn);
        #[cfg(feature = "occupied-cell-removal")]
        {
            use std::collections::hash_map::Entry;
            let Entry::Occupied(mut entry) = self.map.entry(key) else {
                panic!("indexed cell exists");
            };
            let cell = entry.get_mut();
            assert!(cell.remove(item), "indexed item exists");
            if cell.is_empty() {
                entry.remove();
            }
        }
        #[cfg(not(feature = "occupied-cell-removal"))]
        {
            let cell = self.map.get_mut(&key).expect("indexed cell exists");
            assert!(cell.remove(item), "indexed item exists");
            if cell.is_empty() {
                self.map.remove(&key);
            }
        }
    }

    /// Yields all items in cells covering a cube of radius `min_radius`
    /// around `posn`, in arbitrary order. Includes every item within that
    /// Euclidean radius, and may include farther items in the covered cells.
    /// Negative radii yield no items.
    /// The query allocates no storage.
    pub fn nearest(&self, posn: Position, min_radius: i64) -> impl Iterator<Item = &T> {
        let valid = min_radius >= 0 && !self.map.is_empty();
        let bounds = |coordinate: i64| {
            let min = coordinate.saturating_sub(min_radius);
            let max = coordinate.saturating_add(min_radius);
            (min >> self.shift, max >> self.shift)
        };
        let (min_x, max_x) = bounds(posn.x);
        let (min_y, max_y) = bounds(posn.y);
        let (min_z, max_z) = bounds(posn.z);
        let nearby_cells = valid
            .then_some(min_x..=max_x)
            .into_iter()
            .flatten()
            .flat_map(move |x| {
                (min_y..=max_y).flat_map(move |y| (min_z..=max_z).map(move |z| (x, y, z)))
            })
            .filter_map(|key| self.map.get(&key));
        nearby_cells.flat_map(|cell| cell.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_items_preserves_neighbors_and_cleans_up_empty_cells() {
        let mut hash = SpatialHash::new(2);
        let origin = Position { x: 0, y: 0, z: 0 };
        let destination = Position { x: 4, y: 0, z: 0 };
        hash.insert(1, origin);
        hash.insert(2, origin);
        hash.insert(3, destination);
        hash.remove(&1, origin);
        assert_eq!(
            hash.nearest(origin, 0).copied().collect::<Vec<_>>(),
            vec![2]
        );
        hash.remove(&2, origin);
        assert_eq!(hash.map.len(), 1);
        assert_eq!(
            hash.nearest(destination, 0).copied().collect::<Vec<_>>(),
            vec![3]
        );
    }
}
