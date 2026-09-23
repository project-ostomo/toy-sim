//! Records shared by all spatial resolutions, and indexes containing only slot keys.
use ahash::AHashMap;
use slab::Slab;

use crate::{Position, spatial_hash::SpatialHash};

pub(crate) struct Record<T, M> {
    pub(crate) item: T,
    pub(crate) position: Position,
    pub(crate) metadata: M,
}

pub(crate) struct Records<T, M> {
    pub(crate) slots: Slab<Record<T, M>>,
    // Only mutation looks up items; queries get slot keys from spatial cells.
    pub(crate) keys: AHashMap<T, usize>,
}

impl<T, M> Records<T, M> {
    pub(crate) fn new() -> Self {
        Self {
            slots: Slab::new(),
            keys: AHashMap::new(),
        }
    }
}

const SHIFT_STEP: u32 = if cfg!(feature = "spatial-scale-2") {
    1
} else {
    2
};

pub(crate) struct NeighborIndex {
    // Include shift 63 even when the step does not divide it.
    hashes: Vec<SpatialHash<usize>>,
    minimum_shift: u32,
}

fn distance_squared(a: Position, b: Position) -> f64 {
    let dx = a.x.abs_diff(b.x) as f64;
    let dy = a.y.abs_diff(b.y) as f64;
    let dz = a.z.abs_diff(b.z) as f64;
    dx * dx + dy * dy + dz * dz
}

impl NeighborIndex {
    pub(crate) fn new() -> Self {
        Self::with_minimum_cell_shift(0)
    }

    pub(crate) fn with_minimum_cell_shift(minimum_shift: u32) -> Self {
        assert!(minimum_shift <= 63);
        let levels = (63 - minimum_shift).div_ceil(SHIFT_STEP) + 1;
        Self {
            hashes: (0..levels)
                .map(|level| SpatialHash::new((minimum_shift + level * SHIFT_STEP).min(63) as i64))
                .collect(),
            minimum_shift,
        }
    }

    pub(crate) fn insert(&mut self, key: usize, position: Position, old: Option<Position>) {
        for hash in &mut self.hashes {
            // Cells are nested: a match here means every coarser cell also matches.
            if let Some(old) = old {
                if hash.cell_key(old) == hash.cell_key(position) {
                    break;
                }
                hash.remove(&key, old);
            }
            hash.insert(key, position);
        }
    }

    pub(crate) fn remove(&mut self, key: usize, position: Position) {
        for hash in &mut self.hashes {
            hash.remove(&key, position);
        }
    }

    pub(crate) fn nearest<'a, T, M>(
        &'a self,
        records: &'a Slab<Record<T, M>>,
        position: Position,
        radius: i64,
        finer: usize,
    ) -> impl Iterator<Item = (&'a Record<T, M>, f64)> {
        let radius_squared = (radius as f64) * (radius as f64);
        self.candidates(records, position, radius, finer)
            .filter(move |(_, distance)| *distance <= radius_squared)
    }

    /// Includes cell candidates outside the requested sphere so budgeted callers
    /// can charge work before exact distance filtering.
    pub(crate) fn candidates<'a, T, M>(
        &'a self,
        records: &'a Slab<Record<T, M>>,
        position: Position,
        radius: i64,
        finer: usize,
    ) -> impl Iterator<Item = (&'a Record<T, M>, f64)> {
        let shift = 64 - (radius.max(1) as u64 - 1).leading_zeros();
        let level = (shift
            .saturating_sub(self.minimum_shift)
            .div_ceil(SHIFT_STEP) as usize)
            .saturating_sub(finer);
        self.hashes[level]
            .nearest(position, radius)
            .map(move |&key| {
                let record = &records[key];
                let distance = distance_squared(position, record.position);
                (record, distance)
            })
    }

    pub(crate) fn all<'a, T, M>(
        &'a self,
        records: &'a Slab<Record<T, M>>,
        position: Position,
    ) -> impl Iterator<Item = (&'a Record<T, M>, f64)> {
        // Every record occurs once at each level; the coarsest has at most eight cells.
        self.hashes.last().unwrap().iter().map(move |&key| {
            let record = &records[key];
            (record, distance_squared(position, record.position))
        })
    }
}
