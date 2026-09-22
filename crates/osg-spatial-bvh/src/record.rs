use crate::{Aabb, DynamicEntry, Position};

pub const OPTICAL_LUMENS_PER_WATT: f64 = 220.0;

/// Proxy roles share a tree but have different exact predicates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordKind {
    Source,
    Body,
    Collision,
}

/// Shared catalogue and simulation record. IDs are scoped by kind: catalogue
/// slots for Source, game entity IDs for Body/Collision. A source's bounds may
/// cover its entire system so catalogue and stellar queries share one leaf.
#[derive(Clone, Copy, Debug)]
pub struct SpatialRecord {
    pub kind: RecordKind,
    pub id: u64,
    pub position: Position,
    pub radius_m: f64,
}

impl crate::service::SpatialObject for SpatialRecord {
    // An entity may have both an observation proxy and a collision proxy.
    type Id = (RecordKind, u64);

    fn spatial_id(&self) -> Self::Id {
        (self.kind, self.id)
    }
}

impl SpatialRecord {
    pub fn distance(&self, from: Position) -> f64 {
        distance_squared(self.position, from).sqrt()
    }

    pub fn dynamic(self, luminosity: f64, displacement_m: [f64; 3]) -> DynamicEntry<Self> {
        DynamicEntry {
            bounds: Aabb::sphere(self.position, self.radius_m),
            swept_bounds: Aabb::swept_sphere(self.position, displacement_m, self.radius_m),
            luminosity,
            object: self,
        }
    }
}

pub fn distance_squared(a: Position, b: Position) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a.abs_diff(b) as f64 / 1e6).powi(2))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_preserve_local_motion_at_galactic_coordinates_and_saturate() {
        let start = [1_i128 << 100; 3];
        let bounds = Aabb::swept_sphere(start, [0.000_003, -10., 0.], 0.000_001);
        assert!(bounds.min[0] <= start[0] - 1);
        assert!(bounds.max[0] >= start[0] + 4);
        assert!(bounds.min[1] <= start[1] - 10_000_001);
        assert_eq!(
            distance_squared(
                start,
                [start[0] + 3_000_000, start[1] + 4_000_000, start[2]]
            ),
            25.
        );
        let edge = Aabb::sphere([i128::MAX, i128::MIN, 0], 1.);
        assert_eq!(edge.max[0], i128::MAX);
        assert_eq!(edge.min[1], i128::MIN);
    }
}
