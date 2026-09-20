//! Immutable star records and luminosity-bucket KD-trees. No Bevy or database dependency.
mod catalogue;
mod format;
pub use catalogue::{StarCatalogue, VisibilityQuery, VisibleStars};
pub use format::{HEADER_BYTES, RECORD_BYTES};
pub use osg_space::GalacticPosition;

/// Persistent identity, independent of storage order and display name.
/// Namespace 1 is Gaia DR3. Other catalogues/generators must use distinct namespaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct StarId {
    pub namespace: u64,
    pub value: u64,
}
impl StarId {
    pub const GAIA_DR3: u64 = 1;
    pub const fn gaia(value: u64) -> Self {
        Self {
            namespace: Self::GAIA_DR3,
            value,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Star {
    pub id: StarId,
    pub position: GalacticPosition,
    /// Approximate intrinsic luminous output in lumens.
    pub luminosity: f64,
    /// Linear RGB multipliers normalized to unit luminance.
    pub colour: [f32; 3],
}
impl Star {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.luminosity.is_finite() && self.luminosity > 0.,
            "invalid luminosity for {:?}",
            self.id
        );
        anyhow::ensure!(
            self.colour.iter().all(|c| c.is_finite() && *c >= 0.)
                && self.colour.iter().any(|c| *c > 0.),
            "invalid colour for {:?}",
            self.id
        );
        // Leave subtraction headroom. This still spans vastly more than the observable universe.
        anyhow::ensure!(
            [self.position.x, self.position.y, self.position.z]
                .iter()
                .all(|x| (-(1_i128 << 126)..(1_i128 << 126)).contains(x)),
            "star coordinate out of supported range"
        );
        Ok(())
    }
}
pub const PARSEC: f64 = 3.085_677_581_491_367e16;
pub const SOLAR_LUMENS: f64 = 3.6e28;
pub const SOLAR_ABSOLUTE_MAGNITUDE: f64 = 4.83;
/// Threshold for luminosity / distance_in_metres²; the common 4π factor is omitted.
pub fn min_brightness(magnitude: f64) -> f64 {
    SOLAR_LUMENS / (10.0 * PARSEC).powi(2)
        * 10f64.powf((SOLAR_ABSOLUTE_MAGNITUDE - magnitude) / 2.5)
}
