//! Anonymous, optically observed slipdrive effects.
use crate::{GalacticPosition, Id};
use serde::{Deserialize, Serialize};

pub const WAKE_LIFETIME_S: f64 = 300.0;
pub const MAX_WAKE_SPANS_PER_VIEW: usize = 128;
pub const TRANSITION_LIFETIME_S: f64 = 2.0;
pub const WAKE_VIEW_RANGE_M: f64 = 1e10;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlipPresentation {
    pub wakes: Vec<SlipWake>,
    pub transitions: Vec<SlipTransition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlipWake {
    pub view: u64,
    pub id: Id,
    pub start: GalacticPosition,
    pub end: GalacticPosition,
    pub start_ns: u64,
    pub end_ns: u64,
    pub drift_m_s: [f64; 3],
    pub radius_m: f64,
    pub seed: u32,
    /// Longitudinal noise offset, reduced to its periodic phase in public snapshots.
    pub offset_m: f64,
}

impl SlipWake {
    pub fn position(&self, fraction: f64, now_ns: u64) -> GalacticPosition {
        let fraction = fraction.clamp(0.0, 1.0);
        let elapsed_ns = (i128::from(now_ns) - i128::from(self.start_ns)) as f64;
        let age = ((elapsed_ns - (self.end_ns - self.start_ns) as f64 * fraction) * 1e-9).max(0.0);
        self.start.offset_by(
            self.end.relative_to(self.start) * fraction
                + glam::DVec3::from_array(self.drift_m_s) * age,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlipTransition {
    pub view: u64,
    pub id: Id,
    pub time_ns: u64,
    pub position: GalacticPosition,
    pub drift_m_s: [f64; 3],
    pub direction: [f64; 3],
    pub radius_m: f64,
    pub arriving: bool,
    pub seed: u32,
}

pub fn wake_envelope(age_s: f64) -> f64 {
    let age = age_s.max(0.0);
    let tail = ((WAKE_LIFETIME_S - age) / 60.0).clamp(0.0, 1.0);
    (-age / 100.0).exp() * tail * tail * (3.0 - 2.0 * tail)
}
