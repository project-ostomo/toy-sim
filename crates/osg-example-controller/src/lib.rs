//! Standard flight computer: hardware discovery, control allocation and arrival guidance.
pub mod allocation;
mod attitude;
#[cfg(any(target_arch = "wasm32", test))]
mod budget;
pub mod navigation;
pub mod prediction;
use glam::{DMat3, DQuat, DVec3};

use navigation::{Phase, Pursuit};
use osg_ship_api::abi::{self, Contact};
#[cfg(target_arch = "wasm32")]
pub mod chatter;
pub mod hardware;
pub mod missile;
pub mod weapons;
use hardware::{Actuation, Capability, Hardware, Sample};
#[cfg(target_arch = "wasm32")]
pub mod firmware;
#[cfg(any(target_arch = "wasm32", test))]
#[path = "world/local.rs"]
mod local_guidance;
#[cfg(any(target_arch = "wasm32", test))]
#[path = "world/slip.rs"]
mod slip_guidance;
#[cfg(target_arch = "wasm32")]
mod world;

pub struct Pilot {
    devices: Option<Bindings>,
    throttle: f64,
    manual_throttle: f64,
    manual_sample: f64,
    steering: DVec3,
    hold: Option<DQuat>,
    aim: Option<DVec3>,
    contact: Option<u64>,
    pub navigation: Pursuit,
    allocator: allocation::Allocator,
    contacts: Vec<Contact>,
    prediction: prediction::Predictor,
    forecast_changed: bool,
    previous_imu: Option<(f64, DVec3)>,
    pub measured_acceleration: Option<DVec3>,
    pub delivered_thrust: f64,
}

#[derive(Clone)]
pub struct Bindings {
    sensor: Option<u64>,
    accelerometer: Option<u64>,
    thrust: f64,
    propellant_rate: f64,
    engine_power_w: f64,
    generators: std::sync::Arc<[(u64, f64)]>,
    power_limit: f64,
    engine_axis: DVec3,
    control_rotation: DQuat,
    engine_position: DVec3,
    torque_limit: f64,
    torquer_rotation: DQuat,
    imu_rotation: DQuat,
    imu_position: DVec3,
    layout: std::sync::Arc<allocation::Layout>,
    torque_capacity: DVec3,
}
impl Bindings {
    pub fn new(hardware: &Hardware) -> Self {
        let first = |kind| {
            hardware
                .devices
                .iter()
                .find(|device| {
                    device.info.flags & abi::CONTROL_ENABLED != 0 && device.info.kind == kind
                })
                .map(|device| device.info.id)
        };
        let sensor = first(abi::DEVICE_SENSOR);
        let accelerometer = first(abi::DEVICE_ACCELEROMETER);
        let computer = first(abi::DEVICE_COMPUTER);
        let rotation = |id: Option<u64>| {
            id.and_then(|id| hardware.get(id))
                .and_then(|device| attitude::valid_rotation(device.info.rotation))
                .unwrap_or(DQuat::IDENTITY)
        };
        let layout = std::sync::Arc::new(allocation::Layout::new(&hardware.devices));
        let forward = rotation(computer) * DVec3::NEG_Z;
        let (thrust, torque_capacity, moment, propellant_rate) =
            layout.authority(hardware, forward);

        Self {
            sensor,
            accelerometer,
            thrust,
            propellant_rate,
            engine_power_w: hardware
                .devices
                .iter()
                .filter_map(|device| match device.capability {
                    Capability::Engine(spec) if device.info.flags & abi::CONTROL_ENABLED != 0 => {
                        Some(spec.max_power_w)
                    }
                    Capability::Rcs(spec) if device.info.flags & abi::CONTROL_ENABLED != 0 => {
                        Some(3.0 * spec.per_axis_power_w)
                    }
                    _ => None,
                })
                .sum(),
            generators: hardware
                .devices
                .iter()
                .filter_map(|device| match device.capability {
                    Capability::Generator(spec) => Some((device.info.id, spec.max_power_w)),
                    _ => None,
                })
                .collect(),
            power_limit: 1.,
            engine_axis: forward,
            control_rotation: rotation(computer),
            engine_position: forward.cross(moment) / thrust.max(1.),
            torque_limit: torque_capacity.min_element(),
            torquer_rotation: DQuat::IDENTITY,
            imu_rotation: rotation(accelerometer),
            imu_position: accelerometer
                .and_then(|id| hardware.get(id))
                .map_or(DVec3::ZERO, |device| {
                    DVec3::from_array(device.info.position_m)
                }),
            layout,
            torque_capacity,
        }
    }

    fn refresh(&mut self, devices: &Hardware) {
        let (thrust, torque, moment, propellant) = self.layout.authority(devices, self.engine_axis);
        self.thrust = thrust;
        self.torque_capacity = torque;
        self.torque_limit = torque.min_element();
        self.propellant_rate = propellant;
        // A sustainable continuous burn avoids sequential engine brownouts and
        // their uncommanded lever-arm torque. Reserve 10% for avionics/steering.
        // Generator readings can be zero with a full battery, so use available
        // nameplate power. Battery-only designs retain their finite stored burn.
        self.power_limit = if self.generators.is_empty() || self.engine_power_w <= 0. {
            1.
        } else {
            (0.9 * self
                .generators
                .iter()
                .filter(|(h, _)| allocation::Layout::available(devices, *h))
                .map(|(_, w)| w)
                .sum::<f64>()
                / self.engine_power_w)
                .clamp(0., 1.)
        };
        // Equivalent lever perpendicular to the nominal burn direction, for the continuous thrust ceiling.
        self.engine_position = self.engine_axis.cross(moment) / thrust.max(1.);
    }
    fn available(&self, devices: &Hardware) -> bool {
        self.thrust > 0.
            && self.torque_limit > 0.
            && [self.sensor, self.accelerometer]
                .iter()
                .all(|h| h.is_some_and(|h| allocation::Layout::available(devices, h)))
    }
}
impl Default for Pilot {
    fn default() -> Self {
        Self {
            devices: None,
            throttle: 0.,
            manual_throttle: 0.,
            manual_sample: 0.,
            steering: DVec3::ZERO,
            hold: None,
            aim: None,
            contact: None,
            navigation: Pursuit::default(),
            allocator: Default::default(),
            contacts: vec![],
            prediction: Default::default(),
            forecast_changed: false,
            previous_imu: None,
            measured_acceleration: None,
            delivered_thrust: 0.,
        }
    }
}
impl Pilot {
    pub fn forecast(&self) -> Option<&prediction::Forecast> {
        self.prediction.result.as_ref()
    }

    pub fn forecast_changed(&self) -> bool {
        self.forecast_changed
    }

    fn reset_navigation_reference(&mut self) {
        self.navigation = Pursuit::default();
        self.prediction.reset();
        self.forecast_changed = true;
        self.previous_imu = None;
        self.measured_acceleration = None;
        self.throttle = 0.;
    }

    /// Convert a mounted accelerometer to an estimated COM measurement. Rotation
    /// compensation is computed by the guest from gyro differences, not supplied
    /// as a perfect delta-v by the host. Missed samples reset the disturbance fit.
    fn measure(&mut self, obs: &Sample, devices: &Hardware, b: &Bindings) {
        let q = attitude::valid_rotation(obs.rotation);
        let omega = DVec3::from_array(obs.angular_velocity);
        let sample = b
            .accelerometer
            .and_then(|h| devices.get(h))
            .filter(|device| device.available())
            .map(|device| device.accelerometer)
            .filter(|reading| reading.sample_present != 0);
        self.measured_acceleration = None;
        if let (Some(q), Some(sample)) = (q, sample) {
            let age = obs.tick.time_s - sample.sample_time_s;
            let local = DVec3::from_array(sample.acceleration);
            if age >= -1e-6 && age <= 0.15 && local.is_finite() && omega.is_finite() {
                let alpha_world = self
                    .previous_imu
                    .and_then(|(t, w)| {
                        let dt = sample.sample_time_s - t;
                        (dt > 0. && dt <= 0.25).then(|| (omega - w) / dt)
                    })
                    .unwrap_or(DVec3::ZERO);
                let w = q.inverse() * omega;
                let measured = b.imu_rotation * local
                    - (q.inverse() * alpha_world).cross(b.imu_position)
                    - w.cross(w.cross(b.imu_position));
                self.measured_acceleration = Some(q * measured);
                self.previous_imu = Some((sample.sample_time_s, omega));
            } else {
                self.previous_imu = None;
            }
        } else {
            self.previous_imu = None;
        }
    }
    /// Refresh measured state before processing this callback's user requests.
    pub fn observe(&mut self, obs: &Sample, hardware: &Hardware, contacts: &[Contact]) {
        if self.devices.is_none() {
            self.devices = Some(Bindings::new(hardware));
        }

        self.devices.as_mut().unwrap().refresh(hardware);
        let b = self.devices.as_ref().unwrap().clone();
        self.contacts.clear();
        self.contacts.extend_from_slice(contacts);
        self.measure(obs, hardware, &b);
        self.navigation
            .observe(&self.contacts, obs, self.measured_acceleration);
        self.delivered_thrust = b
            .layout
            .columns
            .iter()
            .map(|c| {
                let force = if let Some(axis) = c.axis {
                    hardware
                        .rcs_readings
                        .get(&c.handle)
                        .map_or(0., |r| r.thrust_n[axis])
                } else {
                    hardware.get(c.handle).map_or(0., |device| device.thrust_n)
                };
                force * c.force.dot(b.engine_axis) / c.rating
            })
            .sum();
        self.navigation.capability(
            self.delivered_thrust,
            self.throttle,
            b.thrust,
            obs.tick.dt_s.clamp(0., 2.),
        );
    }

    pub fn control(&mut self, obs: &Sample, hardware: &Hardware) -> Vec<Actuation> {
        let b = self
            .devices
            .as_ref()
            .expect("observe before control")
            .clone();
        let q = attitude::valid_rotation(obs.rotation).unwrap_or(DQuat::IDENTITY);
        let mut out = Vec::new();
        if self.navigation.phase.active()
            && (!b.available(hardware) || self.measured_acceleration.is_none())
        {
            self.navigation
                .pause("Required hardware or accelerometer unavailable");
        }
        let was_active = self.navigation.phase.active();
        self.navigation.guide(obs, &b, obs.tick.dt_s);
        let omega = DVec3::from_array(obs.angular_velocity);
        let inertia = DMat3::from_cols_array(&obs.inertia);
        let navigation_owns = was_active || matches!(self.navigation.phase, Phase::Paused);
        let direction = if self.navigation.phase.active() {
            self.navigation.direction.try_normalize()
        } else if !navigation_owns {
            self.contact
                .and_then(|id| self.contacts.iter().find(|c| c.id == id))
                .and_then(|c| DVec3::from_array(c.position_m).try_normalize())
                .or(self.aim)
        } else {
            None
        };
        if was_active && !self.navigation.phase.active() {
            self.hold = Some(q);
        }
        if let Some(dir) = direction {
            self.hold = Some(attitude::point(q, b.engine_axis, dir));
            self.navigation.pointing_error = attitude::error_angle(q, b.engine_axis, dir);
        }
        self.throttle = if self.navigation.phase.active() {
            // Arrival guidance sets variable throttle and coasts through large turns.
            let alignment = attitude::thrust_alignment(q, b.engine_axis, self.navigation.direction);
            self.navigation.throttle * if alignment > 0.995 { alignment } else { 0. }
        } else if navigation_owns {
            0.
        } else {
            self.manual_throttle
        };
        let mut torque = (b.control_rotation * self.steering) * b.torque_capacity;
        if let Some(target) = self.hold {
            torque = attitude::torque(q, omega, inertia, target, &b);
        } else {
            torque = b.torquer_rotation.inverse() * torque;
        }
        if !torque.is_finite() {
            torque = DVec3::ZERO;
        }
        self.allocator.solve(
            &b.layout,
            hardware,
            b.engine_axis * (self.throttle * b.thrust),
            torque,
            if self.navigation.phase.active() {
                self.navigation.throttle_ceiling
            } else if navigation_owns {
                0.
            } else {
                1.
            },
            &mut out,
        );
        let requested = obs.tick.interest & abi::INTEREST_PATHS != 0;
        self.forecast_changed = self.prediction.update(requested, &self.navigation, obs, &b);
        out
    }

    fn cancel(&mut self, q: DQuat) {
        self.navigation.abort();
        self.prediction.reset();
        self.throttle = 0.;
        self.manual_throttle = 0.;
        self.hold = Some(q);
        self.aim = None;
        self.contact = None;
    }
    pub fn request(
        &mut self,
        kind: u64,
        payload: &[u8],
        obs: &Sample,
        hardware: &Hardware,
    ) -> Result<(), String> {
        use abi::Record;

        let q = attitude::valid_rotation(obs.rotation).unwrap_or(DQuat::IDENTITY);
        let b = self
            .devices
            .as_ref()
            .ok_or("Hardware discovery is incomplete")?
            .clone();
        let result: Result<(), String> = (|| {
            match kind {
                abi::REQUEST_THROTTLE => {
                    let bytes: [u8; 8] =
                        payload.try_into().map_err(|_| "Invalid throttle request")?;
                    let throttle = f64::from_le_bytes(bytes);
                    if !throttle.is_finite() || !(0.0..=1.0).contains(&throttle) {
                        return Err("Invalid throttle".into());
                    }
                    self.manual_throttle = throttle;
                    self.manual_sample = throttle;
                    self.steering = DVec3::ZERO;
                }
                abi::REQUEST_MANUAL => {
                    let abi::ManualRequest { throttle, steering } =
                        abi::ManualRequest::read(payload).ok_or("Invalid manual request")?;
                    if !throttle.is_finite() || !steering.iter().all(|v| v.is_finite()) {
                        return Err("Invalid manual command".into());
                    }
                    let steer = DVec3::from_array(steering).clamp(DVec3::NEG_ONE, DVec3::ONE);
                    let changed =
                        (throttle - self.manual_sample).abs() > 1e-9 || steer != DVec3::ZERO;
                    self.manual_sample = throttle;
                    if !changed
                        && (self.navigation.phase.active()
                            || matches!(self.navigation.phase, Phase::Paused))
                    {
                        return Ok(());
                    }
                    if changed
                        && (self.navigation.phase.active()
                            || matches!(self.navigation.phase, Phase::Paused))
                    {
                        self.cancel(q);
                        self.hold = None;
                    }
                    self.manual_throttle = throttle.clamp(0., 1.);
                    self.steering = steer;
                }
                abi::REQUEST_HOLD_ATTITUDE => {
                    if self.navigation.phase != Phase::Ready {
                        self.cancel(q);
                    }
                    self.hold = Some(q);
                    self.aim = None;
                    self.contact = None;
                }
                abi::REQUEST_STOP_GUIDANCE => {
                    if self.navigation.phase.active()
                        || matches!(self.navigation.phase, Phase::Paused)
                    {
                        self.cancel(q);
                    }
                    self.hold = None;
                    self.aim = None;
                    self.contact = None;
                }
                abi::REQUEST_AIM_DIRECTION => {
                    let v = abi::DirectionRequest::read(payload)
                        .ok_or("Invalid direction request")?
                        .direction;
                    let dir = DVec3::from_array(v)
                        .try_normalize()
                        .ok_or("Invalid direction")?;
                    let throttle = self.manual_throttle;
                    self.cancel(q);
                    self.manual_throttle = throttle;
                    self.manual_sample = throttle;
                    self.steering = DVec3::ZERO;
                    self.aim = Some(dir);
                    self.contact = None;
                    self.hold = None;
                }
                abi::REQUEST_AIM_CONTACT => {
                    let id = abi::ContactRequest::read(payload)
                        .ok_or("Invalid contact request")?
                        .contact;
                    if !self.contacts.iter().any(|c| c.id == id) {
                        return Err("Contact is not sensor-visible".into());
                    }
                    if self.navigation.phase != Phase::Ready {
                        self.cancel(q);
                    }
                    self.contact = Some(id);
                    self.aim = None;
                    self.hold = None;
                }
                abi::REQUEST_SELECT_TARGET => {
                    let id = abi::ContactRequest::read(payload)
                        .ok_or("Invalid target request")?
                        .contact;
                    self.navigation.select(id, &self.contacts, obs)?;
                }
                abi::REQUEST_ENGAGE_NAVIGATION => {
                    let abi::NavigationRequest {
                        throttle_limit,
                        stand_off_m,
                    } = abi::NavigationRequest::read(payload)
                        .ok_or("Invalid navigation request")?;
                    if !b.available(hardware) || self.measured_acceleration.is_none() {
                        return Err(
                        "Need usable thrust, steering authority, contact sensor and inertial sensing".into(),
                    );
                    }
                    self.navigation
                        .start(throttle_limit, stand_off_m, obs.radius_m)?;
                    self.prediction.reset();
                    self.aim = None;
                    self.contact = None;
                    self.hold = Some(q);
                    self.manual_throttle = 0.;
                    self.steering = DVec3::ZERO;
                }
                _ => return Err("Unsupported request".into()),
            }

            Ok(())
        })();

        if let Err(ref error) = result {
            self.navigation.reason = error.clone();
        }

        result
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    #[test]
    fn reused_navigation_contact_does_not_carry_motion_across_orders() {
        let mut pilot = Pilot::default();
        let mut sample = Sample::default();
        let old = Contact {
            id: u64::MAX,
            kind: abi::CONTACT_SHIP,
            position_m: [1000., 0., 0.],
            velocity_m_s: [50., 0., 0.],
            ..Default::default()
        };
        pilot.navigation.select(u64::MAX, &[old], &sample).unwrap();
        pilot.navigation.start(1., 0., 1.).unwrap();
        pilot.navigation.disturbance = DVec3::splat(1e6);
        pilot.prediction.result = Some(prediction::Forecast {
            points: Vec::new(),
            epoch: 0.,
            published_at: 0.,
            snapshot: 9,
            target: u64::MAX,
            target_position: DVec3::X * 1000.,
            frame_velocity: DVec3::ZERO,
            eta: Some(20.),
            fuel_kg: 1.,
        });
        sample.tick.time_s = 0.1;
        let new = Contact {
            id: u64::MAX,
            kind: abi::CONTACT_SHIP,
            position_m: [0., 1e7, 0.],
            velocity_m_s: [0., 30_000., 0.],
            ..Default::default()
        };
        pilot.reset_navigation_reference();
        pilot.navigation.select(u64::MAX, &[new], &sample).unwrap();
        assert_eq!(pilot.navigation.disturbance, DVec3::ZERO);
        assert_eq!(pilot.navigation.u, DVec3::NEG_Y * 30_000.);
        assert!(pilot.prediction.result.is_none());
    }
}
