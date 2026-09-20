//! Bounded, double precision two-body prediction. This never advances simulation state.
use bevy::math::DVec3;
use std::f64::consts::TAU;

#[derive(Clone, Debug)]
pub struct Conic {
    pub r: DVec3,
    pub v: DVec3,
    pub mu: f64,
    pub radius: f64,
}
impl Conic {
    pub fn eccentricity(&self) -> DVec3 {
        self.v.cross(self.r.cross(self.v)) / self.mu - self.r.normalize()
    }
    pub fn normal(&self) -> Option<DVec3> {
        self.r.cross(self.v).try_normalize()
    }
    pub fn period(&self) -> Option<f64> {
        let alpha = 2. / self.r.length() - self.v.length_squared() / self.mu;
        (self.mu > 0. && alpha > 0.).then(|| TAU / (self.mu * alpha.powi(3)).sqrt())
    }
    /// Universal-variable Kepler equation; covers elliptic, parabolic, hyperbolic and radial states.
    pub fn state(&self, seconds: f64) -> Option<(DVec3, DVec3)> {
        if !seconds.is_finite() || !self.r.is_finite() || !self.v.is_finite() {
            return None;
        }
        if self.mu <= 0. {
            return Some((self.r + self.v * seconds, self.v));
        }
        let r0 = self.r.length();
        if r0 <= 0. {
            return None;
        }
        let dt = self.period().map_or(seconds, |p| seconds % p);
        if dt == 0. {
            return Some((self.r, self.v));
        }
        let root = self.mu.sqrt();
        let alpha = 2. / r0 - self.v.length_squared() / self.mu;
        let rv = self.r.dot(self.v) / root;
        let equation = |x: f64| {
            let (c, s) = stumpff(alpha * x * x);
            let f = rv * x * x * c + (1. - alpha * r0) * x * x * x * s + r0 * x - root * dt;
            let derivative = rv * x * (1. - alpha * x * x * s) + (1. - alpha * r0) * x * x * c + r0;
            (f, derivative)
        };
        let sign = dt.signum();
        let mut edge = sign * (root * dt.abs() / r0).max(1.);
        for _ in 0..64 {
            let f = equation(edge).0;
            if !f.is_finite() || f * sign >= 0. {
                break;
            }
            edge *= 2.;
        }
        let (mut lo, mut hi) = if sign > 0. { (0., edge) } else { (edge, 0.) };
        let mut x = (lo + hi) * 0.5;
        for _ in 0..80 {
            let (f, d) = equation(x);
            if f.is_finite() && f.abs() < 1e-11 * (root * dt.abs()).max(1.) {
                break;
            }
            if f.is_nan() {
                if sign > 0. {
                    hi = x;
                } else {
                    lo = x;
                }
            } else if f > 0. {
                hi = x;
            } else {
                lo = x;
            }
            let next = x - f / d;
            x = if next.is_finite() && next > lo + (hi - lo) * 0.1 && next < hi - (hi - lo) * 0.1 {
                next
            } else {
                (lo + hi) * 0.5
            };
        }
        // A failed solve must not manufacture enormous finite positions. Safeguarded
        // Newton steps shrink the bracket even for energetic escape trajectories.
        if !equation(x).0.is_finite() || equation(x).0.abs() > 1e-8 * (root * dt.abs()).max(1.) {
            return None;
        }
        let (c, s) = stumpff(alpha * x * x);
        let f = 1. - x * x / r0 * c;
        let g = dt - x * x * x / root * s;
        let r = f * self.r + g * self.v;
        let len = r.length();
        let v = root / (len * r0) * (alpha * x * x * x * s - x) * self.r
            + (1. - x * x / len * c) * self.v;
        (r.is_finite() && v.is_finite() && len > 0.).then_some((r, v))
    }
    /// Next passage through an inertial ray in this orbital plane.
    pub fn time_at_direction(&self, direction: DVec3) -> Option<f64> {
        let evec = self.eccentricity();
        let e = evec.length();
        let n = self.normal()?;
        let p = if e > 1e-8 {
            evec / e
        } else {
            self.r.normalize()
        };
        let q = n.cross(p);
        let anomaly = |r: DVec3| r.dot(q).atan2(r.dot(p));
        let initial = anomaly(self.r);
        let next = anomaly(direction);
        let angular = self.r.cross(self.v).length();
        let parameter = angular * angular / self.mu;
        if e < 1. - 1e-8 {
            let mean = |nu: f64| {
                let ea = ((1. - e * e).sqrt() * nu.sin()).atan2(e + nu.cos());
                ea - e * ea.sin()
            };
            Some((mean(next) - mean(initial)).rem_euclid(TAU) / TAU * self.period()?)
        } else if e > 1. + 1e-8 {
            let mean = |nu: f64| -> Option<f64> {
                let denominator = 1. + e * nu.cos();
                if denominator <= 0. {
                    return None;
                }
                let h = ((e * e - 1.).sqrt() * nu.sin() / denominator).asinh();
                Some(e * h.sinh() - h)
            };
            let a = parameter / (e * e - 1.);
            let t = (mean(next)? - mean(initial)?) * (a.powi(3) / self.mu).sqrt();
            (t >= 0.).then_some(t)
        } else {
            let barker = |nu: f64| {
                let d = (nu * 0.5).tan();
                d + d.powi(3) / 3.
            };
            let t = 0.5 * (parameter.powi(3) / self.mu).sqrt() * (barker(next) - barker(initial));
            (t >= 0.).then_some(t)
        }
    }
    pub fn apsides(&self) -> Vec<(&'static str, f64)> {
        if self.mu <= 0. || self.normal().is_none() {
            return vec![];
        }
        let e = self.eccentricity();
        if e.length() < 1e-5 {
            return vec![];
        }
        [("PE", e), ("AP", -e)]
            .into_iter()
            .filter_map(|(name, d)| {
                if name == "AP" && self.period().is_none() {
                    return None;
                }
                self.time_at_direction(d).map(|t| (name, t))
            })
            .collect()
    }
    /// Earliest surface crossing, even when an entire impact passage falls between path samples.
    pub fn impact(&self, horizon: f64) -> Option<f64> {
        if self.radius <= 0. {
            return None;
        }
        if self.r.length() <= self.radius {
            return Some(0.);
        }
        let mut previous = 0.;
        let mut times: Vec<_> = (1..=256).map(|i| horizon * i as f64 / 256.).collect();
        times.extend(
            self.apsides()
                .into_iter()
                .filter(|(n, t)| *n == "PE" && *t <= horizon)
                .map(|(_, t)| t),
        );
        times.sort_by(f64::total_cmp);
        for t in times {
            if self.state(t)?.0.length() <= self.radius {
                let (mut lo, mut hi) = (previous, t);
                for _ in 0..48 {
                    let m = (lo + hi) * 0.5;
                    if self.state(m)?.0.length() > self.radius {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                return Some(hi);
            }
            previous = t;
        }
        None
    }
}
fn stumpff(z: f64) -> (f64, f64) {
    if z.abs() < 1e-5 {
        (
            0.5 - z / 24. + z * z / 720. - z.powi(3) / 40320.,
            1. / 6. - z / 120. + z * z / 5040. - z.powi(3) / 362880.,
        )
    } else if z > 0. {
        let x = z.sqrt();
        ((1. - x.cos()) / z, (x - x.sin()) / x.powi(3))
    } else {
        let x = (-z).sqrt();
        ((x.cosh() - 1.) / (-z), (x.sinh() - x) / x.powi(3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn earth(v: f64) -> Conic {
        Conic {
            r: DVec3::X * 7e6,
            v: DVec3::Y * v,
            mu: 3.986004418e14,
            radius: 6.371e6,
        }
    }
    #[test]
    fn circular_period_and_quarter_turn() {
        let c = earth((3.986004418e14_f64 / 7e6).sqrt());
        let p = c.period().unwrap();
        assert!(
            c.state(p * 0.25)
                .unwrap()
                .0
                .abs_diff_eq(DVec3::Y * 7e6, 0.001)
        );
        assert!(c.state(p).unwrap().0.abs_diff_eq(c.r, 0.001));
        assert!(c.apsides().is_empty());
        assert!(c.impact(p).is_none());
    }
    #[test]
    fn energy_momentum_and_reversibility_across_conics() {
        for speed in [5000., 7500., (2. * 3.986004418e14_f64 / 7e6).sqrt(), 12000.] {
            let c = earth(speed);
            let energy = c.v.length_squared() / 2. - c.mu / c.r.length();
            for t in [-1000., 10., 1000., 10000.] {
                let (r, v) = c.state(t).unwrap();
                assert!((v.length_squared() / 2. - c.mu / r.length() - energy).abs() < 0.05);
                assert!(r.cross(v).abs_diff_eq(c.r.cross(c.v), 10.));
                let reverse = Conic { r, v, ..c.clone() }.state(-t).unwrap();
                assert!(reverse.0.abs_diff_eq(c.r, 0.1));
            }
        }
    }
    #[test]
    fn apsides_nodes_and_impact() {
        let c = earth(7000.);
        let events = c.apsides();
        assert_eq!(events.len(), 2);
        let pe = events.iter().find(|(n, _)| *n == "PE").unwrap().1;
        let impact = c.impact(c.period().unwrap()).unwrap();
        assert!(impact < pe);
        assert!((c.state(impact).unwrap().0.length() - c.radius).abs() < 0.001);
        let t = c.time_at_direction(DVec3::Y).unwrap();
        assert!(
            c.state(t)
                .unwrap()
                .0
                .normalize()
                .abs_diff_eq(DVec3::Y, 1e-8)
        );
        let unbound = earth(12000.);
        assert!(unbound.period().is_none());
        assert!(unbound.apsides().iter().all(|(n, _)| *n != "AP"));
    }
    #[test]
    fn no_gravity_is_straight() {
        let c = Conic {
            mu: 0.,
            ..earth(10.)
        };
        assert_eq!(c.state(5.).unwrap().0, c.r + c.v * 5.);
    }
}

#[cfg(test)]
mod escape_regressions {
    use super::*;
    #[test]
    fn energetic_escape_converges_without_growing_an_unphysical_exponential() {
        for speed in [10_000., 50_000., 100_000., 200_000., 500_000., 1e6] {
            let c = Conic {
                r: DVec3::X * 4.6e7,
                v: DVec3::Y * speed,
                mu: 3.986e14,
                radius: 6e6,
            };
            for t in [1., 1000., 100_000., 31_557_600.] {
                let (r, v) = c.state(t).expect("energetic hyperbolic solve");
                assert!(r.length() <= c.r.length() + speed * t * 1.0001);
                let energy = speed * speed * 0.5 - c.mu / c.r.length();
                assert!(
                    (v.length_squared() * 0.5 - c.mu / r.length() - energy).abs()
                        < energy.abs() * 1e-7
                );
            }
        }
    }
}
