//! Geometry and cost decisions shared by the firmware's directive planner.
use glam::DVec3;
use osg_model::travel::slip;

/// The transverse aim offsets trade capture probability against the transfer
/// after arrival. Always retain a center candidate, including at zero risk.
pub fn aim_offsets(direction: DVec3, toward: DVec3, radius: f64) -> [DVec3; 4] {
    let direction = direction.normalize_or_zero();
    let tangent = toward - direction * toward.dot(direction);
    let side = tangent.try_normalize().unwrap_or_else(|| {
        if direction.length_squared() > 0. {
            direction.any_orthonormal_vector()
        } else {
            DVec3::X
        }
    });
    [
        DVec3::ZERO,
        side * radius * 0.4,
        side * radius * 0.7,
        side * radius * 0.9,
    ]
}

pub fn loss_ppm(radius: f64, offset: f64, distance: f64, assisted: bool) -> f64 {
    slip::displaced_capture_loss(
        radius,
        offset,
        (distance * slip::dispersion_rad(assisted)).powi(2),
    ) * 1e6
}

pub fn departure_delay(charge_s: f64, clearance_s: f64) -> f64 {
    charge_s.max(clearance_s)
}

/// Keep an executable plan unless the alternative improves completion time
/// by at least five percent or ten seconds, whichever is greater.
pub fn meaningfully_better(new_seconds: f64, old_seconds: f64) -> bool {
    new_seconds + (old_seconds * 0.05).max(10.) < old_seconds
}

pub fn changed_materially(old: f64, new: f64) -> bool {
    (new - old).abs() > old.abs().max(1.) * 0.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limb_candidates_pay_capture_risk_and_remain_transverse() {
        let offsets = aim_offsets(DVec3::Z, DVec3::new(5., 0., 8.), 100.);
        let risks: Vec<_> = offsets
            .iter()
            .map(|offset| {
                assert_eq!(offset.z, 0.);
                loss_ppm(100., offset.length(), 5e8, false)
            })
            .collect();
        assert!(risks.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(offsets[0], DVec3::ZERO);
    }

    #[test]
    fn charging_overlaps_clearance_and_hysteresis_keeps_close_plans() {
        assert_eq!(departure_delay(120., 90.), 120.);
        assert_eq!(departure_delay(120., 180.), 180.);
        assert!(!meaningfully_better(96., 100.));
        assert!(meaningfully_better(80., 100.));
        assert!(!changed_materially(1000., 1050.));
        assert!(changed_materially(1000., 1200.));
    }
}
