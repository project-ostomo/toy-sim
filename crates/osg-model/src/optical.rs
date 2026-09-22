use crate::{ContactRef, EntityId, Id, Pose, ShipVisual};
use serde::{Deserialize, Serialize};

pub const MAX_OPTICAL_OBSERVATIONS: usize = 8192;
pub const MIN_OPTICAL_FLUX_W_M2: f64 = 1e-12;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpticalObservation {
    pub view: u64,
    pub id: Id,
    pub spatial_instance: Id,
    pub known_entity: Option<EntityId>,
    pub iff: Option<crate::IffIdentity>,
    pub contact: Option<ContactRef>,
    pub pose: Pose,
    pub radius_m: f64,
    pub luminosity_w: f64,
    pub appearance: Option<[u8; 32]>,
    pub visual: ShipVisual,
}

pub fn flux_w_m2(luminosity_w: f64, distance_m: f64, radius_m: f64) -> f64 {
    luminosity_w / (4. * std::f64::consts::PI * distance_m.max(radius_m).max(1.).powi(2))
}

pub fn angular_diameter(radius_m: f64, distance_m: f64) -> f64 {
    2. * (radius_m / distance_m.max(radius_m).max(f64::MIN_POSITIVE)).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photometry_falls_as_inverse_square_and_geometry_is_bounded() {
        assert_eq!(flux_w_m2(100., 10., 1.) / flux_w_m2(100., 20., 1.), 4.);
        assert_eq!(angular_diameter(10., 0.), std::f64::consts::PI);
        assert!((angular_diameter(1., 1e6) - 2e-6).abs() < 1e-12);
    }
}
