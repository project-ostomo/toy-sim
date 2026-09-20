use rand::RngExt;

pub trait StochasticRound {
    fn stochastic_round(self) -> u64;
}

impl StochasticRound for f64 {
    fn stochastic_round(self) -> u64 {
        assert!(
            self.is_finite() && self >= 0.0 && self < 18446744073709551616.0,
            "invalid discrete quantity: {self}"
        );
        let whole = self.floor() as u64;
        let fraction = self.fract();
        if fraction == 0.0 {
            whole
        } else {
            whole
                .checked_add(u64::from(rand::rng().random_bool(fraction)))
                .expect("quantity overflow")
        }
    }
}

pub trait StochasticBalance {
    fn withdraw(&mut self, requested: f64) -> u64;
    fn deposit(&mut self, supplied: f64, capacity: u64) -> u64;
}

impl StochasticBalance for u64 {
    fn withdraw(&mut self, requested: f64) -> u64 {
        let paid = requested.stochastic_round().min(*self);
        *self = self.checked_sub(paid).expect("quantity underflow");
        paid
    }

    fn deposit(&mut self, supplied: f64, capacity: u64) -> u64 {
        let room = capacity
            .checked_sub(*self)
            .expect("quantity exceeds capacity");
        let accepted = supplied.stochastic_round().min(room);
        *self = self.checked_add(accepted).expect("quantity overflow");
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_preserves_whole_units_and_fractional_expectation() {
        assert_eq!(0.0.stochastic_round(), 0);
        assert_eq!(123456.0.stochastic_round(), 123456);
        let mut sum = 0;
        for _ in 0..100_000 {
            let units = 2.4.stochastic_round();
            assert!((2..=3).contains(&units));
            sum += units;
        }
        assert!(sum.abs_diff(240_000) < 1500);
    }

    #[test]
    fn balances_stay_exact_above_floating_point_integer_precision() {
        let mut balance = u64::MAX;
        let paid = balance.withdraw(7.4);
        assert!((7..=8).contains(&paid));
        assert_eq!(balance + paid, u64::MAX);
        assert_eq!(balance.deposit(paid as f64 + 10.0, u64::MAX), paid);
        assert_eq!(balance, u64::MAX);
        assert_eq!(balance.deposit(1.0, u64::MAX), 0);
        let mut empty = 0;
        assert_eq!(empty.withdraw(100.0), 0);
        assert_eq!(empty, 0);
    }

    #[test]
    #[should_panic(expected = "invalid discrete quantity")]
    fn nonfinite_demand_is_a_bug() {
        f64::NAN.stochastic_round();
    }

    #[test]
    #[should_panic(expected = "invalid discrete quantity")]
    fn overflowing_demand_is_a_bug() {
        18446744073709551616.0.stochastic_round();
    }
}
