//! Bounded, warm-started box-constrained least-squares control allocation.
//! Engines contribute both force and lever-arm torque; torquers contribute three axes.
use crate::hardware::{Actuation, Capability, Device, Hardware};
use glam::{DQuat, DVec3};
use toy_sim_ship_api::abi;

#[derive(Clone)]
pub struct Actuator {
    pub handle: u64,
    pub force: DVec3,
    pub torque: DVec3,
    pub axis: Option<usize>,
    pub rating: f64,
    pub propellant_rate: f64,
}
#[derive(Clone, Default)]
pub struct Layout {
    pub columns: Vec<Actuator>,
    pub handles: Vec<(u64, u64)>, // Device kind selects the output record.
}
impl Layout {
    pub fn new(devices: &[Device]) -> Self {
        let mut out = Self::default();
        for d in devices {
            let q = DQuat::from_array(d.info.rotation).normalize();
            let r = DVec3::from_array(d.info.position_m);
            match d.capability {
                Capability::Engine(spec) => {
                    let thrust_n = spec.max_thrust_n;
                    let propellant_kg_s = spec.propellant_units_s * d.resource_mass_kg;
                    out.handles.push((d.info.id, abi::DEVICE_ENGINE));
                    if !(d.info.flags & abi::CONTROL_ENABLED != 0) {
                        continue;
                    }
                    let force = q * DVec3::NEG_Z * thrust_n;
                    out.columns.push(Actuator {
                        handle: d.info.id,
                        force,
                        torque: r.cross(force),
                        axis: None,
                        rating: thrust_n,
                        propellant_rate: propellant_kg_s,
                    });
                }
                Capability::Rcs(spec) => {
                    out.handles.push((d.info.id, abi::DEVICE_RCS));
                    if d.info.flags & abi::CONTROL_ENABLED == 0 {
                        continue;
                    }

                    for axis in 0..3 {
                        let force = q * DVec3::AXES[axis] * spec.per_axis_thrust_n;
                        out.columns.push(Actuator {
                            handle: d.info.id,
                            force,
                            torque: r.cross(force),
                            axis: Some(axis),
                            rating: spec.per_axis_thrust_n,
                            propellant_rate: spec.per_axis_propellant_units_s * d.resource_mass_kg,
                        });
                    }
                }
                Capability::Torquer(spec) => {
                    let torque_nm = spec.per_axis_limit_nm;
                    out.handles.push((d.info.id, abi::DEVICE_TORQUER));
                    if !(d.info.flags & abi::CONTROL_ENABLED != 0) {
                        continue;
                    }
                    for axis in 0..3 {
                        out.columns.push(Actuator {
                            handle: d.info.id,
                            force: DVec3::ZERO,
                            torque: q * DVec3::AXES[axis] * torque_nm,
                            axis: Some(axis),
                            rating: torque_nm,
                            propellant_rate: 0.,
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }
    pub fn available(snapshot: &Hardware, h: u64) -> bool {
        snapshot.get(h).is_some_and(|s| s.available())
    }
    pub fn authority(&self, snapshot: &Hardware, forward: DVec3) -> (f64, DVec3, DVec3, f64) {
        let mut thrust = 0.;
        let mut positive = DVec3::ZERO;
        let mut negative = DVec3::ZERO;
        let mut thrust_torque = DVec3::ZERO;
        let mut propellant = 0.;
        for c in &self.columns {
            if !Self::available(snapshot, c.handle) {
                continue;
            }
            if c.axis.is_some() {
                positive += c.torque.abs();
                negative += c.torque.abs();
                let projection = c.force.dot(forward);
                thrust += projection.abs();
                if projection.abs() > 1e-8 {
                    thrust_torque += c.torque * projection.signum();
                    propellant += c.propellant_rate;
                }
            } else {
                positive += c.torque.max(DVec3::ZERO);
                negative += (-c.torque).max(DVec3::ZERO);
                let projection = c.force.dot(forward).max(0.);
                thrust += projection;
                if projection > 0. {
                    thrust_torque += c.torque;
                    propellant += c.propellant_rate;
                }
            }
        }
        (thrust, positive.min(negative), thrust_torque, propellant)
    }
}
#[derive(Default)]
pub struct Allocator {
    values: Vec<f64>,
    pub residual: f64,
}
impl Allocator {
    pub fn solve(
        &mut self,
        layout: &Layout,
        devices: &Hardware,
        force: DVec3,
        torque: DVec3,
        limit: f64,
        out: &mut Vec<Actuation>,
    ) {
        let initializing = self.values.len() != layout.columns.len();
        self.values.resize(layout.columns.len(), 0.);
        let force_scale = layout
            .columns
            .iter()
            .map(|c| c.force.length())
            .sum::<f64>()
            .max(1.);
        let torque_scale = layout
            .columns
            .iter()
            .map(|c| c.torque.length())
            .sum::<f64>()
            .max(1.);
        // Weight torque error more heavily to reduce unwanted rotation when a burn saturates.
        let fs = 1. / force_scale;
        let ts = 8. / torque_scale;
        let direction = force.try_normalize().unwrap_or(DVec3::ZERO);
        let capacity = layout
            .columns
            .iter()
            .filter(|c| Layout::available(devices, c.handle))
            .map(|c| {
                let projection = c.force.dot(direction);
                if c.axis.is_some() {
                    projection.abs()
                } else {
                    projection.max(0.)
                }
            })
            .sum::<f64>();
        if force == DVec3::ZERO && torque == DVec3::ZERO {
            self.values.fill(0.);
        }
        let mut rf = -force * fs;
        let mut rt = -torque * ts;
        for (c, x) in layout.columns.iter().zip(&mut self.values) {
            if initializing && c.axis.is_none() {
                if c.force.dot(direction) > 0. {
                    *x = force.length() / capacity.max(1.);
                }
            }
            *x = if Layout::available(devices, c.handle) {
                x.clamp(
                    if c.axis.is_some() { -1. } else { 0. },
                    if c.axis.is_some() { 1. } else { limit },
                )
            } else {
                0.
            };
            rf += c.force * (*x * fs);
            rt += c.torque * (*x * ts);
        }
        for _ in 0..24 {
            #[cfg(target_arch = "wasm32")]
            if !toy_sim_ship_api::sdk::budget()
                .is_ok_and(|budget| crate::budget::allocation_allowed(budget, layout.handles.len()))
            {
                break;
            }
            let mut change: f64 = 0.;
            // Solve torque actuators first so a feasible off-centre burn needn't
            // gradually climb out of a zero-throttle warm start.
            for torque_first in [true, false] {
                for (c, x) in layout.columns.iter().zip(&mut self.values) {
                    if c.axis.is_some() != torque_first {
                        continue;
                    }
                    if !Layout::available(devices, c.handle) {
                        continue;
                    }
                    let f = c.force * fs;
                    let t = c.torque * ts;
                    let denominator = f.length_squared() + t.length_squared();
                    if denominator < 1e-20 {
                        continue;
                    }
                    let next = (*x - (f.dot(rf) + t.dot(rt)) / denominator).clamp(
                        if c.axis.is_some() { -1. } else { 0. },
                        if c.axis.is_some() { 1. } else { limit },
                    );
                    let delta = next - *x;
                    *x = next;
                    rf += f * delta;
                    rt += t * delta;
                    change = change.max(delta.abs());
                }
            }
            if change < 1e-8 {
                break;
            }
        }
        self.residual = (rf.length_squared() + rt.length_squared()).sqrt();
        // One output per physical actuator, including zero for failed/excluded devices.
        let mut cursor = 0;
        for &(handle, kind) in &layout.handles {
            let mut v = DVec3::ZERO;
            let mut throttle = 0.;
            while cursor < layout.columns.len() && layout.columns[cursor].handle == handle {
                let c = &layout.columns[cursor];
                if let Some(axis) = c.axis {
                    v[axis] = if self.values[cursor].abs() < 1e-12 {
                        0.
                    } else {
                        self.values[cursor] * c.rating
                    };
                } else {
                    throttle = if self.values[cursor].abs() < 1e-12 {
                        0.
                    } else {
                        self.values[cursor]
                    };
                }
                cursor += 1;
            }
            if kind == abi::DEVICE_RCS {
                out.push(Actuation::Rcs {
                    device: handle,
                    value: abi::RcsSetting {
                        thrust_n: v.to_array(),
                    },
                });
            } else if kind == abi::DEVICE_TORQUER {
                out.push(Actuation::Torque {
                    device: handle,
                    value: abi::TorqueSetting {
                        torque_nm: v.to_array(),
                    },
                });
            } else {
                out.push(Actuation::Throttle {
                    device: handle,
                    value: abi::ThrottleSetting { fraction: throttle },
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pilot, hardware::Sample};
    use abi::Record;

    fn engine(id: u64, position: DVec3, rotation: DQuat) -> Device {
        Device {
            info: abi::DeviceInfo {
                id,
                kind: abi::DEVICE_ENGINE,
                flags: abi::CONTROL_ENABLED,
                position_m: position.to_array(),
                rotation: rotation.to_array(),
                ..Default::default()
            },
            capability: Capability::Engine(abi::EngineSpec {
                max_thrust_n: 1000.,
                propellant_units_s: 1.,
                ..Default::default()
            }),
            status: abi::DeviceStatus {
                flags: abi::POWERED | abi::OPERATIONAL,
            },
            accelerometer: Default::default(),
            thrust_n: 0.,
            resource_mass_kg: 1.,
        }
    }

    fn torquer(id: u64, rating: f64, rotation: DQuat) -> Device {
        let mut device = engine(id, DVec3::ZERO, rotation);
        device.info.kind = abi::DEVICE_TORQUER;
        device.capability = Capability::Torquer(abi::TorquerSpec {
            per_axis_limit_nm: rating,
            max_power_w: 0.,
        });
        device
    }

    fn hardware(devices: Vec<Device>) -> Hardware {
        let mut hardware = Hardware::default();
        hardware.devices = devices;
        hardware
    }

    fn delivered(layout: &Layout, commands: &[Actuation]) -> (DVec3, DVec3) {
        let mut force = DVec3::ZERO;
        let mut torque = DVec3::ZERO;

        for column in &layout.columns {
            let fraction = commands
                .iter()
                .find_map(|command| match command {
                    Actuation::Throttle { device, value } if *device == column.handle => {
                        Some(value.fraction)
                    }
                    Actuation::Torque { device, value } if *device == column.handle => {
                        Some(value.torque_nm[column.axis.unwrap()] / column.rating)
                    }
                    Actuation::Rcs { device, value } if *device == column.handle => {
                        Some(value.thrust_n[column.axis.unwrap()] / column.rating)
                    }
                    _ => None,
                })
                .unwrap();
            assert!((if column.axis.is_some() { -1. } else { 0. }..=1.).contains(&fraction));
            force += column.force * fraction;
            torque += column.torque * fraction;
        }

        (force, torque)
    }

    #[test]
    fn canted_engines_and_rotated_torquer_deliver_combined_force_and_torque() {
        let hardware = hardware(vec![
            engine(1, DVec3::X, DQuat::from_rotation_y(0.25)),
            engine(2, -DVec3::X, DQuat::from_rotation_y(-0.25)),
            torquer(3, 5000., DQuat::from_rotation_x(0.7)),
        ]);
        let layout = Layout::new(&hardware.devices);
        let mut allocator = Allocator::default();
        let force = DVec3::NEG_Z * 700.;
        let torque = DVec3::new(20., -40., 30.);
        let mut out = Vec::new();

        for _ in 0..8 {
            out.clear();
            allocator.solve(&layout, &hardware, force, torque, 1., &mut out);
        }

        let (actual_force, actual_torque) = delivered(&layout, &out);
        assert!(actual_force.distance(force) < 0.1);
        assert!(actual_torque.distance(torque) < 0.1);
    }

    #[test]
    fn rcs_blocks_deliver_translation_and_rotation_with_main_engines_off() {
        let mut devices = Vec::new();
        for (index, position) in [DVec3::X, -DVec3::X, DVec3::Z, -DVec3::Z]
            .into_iter()
            .enumerate()
        {
            let mut device = engine(index as u64 + 1, position, DQuat::from_rotation_y(0.4));
            device.info.kind = abi::DEVICE_RCS;
            device.capability = Capability::Rcs(abi::RcsSpec {
                per_axis_thrust_n: 1000.0,
                per_axis_propellant_units_s: 0.5,
                ..Default::default()
            });
            devices.push(device);
        }

        let hardware = hardware(devices);
        let layout = Layout::new(&hardware.devices);
        let mut allocator = Allocator::default();
        let force = DVec3::new(300.0, -400.0, 200.0);
        let torque = DVec3::new(-100.0, 200.0, 300.0);
        let mut out = Vec::new();
        for _ in 0..8 {
            out.clear();
            allocator.solve(&layout, &hardware, force, torque, 0.0, &mut out);
        }

        let (actual_force, actual_torque) = delivered(&layout, &out);
        assert!(actual_force.distance(force) < 0.1, "{actual_force:?}");
        assert!(actual_torque.distance(torque) < 0.1, "{actual_torque:?}");
    }

    #[test]
    fn off_center_burn_is_compensated_and_excluded_devices_stay_zero() {
        let mut excluded = engine(3, DVec3::ZERO, DQuat::IDENTITY);
        excluded.info.flags = 0;
        let hardware = hardware(vec![
            engine(1, DVec3::X, DQuat::IDENTITY),
            torquer(2, 1000., DQuat::IDENTITY),
            excluded,
        ]);
        let layout = Layout::new(&hardware.devices);
        let mut allocator = Allocator::default();
        let mut out = Vec::new();
        allocator.solve(
            &layout,
            &hardware,
            DVec3::NEG_Z * 300.,
            DVec3::ZERO,
            0.5,
            &mut out,
        );
        let (force, torque) = delivered(&layout, &out);
        assert!((force.z + 300.).abs() < 0.1);
        assert!(torque.length() < 0.1);
        assert!(matches!(
            out[2],
            Actuation::Throttle {
                value: abi::ThrottleSetting { fraction: 0. },
                ..
            }
        ));
    }

    #[test]
    fn failure_and_rank_deficiency_produce_bounded_commands_and_report_residual() {
        let mut hardware = hardware(vec![engine(1, DVec3::ZERO, DQuat::IDENTITY)]);
        let layout = Layout::new(&hardware.devices);
        let mut allocator = Allocator::default();
        let mut out = Vec::new();
        allocator.solve(
            &layout,
            &hardware,
            DVec3::X * 1000.,
            DVec3::Y * 10.,
            1.,
            &mut out,
        );
        delivered(&layout, &out);
        assert!(allocator.residual.is_finite() && allocator.residual > 0.1);
        hardware.devices[0].status.flags = 0;
        out.clear();
        allocator.solve(
            &layout,
            &hardware,
            DVec3::NEG_Z * 1000.,
            DVec3::ZERO,
            1.,
            &mut out,
        );
        assert_eq!(delivered(&layout, &out), (DVec3::ZERO, DVec3::ZERO));
    }

    #[test]
    fn thruster_only_layout_can_steer_without_translation() {
        let hardware = hardware(vec![
            engine(1, DVec3::X, DQuat::IDENTITY),
            engine(2, -DVec3::X, DQuat::from_rotation_y(core::f64::consts::PI)),
        ]);
        let layout = Layout::new(&hardware.devices);
        let mut allocator = Allocator::default();
        let mut out = Vec::new();

        for _ in 0..8 {
            out.clear();
            allocator.solve(
                &layout,
                &hardware,
                DVec3::ZERO,
                DVec3::Y * 400.,
                1.,
                &mut out,
            );
        }

        let (force, torque) = delivered(&layout, &out);
        assert!(force.length() < 0.1);
        assert!(torque.distance(DVec3::Y * 400.) < 0.1);
    }

    #[test]
    fn direction_alignment_preserves_throttle_and_cancels_steering() {
        let hardware = hardware(vec![
            engine(1, DVec3::ZERO, DQuat::IDENTITY),
            torquer(2, 1000., DQuat::IDENTITY),
        ]);
        let sample = Sample {
            flight: abi::FlightState {
                rotation: DQuat::IDENTITY.to_array(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pilot = Pilot::default();
        pilot.observe(&sample, &hardware, &[]);
        pilot
            .request(
                abi::REQUEST_MANUAL,
                abi::ManualRequest {
                    throttle: 0.7,
                    steering: [0.2, 0.1, 0.],
                }
                .bytes(),
                &sample,
                &hardware,
            )
            .unwrap();
        pilot
            .request(
                abi::REQUEST_AIM_DIRECTION,
                abi::DirectionRequest {
                    direction: DVec3::X.to_array(),
                }
                .bytes(),
                &sample,
                &hardware,
            )
            .unwrap();

        assert_eq!(pilot.manual_throttle, 0.7);
        assert_eq!(pilot.throttle, 0.);
        assert_eq!(pilot.steering, DVec3::ZERO);
        assert_eq!(pilot.aim, Some(DVec3::X));
    }

    #[test]
    fn manual_steering_uses_configured_control_orientation() {
        let rotation = DQuat::from_rotation_y(core::f64::consts::FRAC_PI_2);
        let mut computer = engine(2, DVec3::ZERO, rotation);
        computer.info.kind = abi::DEVICE_COMPUTER;
        computer.capability = Capability::Passive;
        let hardware = hardware(vec![torquer(1, 1000., DQuat::IDENTITY), computer]);
        let sample = Sample {
            flight: abi::FlightState {
                rotation: DQuat::IDENTITY.to_array(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pilot = Pilot::default();
        pilot.observe(&sample, &hardware, &[]);
        let request = abi::ManualRequest {
            throttle: 0.,
            steering: [0.2, 0., 0.],
        };
        pilot
            .request(abi::REQUEST_MANUAL, request.bytes(), &sample, &hardware)
            .unwrap();
        let out = pilot.control(&sample, &hardware);
        let (_, torque) = delivered(&Layout::new(&hardware.devices), &out);
        assert!(torque.distance(rotation * DVec3::X * 200.) < 1e-8);
    }
}
