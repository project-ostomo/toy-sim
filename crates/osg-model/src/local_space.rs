use crate::{GalacticPosition, Pose, travel::Target};
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
    /// Only the innermost containing Hill sphere is ineligible for slip capture.
    pub hill_radius_m: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LocalSpace {
    pub obstacles: Vec<LocalObstacle>,
    pub truncated: bool,
}

/// Select the smallest containing Hill sphere, excluding containing ancestors.
pub fn innermost_hill<T>(
    position: GalacticPosition,
    bodies: impl IntoIterator<Item = (T, GalacticPosition, f64)>,
) -> Option<T> {
    bodies
        .into_iter()
        .filter(|(_, center, radius)| {
            *radius > 0. && position.relative_to(*center).length() <= *radius
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(body, _, _)| body)
}

impl LocalSpace {
    pub fn departure_body(
        &self,
        position: GalacticPosition,
        after_seconds: f64,
    ) -> Option<&Target> {
        innermost_hill(
            position,
            self.obstacles.iter().map(|body| {
                (
                    &body.reference,
                    body.pose
                        .position
                        .offset_by(glam::DVec3::from_array(body.pose.velocity) * after_seconds),
                    body.hill_radius_m,
                )
            }),
        )
    }
}
