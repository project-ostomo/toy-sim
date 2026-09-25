//! Bounded, warm-started search over burn/coast/brake intercepts.
use glam::DVec3;

#[derive(Clone, Copy, Debug)]
struct Maneuver {
    remaining: f64,
    burn: f64,
    brake: f64,
    first: DVec3,
    last: DVec3,
    fuel: f64,
}

#[derive(Clone, Copy)]
pub struct State {
    pub error: DVec3,
    pub velocity: DVec3,
    pub gravity: DVec3,
    pub acceleration: f64,
    pub mass: f64,
    pub flow: f64,
    pub fuel: f64,
    pub turn: f64,
}

#[derive(Default, Clone, Debug)]
pub struct Search {
    best: Option<Maneuver>,
    horizon: f64,
    cursor: usize,
}

impl State {
    fn candidate(self, time: f64, burn: f64, brake: f64) -> Option<Maneuver> {
        if !time.is_finite()
            || time > osg_model::travel::MAX_PREDICTION_SECONDS
            || burn <= 0.
            || brake <= 0.
            || time < burn + brake + self.turn
        {
            return None;
        }
        let velocity = -self.velocity - self.gravity * time;
        let displacement = self.error - self.velocity * time - self.gravity * (0.5 * time * time);
        let first =
            (displacement - velocity * (brake * 0.5)) / (burn * (time - (burn + brake) * 0.5));
        let last = (velocity - first * burn) / brake;
        if !first.is_finite() || !last.is_finite() {
            return None;
        }

        // Piecewise integration in the target frame. The measured differential
        // gravity predicts both bodies; mass falls throughout each burn.
        let exhaust = self.acceleration * self.mass / self.flow.max(1e-12);
        let mut mass = self.mass;
        let mut position = DVec3::ZERO;
        let mut speed = self.velocity;
        for (duration, acceleration) in [
            (burn, first),
            (time - burn - brake, DVec3::ZERO),
            (brake, last),
        ] {
            let dt = duration / 8.;
            for _ in 0..8 {
                if acceleration.length() * mass > self.acceleration * self.mass * 1.001 {
                    return None;
                }
                position += speed * dt + (acceleration + self.gravity) * (0.5 * dt * dt);
                speed += (acceleration + self.gravity) * dt;
                mass *= (-acceleration.length() * dt / exhaust).exp();
            }
        }
        let fuel = self.mass - mass;
        if fuel > self.fuel * 0.98
            || !fuel.is_finite()
            || position.distance(self.error) > 0.01
            || speed.length() > 0.001
        {
            return None;
        }
        Some(Maneuver {
            remaining: time,
            burn,
            brake,
            first,
            last,
            fuel,
        })
    }
}

impl Search {
    pub fn estimate(&self) -> Option<(f64, f64)> {
        self.best.map(|plan| (plan.remaining, plan.fuel))
    }

    pub fn update(&mut self, state: State, dt: f64) -> Option<DVec3> {
        let old = self.best.take();
        if let Some(mut plan) = old {
            plan.remaining = (plan.remaining - dt).max(0.);
            plan.burn = (plan.burn - dt).max(0.);
            plan.brake = plan.brake.min(plan.remaining);
            self.best = Some(plan);
        }

        let mut coasting = None;
        if let Some(plan) = self.best {
            if plan.burn <= dt {
                let exhaust = state.acceleration * state.mass / state.flow.max(1e-12);
                let braking_fuel =
                    state.mass * (1. - (-plan.last.length() * plan.brake / exhaust).exp());
                let predicted = state.velocity * plan.remaining
                    + state.gravity * (0.5 * plan.remaining.powi(2))
                    + plan.last * (0.5 * plan.brake.powi(2));
                // Preserve the phase while it still reaches the moving target.
                // Changed motion, gravity or fuel reopens the same small search.
                if predicted.distance(state.error) < (state.error.length() * 0.05).max(20.)
                    && braking_fuel <= state.fuel
                {
                    if plan.remaining > plan.brake + state.turn {
                        coasting = Some(Maneuver {
                            fuel: braking_fuel,
                            ..plan
                        });
                    } else {
                        return None;
                    }
                } else {
                    self.best = None;
                    self.horizon = 0.;
                }
            }
        }

        let seed = 2. * (state.error.length() / state.acceleration).sqrt()
            + 2. * state.velocity.length() / state.acceleration
            + 2. * state.turn;
        self.horizon = self.horizon.max(seed).max(1.);
        // Exactly four candidates and 24 integration steps each per update.
        // Refit the incumbent to the current state before comparing alternatives.
        let incumbent = coasting.or_else(|| {
            self.best
                .and_then(|plan| state.candidate(plan.remaining, plan.burn, plan.brake))
        });
        let time = incumbent.map_or(self.horizon, |plan| plan.remaining);
        let mut best = incumbent;
        for factor in [0.92, 1., 1.12, 1.4] {
            let time = time * factor;
            let fraction = [0.49, 0.35, 0.2, 0.1][self.cursor % 4];
            let burn = (time - state.turn).max(0.) * fraction;
            if let Some(candidate) = state.candidate(time, burn, burn) {
                if best.is_none_or(|old| {
                    candidate.remaining + (old.remaining * 0.05).max(1.) < old.remaining
                        || ((candidate.remaining - old.remaining).abs() < 0.1
                            && candidate.fuel < old.fuel)
                }) {
                    best = Some(candidate);
                }
            }
        }
        self.cursor += 1;
        if let Some(plan) = best {
            self.best = Some(plan);
            self.horizon = plan.remaining;
            Some(if plan.burn <= dt {
                DVec3::ZERO
            } else {
                plan.first
            })
        } else {
            self.best = None;
            self.horizon = (self.horizon * 1.4).min(osg_model::travel::MAX_PREDICTION_SECONDS);
            None
        }
    }

    pub fn braking_direction(&self) -> Option<DVec3> {
        self.best.and_then(|plan| plan.last.try_normalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(fuel: f64) -> State {
        State {
            error: DVec3::new(10_000., 2_000., 0.),
            velocity: DVec3::new(-10., 5., 0.),
            gravity: DVec3::new(0., 0.02, 0.),
            acceleration: 10.,
            mass: 1_000.,
            flow: 1.,
            fuel,
            turn: 2.,
        }
    }

    #[test]
    fn intercept_reserves_braking_fuel_and_trades_time_for_coasting() {
        let mut fast = Search::default();
        let mut economical = Search::default();
        for _ in 0..40 {
            fast.update(state(500.), 0.);
            economical.update(state(25.), 0.);
        }
        let (fast_time, fast_fuel) = fast.estimate().unwrap();
        let (slow_time, slow_fuel) = economical.estimate().unwrap();
        assert!(slow_time > fast_time * 1.5);
        assert!(slow_fuel <= 25. && fast_fuel > slow_fuel);
        assert!(fast_time < 100.);
    }

    #[test]
    fn changing_target_motion_reopens_search_during_coast() {
        let mut search = Search::default();
        let initial = state(25.);
        for _ in 0..40 {
            search.update(initial, 0.);
        }
        let plan = search.best.unwrap();
        search.best.as_mut().unwrap().burn = 0.;
        let changed = State {
            error: initial.error + DVec3::Z * 10_000.,
            ..initial
        };
        for _ in 0..40 {
            search.update(changed, 0.);
        }
        assert!(search.best.unwrap().first.z > 0.);
        assert_ne!(search.best.unwrap().remaining, plan.remaining);
    }
}
