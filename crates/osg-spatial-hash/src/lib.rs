mod luminosity_map;
mod neighbor_index;
mod neighbor_map;
mod spatial_cell;
mod spatial_hash;

pub use luminosity_map::LuminosityMap;
pub use neighbor_map::NeighborMap;

/// An integer position type used as the main key for this whole crate
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}
