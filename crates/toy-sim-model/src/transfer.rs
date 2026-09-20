#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransferCost {
    pub seconds_per_kg: f64,
}

impl Default for TransferCost {
    fn default() -> Self {
        Self {
            seconds_per_kg: 10.,
        }
    }
}

impl TransferCost {
    pub fn cruise_speed(
        self,
        distance: f64,
        closing_speed: f64,
        acceleration: f64,
        flow_kg_s: f64,
    ) -> f64 {
        if acceleration <= 0. || distance <= 0. {
            return 0.;
        }
        let closing = closing_speed.max(0.);
        let time_coefficient = 1. / acceleration;
        let fuel_coefficient = 2. * self.seconds_per_kg * flow_kg_s.max(0.) / acceleration;
        ((distance + closing * closing / (2. * acceleration))
            / (time_coefficient + fuel_coefficient))
            .sqrt()
            .max(closing)
    }

    pub fn estimate(self, distance: f64, acceleration: f64, flow_kg_s: f64) -> (f64, f64) {
        if distance <= 0. {
            return (0., 0.);
        }
        if acceleration <= 0. {
            return (f64::INFINITY, f64::INFINITY);
        }
        let speed = self
            .cruise_speed(distance, 0., acceleration, flow_kg_s)
            .min((distance * acceleration).sqrt());
        let burn = 2. * speed / acceleration;
        let coast = (distance - speed * speed / acceleration).max(0.) / speed;
        (burn + coast, burn * flow_kg_s)
    }

    pub fn remaining(
        self,
        distance: f64,
        closing_speed: f64,
        acceleration: f64,
        flow_kg_s: f64,
    ) -> (f64, f64) {
        if acceleration <= 0. {
            return (f64::INFINITY, f64::INFINITY);
        }
        let stopping_distance = closing_speed * closing_speed / (2. * acceleration);
        if closing_speed < 0. || stopping_distance > distance {
            let extra_distance = if closing_speed < 0. {
                distance + stopping_distance
            } else {
                stopping_distance - distance
            };
            let (time, fuel) = self.estimate(extra_distance, acceleration, flow_kg_s);
            let braking_s = closing_speed.abs() / acceleration;
            return (braking_s + time, braking_s * flow_kg_s + fuel);
        }
        let peak = self
            .cruise_speed(distance, closing_speed, acceleration, flow_kg_s)
            .min((acceleration * distance + closing_speed * closing_speed * 0.5).sqrt());
        if peak <= 0. {
            return (0., 0.);
        }
        let burn_distance =
            (2. * peak * peak - closing_speed * closing_speed) / (2. * acceleration);
        let burn_s = (2. * peak - closing_speed) / acceleration;
        (
            burn_s + (distance - burn_distance).max(0.) / peak,
            burn_s * flow_kg_s,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_time_accounts_for_velocity_and_overshoot() {
        let cost = TransferCost::default();
        let rest = cost.estimate(1000., 2., 3.).0;
        assert!((cost.remaining(1000., 0., 2., 3.).0 - rest).abs() < 1e-8);
        assert!(cost.remaining(1000., -10., 2., 3.).0 > rest);
        assert!(cost.remaining(1000., 10., 2., 3.).0 < rest);
        let overshoot = cost.remaining(100., 100., 2., 3.).0;
        assert!((overshoot - (50. + cost.estimate(2400., 2., 3.).0)).abs() < 1e-8);
        assert_eq!(cost.remaining(0., 0., 2., 3.).0, 0.);
        assert!(cost.remaining(100., 0., 0., 3.).0.is_infinite());
    }

    #[test]
    fn weighted_transfer_beats_neighbouring_cruise_speeds() {
        let distance = 1e7;
        let acceleration = 2.;
        let flow = 3.;
        let weights = TransferCost::default();
        let optimum = weights.cruise_speed(distance, 0., acceleration, flow);
        let cost = |v: f64| {
            distance / v + v / acceleration + weights.seconds_per_kg * 2. * v / acceleration * flow
        };
        for scale in [0.5, 0.9, 1.1, 2.] {
            assert!(cost(optimum) < cost(optimum * scale));
        }
        let fast = TransferCost { seconds_per_kg: 0. };
        let (fast_time, fast_fuel) = fast.estimate(distance, acceleration, flow);
        let (time, fuel) = weights.estimate(distance, acceleration, flow);
        assert!(time > fast_time && fuel < fast_fuel);
        assert!((fast_time - 2. * (distance / acceleration).sqrt()).abs() < 1e-8);
    }

    #[test]
    fn existing_momentum_is_not_discarded_to_match_a_lower_cruise_speed() {
        assert!(TransferCost::default().cruise_speed(1e5, 1000., 2., 3.) >= 1000.);
    }
}
