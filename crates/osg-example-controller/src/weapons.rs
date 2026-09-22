//! Engagement policy belongs to the bundled firmware, independently of flight control.
use crate::hardware::{Capability, Hardware, Sample};
use glam::{DQuat, DVec3};
use osg_ship_api::abi;

#[derive(Default)]
pub struct WeaponsController {
    target: u64,
    firing: bool,
    maximum_flight_time: f64,
    contacts: Vec<abi::Contact>,
    last_seen: f64,
    reason: String,
}

pub struct WeaponOutput {
    pub settings: Vec<(u64, abi::WeaponSetting)>,
    pub state: abi::WeaponsState,
    pub rows: Vec<abi::WeaponInstrument>,
    pub markers: Vec<abi::SpatialMarker>,
}

/// Earliest constant-velocity intercept, using stable quadratic roots.
pub fn intercept(r: DVec3, v: DVec3, speed: f64, limit: f64) -> Option<(DVec3, f64)> {
    if !r.is_finite()
        || !v.is_finite()
        || !speed.is_finite()
        || speed <= 0.0
        || r.length_squared() < 1e-12
    {
        return None;
    }
    let a = v.length_squared() - speed * speed;
    let b = 2.0 * r.dot(v);
    let c = r.length_squared();
    let time = if a.abs() < 1e-10 * speed * speed {
        if b >= 0.0 {
            return None;
        }
        -c / b
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            return None;
        }
        let q = -0.5 * (b + discriminant.sqrt().copysign(b));
        [q / a, c / q]
            .into_iter()
            .filter(|t| t.is_finite() && *t > 0.0)
            .min_by(f64::total_cmp)?
    };
    if time > limit || !time.is_finite() {
        return None;
    }
    Some(((r + v * time).normalize(), time))
}

impl WeaponsController {
    pub fn observe(&mut self, sample: &Sample, contacts: &[abi::Contact]) {
        self.contacts.clear();
        self.contacts.extend_from_slice(contacts);
        if self.contacts.iter().any(|c| c.id == self.target) {
            self.last_seen = sample.tick.time_s;
        }
    }

    pub fn mark_target(&mut self, request: abi::MarkTargetRequest, now: f64) -> Result<(), String> {
        if !request.maximum_flight_time_s.is_finite()
            || !(0.01..=60.0).contains(&request.maximum_flight_time_s)
        {
            return Err("Invalid projectile flight time".into());
        }
        if !self
            .contacts
            .iter()
            .any(|c| c.id == request.contact && c.kind == abi::CONTACT_SHIP)
        {
            return Err("Target ship is not sensor-visible".into());
        }
        self.firing = false;
        self.target = request.contact;
        self.maximum_flight_time = request.maximum_flight_time_s;
        self.last_seen = now;
        self.reason.clear();
        Ok(())
    }

    pub fn start_firing(&mut self) -> Result<(), String> {
        if self.target == 0 {
            return Err("No marked target".into());
        }
        self.firing = true;
        Ok(())
    }

    pub fn stop_firing(&mut self) {
        self.firing = false;
    }

    pub fn unmark_target(&mut self) {
        self.firing = false;
        self.target = 0;
        self.reason.clear();
    }

    pub fn update(&mut self, sample: &Sample, hardware: &Hardware) -> WeaponOutput {
        let now = sample.tick.time_s;
        let dt = sample.tick.physics_dt_s;
        if self.target != 0
            && !self
                .contacts
                .iter()
                .any(|contact| contact.id == self.target)
        {
            self.unmark_target();
            self.reason = "Target lost".into();
        }
        let contact = self.contacts.iter().find(|c| c.id == self.target);
        let mut settings = Vec::new();
        let mut rows = Vec::new();
        let mut markers = Vec::new();
        let rotation = DQuat::from_array(sample.flight.rotation);
        let angular = DVec3::from_array(sample.flight.angular_velocity);
        let mut ready = false;

        for device in &hardware.devices {
            let Capability::Weapon(spec) = device.capability else {
                continue;
            };
            if device.info.flags & abi::CONTROL_ENABLED == 0 {
                continue;
            }
            let reading = hardware
                .weapon_readings
                .get(&device.info.id)
                .copied()
                .unwrap_or_default();
            let mount = rotation * DQuat::from_array(device.info.rotation);
            let barrel = mount
                * DQuat::from_rotation_y(reading.yaw_rad)
                * DQuat::from_rotation_x(reading.pitch_rad);
            let muzzle = rotation * DVec3::from_array(device.info.position_m)
                + mount * DVec3::from_array(spec.pivot_device_m)
                + barrel * DVec3::from_array(spec.muzzle_offset_m);
            let bore = barrel * DVec3::NEG_Z;
            let solution = contact.and_then(|c| {
                if spec.beam_power_w > 0.0 {
                    let offset = DVec3::from_array(c.position_m) - muzzle;
                    return (offset.length() <= spec.beam_range_m)
                        .then(|| (offset.normalize(), 0.0));
                }
                intercept(
                    DVec3::from_array(c.position_m) - muzzle,
                    DVec3::from_array(c.velocity_m_s) - angular.cross(muzzle),
                    spec.muzzle_speed_m_s,
                    self.maximum_flight_time,
                )
            });
            let (direction, flight_time) = solution.unwrap_or((bore, 0.0));
            let error = bore.angle_between(direction);
            let tolerance = contact.map_or(0.0, |c| {
                (0.5 * c.radius_m / DVec3::from_array(c.position_m).length()
                    - spec.dispersion_half_angle_rad)
                    .clamp(0.0, 0.005)
            });
            let next = contact.and_then(|c| {
                if spec.beam_power_w > 0.0 {
                    let offset = DVec3::from_array(c.position_m)
                        + DVec3::from_array(c.velocity_m_s) * dt
                        - muzzle;
                    return Some((offset.normalize(), 0.0));
                }
                intercept(
                    DVec3::from_array(c.position_m) + DVec3::from_array(c.velocity_m_s) * dt
                        - muzzle,
                    DVec3::from_array(c.velocity_m_s) - angular.cross(muzzle),
                    spec.muzzle_speed_m_s,
                    self.maximum_flight_time,
                )
            });
            let rate = next.map_or(DVec3::ZERO, |(next, _)| {
                direction.cross(next) / dt.max(1e-6)
            });
            let available = solution.is_some()
                && tolerance > 0.0
                && device.available()
                && reading.ammunition_units > 0
                && reading.battery_energy_j as f64 >= reading.shot_energy_j
                && reading.inhibit_flags & abi::WEAPON_PROPELLANT == 0;
            ready |= available && error <= tolerance;
            settings.push((
                device.info.id,
                abi::WeaponSetting {
                    aim_direction: direction.to_array(),
                    aim_angular_velocity_rad_s: rate.to_array(),
                    maximum_pointing_error_rad: tolerance,
                    valid_until_s: now + dt,
                    trigger: u64::from(available && self.firing),
                },
            ));
            let mut marker_id = 0;
            if solution.is_some()
                && markers.len() < 8
                && sample.tick.interest & abi::INTEREST_MARKERS != 0
            {
                marker_id = 16 + markers.len() as u64;
                markers.push(abi::SpatialMarker {
                    meta: abi::SpatialMeta {
                        id: marker_id,
                        role: abi::MARKER_AIM,
                        valid_until_s: now + 2.0,
                        label: abi::Text64::new("Weapon aim"),
                    },
                    frame: abi::SpatialFrame {
                        kind: abi::FRAME_SHIP,
                        ..Default::default()
                    },
                    offset_m: (muzzle + direction * (spec.muzzle_speed_m_s * flight_time))
                        .to_array(),
                    ..Default::default()
                });
            }
            rows.push(abi::WeaponInstrument {
                device: device.info.id,
                aim_marker: marker_id,
                solution_flags: u64::from(solution.is_some()),
                time_of_flight_s: flight_time,
                pointing_error_rad: error,
                reading,
            });
        }

        let reason = if self.target == 0 {
            self.reason.as_str()
        } else if contact.is_none() {
            "Reacquiring target"
        } else if !self.firing {
            "Target marked"
        } else if ready {
            "Firing"
        } else {
            "Tracking"
        };
        WeaponOutput {
            settings,
            rows,
            markers,
            state: abi::WeaponsState {
                valid_until_s: now + 2.0,
                mode: if !self.firing {
                    abi::WEAPONS_HOLD
                } else {
                    abi::WEAPONS_FIRING
                },
                target_contact: self.target,
                reason: abi::Text256::new(reason),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_and_firing_are_independent_and_unmark_is_safe() {
        let mut controller = WeaponsController::default();
        let sample = Sample {
            tick: abi::TickContext {
                physics_dt_s: osg_model::TICK_SECONDS,
                ..Default::default()
            },
            flight: abi::FlightState {
                rotation: [0., 0., 0., 1.],
                ..Default::default()
            },
            propellant_kg: 0.,
        };
        let hardware = Hardware::default();
        let contacts = [abi::Contact {
            id: 7,
            kind: abi::CONTACT_SHIP,
            ..Default::default()
        }];
        controller.observe(&sample, &contacts);
        assert!(controller.start_firing().is_err());
        controller
            .mark_target(
                abi::MarkTargetRequest {
                    contact: 7,
                    maximum_flight_time_s: 30.,
                },
                0.,
            )
            .unwrap();
        let marked = controller.update(&sample, &hardware).state;
        assert_eq!(marked.target_contact, 7);
        assert_eq!(marked.mode, abi::WEAPONS_HOLD);
        controller.start_firing().unwrap();
        assert_eq!(
            controller.update(&sample, &hardware).state.mode,
            abi::WEAPONS_FIRING
        );
        controller.stop_firing();
        for _ in 0..10 {
            controller.observe(&sample, &contacts);
            let stopped = controller.update(&sample, &hardware).state;
            assert_eq!(stopped.target_contact, 7);
            assert_eq!(stopped.mode, abi::WEAPONS_HOLD);
        }
        controller.start_firing().unwrap();
        controller.unmark_target();
        let unmarked = controller.update(&sample, &hardware).state;
        assert_eq!(unmarked.target_contact, 0);
        assert_eq!(unmarked.mode, abi::WEAPONS_HOLD);
        assert!(controller.start_firing().is_err());
    }

    #[test]
    fn intercept_tracks_crossing_target_and_rejects_escape() {
        let r = DVec3::X * 5000.0;
        let v = DVec3::Y * 1000.0;
        let (direction, t) = intercept(r, v, 5000.0, 2.0).unwrap();
        assert!((direction * (5000.0 * t) - (r + v * t)).length() < 1e-8);
        assert!(intercept(r, DVec3::X * 6000.0, 5000.0, 2.0).is_none());
        assert!(intercept(r, v, 5000.0, 0.1).is_none());
    }
}
