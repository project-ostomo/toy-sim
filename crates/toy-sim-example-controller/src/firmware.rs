//! The bundled controller's syscall loop. Caches and scheduling belong to this firmware.
use crate::{
    Pilot,
    hardware::{Hardware, Sample},
    navigation::Phase,
};
use toy_sim_ship_api::{abi, sdk};

const OWN_PATH: u64 = 1;
const TARGET_PATH: u64 = 2;
const TARGET_MARKER: u64 = 3;
const ARRIVAL_MARKER: u64 = 4;

#[derive(Default)]
pub struct Computer {
    pilot: Pilot,
    hardware: Hardware,
    weapons: crate::weapons::WeaponsController,
}

impl Computer {
    pub fn run(&mut self) -> Result<abi::TickContext, i32> {
        let tick = sdk::tick()?;

        if !self.hardware.discover(&tick)? {
            return Ok(tick);
        }

        self.hardware.measure()?;
        let propellant_kg = match self.hardware.propellant {
            Some(resource) => sdk::resource(resource.id)?.units * resource.unit_mass_kg,
            None => 0.,
        };
        let sample = Sample {
            tick,
            flight: sdk::flight()?,
            propellant_kg,
        };
        let mut contacts = [abi::Contact::default(); abi::MAX_CONTACTS as usize];
        let count = self
            .hardware
            .devices
            .iter()
            .find(|device| device.info.kind == abi::DEVICE_SENSOR && device.available())
            .and_then(|sensor| sdk::scan(sensor.info.id, &mut contacts).ok())
            .unwrap_or(0);
        self.weapons.observe(&sample, &contacts[..count]);
        self.pilot
            .observe(&sample, &self.hardware, &contacts[..count]);

        for index in 0..tick.request_count as u32 {
            let request = sdk::request(index)?;
            let result = self.dispatch(index, request.kind, &sample);
            let (status, message) = match &result {
                Ok(()) => (abi::REPLY_ACCEPTED, ""),
                Err(message) => (abi::REPLY_REJECTED, message.as_str()),
            };
            sdk::request_reply(request.id, status, message)?;
        }

        for actuation in self.pilot.control(&sample, &self.hardware) {
            actuation.apply()?;
        }

        let weapons = self.weapons.update(&sample, &self.hardware);
        for (id, setting) in &weapons.settings {
            sdk::device_write(*id, abi::SET_WEAPON, setting)?;
        }
        for marker in &weapons.markers {
            sdk::marker(marker)?;
        }
        for id in (16 + weapons.markers.len() as u64)..24 {
            sdk::remove_spatial(id)?;
        }
        sdk::weapons(&weapons.state, &weapons.rows)?;

        self.publish(&sample)?;
        sdk::interval(0.)?;
        Ok(tick)
    }

    fn dispatch(&mut self, index: u32, kind: u64, sample: &Sample) -> Result<(), String> {
        use abi::Record;

        let failed = |_| "Could not read request payload".to_owned();
        match kind {
            abi::REQUEST_ENGAGE_WEAPONS => {
                if !self.hardware.devices.iter().any(|d| {
                    d.info.kind == abi::DEVICE_WEAPON && d.info.flags & abi::CONTROL_ENABLED != 0
                }) {
                    return Err("No weapons available for automatic control".into());
                }
                let value = sdk::request_read(index, kind).map_err(failed)?;
                self.weapons.engage(value, sample.tick.time_s)
            }
            abi::REQUEST_HOLD_FIRE => {
                self.weapons.hold_fire();
                Ok(())
            }
            abi::REQUEST_MANUAL => {
                let value: abi::ManualRequest = sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_AIM_DIRECTION => {
                let value: abi::DirectionRequest =
                    sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_AIM_CONTACT | abi::REQUEST_SELECT_TARGET => {
                let value: abi::ContactRequest = sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_ENGAGE_NAVIGATION => {
                let value: abi::NavigationRequest =
                    sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_HOLD_ATTITUDE | abi::REQUEST_STOP_GUIDANCE => {
                self.pilot.request(kind, &[], sample, &self.hardware)
            }
            _ => Err("Unsupported request".into()),
        }
    }

    fn publish(&mut self, sample: &Sample) -> Result<(), i32> {
        let pilot = &self.pilot;
        let navigation = &pilot.navigation;
        let until = sample.tick.time_s + 2.;
        let guidance = navigation.phase.active() || navigation.phase == Phase::Paused;
        sdk::attitude(&abi::AttitudeState {
            valid_until_s: until,
            mode: if guidance {
                abi::ATTITUDE_GUIDANCE
            } else if pilot.hold.is_some() {
                abi::ATTITUDE_HOLD
            } else {
                abi::ATTITUDE_MANUAL
            },
            present: if pilot.hold.is_some() {
                abi::ATTITUDE_REFERENCE
            } else {
                0
            },
            reference: pilot.hold.map_or([0.; 4], |rotation| rotation.to_array()),
            control_error: pilot.allocator.residual,
        })?;

        if pilot.forecast_changed {
            self.publish_forecast(until)?;
        }

        let forecast = pilot.prediction.result.as_ref();
        let target = navigation.target.as_ref().map_or(0, |contact| contact.id);
        let arrival = forecast.and_then(|forecast| forecast.eta_at(sample.tick.time_s));
        sdk::navigation(&abi::NavigationState {
            valid_until_s: until,
            status: match navigation.phase {
                Phase::Ready => abi::NAV_IDLE,
                Phase::Paused => abi::NAV_SUSPENDED,
                Phase::Pursuing => abi::NAV_ACTIVE,
            },
            target_contact: target,
            own_path: if forecast.is_some() { OWN_PATH } else { 0 },
            target_path: if forecast.is_some() { TARGET_PATH } else { 0 },
            present: if arrival.is_some() {
                abi::NAV_ARRIVAL
            } else {
                0
            } | if forecast.is_some() { abi::NAV_FUEL } else { 0 },
            throttle_limit: navigation.limit,
            throttle: pilot.throttle,
            // Omitted optional measurements must be zero in the instrument ABI.
            stand_off_m: 0.0,
            approach_speed_limit_m_s: 0.0,
            braking_distance_m: 0.0,
            arrival_time_s: arrival.map_or(0., |remaining| sample.tick.time_s + remaining),
            predicted_fuel_kg: forecast.map_or(0., |forecast| forecast.fuel_kg),
            reason: abi::Text256::new(&navigation.reason),
        })?;
        match sdk::contacts(&abi::ContactsState {
            valid_until_s: until,
            selected_contact: target,
        }) {
            // A missing or unpowered sensor may have no admitted scan yet.
            Ok(()) | Err(abi::ERR_UNAVAILABLE) => {}
            Err(error) => return Err(error),
        }

        if target != 0 && sample.tick.interest & abi::INTEREST_MARKERS != 0 {
            // The host resolves only this contact's admitted sensor history.
            // No high-speed displacement correction or live entity query is needed.
            let result = sdk::marker(&abi::SpatialMarker {
                meta: abi::SpatialMeta {
                    id: TARGET_MARKER,
                    role: abi::MARKER_TARGET,
                    valid_until_s: until,
                    label: abi::Text64::new("Target"),
                },
                frame: abi::SpatialFrame {
                    kind: abi::FRAME_CONTACT,
                    reference: target,
                    ..Default::default()
                },
                ..Default::default()
            });

            if result != Err(abi::ERR_UNAVAILABLE) {
                result?;
            }
        } else {
            sdk::remove_spatial(TARGET_MARKER)?;
        }

        Ok(())
    }

    fn publish_forecast(&self, until: f64) -> Result<(), i32> {
        let Some(forecast) = &self.pilot.prediction.result else {
            for id in [OWN_PATH, TARGET_PATH, ARRIVAL_MARKER] {
                sdk::remove_spatial(id)?;
            }

            return Ok(());
        };
        let frame = abi::SpatialFrame {
            kind: abi::FRAME_SNAPSHOT,
            reference: forecast.snapshot,
            origin_velocity_m_s: forecast.frame_velocity.to_array(),
        };
        let own = abi::SpatialPath {
            meta: abi::SpatialMeta {
                id: OWN_PATH,
                role: abi::PATH_OWN_FORECAST,
                valid_until_s: until,
                label: abi::Text64::new("Planned trajectory"),
            },
            frame,
            kind: abi::PATH_TIMED,
            subject_contact: 0,
        };
        let mut vertices = [abi::SpatialVertex::default(); abi::MAX_PATH_VERTICES as usize];

        for (vertex, point) in vertices.iter_mut().zip(&forecast.points) {
            *vertex = abi::SpatialVertex {
                time_s: forecast.epoch + point.seconds,
                position_m: point.r.to_array(),
            };
        }

        sdk::path(&own, &vertices[..forecast.points.len()])?;
        let target = abi::SpatialPath {
            meta: abi::SpatialMeta {
                id: TARGET_PATH,
                role: abi::PATH_CONTACT_FORECAST,
                valid_until_s: until,
                label: abi::Text64::new("Target forecast"),
            },
            frame,
            kind: abi::PATH_TIMED,
            subject_contact: forecast.target,
        };
        let last_time = vertices[forecast.points.len() - 1].time_s;
        sdk::path(
            &target,
            &[
                abi::SpatialVertex {
                    time_s: forecast.epoch,
                    position_m: forecast.target_position.to_array(),
                },
                abi::SpatialVertex {
                    time_s: last_time,
                    position_m: forecast.target_position.to_array(),
                },
            ],
        )?;

        if let Some(eta) = forecast.eta {
            sdk::marker(&abi::SpatialMarker {
                meta: abi::SpatialMeta {
                    id: ARRIVAL_MARKER,
                    role: abi::MARKER_EVENT,
                    valid_until_s: until,
                    label: abi::Text64::new("Closest approach"),
                },
                frame: abi::SpatialFrame {
                    kind: abi::FRAME_PATH,
                    reference: OWN_PATH,
                    ..Default::default()
                },
                time_mode: abi::TIME_FIXED,
                time_s: forecast.epoch + eta,
                ..Default::default()
            })?;
        } else {
            sdk::remove_spatial(ARRIVAL_MARKER)?;
        }

        Ok(())
    }
}

#[cfg(feature = "firmware")]
#[unsafe(no_mangle)]
extern "C" fn ship_api_version() -> u32 {
    abi::VERSION
}

#[cfg(feature = "firmware")]
#[unsafe(no_mangle)]
extern "C" fn ship_tick() {
    static mut COMPUTER: Option<Computer> = None;

    // The host enters one callback at a time and rejects shared Wasm memories.
    let computer = unsafe { &mut *core::ptr::addr_of_mut!(COMPUTER) };
    computer
        .get_or_insert_with(Computer::default)
        .run()
        .expect("flight computer syscall failed");
}
