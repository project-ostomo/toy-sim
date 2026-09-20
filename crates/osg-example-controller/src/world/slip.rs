use osg_model::{GalacticPosition, travel::MAX_PREDICTION_SECONDS};
use osg_ship_api::abi;

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
) -> Result<Solution, i32> {
    let mut preparation_s: f64 = 0.;
    let mut duration_s: f64 = 0.;
    for _ in 0..4 {
        let departure = preparation_s;
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
        let target = GalacticPosition::from_meters(DVec3::X * 1e16);
        let velocity = DVec3::Y * 30_000.;
        for charge_remaining in [120., 10., 0.] {
            let mut evaluations = 0;
            let solution = intercept(
                |after| Ok(GalacticPosition::from_meters(DVec3::Z * after * 100.)),
                |after| Ok(target.offset_by(velocity * after)),
                |origin, destination, departure, arrival| {
                    evaluations += 1;
                    assert_eq!(
                        origin,
                        GalacticPosition::from_meters(DVec3::Z * departure * 100.)
                    );
                    assert_eq!(destination, target.offset_by(velocity * arrival));
                    Ok((
                        true,
                        charge_remaining,
                        destination.relative_to(origin).length() / 1.25e14,
                    ))
                },
            )
            .unwrap();
            assert_eq!(evaluations, 2);
            assert!((solution.seconds - charge_remaining - 80.).abs() < 0.001);
            assert!(
                solution
                    .destination
                    .relative_to(target.offset_by(velocity * solution.seconds))
                    .length()
                    < 1.
            );
        }
    }

    #[test]
    fn inadmissible_or_nonconverging_intercepts_are_bounded_failures() {
        let result = intercept(
            |_| Ok(GalacticPosition::ZERO),
            |_| Ok(GalacticPosition::ZERO),
            |_, _, _, _| Ok((false, 20., 80.)),
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
        );
        assert_eq!(result.err(), Some(abi::ERR_UNAVAILABLE));
        assert_eq!(evaluations, 4);
    }
}
