//! Shared slip geometry, probability, and resource calculations.

pub const AU_M: f64 = 149_597_870_700.0;
pub const LY_M: f64 = 9.460_730_472_580_8e15;
pub const SOLAR_MASS_KG: f64 = 1.988_47e30;
pub const DISPERSION_RAD: f64 = 1.074_392_580_830_121_9e-7;
pub const BEACON_PRECISION: f64 = 36.0;
pub const CRUISE_SPEED_LY_S: f64 = 0.3;
pub const MIN_TRANSIT_SECONDS: f64 = 3.0;
pub const CHARGE_J_PER_KG_LY: f64 = 5_000.0;
pub const MIN_CHARGE_SECONDS: f64 = 10.0;
pub const EXOTIC_RESOURCE: &str = "exotic_fuel";

pub fn charging_energy_j(mass_kg: f64, distance_ly: f64) -> f64 {
    CHARGE_J_PER_KG_LY * mass_kg * distance_ly.max(0.0)
}

pub fn exclusion_radius_m(mass_kg: f64) -> f64 {
    0.08 * AU_M * (mass_kg.max(0.0) / SOLAR_MASS_KG).cbrt()
}

pub fn dispersion_rad(assisted: bool) -> f64 {
    DISPERSION_RAD / if assisted { BEACON_PRECISION } else { 1.0 }
}

/// Independent transverse increments telescope to variance (sigma * distance)^2.
pub fn walk_variance(progress_m: f64, step_m: f64, assisted: bool) -> f64 {
    dispersion_rad(assisted).powi(2)
        * step_m.max(0.0)
        * (2.0 * progress_m.max(0.0) + step_m.max(0.0))
}

#[test]
fn diffusion_and_conditional_capture_calibration() {
    for assisted in [false, true] {
        let whole = walk_variance(0.0, 100.0 * LY_M, assisted);
        let split = walk_variance(0.0, 30.0 * LY_M, assisted)
            + walk_variance(30.0 * LY_M, 70.0 * LY_M, assisted);
        assert!((whole / split - 1.0).abs() < 1e-14);
    }
    // Noncentral chi-square survival probabilities (two degrees of freedom).
    for (radius, offset, variance, expected) in [
        (2.0, 0.5, 1.0, 0.16914063850946723),
        (2.0, 2.0, 1.0, 0.6035009606119934),
        (2.0, 3.0, 1.0, 0.8867207544023924),
        (5.0, 1.0, 1.0, 0.00007436210694179456),
        (1.0, 0.99, 0.0001, 0.15987427315134783),
        (1.0, 1.01, 0.0001, 0.8425456155934448),
    ] {
        let actual = displaced_capture_loss(radius, offset, variance);
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}

/// Probability that an isotropic Gaussian endpoint misses a circular capture area.
/// Integrate radial Gaussian tails; avoid subtracting nearly equal probabilities
/// for a well-centered, low-risk approach.
pub fn displaced_capture_loss(radius: f64, offset: f64, variance: f64) -> f64 {
    if radius <= 0.0 {
        return 1.0;
    }
    if variance <= 0.0 {
        return f64::from(offset > radius);
    }
    if offset == 0.0 {
        return (-radius.powi(2) / (2.0 * variance)).exp();
    }
    let samples = 256;
    let pi = std::f64::consts::PI;
    let mut sum = 0.0;
    if offset <= radius {
        for i in 0..samples {
            let angle = pi * (i as f64 + 0.5) / samples as f64;
            let root = (radius.powi(2) - (offset * angle.sin()).powi(2))
                .max(0.0)
                .sqrt();
            let along = offset * angle.cos();
            let distance = if along > 0.0 {
                (radius - offset) * (radius + offset) / (root + along)
            } else {
                root - along
            };
            sum += (-distance.powi(2) / (2.0 * variance)).exp();
        }
        sum / samples as f64
    } else {
        let limit = (radius / offset).asin();
        for i in 0..samples {
            let theta = 0.5 * pi * (i as f64 + 0.5) / samples as f64;
            let angle = limit * theta.sin();
            let root = (radius.powi(2) - (offset * angle.sin()).powi(2))
                .max(0.0)
                .sqrt();
            let far = offset * angle.cos() + root;
            let near = (offset - radius) * (offset + radius) / far;
            sum += ((-near.powi(2) / (2.0 * variance)).exp()
                - (-far.powi(2) / (2.0 * variance)).exp())
                * limit
                * theta.cos();
        }
        (1.0 - sum / (2.0 * samples as f64)).clamp(0.0, 1.0)
    }
}

pub fn cruise_speed_ly_s(assisted: bool) -> f64 {
    CRUISE_SPEED_LY_S / if assisted { 1.0 } else { 10.0 }
}

pub fn flight_seconds(distance_m: f64, assisted: bool) -> f64 {
    (distance_m.max(0.0) / LY_M / cruise_speed_ly_s(assisted)).max(MIN_TRANSIT_SECONDS)
}

pub fn capture_probability(radius_m: f64, distance_m: f64, assisted: bool) -> f64 {
    if radius_m <= 0.0 {
        return 0.0;
    }
    if distance_m <= 0.0 {
        return 1.0;
    }
    let ratio = radius_m / (distance_m * dispersion_rad(assisted));
    -(-0.5 * ratio * ratio).exp_m1()
}

/// Compute loss directly so very small ppm budgets remain representable.
pub fn capture_loss_ppm(radius_m: f64, distance_m: f64, assisted: bool) -> f64 {
    if radius_m <= 0.0 {
        return 1_000_000.0;
    }
    if distance_m <= 0.0 {
        return 0.0;
    }
    let ratio = radius_m / (distance_m * dispersion_rad(assisted));
    (-0.5 * ratio * ratio).exp() * 1e6
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

pub const MAX_DELTA_V_PER_LY_M_S: f64 = 10_000.0;

/// Peregrine: 5000 kg full exotic tank and 85811 kg departure mass,
/// including its default half-filled propulsion tank and shield coolant.
pub const DELTA_V_FUEL_KG_PER_KG_M_S: f64 = 5000.0 / (85811.0 * 100_000.0);

pub fn earned_delta_v_m_s(requested_m_s: f64, distance_ly: f64) -> f64 {
    requested_m_s
        .max(0.0)
        .min(distance_ly.max(0.0) * MAX_DELTA_V_PER_LY_M_S)
}

pub fn transit_fuel_kg(mass_kg: f64, distance_ly: f64, requested_delta_v_m_s: f64) -> f64 {
    exotic_fuel_kg(mass_kg, distance_ly)
        + DELTA_V_FUEL_KG_PER_KG_M_S
            * mass_kg
            * earned_delta_v_m_s(requested_delta_v_m_s, distance_ly)
}

/// Largest affordable distance, including velocity change earned along the way.
pub fn transit_range_ly(mass_kg: f64, fuel_kg: f64, requested_delta_v_m_s: f64) -> f64 {
    let mut upper = exotic_range_ly(mass_kg, fuel_kg);
    if requested_delta_v_m_s <= 0.0 {
        return upper;
    }
    let mut lower = 0.0;
    for _ in 0..64 {
        let middle = (lower + upper) * 0.5;
        if transit_fuel_kg(mass_kg, middle, requested_delta_v_m_s) <= fuel_kg {
            lower = middle;
        } else {
            upper = middle;
        }
    }
    lower
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
    fn arrival_velocity_cost_is_linear_and_earned_by_distance() {
        assert_eq!(earned_delta_v_m_s(100_000.0, 0.0), 0.0);
        assert_eq!(earned_delta_v_m_s(100_000.0, 0.25), 2500.0);
        assert_eq!(earned_delta_v_m_s(100_000.0, 20.0), 100_000.0);
        let cost = |dv| transit_fuel_kg(85811.0, 20.0, dv) - exotic_fuel_kg(85811.0, 20.0);
        assert!((cost(100_000.0) - 5000.0).abs() < 1e-9);
        assert!((cost(50_000.0) * 2.0 - cost(100_000.0)).abs() < 1e-9);
        for distance in [0.0001, 0.5, 10.0, 100.0] {
            let fuel = transit_fuel_kg(85811.0, distance, 100_000.0);
            let affordable = transit_range_ly(85811.0, fuel, 100_000.0);
            assert!((affordable - distance).abs() < distance * 1e-12);
        }
    }

    #[test]
    fn fixed_dispersion_preserves_capture_odds_with_tenfold_radii() {
        let radius = exclusion_radius_m(SOLAR_MASS_KG);
        assert!((radius / AU_M - 0.08).abs() < 1e-12);
        assert!((capture_probability(radius, 10.0 * LY_M, false) - 0.5).abs() < 1e-12);
        assert!(capture_loss_ppm(radius, 100.0 * LY_M, true) < 130.0);
        assert_eq!(flight_seconds(1000.0, false), 3.0);
        assert!((flight_seconds(LY_M, false) / flight_seconds(LY_M, true) - 10.0).abs() < 1e-12);
        assert!((flight_seconds(LY_M, true) - 1.0 / CRUISE_SPEED_LY_S).abs() < 1e-12);
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
