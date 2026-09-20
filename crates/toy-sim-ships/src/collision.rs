use crate::{CompiledShipDesign, GRID};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollisionBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum CollisionVolume {
    #[default]
    Box,
    Cylinder,
    Compound {
        boxes: Vec<CollisionBox>,
    },
    HubAndRings {
        hub_radius_m: f64,
        inner_radius_m: f64,
        outer_radius_m: f64,
        ring_offset_m: f64,
        ring_half_depth_m: f64,
    },
    Hangar {
        inner_radius_m: f64,
        rear_depth_m: f64,
    },
}

impl CollisionVolume {
    pub fn valid(&self) -> bool {
        let positive = |n: f64| n.is_finite() && n > 0.0;
        match *self {
            Self::Box | Self::Cylinder => true,
            Self::Compound { ref boxes } => {
                !boxes.is_empty()
                    && boxes.len() <= 4096
                    && boxes.iter().all(|b| {
                        b.min.iter().chain(&b.max).all(|v| v.is_finite())
                            && (0..3).all(|i| b.min[i] < b.max[i])
                    })
            }
            Self::HubAndRings {
                hub_radius_m,
                inner_radius_m,
                outer_radius_m,
                ring_offset_m,
                ring_half_depth_m,
            } => {
                [
                    hub_radius_m,
                    inner_radius_m,
                    outer_radius_m,
                    ring_offset_m,
                    ring_half_depth_m,
                ]
                .into_iter()
                .all(positive)
                    && hub_radius_m < inner_radius_m
                    && inner_radius_m < outer_radius_m
            }
            Self::Hangar {
                inner_radius_m,
                rear_depth_m,
            } => positive(inner_radius_m) && positive(rear_depth_m),
        }
    }

    fn contains(&self, point: DVec3, half: DVec3, margin: f64) -> bool {
        if (point.abs() - half).max_element() > margin {
            return false;
        }
        let radial = point.truncate().length();
        match *self {
            Self::Box => true,
            Self::Compound { ref boxes } => boxes.iter().any(|b| {
                (DVec3::from_array(b.min) - point).max_element() <= margin
                    && (point - DVec3::from_array(b.max)).max_element() <= margin
            }),
            Self::Cylinder => radial <= half.x.min(half.y) + margin,
            Self::HubAndRings {
                hub_radius_m,
                inner_radius_m,
                outer_radius_m,
                ring_offset_m,
                ring_half_depth_m,
            } => {
                radial <= hub_radius_m + margin
                    || (radial <= outer_radius_m + margin
                        && radial >= inner_radius_m - margin
                        && (point.z.abs() - ring_offset_m).abs() <= ring_half_depth_m + margin)
                    || (radial <= inner_radius_m
                        && (point.z.abs() - ring_offset_m).abs() <= 2.0 + margin)
            }
            Self::Hangar {
                inner_radius_m,
                rear_depth_m,
            } => {
                radial <= half.x.min(half.y) + margin
                    && (radial >= inner_radius_m - margin
                        || point.z >= half.z - rear_depth_m - margin)
            }
        }
    }
}

pub fn voxel_boxes(design: &CompiledShipDesign) -> Vec<(DVec3, DVec3)> {
    let mut cells = BTreeSet::new();
    for part in &design.parts {
        let (lo, hi) = part.bounds();
        let low = lo.floor().as_ivec3();
        let high = hi.ceil().as_ivec3();
        let half = DVec3::from_array(part.definition.dimensions.map(|n| n as f64 * GRID)) * 0.5;
        for x in low.x..high.x {
            for y in low.y..high.y {
                for z in low.z..high.z {
                    let point = DVec3::new(x as f64, y as f64, z as f64) + DVec3::splat(0.5);
                    let local = part.rotation.transpose() * (point - part.centre);
                    if part
                        .definition
                        .collision
                        .contains(local, half, 3.0_f64.sqrt() * 0.5)
                    {
                        cells.insert([x, y, z]);
                    }
                }
            }
        }
    }
    let mut boxes = Vec::new();
    while let Some(&[x, y, z]) = cells.first() {
        let mut end_z = z + 1;
        while cells.contains(&[x, y, end_z]) {
            end_z += 1;
        }
        let mut end_y = y + 1;
        while (z..end_z).all(|z| cells.contains(&[x, end_y, z])) {
            end_y += 1;
        }
        let mut end_x = x + 1;
        while (y..end_y).all(|y| (z..end_z).all(|z| cells.contains(&[end_x, y, z]))) {
            end_x += 1;
        }
        for i in x..end_x {
            for j in y..end_y {
                for k in z..end_z {
                    cells.remove(&[i, j, k]);
                }
            }
        }
        let low = DVec3::new(x as f64, y as f64, z as f64);
        let high = DVec3::new(end_x as f64, end_y as f64, end_z as f64);
        boxes.push(((low + high) * 0.5 - design.centre, (high - low) * 0.5));
    }
    boxes
}

pub(crate) fn parts_overlap(a: &crate::PreparedPart, b: &crate::PreparedPart) -> bool {
    let (a0, a1) = a.bounds();
    let (b0, b1) = b.bounds();
    let low = a0.max(b0);
    let high = a1.min(b1);
    let size = high - low;
    if size.min_element() < 1e-6 {
        return false;
    }
    if matches!(a.definition.collision, CollisionVolume::Box)
        && matches!(b.definition.collision, CollisionVolume::Box)
    {
        return true;
    }
    let count = size.ceil().max(DVec3::ONE).as_uvec3();
    if count.as_dvec3().element_product() > 1e6 {
        return true;
    }
    let contains = |part: &crate::PreparedPart, point: DVec3| {
        let local = part.rotation.transpose() * (point - part.centre);
        let half = DVec3::from_array(part.definition.dimensions.map(|n| n as f64 * GRID)) * 0.5;
        part.definition.collision.contains(local, half, 0.)
    };
    for x in 0..count.x {
        for y in 0..count.y {
            for z in 0..count.z {
                let point = low
                    + size * (DVec3::new(x as f64, y as f64, z as f64) + DVec3::splat(0.5))
                        / count.as_dvec3();
                if contains(a, point) && contains(b, point) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_volume_keeps_empty_mounting_space_and_expands_for_voxels() {
        let volume = CollisionVolume::Compound {
            boxes: vec![
                CollisionBox {
                    min: [-4.0, -2.0, -3.0],
                    max: [-2.0, 2.0, 3.0],
                },
                CollisionBox {
                    min: [2.0, -2.0, -3.0],
                    max: [4.0, 2.0, 3.0],
                },
            ],
        };
        let half = DVec3::new(4.0, 2.0, 3.0);
        assert!(volume.valid());
        assert!(!volume.contains(DVec3::ZERO, half, 0.0));
        assert!(volume.contains(DVec3::new(3.0, 0.0, 0.0), half, 0.0));
        assert!(!volume.contains(DVec3::new(1.5, 0.0, 0.0), half, 0.0));
        assert!(volume.contains(DVec3::new(1.5, 0.0, 0.0), half, 0.6));
        assert!(!volume.contains(DVec3::new(5.0, 0.0, 0.0), half, 0.6));
    }

    #[test]
    fn compound_volume_rejects_empty_inverted_and_nonfinite_boxes() {
        assert!(!CollisionVolume::Compound { boxes: vec![] }.valid());
        for max in [[-1.0; 3], [0.0; 3], [f64::NAN; 3], [f64::INFINITY; 3]] {
            assert!(
                !CollisionVolume::Compound {
                    boxes: vec![CollisionBox { min: [0.0; 3], max }],
                }
                .valid()
            );
        }
    }
}
