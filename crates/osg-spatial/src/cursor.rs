use super::{CellKey, GalacticPosition, QueryStats, SpatialHash, distance_squared, padded_radius};

#[derive(Clone, Debug)]
enum Pending {
    Cell(CellKey),
    Occupants { key: CellKey, offset: usize },
}

/// A traversal checkpoint for one unchanged index. Retain the same immutable
/// index snapshot while paging. A changed or different index invalidates it.
#[derive(Clone, Debug)]
pub struct RangeCursor {
    identity: u64,
    generation: u64,
    centre: GalacticPosition,
    radius_m: f64,
    extents: bool,
    pending: Vec<Pending>,
}

#[derive(Clone, Debug, Default)]
pub struct RangeBatch {
    pub ids: Vec<u32>,
    pub stats: QueryStats,
    pub complete: bool,
    pub invalidated: bool,
}

impl QueryStats {
    /// Each visited cell and each tested entry costs one unit. Results can cost
    /// additional caller-defined gas for tag matching or serialization.
    pub fn work(&self) -> usize {
        self.cells_visited + self.candidates
    }
}

impl SpatialHash {
    pub fn range_cursor(
        &self,
        centre: GalacticPosition,
        radius_m: f64,
        extents: bool,
    ) -> RangeCursor {
        let pending = if radius_m.is_nan() || radius_m < 0.0 {
            Vec::new()
        } else {
            self.positions
                .roots
                .iter()
                .copied()
                .map(Pending::Cell)
                .collect()
        };
        RangeCursor {
            identity: self.identity,
            generation: self.generation,
            centre,
            radius_m,
            extents,
            pending,
        }
    }

    /// Visits at most `max_work` cells and entries combined, returning at most
    /// `max_results` hits. This also bounds a leaf containing coincident objects.
    /// Each batch is sorted by ID. The caller can sort a completed aggregate if
    /// it needs a globally ordered result independent of insertion history.
    pub fn advance_range(
        &self,
        cursor: &mut RangeCursor,
        max_work: usize,
        max_results: usize,
    ) -> RangeBatch {
        let mut batch = RangeBatch::default();
        if cursor.identity != self.identity || cursor.generation != self.generation {
            batch.invalidated = true;
            batch.complete = true;
            cursor.pending.clear();
            return batch;
        }
        while batch.stats.work() < max_work && batch.ids.len() < max_results {
            let Some(next) = cursor.pending.pop() else {
                break;
            };
            match next {
                Pending::Cell(key) => {
                    let cell = &self.positions.nodes[&key];
                    batch.stats.cells_visited += 1;
                    let radius = cursor.radius_m
                        + if cursor.extents {
                            cell.max_radius_m
                        } else {
                            0.0
                        };
                    let radius = padded_radius(radius);
                    if key.distance_squared(cursor.centre) > radius * radius {
                        continue;
                    }
                    if cell.children.is_empty() {
                        cursor.pending.push(Pending::Occupants { key, offset: 0 });
                    } else {
                        cursor
                            .pending
                            .extend(cell.children.iter().copied().map(Pending::Cell));
                    }
                }
                Pending::Occupants { key, offset } => {
                    let cell = &self.positions.nodes[&key];
                    let id = cell.occupants[offset];
                    batch.stats.candidates += 1;
                    if offset + 1 < cell.occupants.len() {
                        cursor.pending.push(Pending::Occupants {
                            key,
                            offset: offset + 1,
                        });
                    }
                    let entry = self.entries[&id];
                    let radius =
                        cursor.radius_m + if cursor.extents { entry.radius_m } else { 0.0 };
                    if distance_squared(entry.position, cursor.centre) <= radius * radius {
                        batch.ids.push(id);
                    }
                }
            }
        }
        batch.complete = cursor.pending.is_empty();
        batch.ids.sort_unstable();
        batch
    }
}
