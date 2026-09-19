//! Weighted time and propellant guidance with coasting, lateral correction,
//! and a braking envelope that includes attitude response.
use crate::hardware::Sample;
use crate::{Bindings, attitude};
use glam::{DMat3, DVec3};
use toy_sim_ship_api::abi::{self, Contact};

pub const CONTACT_GRACE_S: f64 = 2.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Ready,
    Pursuing,
    Paused,
}
impl Phase {
    pub fn active(self) -> bool {
        self == Self::Pursuing
    }
}

#[derive(Clone, Debug)]
pub struct Pursuit {
    pub target: Option<Contact>,
    pub preferences: toy_sim_model::travel::PlanningPreferences,
    pub visible: bool,
    pub phase: Phase,
    pub reason: String,
    pub limit: f64,
    pub throttle: f64,
    pub throttle_ceiling: f64,
    pub stand_off: f64,
    pub offset: DVec3,
    pub turn_allowance: f64,
    pub allowed_speed: f64,
    pub speed_limit: f64,
    pub stopping_distance: f64,
    /// Target minus ship position; ship minus target velocity.
    pub r: DVec3,
    pub u: DVec3,
    /// External ship minus target acceleration, excluding our own thrust.
    pub disturbance: DVec3,
    previous: Option<(f64, DVec3)>,
    last_seen: Option<f64>,
    state_time: Option<f64>,
    pub effectiveness: f64,
    pub acceleration: DVec3,
    pub pointing_error: f64,
    pub direction: DVec3,
}
impl Default for Pursuit {
    fn default() -> Self {
        Self {
            target: None,
            preferences: Default::default(),
            visible: false,
            phase: Phase::Ready,
            reason: String::new(),
            limit: 1.,
            throttle: 0.,
            throttle_ceiling: 1.,
            stand_off: 0.,
            offset: DVec3::ZERO,
            turn_allowance: 0.,
            allowed_speed: 0.,
            speed_limit: f64::INFINITY,
            stopping_distance: 0.,
            r: DVec3::ZERO,
            u: DVec3::ZERO,
            disturbance: DVec3::ZERO,
            previous: None,
            last_seen: None,
            state_time: None,
            effectiveness: 1.,
            acceleration: DVec3::ZERO,
            pointing_error: 0.,
            direction: DVec3::ZERO,
        }
    }
}

pub fn arrival_speed(distance: f64, acceleration: f64, response: f64) -> f64 {
    let delayed = acceleration * response;
    ((delayed * delayed + acceleration * distance).sqrt() - delayed).min(distance / (4. * response))
}

pub fn economical_rendezvous(
    error: DVec3,
    velocity: DVec3,
    disturbance: DVec3,
    acceleration: f64,
    response: f64,
    flow_kg_s: f64,
    cost: toy_sim_model::transfer::TransferCost,
    speed_limit: f64,
) -> (DVec3, f64) {
    let direction = error.normalize_or_zero();
    let speed = cost
        .cruise_speed(
            error.length(),
            velocity.dot(direction),
            acceleration,
            flow_kg_s,
        )
        .min(arrival_speed(error.length(), acceleration, response))
        .min(speed_limit);
    let requested = (direction * speed - velocity) / response - disturbance;
    (requested.clamp_length_max(acceleration), speed)
}

impl Pursuit {
    pub fn select(&mut self, id: u64, contacts: &[Contact], obs: &Sample) -> Result<(), String> {
        if self.phase.active() {
            return Err("Abort before changing target".into());
        }
        let c = contacts
            .iter()
            .find(|c| c.id == id && c.kind == abi::CONTACT_SHIP)
            .ok_or("Target ship is not sensor-visible")?;
        *self = Self {
            target: Some(c.clone()),
            ..Self::default()
        };
        self.observe(contacts, obs, None);
        if !self.visible {
            return Err("Invalid target telemetry".into());
        }
        Ok(())
    }
    pub fn start(&mut self, limit: f64, stand_off: f64, own_radius: f64) -> Result<(), String> {
        if self.phase.active() {
            return Err("Pursuit already engaged".into());
        }
        if !self.visible {
            return Err("Select a sensor-visible target ship".into());
        }
        if !limit.is_finite()
            || limit <= 0.
            || limit > 1.
            || !stand_off.is_finite()
            || !(0. ..=1e6).contains(&stand_off)
            || !own_radius.is_finite()
            || own_radius < 0.
        {
            return Err("Invalid throttle limit".into());
        }
        self.limit = limit;
        self.stand_off = stand_off;
        self.offset = -self.r.normalize_or_zero() * stand_off;
        self.effectiveness = 1.;
        self.phase = Phase::Pursuing;
        self.reason.clear();
        Ok(())
    }
    pub fn pause(&mut self, reason: &str) {
        self.phase = Phase::Paused;
        self.reason = reason.into();
        self.acceleration = DVec3::ZERO;
        self.throttle = 0.;
    }
    pub fn abort(&mut self) {
        self.phase = Phase::Ready;
        self.reason = "Aborted".into();
        self.acceleration = DVec3::ZERO;
        self.throttle = 0.;
    }
    pub fn observe(&mut self, contacts: &[Contact], obs: &Sample, measured: Option<DVec3>) {
        let found = self.target.as_ref().and_then(|target| {
            contacts
                .iter()
                .find(|c| c.id == target.id && c.kind == abi::CONTACT_SHIP)
        });
        let Some(c) = found else {
            self.visible = false;
            self.previous = None;
            if self.phase.active() {
                let age = self
                    .last_seen
                    .map_or(f64::INFINITY, |t| obs.tick.time_s - t);
                let dt = self
                    .state_time
                    .map_or(f64::INFINITY, |t| obs.tick.time_s - t);
                if !age.is_finite()
                    || !(0. ..=CONTACT_GRACE_S).contains(&age)
                    || !dt.is_finite()
                    || !(0. ..=CONTACT_GRACE_S).contains(&dt)
                {
                    self.pause("Target lost - re-engage after reacquisition");
                } else if let Some(a) = measured.filter(|a| a.is_finite()) {
                    // IMU reports the previous tick's delivered thrust. Keep the
                    // target's last estimated external motion, not its hidden state.
                    let old_u = self.u;
                    self.u += (a + self.disturbance) * dt;
                    self.r -= (old_u + self.u) * (0.5 * dt);
                    self.state_time = Some(obs.tick.time_s);
                    self.reason = "Target lost - pursuing last observed motion".into();
                } else {
                    self.pause("Inertial sensing unavailable");
                }
            }
            return;
        };
        let r = DVec3::from_array(c.position_m);
        let u = -DVec3::from_array(c.velocity_m_s);
        if !r.is_finite()
            || !u.is_finite()
            || !obs.tick.time_s.is_finite()
            || !c.radius_m.is_finite()
            || c.radius_m < 0.
        {
            self.visible = false;
            self.previous = None;
            if self.phase.active() {
                self.pause("Invalid navigation telemetry");
            }
            return;
        }
        if let Some((time, old_u)) = self.previous {
            let dt = obs.tick.time_s - time;
            if dt > 0.
                && dt <= 0.25
                && let Some(a) = measured.filter(|a| a.is_finite())
            {
                let sample = (u - old_u) / dt - a;
                self.disturbance += (sample - self.disturbance) * (1. - (-dt).exp());
            } else {
                self.disturbance = DVec3::ZERO;
            }
        } else {
            self.disturbance = DVec3::ZERO;
        }
        self.r = r;
        self.u = u;
        self.target = Some(c.clone());
        self.visible = true;
        self.previous = Some((obs.tick.time_s, u));
        self.last_seen = Some(obs.tick.time_s);
        self.state_time = Some(obs.tick.time_s);
        if self.phase.active() {
            self.reason.clear();
        }
    }
    pub fn tracking_remaining(&self, now: f64) -> f64 {
        self.last_seen.map_or(0., |t| {
            (CONTACT_GRACE_S - (now - t)).clamp(0., CONTACT_GRACE_S)
        })
    }
    pub fn capability(&mut self, actual: f64, previous_throttle: f64, rated: f64, dt: f64) {
        if previous_throttle > 0.05 && actual.is_finite() && rated > 0. {
            let ratio = (actual / (rated * previous_throttle)).clamp(0., 1.);
            if ratio < self.effectiveness {
                self.effectiveness = ratio;
            } else {
                self.effectiveness += (ratio - self.effectiveness) * (1. - (-dt / 2.).exp());
            }
        }
    }
    pub fn error(&self) -> DVec3 {
        self.r + self.offset
    }
    pub fn guide(&mut self, obs: &Sample, b: &Bindings, dt: f64) {
        if !self.phase.active() {
            return;
        }
        let inertia = DMat3::from_cols_array(&obs.inertia);
        let Some(q) = attitude::valid_rotation(obs.rotation) else {
            self.pause("Invalid attitude");
            return;
        };
        if !obs.mass_kg.is_finite()
            || obs.mass_kg <= 0.
            || !inertia.is_finite()
            || inertia.determinant() <= 0.
            || !DVec3::from_array(obs.angular_velocity).is_finite()
            || !dt.is_finite()
            || dt <= 0.
            || !self.r.is_finite()
            || !self.u.is_finite()
            || !self.disturbance.is_finite()
        {
            self.pause("Invalid flight telemetry");
            return;
        }
        // A continuous burn must leave enough torque to point an off-centre
        // engine. This hardware ceiling does not depend on alignment or speed.
        let moment =
            b.torquer_rotation.inverse() * b.engine_position.cross(b.engine_axis) * b.thrust;
        self.throttle_ceiling = (0..3)
            .filter(|&i| moment[i].abs() > 1e-9)
            .map(|i| 0.8 * b.torque_capacity[i] / moment[i].abs())
            .fold(self.limit.min(b.power_limit), f64::min);
        let a = b.thrust * self.effectiveness * self.throttle_ceiling / obs.mass_kg;
        if !a.is_finite() || a <= 1e-6 {
            self.pause("No usable thrust");
            return;
        }
        self.turn_allowance = attitude::turn_allowance(inertia, b);
        let error = self.error();
        let response = (2. * self.turn_allowance + 2.).max(2.);
        let (command, speed) = economical_rendezvous(
            error,
            self.u,
            self.disturbance,
            a,
            response,
            b.propellant_rate * self.throttle_ceiling,
            self.preferences.cost(obs.mass_kg),
            self.speed_limit,
        );
        self.allowed_speed = speed;
        self.stopping_distance = self.u.length_squared() / (2. * a) + self.u.length() * response;
        self.acceleration = command;
        if error.length() <= 2. && self.u.length() <= 0.5 {
            self.phase = Phase::Ready;
            self.acceleration = DVec3::ZERO;
            self.throttle = 0.;
            return;
        }
        if !self.acceleration.is_finite() {
            self.pause("Invalid guidance command");
            return;
        }
        if let Some(direction) = self.acceleration.try_normalize() {
            self.direction = direction;
        } else if self.direction == DVec3::ZERO {
            self.direction = q * b.engine_axis;
        }
        self.throttle = self.throttle_ceiling * (self.acceleration.length() / a).clamp(0., 1.);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contact(velocity: f64) -> Contact {
        Contact {
            id: 1,
            kind: abi::CONTACT_SHIP,
            position_m: [1000., 0., 0.],
            velocity_m_s: [-velocity, 0., 0.],
            ..Default::default()
        }
    }
    #[test]
    fn accelerometer_distinguishes_own_acceleration_and_gaps_reset_the_fit() {
        let mut nav = Pursuit::default();
        let mut obs = Sample::default();
        nav.select(1, &[contact(0.)], &obs).unwrap();
        obs.tick.time_s = 0.1;
        nav.observe(&[contact(0.2)], &obs, Some(DVec3::X * 2.));
        assert!(nav.disturbance.length() < 1e-12);
        obs.tick.time_s = 0.2;
        nav.observe(&[contact(0.5)], &obs, Some(DVec3::X * 2.));
        assert!((nav.disturbance.x - (1. - (-0.1f64).exp())).abs() < 1e-10);
        obs.tick.time_s = 1.;
        nav.observe(&[contact(4.)], &obs, Some(DVec3::X * 2.));
        assert_eq!(nav.disturbance, DVec3::ZERO);
        nav.observe(&[], &obs, None);
        obs.tick.time_s = 1.1;
        nav.observe(&[contact(20.)], &obs, Some(DVec3::ZERO));
        assert_eq!(nav.disturbance, DVec3::ZERO);
    }
    #[test]
    fn rendezvous_brakes_and_matches_terminal_velocity() {
        let braking = economical_rendezvous(
            DVec3::X * 100.,
            DVec3::X * 100.,
            DVec3::ZERO,
            10.,
            2.,
            1.,
            toy_sim_model::travel::PlanningPreferences::default().cost(1000.),
            f64::INFINITY,
        )
        .0;
        assert!(braking.x < 0.);
        let mut position = DVec3::ZERO;
        let mut velocity = DVec3::ZERO;
        let target = DVec3::X * 1000.;
        for _ in 0..3000 {
            let error = target - position;
            if error.length() <= 2. && velocity.length() <= 0.5 {
                break;
            }
            velocity += economical_rendezvous(
                error,
                velocity,
                DVec3::ZERO,
                10.,
                2.,
                1.,
                toy_sim_model::travel::PlanningPreferences::default().cost(1000.),
                f64::INFINITY,
            )
            .0 * 0.1;
            position += velocity * 0.1;
        }
        assert!((target - position).length() <= 2.);
        assert!(velocity.length() <= 0.5);
    }

    #[test]
    fn engagement_validates_limit_and_can_retry_hardware() {
        let mut nav = Pursuit::default();
        let obs = Sample::default();
        nav.select(1, &[contact(0.)], &obs).unwrap();
        for limit in [f64::NAN, 0., -1., 1.1] {
            assert!(nav.start(limit, 100., 5.).is_err());
        }
        nav.start(1., 100., 5.).unwrap();
        assert!(nav.select(1, &[contact(0.)], &obs).is_err());
        nav.pause("Hardware unavailable");
        nav.effectiveness = 0.;
        nav.start(1., 100., 5.).unwrap();
        assert_eq!(nav.effectiveness, 1.);
    }
    #[test]
    fn grace_extrapolates_only_observed_motion_and_does_not_extend_itself() {
        let mut nav = Pursuit::default();
        let mut obs = Sample::default();
        nav.select(1, &[contact(10.)], &obs).unwrap();
        nav.start(1., 100., 5.).unwrap();
        for i in 1..=20 {
            obs.tick.time_s = i as f64 * 0.1;
            nav.observe(&[], &obs, Some(DVec3::ZERO));
            assert!(nav.phase.active());
            assert!((nav.r.x - (1000. - obs.tick.time_s * 10.)).abs() < 1e-8);
        }
        obs.tick.time_s = 2.1;
        nav.observe(&[], &obs, Some(DVec3::ZERO));
        assert_eq!(nav.phase, Phase::Paused);
        nav.observe(&[contact(10.)], &obs, Some(DVec3::ZERO));
        assert!(nav.visible);
        assert_eq!(nav.phase, Phase::Paused);
    }
    #[test]
    fn economic_guidance_coasts_then_brakes_without_discarding_momentum() {
        let velocity = DVec3::X * 200.;
        let (coast, _) = economical_rendezvous(
            DVec3::X * 1e5,
            velocity,
            DVec3::ZERO,
            10.,
            2.,
            5.,
            toy_sim_model::transfer::TransferCost::default(),
            f64::INFINITY,
        );
        assert!(coast.length() < 1e-9);
        let (brake, _) = economical_rendezvous(
            DVec3::X * 100.,
            velocity,
            DVec3::ZERO,
            10.,
            2.,
            5.,
            toy_sim_model::transfer::TransferCost::default(),
            f64::INFINITY,
        );
        assert!(brake.x < 0.);
    }

    #[test]
    fn local_gate_speed_limit_brakes_an_inbound_ship_and_bounds_its_cruise() {
        let mut distance = 100_000.;
        let mut velocity = DVec3::Z * 120.;
        let cost = toy_sim_model::transfer::TransferCost::default();
        for _ in 0..1000 {
            let (acceleration, allowed) = economical_rendezvous(
                DVec3::Z * distance,
                velocity,
                DVec3::ZERO,
                10.,
                2.,
                1.,
                cost,
                40.,
            );
            assert!(allowed <= 40.);
            if velocity.z > 40. {
                assert!(acceleration.z < 0.);
            }
            velocity += acceleration * 0.1;
            distance -= velocity.z * 0.1;
        }
        assert!((velocity.z - 40.).abs() < 1e-6);
        assert!(distance > 90_000.);
    }
}
