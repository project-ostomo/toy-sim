//! Spatial relationships sampled once per simulation tick.

use crate::{Id, travel::CelestialRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocationRegion {
    System,
    #[default]
    Interstellar,
    SlipTransit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationContext {
    pub region: LocationRegion,
    pub system: Option<Id>,
    pub primary: Option<CelestialRef>,
    /// Ancestors followed by the primary, from the system root inward.
    pub hierarchy: Vec<CelestialRef>,
    pub sample_tick: u64,
}
