use toy_sim_model::{GalacticPosition, travel::MAX_PREDICTION_SECONDS};
use toy_sim_ship_api::abi;

pub struct Solution {
    pub destination: GalacticPosition,
    pub seconds: f64,
}

pub fn intercept(
    mut origin_at: impl FnMut(f64) -> Result<GalacticPosition, i32>,
    mut destination_at: impl FnMut(f64) -> Result<GalacticPosition, i32>,
    mut eligibility: impl FnMut(
        GalacticPosition,
        GalacticPosition,
        f64,
        f64,
    ) -> Result<(bool, f64, f64), i32>,
    start_after_seconds: f64,
) -> Result<Solution, i32> {
    let mut preparation_s = 0.;
    let mut duration_s = 0.;
    for _ in 0..4 {
        let departure = start_after_seconds + preparation_s;
        let arrival = departure + duration_s;
        if !arrival.is_finite() || !(0.0..=MAX_PREDICTION_SECONDS).contains(&arrival) {
            return Err(abi::ERR_UNAVAILABLE);
        }
        let origin = origin_at(departure)?;
        let destination = destination_at(arrival)?;
        let (ready, preparation, duration) = eligibility(origin, destination, departure, arrival)?;
        if !preparation.is_finite() || preparation < 0. || !duration.is_finite() || duration < 0. {
            return Err(abi::ERR_UNAVAILABLE);
        }
        let converged =
            (preparation - preparation_s).abs() < 0.001 && (duration - duration_s).abs() < 0.001;
        preparation_s = preparation;
        duration_s = duration;
        if converged {
            return ready
                .then_some(Solution {
                    destination,
                    seconds: preparation_s + duration_s,
                })
                .ok_or(abi::ERR_UNAVAILABLE);
        }
    }
    Err(abi::ERR_UNAVAILABLE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn moving_destination_is_led_through_remaining_charge_and_transit() {
        let gate = GalacticPosition::from_meters(DVec3::X * 1e16);
        let velocity = DVec3::Y * 30_000.;
        for charge_remaining in [120., 10., 0.] {
            let mut evaluations = 0;
            let solution = intercept(
                |after| Ok(GalacticPosition::from_meters(DVec3::Z * after * 100.)),
                |after| Ok(gate.offset_by(velocity * after)),
                |origin, destination, departure, arrival| {
                    evaluations += 1;
                    assert_eq!(
                        origin,
                        GalacticPosition::from_meters(DVec3::Z * departure * 100.)
                    );
                    assert_eq!(destination, gate.offset_by(velocity * arrival));
                    Ok((true, charge_remaining, 80.))
                },
                0.,
            )
            .unwrap();
            assert_eq!(evaluations, 2);
            assert_eq!(solution.seconds, charge_remaining + 80.);
            assert_eq!(
                solution.destination,
                gate.offset_by(velocity * solution.seconds)
            );
        }
    }

    #[test]
    fn future_leg_checks_departure_and_arrival_at_their_own_times() {
        let solution = intercept(
            |after| Ok(GalacticPosition::from_meters(DVec3::X * after)),
            |after| Ok(GalacticPosition::from_meters(DVec3::Y * after)),
            |_, _, departure, arrival| Ok((departure == 1720. && arrival == 1800., 20., 80.)),
            1700.,
        )
        .unwrap();
        assert_eq!(solution.seconds, 100.);
        assert_eq!(
            solution.destination,
            GalacticPosition::from_meters(DVec3::Y * 1800.)
        );
    }

    #[test]
    fn inadmissible_or_nonconverging_intercepts_are_bounded_failures() {
        let result = intercept(
            |_| Ok(GalacticPosition::ZERO),
            |_| Ok(GalacticPosition::ZERO),
            |_, _, _, _| Ok((false, 20., 80.)),
            0.,
        );
        assert_eq!(result.err(), Some(abi::ERR_UNAVAILABLE));
        let mut evaluations = 0;
        let result = intercept(
            |_| Ok(GalacticPosition::ZERO),
            |_| Ok(GalacticPosition::ZERO),
            |_, _, _, arrival| {
                evaluations += 1;
                Ok((true, 0., arrival + 10.))
            },
            0.,
        );
        assert_eq!(result.err(), Some(abi::ERR_UNAVAILABLE));
        assert_eq!(evaluations, 4);
    }
}
