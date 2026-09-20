use crate::{Pose, travel::Target};
use serde::{Deserialize, Serialize};

pub const MAX_LOCAL_OBSTACLES: usize = 1024;
pub const MAX_RANGE_M: f64 = 1e12;
pub const QUERY_GAS: u64 = 262_144;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalObstacle {
    pub reference: Target,
    pub pose: Pose,
    pub radius_m: f64,
    pub slip_exclusion_m: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LocalSpace {
    pub obstacles: Vec<LocalObstacle>,
    pub truncated: bool,
}
