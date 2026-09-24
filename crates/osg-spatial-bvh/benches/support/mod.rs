use osg_spatial_bvh::{Aabb, BvhEntry, Position};

// Use the same approximate optical conversion currently used by the server.
pub const LUMENS_PER_OPTICAL_WATT: f64 = 220.0;

#[derive(Clone, Copy)]
pub struct Star {
    pub id: u64,
    pub position: Position,
    pub luminosity_w: f64,
}

pub fn bvh_entry(star: Star) -> BvhEntry<u64> {
    BvhEntry {
        object: star.id,
        bounds: Aabb {
            min: star.position,
            max: star.position,
        },
        luminosity: star.luminosity_w,
    }
}
