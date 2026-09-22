use osg_spatial_bvh::{LuminosityBvh, Position, QueryBudget, SpatialQuery, SpatialService};

use crate::support::{Star, bvh_entry};

// Benchmark-only interface for running identical workloads with each public API.
pub trait Index: Sized {
    const NAME: &'static str;

    fn build(stars: &[Star]) -> Self;
    fn visible(&self, from: Position, threshold: f64) -> impl Iterator<Item = u64>;
}

impl Index for LuminosityBvh<u64> {
    const NAME: &'static str = "luminosity_bvh";

    fn build(stars: &[Star]) -> Self {
        Self::build(stars.iter().copied().map(bvh_entry)).0
    }

    fn visible(&self, from: Position, threshold: f64) -> impl Iterator<Item = u64> {
        self.visible_from(from, threshold).copied()
    }
}

impl Index for SpatialService<u64> {
    const NAME: &'static str = "spatial_service";

    fn build(stars: &[Star]) -> Self {
        Self::new(stars.iter().copied().map(bvh_entry))
    }

    fn visible(&self, from: Position, threshold: f64) -> impl Iterator<Item = u64> {
        let mut cursor = self.query(SpatialQuery::Visibility {
            observer: from,
            observer_radius_m: 0.0,
            min_flux_w_m2: threshold,
        });

        std::iter::from_fn(move || {
            if cursor.is_complete() {
                return None;
            }
            Some(
                cursor
                    .advance(QueryBudget {
                        max_work: 4096,
                        max_results: 256,
                    })
                    .objects,
            )
        })
        .flatten()
        .copied()
    }
}
