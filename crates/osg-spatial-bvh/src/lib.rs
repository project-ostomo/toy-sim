//! Spatial indexes with aggregate luminosity bounds.
//!
//! Coordinates are integer micrometres. Luminosity is total isotropic power in
//! watts. Bounds may enclose an instantaneous shape, padded geometry, or an
//! entire motion interval, as chosen by the caller.

pub mod luminosity_bvh;
pub mod luminosity_forest;
mod record;
pub mod service;
pub use record::{OPTICAL_LUMENS_PER_WATT, RecordKind, SpatialRecord, distance_squared};

pub use luminosity_bvh::{BvhEntry, LuminosityBvh};
pub use luminosity_forest::LuminosityForest;
pub use service::{
    DynamicEntry, QueryBatch, QueryBudget, QueryCursor, QueryStats, SegmentHit, SpatialObject,
    SpatialQuery, SpatialService,
};

// Keep the existing BVH tests' imports unchanged.
#[cfg(test)]
use luminosity_bvh::NodeKind;

#[cfg(test)]
mod tests;

/// Three Cartesian coordinates in integer micrometres.
pub type Position = [i128; 3];

/// A closed axis-aligned bounding box.
///
/// Each component of `min` must be less than or equal to the corresponding
/// component of `max`. Zero-size boxes are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aabb {
    pub min: Position,
    pub max: Position,
}

impl Aabb {
    /// Outward-rounded bounds in integer micrometres. Saturation at the edge
    /// of the coordinate domain is conservative.
    pub fn sphere(centre: Position, radius_m: f64) -> Self {
        assert!(radius_m.is_finite() && radius_m >= 0.0);
        let extent = (radius_m * 1e6).ceil();
        if extent >= i128::MAX as f64 {
            return Self {
                min: [i128::MIN; 3],
                max: [i128::MAX; 3],
            };
        }
        let extent = extent as i128;
        Self {
            min: centre.map(|v| v.saturating_sub(extent)),
            max: centre.map(|v| v.saturating_add(extent)),
        }
    }

    /// Enclose a translating sphere, including conversion roundoff.
    pub fn swept_sphere(start: Position, displacement_m: [f64; 3], radius_m: f64) -> Self {
        assert!(displacement_m.iter().all(|v| v.is_finite()));
        let end = std::array::from_fn(|axis| {
            start[axis].saturating_add((displacement_m[axis] * 1e6).round() as i128)
        });
        let error =
            displacement_m.iter().map(|v| v.abs()).fold(0.0, f64::max) * 64.0 * f64::EPSILON + 1e-6;
        Self::sphere(start, radius_m + error).union(Self::sphere(end, radius_m + error))
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|axis| self.min[axis].min(other.min[axis])),
            max: std::array::from_fn(|axis| self.max[axis].max(other.max[axis])),
        }
    }

    fn surface_area(self) -> f64 {
        // Subtract integers before conversion to preserve small local extents.
        // Even the full i128 coordinate span has a finite area in f64.
        let [x, y, z] = std::array::from_fn(|axis| {
            self.min[axis].abs_diff(self.max[axis]) as f64 / 1_000_000.0
        });

        2.0 * (x * y + y * z + z * x)
    }

    fn distance_squared(self, from: Position) -> f64 {
        (0..3)
            .map(|axis| {
                let closest = from[axis].clamp(self.min[axis], self.max[axis]);
                let distance = from[axis].abs_diff(closest) as f64 / 1_000_000.0;
                distance * distance
            })
            .sum()
    }
}
