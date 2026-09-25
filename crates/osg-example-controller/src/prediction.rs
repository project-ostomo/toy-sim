//! Complete, budgeted forward forecasts. Prediction never changes flight commands.
use crate::hardware::Sample;
use crate::{
    Bindings, attitude,
    navigation::{Phase, Pursuit},
};
use glam::{DMat3, DQuat, DVec3};
use osg_ship_api::abi;

const REFRESH_S: f64 = 0.5;
const HORIZON_S: f64 = 3600.;
const MAX_SAMPLES: usize = 4096;
fn budget() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        osg_ship_api::sdk::budget().is_ok_and(crate::budget::forecast_allowed)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Point {
    /// Observer-relative displacement in the snapshot's translating frame.
    pub r: DVec3,
    pub seconds: f64,
    pub speed: f64,
    pub allowed: f64,
}
#[derive(Clone, Debug)]
pub struct Forecast {
    pub points: Vec<Point>,
    pub epoch: f64,
    pub published_at: f64,
    pub snapshot: u64,
    pub target: u64,
    pub target_position: DVec3,
    pub frame_velocity: DVec3,
    pub eta: Option<f64>,
    pub fuel_kg: f64,
}
impl Forecast {
    pub fn eta_at(&self, now: f64) -> Option<f64> {
        (now >= self.published_at && now < self.published_at + 2.)
            .then(|| self.eta.map(|t| (t - (now - self.epoch)).max(0.)))
            .flatten()
    }
}
#[derive(Clone, Copy)]
struct State {
    r: DVec3,
    u: DVec3,
    q: DQuat,
    omega: DVec3,
    direction: DVec3,
    mass: f64,
    fuel: f64,
    time: f64,
}
struct Rollout {
    guidance: Pursuit,
    state: State,
    bindings: Bindings,
    inertia: DMat3,
    inverse: DMat3,
    initial_r: DVec3,
    offset: DVec3,
    disturbance: DVec3,
    force: f64,
    ceiling: f64,
    limit: f64,
    speed_limit: f64,
    initial_fuel: f64,
    epoch: f64,
    frame_velocity: DVec3,
    snapshot: u64,
    target: u64,
    visible: bool,
    horizon: f64,
    samples: Vec<Point>,
    eta: Option<f64>,
    done: bool,
    // Best-first timed simplification, resumable under the same fuel allowance.
    sections: Vec<Section>,
    chosen: Vec<usize>,
}
struct Section {
    lo: usize,
    hi: usize,
    split: usize,
    error: f64,
    cursor: usize,
}
impl Rollout {
    fn new(nav: &Pursuit, obs: &Sample, b: &Bindings) -> Option<Self> {
        let q = attitude::valid_rotation(obs.rotation)?;
        let fuel = obs.propellant_kg.max(0.);
        let inertia = DMat3::from_cols_array(&obs.inertia);
        Some(Self {
            guidance: nav.clone(),
            state: State {
                r: nav.r,
                u: nav.u,
                q,
                omega: DVec3::from_array(obs.angular_velocity),
                direction: nav.direction,
                mass: obs.mass_kg,
                fuel,
                time: 0.,
            },
            bindings: b.clone(),
            inertia,
            inverse: inertia.inverse(),
            initial_r: nav.r,
            offset: nav.offset,
            disturbance: nav.disturbance,
            force: b.thrust * nav.effectiveness * nav.throttle_ceiling,
            ceiling: nav.throttle_ceiling,
            limit: nav.limit,
            speed_limit: nav.speed_limit,
            initial_fuel: fuel,
            epoch: obs.tick.time_s,
            frame_velocity: DVec3::from_array(obs.velocity) - nav.u,
            snapshot: obs.tick.snapshot,
            target: nav.target.as_ref()?.id,
            visible: nav.visible,
            horizon: if nav.visible {
                HORIZON_S
            } else {
                nav.tracking_remaining(obs.tick.time_s)
            },
            samples: vec![Point {
                r: DVec3::ZERO,
                seconds: 0.,
                speed: nav.u.length(),
                allowed: nav.allowed_speed,
            }],
            eta: None,
            done: false,
            sections: Vec::new(),
            chosen: Vec::new(),
        })
    }
    fn obsolete(&self, nav: &Pursuit, b: &Bindings) -> bool {
        nav.target.as_ref().is_none_or(|t| t.id != self.target)
            || nav.offset != self.offset
            || nav.limit != self.limit
            || nav.speed_limit != self.speed_limit
            || nav.visible != self.visible
            || (nav.throttle_ceiling - self.ceiling).abs() > 0.01
            || (b.thrust * nav.effectiveness * nav.throttle_ceiling - self.force).abs()
                > self.force * 0.1
            || nav.disturbance.distance(self.disturbance)
                > (self.force / self.state.mass * 0.05).max(0.5)
    }
    fn point(&self) -> Point {
        Point {
            r: self.initial_r - self.state.r,
            seconds: self.state.time,
            speed: self.state.u.length(),
            allowed: self.guidance.allowed_speed,
        }
    }

    fn advance(&mut self) {
        let s = self.state;
        let distance = (s.r + self.offset).length();
        let angle = attitude::error_angle(s.q, self.bindings.engine_axis, s.direction);
        let fine = s.time < 1. || distance < 100. || angle > 0.05 || s.omega.length() > 0.08;
        let dt = if fine {
            0.1
        } else {
            (0.05 * (distance / (self.force / s.mass).max(1e-9)).sqrt()).clamp(0.1, 5.)
        }
        .min(self.horizon - s.time);
        if dt <= 1e-8 {
            self.done = true;
            return;
        }

        // Forecast the actual follower, including its persistent maneuver and
        // phase hysteresis. The first command was already computed by control.
        if self.eta.is_none() && (s.time > 0. || !self.guidance.phase.active()) {
            let sample = Sample {
                flight: abi::FlightState {
                    rotation: s.q.to_array(),
                    angular_velocity: s.omega.to_array(),
                    mass_kg: s.mass,
                    inertia: self.inertia.to_cols_array(),
                    ..Default::default()
                },
                propellant_kg: s.fuel,
                ..Default::default()
            };
            self.guidance.r = s.r;
            self.guidance.u = s.u;
            self.guidance.phase = Phase::Pursuing;
            self.guidance.disturbance = self.disturbance;
            let mut bindings = self.bindings.clone();
            bindings.thrust = self.force / (self.guidance.effectiveness * self.ceiling).max(1e-12);
            self.guidance.guide(&sample, &bindings, dt);
        }
        let command = if self.eta.is_some() {
            DVec3::ZERO
        } else {
            self.guidance.acceleration
        };
        let direction = self
            .guidance
            .direction
            .try_normalize()
            .unwrap_or(s.direction);
        let alignment = (s.q * self.bindings.engine_axis).dot(direction);
        let fraction = if alignment > 0.995 {
            (command.length() / (self.force / s.mass).max(1e-12)).min(1.) * alignment
        } else {
            0.
        };
        let wanted = self.bindings.propellant_rate * self.ceiling * fraction * dt;
        let used = wanted.min(s.fuel);
        let available = if wanted > 0. { used / wanted } else { 1. };
        let acceleration =
            s.q * self.bindings.engine_axis * (self.force / s.mass * fraction * available)
                + self.disturbance;
        let mut next = State {
            r: s.r - s.u * dt - acceleration * (0.5 * dt * dt),
            u: s.u + acceleration * dt,
            mass: (s.mass - used).max(1.),
            fuel: s.fuel - used,
            time: s.time + dt,
            direction,
            ..s
        };
        if fine {
            next.r = s.r - next.u * dt;
            let target = attitude::point(s.q, self.bindings.engine_axis, direction);
            let torque = self.bindings.torquer_rotation
                * attitude::torque(s.q, s.omega, self.inertia, target, &self.bindings);
            let momentum = s.q * (self.inertia * (s.q.inverse() * s.omega)) + s.q * torque * dt;
            let kicked = s.q * (self.inverse * (s.q.inverse() * momentum));
            next.q = (DQuat::from_scaled_axis(kicked * dt) * s.q).normalize();
            next.omega = next.q * (self.inverse * (next.q.inverse() * momentum));
        } else {
            next.q = attitude::point(s.q, self.bindings.engine_axis, direction);
            next.omega = DVec3::ZERO;
        }
        if !next.r.is_finite()
            || !next.u.is_finite()
            || !next.q.is_finite()
            || !next.omega.is_finite()
        {
            self.done = true;
            return;
        }
        self.state = next;
        self.samples.push(self.point());
        if self.eta.is_none() && (next.r + self.offset).length() <= 2. && next.u.length() <= 0.5 {
            self.eta = Some(next.time);
        }
        if self.eta.is_some_and(|arrival| next.time >= arrival + 10.) {
            self.done = true;
        }
        if next.time >= self.horizon - 1e-8
            || self.samples.len() >= MAX_SAMPLES
            || (next.fuel <= 1e-9 && wanted > 0.)
        {
            self.done = true;
        }
    }
    fn section(lo: usize, hi: usize) -> Section {
        Section {
            lo,
            hi,
            split: lo,
            error: 0.,
            cursor: lo + 1,
        }
    }
    fn scan_sections(&mut self) -> bool {
        for section in &mut self.sections {
            let a = self.samples[section.lo];
            let b = self.samples[section.hi];
            while section.cursor < section.hi {
                if section.cursor % 32 == 0 && !budget() {
                    return false;
                }
                let i = section.cursor;
                let p = self.samples[i];
                let t = (p.seconds - a.seconds) / (b.seconds - a.seconds);
                // Time interpolation matters even on straight accelerating burns.
                let tolerance =
                    ((self.initial_r + self.offset - p.r).length() * 0.001).clamp(0.05, 100.);
                let error = p.r.distance(a.r.lerp(b.r, t)) / tolerance;
                if error > section.error {
                    section.split = i;
                    section.error = error;
                }
                section.cursor += 1;
            }
        }
        true
    }
    fn compress(&mut self) -> bool {
        if self.chosen.is_empty() {
            if self.samples.len() < 2 {
                return true;
            }
            self.chosen = vec![0, self.samples.len() - 1];
            // Keep the first second at physical tick resolution for close viewing.
            for i in 1..self.samples.len() - 1 {
                if self.samples[i].seconds <= 1.00001 {
                    self.chosen.push(i);
                }
            }
            self.chosen.sort_unstable();
            for pair in self.chosen.windows(2) {
                self.sections.push(Self::section(pair[0], pair[1]));
            }
        }
        // Timed vertices stay anchored to their original observation snapshot.
        while self.chosen.len() < abi::MAX_PATH_VERTICES as usize {
            if !budget() || !self.scan_sections() {
                return false;
            }
            let Some((index, best)) = self
                .sections
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.error.total_cmp(&b.1.error))
            else {
                break;
            };
            if best.error <= 1. {
                break;
            }
            let section = self.sections.swap_remove(index);
            self.chosen.push(section.split);
            self.sections.push(Self::section(section.lo, section.split));
            self.sections.push(Self::section(section.split, section.hi));
        }
        true
    }
    fn publish(mut self, published_at: f64) -> Option<Forecast> {
        self.chosen.sort_unstable();
        let points: Vec<_> = self
            .chosen
            .into_iter()
            .map(|index| self.samples[index])
            .collect();

        (points.len() > 1).then_some(Forecast {
            points,
            epoch: self.epoch,
            published_at,
            snapshot: self.snapshot,
            target: self.target,
            target_position: self.initial_r,
            frame_velocity: self.frame_velocity,
            eta: self.eta,
            fuel_kg: self.initial_fuel - self.state.fuel,
        })
    }
}

#[derive(Default)]
pub struct Predictor {
    pub result: Option<Forecast>,
    pending: Option<Rollout>,
    reference: Option<(u64, DVec3, f64, f64, DVec3, bool)>,
    last_time: Option<f64>,
    refreshed: Option<f64>,
    clear_pending: bool,
    retired_snapshots: Vec<u64>,
}
impl Predictor {
    fn retire_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.retired_snapshots.push(pending.snapshot);
        }
    }

    fn retire_result(&mut self) {
        if let Some(result) = self.result.take() {
            self.retired_snapshots.push(result.snapshot);
        }
    }

    pub fn reset(&mut self) {
        self.retire_pending();
        self.retire_result();
        self.reference = None;
        self.refreshed = None;
        self.last_time = None;
        self.clear_pending = true;
    }

    /// True publishes/replaces a path (including an explicit clear). False retains
    /// the previous atomic publication with its original epoch and expiry.
    pub fn update(&mut self, requested: bool, nav: &Pursuit, obs: &Sample, b: &Bindings) -> bool {
        #[cfg(target_arch = "wasm32")]
        for id in self.retired_snapshots.drain(..) {
            let _ = osg_ship_api::sdk::drop_snapshot(id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.retired_snapshots.clear();

        let dt = self.last_time.map_or(0., |t| obs.tick.time_s - t);
        self.last_time = Some(obs.tick.time_s);
        if !requested || !nav.phase.active() {
            self.retire_result();
            self.retire_pending();
            self.reference = None;
            self.refreshed = None;
            return true;
        }
        let key = (
            nav.target.as_ref().map_or(0, |t| t.id),
            nav.offset,
            nav.limit,
            nav.throttle_ceiling,
            nav.disturbance,
            nav.visible,
        );
        let changed = self.reference.is_none_or(|old| {
            old.0 != key.0
                || old.1 != key.1
                || old.2 != key.2
                || (old.3 - key.3).abs() > 0.01
                || old.4.distance(key.4)
                    > (b.thrust * nav.effectiveness / obs.mass_kg * 0.05).max(0.5)
                || old.5 != key.5
        });
        let gap = !dt.is_finite() || dt < 0. || dt > 0.15;
        let restart = gap || self.pending.as_ref().is_some_and(|p| p.obsolete(nav, b));
        if restart {
            self.retire_pending();
        }
        if self.pending.is_none()
            && (changed
                || restart
                || self
                    .refreshed
                    .is_none_or(|t| obs.tick.time_s - t >= REFRESH_S - 1e-8))
        {
            #[cfg(target_arch = "wasm32")]
            if osg_ship_api::sdk::keep_snapshot(obs.tick.snapshot).is_err() {
                return core::mem::take(&mut self.clear_pending);
            }

            self.pending = Rollout::new(nav, obs, b);

            if self.pending.is_none() {
                self.retired_snapshots.push(obs.tick.snapshot);
            }
            self.reference = Some(key);
        }
        let Some(p) = &mut self.pending else {
            return false;
        };
        // Optional forecasts leave gas for actuation and instrument publication.
        for _ in 0..512 {
            if p.done || !budget() {
                break;
            }
            p.advance();
        }
        if !p.done || !p.compress() {
            return core::mem::take(&mut self.clear_pending);
        }
        self.refreshed = self.pending.as_ref().map(|p| p.epoch);
        self.retire_result();
        let pending = self.pending.take().unwrap();
        let snapshot = pending.snapshot;
        self.result = pending.publish(obs.tick.time_s);

        if self.result.is_none() {
            self.retired_snapshots.push(snapshot);
        }
        self.clear_pending = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_ship_api::abi::Contact;
    fn synthetic() -> Rollout {
        let mut nav = Pursuit::default();
        nav.target = Some(Contact {
            id: 1,
            kind: abi::CONTACT_SHIP,
            ..Default::default()
        });
        nav.r = DVec3::X * 10_000.;
        let obs = Sample {
            flight: abi::FlightState {
                rotation: [0., 0., 0., 1.],
                mass_kg: 1000.,
                ..Default::default()
            },
            ..Default::default()
        };
        Rollout::new(
            &nav,
            &obs,
            &Bindings::new(&crate::hardware::Hardware::default()),
        )
        .unwrap()
    }
    fn powered(fuel: f64, flow: f64) -> Rollout {
        let mut p = synthetic();
        p.inertia = DMat3::IDENTITY * 100.;
        p.inverse = p.inertia.inverse();
        p.bindings.engine_axis = DVec3::X;
        p.bindings.torque_limit = 1000.;
        p.bindings.propellant_rate = flow;
        p.state.direction = DVec3::X;
        p.state.fuel = fuel;
        p.initial_fuel = fuel;
        p.force = 1000.;
        p.horizon = HORIZON_S;
        p
    }
    #[test]
    fn insufficient_fuel_never_produces_a_false_arrival_eta() {
        let mut p = powered(2., 1.);
        for _ in 0..MAX_SAMPLES {
            if p.done {
                break;
            }
            p.advance();
        }
        assert!(p.done);
        assert!(p.state.fuel >= 0.);
        assert!(p.eta.is_none());
        assert!(p.samples.windows(2).all(|w| w[1].seconds > w[0].seconds));
    }
    #[test]
    fn unreachable_target_stops_at_the_safety_horizon_without_an_eta() {
        let mut p = powered(0., 0.);
        p.state.u = -DVec3::X * 1e6;
        for _ in 0..MAX_SAMPLES {
            if p.done {
                break;
            }
            p.advance();
        }
        assert!(p.done);
        assert!((p.state.time - HORIZON_S).abs() < 1e-8);
        assert!(p.eta.is_none());
        assert!(p.state.r.is_finite() && p.state.u.is_finite());
    }
    #[test]
    fn compression_preserves_timing_on_a_straight_accelerating_path() {
        let mut p = synthetic();
        p.samples = (0..=1000)
            .map(|i| {
                let time = i as f64 * 0.1;
                Point {
                    r: DVec3::X * time * time,
                    seconds: time,
                    speed: 2. * time,
                    allowed: 0.,
                }
            })
            .collect();
        let original = p.samples.clone();
        assert!(p.compress());
        let forecast = p.publish(0.).unwrap();
        assert!(forecast.points.len() > 10 && forecast.points.len() <= 128);
        assert_eq!(forecast.points.last().unwrap().seconds, 100.);
        for point in original {
            let j = forecast
                .points
                .partition_point(|p| p.seconds < point.seconds)
                .clamp(1, forecast.points.len() - 1);
            let a = forecast.points[j - 1];
            let b = forecast.points[j];
            let predicted =
                a.r.lerp(b.r, (point.seconds - a.seconds) / (b.seconds - a.seconds));
            assert!(
                predicted.distance(point.r) < 10.1,
                "time={} error={}",
                point.seconds,
                predicted.distance(point.r)
            );
        }
    }
    #[test]
    fn delayed_publication_preserves_the_original_epoch_and_vertices() {
        let mut p = synthetic();
        p.frame_velocity = DVec3::Y * 500.;
        p.snapshot = 42;
        p.samples.push(Point {
            r: DVec3::X * 20.,
            seconds: 2.,
            speed: 0.,
            allowed: 0.,
        });
        assert!(p.compress());
        let out = p.publish(0.).unwrap();
        assert_eq!(out.epoch, 0.);
        assert_eq!(out.snapshot, 42);
        assert_eq!(out.points[0].r, DVec3::ZERO);
        assert_eq!(out.points[1].seconds, 2.);
        assert_eq!(out.points[1].r, DVec3::X * 20.);
    }
}
