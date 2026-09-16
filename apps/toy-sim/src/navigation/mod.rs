//! Native navigation data. Guest ABI and flight decisions are intentionally independent of this module.
pub mod conic;
pub mod geometry;
use crate::{orrery::Universe, precision::GalacticPosition};
use bevy::{math::DVec3, prelude::*};
use conic::Conic;
use toy_sim_ship_wasm::{
    Session,
    spatial::{self, Path},
};

#[derive(Clone, Debug)]
pub struct ObservedTarget {
    pub id: u64,
    pub name: String,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub epoch: f64,
    pub age: f64,
}

/// Presentation inputs resolved from admitted observations at a single clock instant.
#[derive(Default)]
pub struct Snapshots {
    pub target: Option<ObservedTarget>,
    pub plan: Option<Path>,
    pub target_plan: Option<Path>,
}

impl Snapshots {
    pub fn at(session: &Session, time: f64) -> Self {
        let navigation = session
            .navigation
            .filter(|state| state.valid_until_s > time);
        let target_id = navigation.map_or(0, |state| state.target_contact);
        let target = session.spatial.tracks.get(&target_id).and_then(|track| {
            let estimate = track.at(time)?;
            Some(ObservedTarget {
                id: target_id,
                name: track.contact.name.clone(),
                position: position(estimate.position),
                velocity: DVec3::from_array(estimate.velocity),
                epoch: time,
                age: (time - track.latest.epoch).max(0.),
            })
        });
        let path = |id| {
            session
                .spatial
                .paths
                .get(&id)
                .filter(|path| path.active(time))
                .cloned()
        };

        Self {
            target,
            plan: navigation.and_then(|state| path(state.own_path)),
            target_plan: navigation.and_then(|state| path(state.target_path)),
        }
    }
}

pub fn position(value: spatial::Position) -> GalacticPosition {
    GalacticPosition::new(value[0], value[1], value[2])
}

fn path_relative(path: &Path, origin: GalacticPosition, time: f64) -> Option<DVec3> {
    spatial::relative(path.at(time)?, [origin.x, origin.y, origin.z]).map(DVec3::from_array)
}

/// Local displacements are checked before composing an absolute coordinate. A bad or
/// unrepresentable guest/predicted point suppresses that estimate, never crashes the client.
pub(crate) fn checked_offset(origin: GalacticPosition, offset: DVec3) -> Option<GalacticPosition> {
    if !offset.is_finite() || offset.abs().max_element() >= i128::MAX as f64 / 1e6 {
        return None;
    }
    let d = GalacticPosition::from_meters(offset);
    Some(GalacticPosition::new(
        origin.x.checked_add(d.x)?,
        origin.y.checked_add(d.y)?,
        origin.z.checked_add(d.z)?,
    ))
}
fn epoch(t: f64) -> hifitime::Epoch {
    hifitime::Epoch::from_mjd_utc(0.) + hifitime::Duration::from_seconds(t)
}
#[derive(Clone)]
pub struct Orbit {
    pub conic: Conic,
    pub primary: Option<String>,
    pub origin: GalacticPosition,
    pub epoch: f64,
}
impl Orbit {
    pub fn fit(
        universe: &Universe,
        position: GalacticPosition,
        velocity: DVec3,
        time: f64,
    ) -> Self {
        let primary = universe
            .iter()
            .filter(|b| universe.gravity_applies(&b.name, position))
            .filter_map(|b| {
                let p = universe.solve_position(&b.name, epoch(time))?;
                let r = position.relative_to(p);
                let mu = crate::physics::GRAVITATIONAL_CONSTANT * b.mass;
                Some((b, p, r, mu, mu / r.length_squared().max(1.)))
            })
            .max_by(|a, b| a.4.total_cmp(&b.4));
        if let Some((b, p, r, mu, _)) = primary {
            Self {
                conic: Conic {
                    r,
                    v: velocity
                        - universe
                            .solve_velocity(&b.name, epoch(time))
                            .unwrap_or_default(),
                    mu,
                    radius: b.radius,
                },
                primary: Some(b.name.to_string()),
                origin: p,
                epoch: time,
            }
        } else {
            Self {
                conic: Conic {
                    r: DVec3::ZERO,
                    v: velocity,
                    mu: 0.,
                    radius: 0.,
                },
                primary: None,
                origin: position,
                epoch: time,
            }
        }
    }
    pub fn position(&self, universe: &Universe, time: f64) -> Option<GalacticPosition> {
        let origin = self
            .primary
            .as_ref()
            .and_then(|b| universe.solve_position(b, epoch(time)))
            .unwrap_or(self.origin);
        checked_offset(origin, self.conic.state(time - self.epoch)?.0)
    }
    pub fn velocity(&self, universe: &Universe, time: f64) -> Option<DVec3> {
        let velocity = self
            .primary
            .as_ref()
            .and_then(|b| universe.solve_velocity(b, epoch(time)))
            .unwrap_or_default();
        Some(velocity + self.conic.state(time - self.epoch)?.1)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub t: f64,
    pub p: DVec3,
}
#[derive(Default)]
pub struct Curve {
    pub points: Vec<Sample>,
    pub impact: bool,
    /// Exact coast geometry in this curve's frame, independent of the cached samples.
    pub analytic: Option<geometry::AnalyticCurve>,
}
impl Curve {
    pub fn at(&self, t: f64) -> Option<DVec3> {
        let first = self.points.first()?;
        let last = self.points.last()?;
        if t < first.t || t > last.t {
            return None;
        }
        if let Some(analytic) = &self.analytic {
            return analytic.at(t);
        }
        let i = self.points.partition_point(|p| p.t < t);
        if i == 0 {
            return Some(first.p);
        }
        let (a, b) = (
            self.points[i - 1],
            self.points[i.min(self.points.len() - 1)],
        );
        Some(a.p.lerp(b.p, ((t - a.t) / (b.t - a.t).max(1e-12)).clamp(0., 1.)))
    }
}
pub struct Annotation {
    pub p: DVec3,
    pub title: String,
    pub detail: String,
    pub kind: usize,
}
pub struct Encounter {
    pub seconds: f64,
    pub range: f64,
    pub speed: f64,
    pub ship: DVec3,
    pub target: DVec3,
}
pub struct Model {
    pub anchor: GalacticPosition,
    pub own: Curve,
    pub target: Curve,
    pub plan: Curve,
    pub annotations: Vec<Annotation>,
    pub encounter: Option<Encounter>,
    pub horizon: f64,
    pub primary: String,
    pub radius: f64,
    pub target_age: Option<f64>,
    pub plan_age: Option<f64>,
    pub period: Option<f64>,
    pub occluders: Vec<(DVec3, f64)>,
}
/// The only inputs involving the target are admitted sensor observations. No target entity query exists.
impl Model {
    pub fn build(
        universe: &Universe,
        position: GalacticPosition,
        velocity: DVec3,
        now: f64,
        snapshots: &Snapshots,
    ) -> Self {
        let own = Orbit::fit(universe, position, velocity, now);
        let anchor = own.origin;
        let period = own.conic.period();
        let publication = snapshots.plan.as_ref().filter(|p| p.active(now));
        let plan_end = publication
            .and_then(|p| p.vertices.last().map(|vertex| vertex.time_s - now))
            .unwrap_or(0.);
        // Near-parabolic periods tend to infinity. Bound preview work and ephemeris time.
        let horizon = period
            .unwrap_or(3600.)
            .max(plan_end)
            .clamp(1., 365.25 * 86400.);
        let frame = |orbit: &Orbit, t: f64| -> Option<DVec3> {
            // The common primary translation cancels exactly. Avoid thousands of
            // redundant celestial ephemeris solves for this overwhelmingly common case.
            if orbit.primary == own.primary {
                let r = orbit.conic.state(now + t - orbit.epoch)?.0;
                return Some(if own.primary.is_some() {
                    r
                } else {
                    r + orbit.origin.relative_to(anchor)
                });
            }
            let abs = orbit.position(universe, now + t)?;
            let origin = own
                .primary
                .as_ref()
                .and_then(|b| universe.solve_position(b, epoch(now + t)))
                .unwrap_or(anchor);
            Some(
                GalacticPosition::new(
                    abs.x.checked_sub(origin.x)?,
                    abs.y.checked_sub(origin.y)?,
                    abs.z.checked_sub(origin.z)?,
                )
                .to_meters_64(),
            )
        };
        let own_end = own.conic.impact(horizon).unwrap_or(horizon);
        let own_at = |t: f64| own.conic.state(t).map(|s| s.0);
        let mut own_curve = sample_curve(
            0.,
            own_end.min(period.unwrap_or(horizon)),
            own_at,
            own.conic.r.length().max(1.) * 1e-4,
            own_end < horizon,
        );
        own_curve.analytic = Some(geometry::AnalyticCurve {
            conic: own.conic.clone(),
            offset: DVec3::ZERO,
        });
        let target = snapshots
            .target
            .as_ref()
            .map(|s| Orbit::fit(universe, s.position, s.velocity, s.epoch));
        let target_end = target
            .as_ref()
            .and_then(|o| {
                o.conic
                    .impact((now - o.epoch + horizon).max(0.))
                    .map(|t| t + o.epoch - now)
            })
            .unwrap_or(horizon);
        let target_publication = snapshots
            .target_plan
            .as_ref()
            .filter(|path| path.active(now));
        let target_end = target_publication
            .and_then(|path| path.vertices.last().map(|vertex| vertex.time_s - now))
            .unwrap_or(target_end);
        let target_at = |t: f64| {
            if let Some(path) = target_publication {
                let origin = own
                    .primary
                    .as_ref()
                    .and_then(|body| universe.solve_position(body, epoch(now + t)))
                    .unwrap_or(anchor);
                return path_relative(path, origin, now + t);
            }

            if t > target_end {
                None
            } else {
                frame(target.as_ref()?, t)
            }
        };
        let mut target_curve = sample_curve(
            0.,
            target_end.min(horizon),
            target_at,
            own.conic.r.length().max(1.) * 1e-4,
            target_end < horizon,
        );
        if let Some(target) = target
            .as_ref()
            .filter(|o| target_publication.is_none() && o.primary == own.primary)
        {
            if let Some((r, v)) = target.conic.state(now - target.epoch) {
                target_curve.analytic = Some(geometry::AnalyticCurve {
                    conic: Conic {
                        r,
                        v,
                        ..target.conic.clone()
                    },
                    offset: if own.primary.is_none() {
                        target.origin.relative_to(anchor)
                    } else {
                        DVec3::ZERO
                    },
                });
            }
        }
        let plan_at = |t: f64| {
            let primary = own
                .primary
                .as_ref()
                .and_then(|b| universe.solve_position(b, epoch(now + t)))
                .unwrap_or(anchor);
            path_relative(publication?, primary, now + t)
        };
        let plan_start = publication
            .and_then(|p| {
                p.vertices
                    .first()
                    .map(|vertex| (vertex.time_s - now).max(0.))
            })
            .unwrap_or(0.);
        let plan_limit = plan_end.min(horizon);
        let breaks: Vec<_> = publication
            .map(|p| {
                p.vertices
                    .iter()
                    .map(|vertex| vertex.time_s - now)
                    .collect()
            })
            .unwrap_or_default();
        let mut plan_curve = sample_intervals(plan_start, plan_limit, plan_at, 0.1, false, &breaks);
        clip_surface(&mut plan_curve, own.conic.radius);
        let mut model = Self {
            anchor,
            own: own_curve,
            target: target_curve,
            plan: plan_curve,
            annotations: vec![],
            encounter: None,
            horizon,
            primary: own
                .primary
                .clone()
                .unwrap_or_else(|| "Inertial coast".into()),
            radius: own.conic.radius,
            target_age: snapshots.target.as_ref().map(|target| target.age),
            plan_age: publication.map(|p| (now - p.published_at).max(0.)),
            period,
            occluders: vec![],
        };
        for b in universe
            .iter()
            .filter(|b| universe.gravity_applies(&b.name, position))
        {
            if let Some(p) = universe.solve_position(&b.name, epoch(now)) {
                model.occluders.push((p.relative_to(anchor), b.radius));
            }
        }
        let mut annotate =
            |t: f64, title: String, detail: String, kind: usize, p: Option<DVec3>| {
                if t >= 0.
                    && t <= horizon
                    && let Some(p) = p
                {
                    model.annotations.push(Annotation {
                        p,
                        title,
                        detail,
                        kind,
                    });
                }
            };
        annotate(
            0.,
            "SHIP".into(),
            "Current position · coast orbit".into(),
            0,
            own_at(0.),
        );
        if let Some(s) = &snapshots.target {
            annotate(
                0.,
                "TARGET".into(),
                format!("{} · observation {:.1} s old", s.name, s.age),
                2,
                Some(s.position.relative_to(anchor)),
            );
        }
        for (label, t) in own.conic.apsides() {
            if t <= own_end
                && let Some(p) = own_at(t)
            {
                annotate(
                    t,
                    label.into(),
                    format!(
                        "{} altitude · T+{}",
                        distance(p.length() - own.conic.radius),
                        duration(t)
                    ),
                    0,
                    own_at(t),
                );
            }
        }
        if let Some(target) = target.as_ref().filter(|_| target_publication.is_none()) {
            for (label, relative_time) in target.conic.apsides() {
                let mut t = target.epoch + relative_time - now;
                if let Some(period) = target.conic.period() {
                    t = t.rem_euclid(period);
                }
                if t >= 0.
                    && t <= target_end
                    && let Some((r, _)) = target.conic.state(now + t - target.epoch)
                {
                    annotate(
                        t,
                        format!("TARGET {label}"),
                        format!(
                            "{} altitude · T+{}",
                            distance(r.length() - target.conic.radius),
                            duration(t)
                        ),
                        2,
                        target_at(t),
                    );
                }
            }
            if target.primary == own.primary
                && let (Some(a), Some(b)) = (own.conic.normal(), target.conic.normal())
            {
                if let Some(node) = a
                    .cross(b)
                    .try_normalize()
                    .filter(|_| a.cross(b).length() > 1e-4)
                {
                    for direction in [node, -node] {
                        if let Some(t) = own
                            .conic
                            .time_at_direction(direction)
                            .filter(|t| *t <= own_end)
                        {
                            let Some((p, v)) = own.conic.state(t) else {
                                continue;
                            };
                            let label = if v.dot(b) > 0. { "AN" } else { "DN" };
                            annotate(
                                t,
                                label.into(),
                                format!("Relative plane crossing · T+{}", duration(t)),
                                0,
                                Some(p),
                            );
                        }
                    }
                }
            }
        }
        for (curve, kind) in [(&model.own, 0), (&model.target, 2), (&model.plan, 1)] {
            if let Some(s) = curve.points.last().filter(|_| curve.impact || kind == 1) {
                annotate(
                    s.t,
                    if curve.impact { "IMPACT" } else { "PLAN END" }.into(),
                    format!("T+{}", duration(s.t)),
                    kind,
                    Some(s.p),
                );
            }
        }
        let using_plan = !model.plan.points.is_empty();
        let (start, end) = if using_plan {
            (plan_start, model.plan.points.last().unwrap().t)
        } else {
            (0., own_end.min(target_end))
        };
        let pair = |t: f64| {
            Some((
                if using_plan { plan_at(t)? } else { own_at(t)? },
                target_at(t)?,
            ))
        };
        if let Some((t, a, b)) = closest(start, end, pair) {
            let dt = 0.01;
            let speed = if using_plan {
                let lo = (t - dt).max(start);
                let hi = (t + dt).min(end);
                pair(lo)
                    .zip(pair(hi))
                    .map(|((a, b), (c, d))| ((c - d) - (a - b)).length() / (hi - lo).max(1e-9))
                    .unwrap_or(0.)
            } else {
                target
                    .as_ref()
                    .and_then(|o| o.velocity(universe, now + t))
                    .zip(own.velocity(universe, now + t))
                    .map(|(a, b)| (a - b).length())
                    .unwrap_or(0.)
            };
            model.encounter = Some(Encounter {
                seconds: t,
                range: (a - b).length(),
                speed,
                ship: a,
                target: b,
            });
            model.annotations.insert(
                0,
                Annotation {
                    p: b,
                    title: "CA · TARGET".into(),
                    detail: format!(
                        "T+{} · {} · {:.1} m/s relative",
                        duration(t),
                        distance((a - b).length()),
                        speed
                    ),
                    kind: 2,
                },
            );
            model.annotations.insert(
                0,
                Annotation {
                    p: a,
                    title: "CLOSEST APPROACH".into(),
                    detail: format!("{} · T+{}", distance((a - b).length()), duration(t)),
                    kind: 1,
                },
            );
        }
        model.annotations.truncate(32);
        model
    }
}
/// Intersect sampled plan segments with the primary surface, including chords whose
/// endpoints are both outside the body. Retain the interpolated entry point and time.
fn clip_surface(curve: &mut Curve, radius: f64) {
    if radius <= 0. || curve.points.is_empty() {
        return;
    }
    if curve.points[0].p.length() <= radius {
        curve.points.truncate(1);
        curve.impact = true;
        return;
    }
    for i in 1..curve.points.len() {
        let a = curve.points[i - 1];
        let b = curve.points[i];
        let delta = b.p - a.p;
        let length = delta.length();
        if length <= 0. {
            continue;
        }
        let dir = delta / length;
        let along = -a.p.dot(dir);
        let perpendicular = a.p.cross(dir).length_squared();
        if perpendicular > radius * radius {
            continue;
        }
        let entry = along - (radius * radius - perpendicular).sqrt();
        if entry >= 0. && entry <= length {
            let fraction = entry / length;
            curve.points[i] = Sample {
                t: a.t + (b.t - a.t) * fraction,
                p: a.p + dir * entry,
            };
            curve.points.truncate(i + 1);
            curve.impact = true;
            return;
        }
    }
}

pub fn sample_curve(
    start: f64,
    end: f64,
    f: impl Fn(f64) -> Option<DVec3>,
    tolerance: f64,
    impact: bool,
) -> Curve {
    sample_intervals(start, end, f, tolerance, impact, &[])
}
fn sample_intervals(
    start: f64,
    end: f64,
    f: impl Fn(f64) -> Option<DVec3>,
    tolerance: f64,
    impact: bool,
    breaks: &[f64],
) -> Curve {
    if end < start {
        return Curve::default();
    }
    let mut points = Vec::with_capacity(1025);
    // Begin with 64 intervals so full closed curves and narrow periapses cannot alias to a chord.
    fn refine(
        a: Sample,
        b: Sample,
        f: &impl Fn(f64) -> Option<DVec3>,
        tol: f64,
        depth: u32,
        max_depth: u32,
        out: &mut Vec<Sample>,
    ) {
        let t = (a.t + b.t) * 0.5;
        if let Some(p) = f(t) {
            if depth < max_depth && p.distance((a.p + b.p) * 0.5) > tol {
                let m = Sample { t, p };
                refine(a, m, f, tol, depth + 1, max_depth, out);
                refine(m, b, f, tol, depth + 1, max_depth, out);
                return;
            }
        }
        out.push(b);
    }
    if let Some(p) = f(start) {
        points.push(Sample { t: start, p });
    } else {
        return Curve::default();
    }
    let mut times: Vec<_> = (1..=64)
        .map(|i| start + (end - start) * i as f64 / 64.)
        .collect();
    times.extend(breaks.iter().copied().filter(|t| *t > start && *t < end));
    times.sort_by(f64::total_cmp);
    times.dedup();
    let depth = (1024 / times.len().max(1)).ilog2();
    for t in times {
        let Some(p) = f(t) else {
            break;
        };
        let a = *points.last().unwrap();
        refine(a, Sample { t, p }, &f, tolerance, 0, depth, &mut points);
    }
    Curve {
        points,
        impact,
        analytic: None,
    }
}
fn closest(
    start: f64,
    end: f64,
    pair: impl Fn(f64) -> Option<(DVec3, DVec3)>,
) -> Option<(f64, DVec3, DVec3)> {
    if end < start {
        return None;
    }
    let cost = |t| {
        pair(t)
            .map(|(a, b)| (a - b).length_squared())
            .unwrap_or(f64::INFINITY)
    };
    let step = (end - start) / 256.;
    let index = (0..=256)
        .min_by(|a, b| cost(start + *a as f64 * step).total_cmp(&cost(start + *b as f64 * step)))?;
    let mut lo = (start + (index as f64 - 1.) * step).max(start);
    let mut hi = (lo + 2. * step).min(end);
    for _ in 0..48 {
        let a = lo + (hi - lo) / 3.;
        let b = hi - (hi - lo) / 3.;
        if cost(a) < cost(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let t = [start, end, (lo + hi) * 0.5]
        .into_iter()
        .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))?;
    let (a, b) = pair(t)?;
    Some((t, a, b))
}
pub fn distance(m: f64) -> String {
    if m.abs() >= 1e9 {
        format!("{:.2} Gm", m / 1e9)
    } else if m.abs() >= 1e6 {
        format!("{:.2} Mm", m / 1e6)
    } else if m.abs() >= 1000. {
        format!("{:.1} km", m / 1000.)
    } else {
        format!("{m:.0} m")
    }
}
pub fn duration(t: f64) -> String {
    if t >= 3600. {
        format!("{:.1} h", t / 3600.)
    } else if t >= 60. {
        format!("{:.1} min", t / 60.)
    } else {
        format!("{t:.1} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_target_uses_sensor_estimate_while_forecast_retains_its_own_geometry() {
        let universe = Universe::init(crate::orrery::example_config()).unwrap();
        let now = 100.;
        let body = crate::scenario::INITIAL_SCENARIO.body;
        let center = universe.solve_position(body, epoch(now)).unwrap();
        let ship = center.offset_by(DVec3::X * 4e7);
        let velocity = universe.solve_velocity(body, epoch(now)).unwrap() + DVec3::Y * 3000.;
        let measured = ship.offset_by(DVec3::Y * 1000.);
        let target_path = Path {
            header: toy_sim_ship_api::abi::SpatialPath {
                meta: toy_sim_ship_api::abi::SpatialMeta {
                    id: 2,
                    valid_until_s: now + 2.,
                    ..Default::default()
                },
                kind: toy_sim_ship_api::abi::PATH_TIMED,
                ..Default::default()
            },
            snapshot: Some(spatial::Snapshot {
                epoch: now,
                origin: [ship.x, ship.y, ship.z],
                ..Default::default()
            }),
            vertices: vec![
                toy_sim_ship_api::abi::SpatialVertex {
                    time_s: now,
                    position_m: [0., 2000., 0.],
                },
                toy_sim_ship_api::abi::SpatialVertex {
                    time_s: now + 10.,
                    position_m: [0., 2100., 0.],
                },
            ]
            .into(),
            revision: 1,
            published_at: now,
        };
        let snapshots = Snapshots {
            target: Some(ObservedTarget {
                id: 7,
                name: "Target".into(),
                position: measured,
                velocity,
                epoch: now,
                age: 0.5,
            }),
            target_plan: Some(target_path),
            ..Default::default()
        };
        let model = Model::build(&universe, ship, velocity, now, &snapshots);
        let marker = model
            .annotations
            .iter()
            .find(|annotation| annotation.title == "TARGET")
            .unwrap();
        assert!(
            marker
                .p
                .abs_diff_eq(measured.relative_to(model.anchor), 1e-6)
        );
        assert!(model.target.points.len() >= 2);
        assert!(model.target.points[0].p.distance(marker.p) > 999.);
        assert_eq!(model.target.points.last().unwrap().t, 10.);
    }

    #[test]
    fn closest_approach_uses_synchronous_positions_and_endpoints() {
        let pair = |t| Some((DVec3::X * t, DVec3::new(10., 3., 0.)));
        let (t, a, b) = closest(0., 20., pair).unwrap();
        assert!((t - 10.).abs() < 1e-5);
        assert!((a.distance(b) - 3.).abs() < 1e-6);
        assert_eq!(closest(0., 5., pair).unwrap().0, 5.);
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    #[test]
    fn unrepresentable_offsets_are_rejected_without_integer_overflow() {
        assert!(checked_offset(GalacticPosition::ZERO, DVec3::splat(1e300)).is_none());
        assert!(checked_offset(GalacticPosition::new(i128::MAX, 0, 0), DVec3::X).is_none());
        assert!(checked_offset(GalacticPosition::ZERO, DVec3::NAN).is_none());
    }
    #[test]
    fn plan_chord_stops_at_surface_even_with_both_samples_outside() {
        let mut path = Curve {
            points: vec![
                Sample {
                    t: 0.,
                    p: DVec3::X * 10.,
                },
                Sample {
                    t: 20.,
                    p: -DVec3::X * 10.,
                },
            ],
            impact: false,
            analytic: None,
        };
        clip_surface(&mut path, 5.);
        assert!(path.impact);
        let impact = path.points.last().unwrap();
        assert_eq!(impact.t, 5.);
        assert_eq!(impact.p, DVec3::X * 5.);
    }
}
