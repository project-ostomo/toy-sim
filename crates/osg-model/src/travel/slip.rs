//! Shared slip geometry, probability, and resource calculations.

pub const AU_M: f64 = 149_597_870_700.0;
pub const LY_M: f64 = 9.460_730_472_580_8e15;
pub const SOLAR_MASS_KG: f64 = 1.988_47e30;
pub const DISPERSION_FLOOR_RAD: f64 = 1.074_392_580_830_121_9e-7;
pub const DISPERSION_SPEED_COEFFICIENT: f64 = 0.000_954_954_987_734_015_4;
pub const BEACON_PRECISION: f64 = 36.0;
pub const MAX_SPEED_LY_S: f64 = 1.0;
pub const CHARGE_J_PER_KG: f64 = 500_000.0;
pub const MIN_CHARGE_SECONDS: f64 = 10.0;
pub const EXOTIC_RESOURCE: &str = "exotic_fuel";

pub fn exclusion_radius_m(mass_kg: f64) -> f64 {
    0.08 * AU_M * (mass_kg.max(0.0) / SOLAR_MASS_KG).cbrt()
}

pub fn dispersion_rad(speed_ly_s: f64, assisted: bool) -> f64 {
    (DISPERSION_SPEED_COEFFICIENT * speed_ly_s.powi(2)).max(DISPERSION_FLOOR_RAD)
        / if assisted { BEACON_PRECISION } else { 1.0 }
}

pub fn capture_probability(radius_m: f64, distance_m: f64, speed_ly_s: f64, assisted: bool) -> f64 {
    if radius_m <= 0.0 {
        return 0.0;
    }
    if distance_m <= 0.0 {
        return 1.0;
    }
    let ratio = radius_m / (distance_m * dispersion_rad(speed_ly_s, assisted));
    -(-0.5 * ratio * ratio).exp_m1()
}

/// Compute loss directly so very small ppm budgets remain representable.
pub fn capture_loss_ppm(radius_m: f64, distance_m: f64, speed_ly_s: f64, assisted: bool) -> f64 {
    if radius_m <= 0.0 {
        return 1_000_000.0;
    }
    if distance_m <= 0.0 {
        return 0.0;
    }
    let ratio = radius_m / (distance_m * dispersion_rad(speed_ly_s, assisted));
    (-0.5 * ratio * ratio).exp() * 1e6
}

pub fn fastest_speed_ly_s(
    radius_m: f64,
    distance_m: f64,
    max_loss_ppm: f64,
    assisted: bool,
) -> Option<f64> {
    if !radius_m.is_finite()
        || radius_m <= 0.0
        || !distance_m.is_finite()
        || distance_m < 0.0
        || !max_loss_ppm.is_finite()
        || !(0.0..=1_000_000.0).contains(&max_loss_ppm)
    {
        return None;
    }
    if max_loss_ppm == 1_000_000.0 || distance_m == 0.0 {
        return Some(MAX_SPEED_LY_S);
    }
    if max_loss_ppm == 0.0 {
        return None;
    }
    let sigma = radius_m / distance_m / (-2.0 * (max_loss_ppm / 1e6).ln()).sqrt();
    let unassisted_sigma = sigma * if assisted { BEACON_PRECISION } else { 1.0 };
    if unassisted_sigma < DISPERSION_FLOOR_RAD {
        return None;
    }
    Some(
        (unassisted_sigma / DISPERSION_SPEED_COEFFICIENT)
            .sqrt()
            .min(MAX_SPEED_LY_S),
    )
}

pub fn exotic_fuel_kg(departure_mass_kg: f64, distance_ly: f64) -> f64 {
    1e-5 * departure_mass_kg * distance_ly.max(0.0).powf(1.2)
}

pub fn exotic_range_ly(departure_mass_kg: f64, fuel_kg: f64) -> f64 {
    if departure_mass_kg <= 0.0 {
        return 0.0;
    }
    (fuel_kg.max(0.0) / (1e-5 * departure_mass_kg)).powf(1.0 / 1.2)
}

pub fn log_loss_from_ppm(loss_ppm: f64) -> f64 {
    -(-loss_ppm / 1e6).ln_1p()
}

pub fn ppm_from_log_loss(log_loss: f64) -> f64 {
    -(-log_loss).exp_m1() * 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_and_assisted_calibration_match_design() {
        let radius = exclusion_radius_m(SOLAR_MASS_KG);
        let distance = 10.0 * LY_M;
        assert!((capture_probability(radius, distance, 0.0, false) - 0.5).abs() < 1e-12);
        assert!(fastest_speed_ly_s(radius, distance, 100.0, false).is_none());
        let speed = fastest_speed_ly_s(radius, distance, 100.0, true).unwrap();
        assert!((10.0 / speed - 300.0).abs() < 1e-8);
        assert!((capture_probability(radius, distance, speed, true) - 0.9999).abs() < 1e-12);
    }

    #[test]
    fn fuel_and_itinerary_risk_compose() {
        let mass = 100_000.0;
        let distance = 300.0;
        let fuel = exotic_fuel_kg(mass, distance);
        assert!((exotic_range_ly(mass, fuel) - distance).abs() < 1e-10);
        let leg = log_loss_from_ppm(100.0) / 7.0;
        assert!((ppm_from_log_loss(leg * 7.0) - 100.0).abs() < 1e-10);
        assert!(exotic_fuel_kg(mass, 150.0) * 2.0 < fuel);
    }
}
