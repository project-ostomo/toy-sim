//! Fuel-constrained intercept guidance with coasting, lateral correction,
//! and a braking envelope that includes attitude response.
use crate::hardware::Sample;
use crate::{Bindings, attitude};
use glam::{DMat3, DVec3};
use osg_ship_api::abi::{self, Contact};

pub const OBSERVATION_LEASE_S: f64 = 2.;

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
    pub effectiveness: f64,
    pub acceleration: DVec3,
    pub pointing_error: f64,
    braking: bool,
    pub direction: DVec3,
    intercept: crate::rendezvous::Search,
}
impl Default for Pursuit {
    fn default() -> Self {
        Self {
            target: None,
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
            effectiveness: 1.,
            acceleration: DVec3::ZERO,
            pointing_error: 0.,
            braking: false,
            direction: DVec3::ZERO,
            intercept: Default::default(),
        }
    }
}

pub fn arrival_speed(distance: f64, acceleration: f64, turn_time: f64) -> f64 {
    let braking = 0.9 * acceleration.max(0.);
    let delayed = braking * turn_time.max(0.);
    ((delayed * delayed + 2. * braking * distance.max(0.)).sqrt() - delayed).max(0.)
}

#[cfg(test)]
fn economical_rendezvous(
    error: DVec3,
    velocity: DVec3,
    disturbance: DVec3,
    acceleration: f64,
    turn_time: f64,
    flow_kg_s: f64,
    cost: osg_model::transfer::TransferCost,
    speed_limit: f64,
) -> (DVec3, f64) {
    let mut braking = false;
    burn_guidance(
        error,
        velocity,
        disturbance,
        acceleration,
        turn_time,
        flow_kg_s,
        cost,
        speed_limit,
        0.1,
        &mut braking,
    )
}

fn burn_guidance(
    error: DVec3,
    velocity: DVec3,
    disturbance: DVec3,
    acceleration: f64,
    turn_time: f64,
    flow_kg_s: f64,
    cost: osg_model::transfer::TransferCost,
    speed_limit: f64,
    dt: f64,
    braking: &mut bool,
) -> (DVec3, f64) {
    let distance = error.length();
    let direction = error.normalize_or_zero();
    let closing = velocity.dot(direction);
    let lateral = velocity - direction * closing;
    let brake_a = 0.9 * acceleration;
    let stopping = velocity.length_squared() / (2. * brake_a) + closing.max(0.) * (turn_time + dt);
    if closing < -0.5 || (velocity.length() < 0.5 && distance > 2.) {
        *braking = false;
    }
    if closing > 0. && distance <= stopping {
        *braking = true;
    }
    let cruise = cost
        .cruise_speed(distance, closing, acceleration, flow_kg_s)
        .min(speed_limit);
    let response = 0.5;
    let command = if distance <= 2. && velocity.length() <= acceleration * response {
        (error * 0.5 - velocity) / response - disturbance
    } else if *braking {
        let envelope = (2. * brake_a * distance).sqrt();
        let speed = envelope.min(cruise);
        let feedforward = if envelope <= cruise { -brake_a } else { 0. };
        let radial = feedforward + (speed - closing) / response;
        direction * radial.min(0.) - lateral / response - disturbance
    } else {
        direction * ((cruise - closing) / response).clamp(-acceleration, acceleration)
            - lateral / response
            - disturbance
    };
    (command.clamp_length_max(acceleration), cruise)
}

impl Pursuit {
    pub fn intercept_estimate(&self) -> Option<(f64, f64)> {
        self.intercept.estimate()
    }

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
        self.braking = false;
        self.phase = Phase::Pursuing;
        self.intercept = Default::default();
        self.reason.clear();
        Ok(())
    }
    pub fn pause(&mut self, reason: &str) {
        self.braking = false;
        self.phase = Phase::Paused;
        self.reason = reason.into();
        self.acceleration = DVec3::ZERO;
        self.throttle = 0.;
    }
    pub fn abort(&mut self) {
        self.braking = false;
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
                self.pause("Target lost - re-engage after reacquisition");
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
        if self.phase.active() {
            self.reason.clear();
        }
    }
    pub fn tracking_remaining(&self, now: f64) -> f64 {
        if !self.visible {
            return 0.;
        }
        self.last_seen.map_or(0., |t| {
            (OBSERVATION_LEASE_S - (now - t)).clamp(0., OBSERVATION_LEASE_S)
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
        let braking_direction = -self.u.normalize_or_zero();
        let angle = attitude::error_angle(q, b.engine_axis, braking_direction);
        let turn_time = if self.u.length() > 0.5 {
            self.turn_allowance * (angle / core::f64::consts::PI).sqrt()
        } else {
            0.
        };
        let flow = b.propellant_rate * self.throttle_ceiling;
        let exhaust = a * obs.mass_kg / flow.max(1e-12);
        let delta_v = exhaust * (obs.mass_kg / (obs.mass_kg - obs.propellant_kg).max(1.)).ln();
        // Reserve velocity matching before allowing further acceleration.
        let fuel_speed = ((delta_v * 0.98 - self.u.length()).max(0.) * 0.5
            + self.u.dot(error.normalize_or_zero()).max(0.))
        .max(0.);
        let (feedback, speed) = burn_guidance(
            error,
            self.u,
            self.disturbance,
            a,
            turn_time,
            flow,
            osg_model::transfer::TransferCost { seconds_per_kg: 0. },
            self.speed_limit.min(fuel_speed),
            dt,
            &mut self.braking,
        );
        let planned = self.intercept.update(
            crate::rendezvous::State {
                error,
                velocity: self.u,
                gravity: self.disturbance,
                acceleration: a,
                mass: obs.mass_kg,
                flow,
                fuel: obs.propellant_kg,
                turn: self.turn_allowance,
            },
            dt,
        );
        let command = if error.length() < 100. {
            // Allow time to rotate between terminal corrections.
            let response = (2. * self.turn_allowance + 1.).max(2.);
            (error / response.powi(2) - self.u * (2. / response) - self.disturbance)
                .clamp_length_max(a)
        } else if self.braking || self.u.length() > self.speed_limit {
            feedback
        } else {
            planned.unwrap_or_else(|| {
                // While searching for a fuel-feasible intercept, retain momentum
                // and correct drift without starting an unbudgeted burn.
                if feedback.dot(self.u) < 0. {
                    feedback
                } else {
                    DVec3::ZERO
                }
            })
        };
        self.allowed_speed = speed;
        self.stopping_distance =
            self.u.length_squared() / (2. * 0.9 * a) + self.u.length() * (turn_time + dt);
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
        } else if let Some(direction) = self.intercept.braking_direction() {
            self.direction = direction;
        } else if self.u.length() > 0.5 {
            self.direction = -self.u.normalize();
        } else if self.direction == DVec3::ZERO {
            self.direction = q * b.engine_axis;
        }
        self.throttle = self.throttle_ceiling * (self.acceleration.length() / a).clamp(0., 1.);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_intercept_flips_brakes_and_finishes_with_limited_fuel() {
        for fuel in [500., 25.] {
            let mut bindings = Bindings::new(&crate::hardware::Hardware::default());
            bindings.thrust = 10_000.;
            bindings.propellant_rate = 1.;
            bindings.engine_axis = DVec3::X;
            bindings.torque_limit = 1000.;
            bindings.torque_capacity = DVec3::splat(1000.);
            let mut obs = Sample::default();
            obs.flight.rotation = [0., 0., 0., 1.];
            obs.flight.mass_kg = 1000.;
            obs.flight.inertia = (DMat3::IDENTITY * 100.).to_cols_array();
            obs.propellant_kg = fuel;
            let mut nav = Pursuit::default();
            nav.phase = Phase::Pursuing;
            nav.r = DVec3::new(10_000., 2_000., 0.);
            nav.u = DVec3::new(-10., 5., 0.);
            let mut braking_ticks = 0;
            let mut elapsed = 0.;
            for _ in 0..12_000 {
                if !nav.phase.active() {
                    break;
                }
                nav.guide(&obs, &bindings, 0.1);
                let q = glam::DQuat::from_array(obs.rotation);
                let target = attitude::point(q, bindings.engine_axis, nav.direction);
                let angle = q.angle_between(target);
                let q = q.slerp(target, (0.1 / angle.max(0.1)).min(1.));
                obs.flight.rotation = q.to_array();
                let throttle = if (q * DVec3::X).dot(nav.direction) > 0.995 {
                    nav.throttle
                } else {
                    0.
                };
                let acceleration = q * DVec3::X * (bindings.thrust * throttle / obs.mass_kg);
                if acceleration.dot(nav.u) < 0. {
                    braking_ticks += 1;
                }
                nav.r -= nav.u * 0.1 + acceleration * 0.005;
                nav.u += acceleration * 0.1;
                obs.propellant_kg -= throttle * 0.1;
                obs.flight.mass_kg -= throttle * 0.1;
                elapsed += 0.1;
                assert!(obs.propellant_kg >= 0., "burn consumed braking reserve");
            }
            assert_eq!(
                nav.phase,
                Phase::Ready,
                "fuel {fuel}, elapsed {elapsed}, r {:?}, v {:?}",
                nav.r,
                nav.u
            );
            assert!(nav.r.length() < 3. && nav.u.length() < 0.6);
            assert!(braking_ticks > 10);
            if fuel > 100. {
                assert!(elapsed < 110., "arrival {elapsed}");
            }
        }
    }

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
            osg_model::transfer::TransferCost::default(),
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
                osg_model::transfer::TransferCost::default(),
                f64::INFINITY,
            )
            .0 * 0.1;
            position += velocity * 0.1;
        }
        assert!((target - position).length() <= 2.);
        assert!(velocity.length() <= 0.5);
    }

    #[test]
    fn fastest_transfer_burns_hard_and_brakes_with_reserve() {
        let mut position = DVec3::ZERO;
        let mut velocity = DVec3::ZERO;
        let target = DVec3::X * 10_000.;
        let mut braking = false;
        let mut peak_speed: f64 = 0.;
        let mut reserved_braking_ticks = 0;
        let mut elapsed = 0.;
        for _ in 0..900 {
            let error = target - position;
            if error.length() <= 2. && velocity.length() <= 0.5 {
                break;
            }
            let (command, _) = burn_guidance(
                error,
                velocity,
                DVec3::ZERO,
                10.,
                0.,
                1.,
                osg_model::transfer::TransferCost { seconds_per_kg: 0. },
                f64::INFINITY,
                0.1,
                &mut braking,
            );
            if command.x < -8. && command.x > -9.5 {
                reserved_braking_ticks += 1;
            }
            position += velocity * 0.1 + command * 0.005;
            velocity += command * 0.1;
            peak_speed = peak_speed.max(velocity.length());
            elapsed += 0.1;
        }
        assert!((target - position).length() <= 2.);
        assert!(velocity.length() <= 0.5);
        assert!(peak_speed > 280.);
        assert!(reserved_braking_ticks > 100);
        assert!(elapsed < 80.);
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
    fn losing_detection_pauses_guidance_until_explicit_reengagement() {
        let mut nav = Pursuit::default();
        let mut obs = Sample::default();
        nav.select(1, &[contact(10.)], &obs).unwrap();
        nav.start(1., 100., 5.).unwrap();

        obs.tick.time_s = 0.1;
        nav.observe(&[], &obs, Some(DVec3::ZERO));
        assert_eq!(nav.phase, Phase::Paused);
        assert!(!nav.visible);
        assert_eq!(nav.throttle, 0.);
        assert_eq!(nav.tracking_remaining(obs.tick.time_s), 0.);

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
            osg_model::transfer::TransferCost::default(),
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
            osg_model::transfer::TransferCost::default(),
            f64::INFINITY,
        );
        assert!(brake.x < 0.);
    }

    #[test]
    fn local_gate_speed_limit_brakes_an_inbound_ship_and_bounds_its_cruise() {
        let mut distance = 100_000.;
        let mut velocity = DVec3::Z * 120.;
        let cost = osg_model::transfer::TransferCost::default();
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
