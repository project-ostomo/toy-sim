//! Exact conic geometry for presentation. Time-based predictions still use the solver.
use super::conic::Conic;
use bevy::math::{DVec3, DVec4};
use std::f64::consts::{FRAC_PI_8, TAU};

#[derive(Clone, Debug)]
pub struct AnalyticCurve {
    pub conic: Conic,
    pub offset: DVec3,
}

/// Homogeneous quadratic: xyz is position multiplied by weight, w is weight.
/// Keeps perspective projection and clipping exact until the screen-space cubic fit.
#[derive(Clone, Copy, Debug)]
pub struct RationalArc(pub [DVec4; 3]);
impl RationalArc {
    pub fn point(&self, t: f64) -> DVec3 {
        let h = self.0[0]
            .lerp(self.0[1], t)
            .lerp(self.0[1].lerp(self.0[2], t), t);
        h.truncate() / h.w
    }
    pub fn split(&self, t: f64) -> (Self, Self) {
        let a = self.0[0].lerp(self.0[1], t);
        let b = self.0[1].lerp(self.0[2], t);
        let m = a.lerp(b, t);
        (Self([self.0[0], a, m]), Self([m, b, self.0[2]]))
    }
    pub fn interval(&self, a: f64, b: f64) -> Self {
        let left = self.split(b).0;
        if a <= 0. { left } else { left.split(a / b).1 }
    }
}

impl AnalyticCurve {
    pub fn at(&self, t: f64) -> Option<DVec3> {
        Some(self.conic.state(t)?.0 + self.offset)
    }

    /// Parameterize the physical conic by angle, independent of orbital speed.
    /// A closed orbit needs only sixteen exact arcs at any eccentricity or zoom.
    pub fn arcs(&self, start: f64, end: f64) -> Option<Vec<RationalArc>> {
        if end <= start {
            return Some(vec![]);
        }
        let end = self.conic.period().map_or(end, |p| end.min(start + p));
        let a = self.conic.state(start)?.0;
        let b = self.conic.state(end)?.0;
        if self.conic.mu <= 0. {
            return Some(vec![RationalArc([
                (a + self.offset).extend(1.),
                ((a + b) * 0.5 + self.offset).extend(1.),
                (b + self.offset).extend(1.),
            ])]);
        }
        let normal = self.conic.normal()?;
        let u = a.try_normalize()?;
        let v = normal.cross(u);
        let e = self.conic.eccentricity();
        let parameter = self.conic.r.cross(self.conic.v).length_squared() / self.conic.mu;
        let full = self
            .conic
            .period()
            .is_some_and(|p| end - start >= p * (1. - 1e-12));
        let angle = if full {
            TAU
        } else {
            b.dot(v).atan2(b.dot(u)).rem_euclid(TAU)
        };
        let count = (angle / FRAC_PI_8).ceil().max(1.) as usize;
        let ray = |t: f64| u * t.cos() + v * t.sin();
        let control = |direction: DVec3, circle_weight: f64| {
            let w = circle_weight + e.dot(direction);
            (parameter * direction + self.offset * w).extend(w)
        };
        let mut arcs = Vec::with_capacity(count);
        for i in 0..count {
            let lo = angle * i as f64 / count as f64;
            let hi = angle * (i + 1) as f64 / count as f64;
            arcs.push(RationalArc([
                control(ray(lo), 1.),
                control(ray((lo + hi) * 0.5), ((hi - lo) * 0.5).cos()),
                control(ray(hi), 1.),
            ]));
        }
        // Keep the attachment point exact even when conic coefficients suffer cancellation.
        let first = &mut arcs.first_mut()?.0[0];
        *first = (a + self.offset).extend(1.) * first.w;
        let last = &mut arcs.last_mut()?.0[2];
        *last = ((if full { a } else { b }) + self.offset).extend(1.) * last.w;
        arcs.iter()
            .all(|arc| arc.0.iter().all(|h| h.is_finite()))
            .then_some(arcs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcs_follow_elliptic_parabolic_and_hyperbolic_equations() {
        let mu = 4e14;
        let radius = 4e7;
        for eccentricity in [0., 0.8, 0.9999, 1., 1.5] {
            let curve = AnalyticCurve {
                conic: Conic {
                    r: DVec3::X * radius,
                    v: DVec3::Y * (mu * (1. + eccentricity) / radius).sqrt(),
                    mu,
                    radius: 1.,
                },
                offset: DVec3::new(100., 200., -300.),
            };
            let duration = curve.conic.period().unwrap_or(100_000.);
            let arcs = curve.arcs(0., duration).unwrap();
            assert!(arcs.len() <= 16);
            assert!(arcs[0].point(0.).distance(curve.conic.r + curve.offset) < 1e-7);
            for arc in &arcs {
                for i in 0..101 {
                    let p = arc.point(i as f64 / 100.) - curve.offset;
                    let parameter = radius * (1. + eccentricity);
                    let expected = parameter / (1. + eccentricity * p.normalize().x);
                    assert!(
                        (p.length() / expected - 1.).abs() < 1e-7,
                        "e={eccentricity}, p={p:?}"
                    );
                    assert!(p.z.abs() < 0.01);
                }
            }
            assert!(
                arcs.last()
                    .unwrap()
                    .point(1.)
                    .distance(curve.at(duration).unwrap())
                    < 0.01
            );
        }
    }

    #[test]
    fn rational_split_preserves_curve_and_partial_orbit_endpoints() {
        let curve = AnalyticCurve {
            conic: Conic {
                r: DVec3::X * 100.,
                v: DVec3::Y * 10.,
                mu: 10_000.,
                radius: 1.,
            },
            offset: DVec3::ZERO,
        };
        let arcs = curve.arcs(1., 20.).unwrap();
        assert!(arcs[0].point(0.).distance(curve.at(1.).unwrap()) < 1e-10);
        assert!(
            arcs.last()
                .unwrap()
                .point(1.)
                .distance(curve.at(20.).unwrap())
                < 1e-10
        );
        for arc in arcs {
            let part = arc.interval(0.2, 0.7);
            for i in 0..11 {
                let t = i as f64 / 10.;
                assert!(part.point(t).distance(arc.point(0.2 + t * 0.5)) < 1e-10);
            }
        }
    }
}
