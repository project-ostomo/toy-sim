use super::*;
use bevy::math::DQuat;
use osg_example_controller::hardware::{Actuation, Capability, Device, Hardware, Sample};
use osg_example_controller::{Pilot, navigation::Phase};
use osg_ship_api::abi;
use osg_ship_wasm::Command;

#[derive(Debug)]
struct Step {
    devices: Vec<DeviceCommand>,
    results: Vec<Result<(), String>>,
}

impl Step {
    fn valid(&self) -> bool {
        self.devices.iter().all(|command| match command.setting {
            DeviceSetting::Throttle(value) => value.is_finite() && (0. ..=1.).contains(&value),
            DeviceSetting::TorqueNm(value) => value.iter().all(|value| value.is_finite()),
            _ => false,
        })
    }
}

use crate::sim::{
    hardware::{self, fixtures::HardwareFixture},
    vessel::{ShipCatalogue, WasmRuntime, spawn_ship},
};
use bevy::ecs::system::RunSystemOnce;
use osg_ships::*;
use std::sync::Arc;

fn guidance_catalogue() -> Catalogue {
    let mut catalogue = Catalogue::builtin();
    let engine = catalogue
        .parts
        .iter_mut()
        .find(|part| part.id == "engine")
        .unwrap();
    engine.dimensions = [10, 10, 20];
    if let Equipment::Engine {
        thrust_n, power_w, ..
    } = &mut engine.equipment
    {
        *thrust_n = 200_000.0;
        *power_w = 25e9;
    }
    let generator = catalogue
        .parts
        .iter_mut()
        .find(|part| part.id == "generator")
        .unwrap();
    generator.equipment = Equipment::Generator {
        power_w: 100e9,
        fuel_kg_s: 10_000.0,
        efficiency: 0.4,
    };
    let battery = catalogue
        .parts
        .iter_mut()
        .find(|part| part.id == "battery")
        .unwrap();
    battery.equipment = Equipment::Battery {
        capacity_j: 10_000_000_000,
    };
    catalogue
}

fn fixture(design: &CompiledShipDesign, catalogue: &Catalogue) -> HardwareFixture {
    let mut app = App::new();
    app.add_plugins(bevy::app::TaskPoolPlugin::default());
    app.insert_resource(ShipCatalogue(catalogue.clone()))
        .insert_resource(WasmRuntime::default())
        .insert_resource(Time::<Fixed>::from_duration(osg_model::TICK_DURATION));
    let design = Arc::new(design.clone());
    let ship = spawn_ship(
        app.world_mut(),
        design.clone(),
        PreciseTransform::default(),
        DVec3::ZERO,
        "Rendezvous fixture".into(),
    )
    .unwrap();
    app.world_mut()
        .run_system_once(hardware::initialize)
        .unwrap();
    hardware::install(&mut app);
    HardwareFixture { app, ship, design }
}

struct AppliedForces {
    force: DVec3,
    torque: DVec3,
    mass: f64,
    inertia: bevy::math::DMat3,
}

fn advance(fixture: &mut HardwareFixture) -> AppliedForces {
    let capacity = fixture.design.battery_j;
    fixture.set_inventory(|inventory| inventory.energy_j = capacity);
    let wrench = fixture.advance();
    let mass = fixture.app.world().get::<MassProps>(fixture.ship).unwrap();
    AppliedForces {
        force: wrench.force,
        torque: wrench.torque,
        mass: mass.mass,
        inertia: mass.inertia,
    }
}

struct Flight {
    pilot: Pilot,
    origins: Vec<DVec3>,
    cat: Catalogue,
    design: CompiledShipDesign,
    hardware: HardwareFixture,
    p: DVec3,
    v: DVec3,
    target: DVec3,
    target_v: DVec3,
    q: DQuat,
    w: DVec3,
    thrust_factor: f64,
    min_separation: f64,
    min_radius: f64,
    imu: AccelerometerState,
    time: f64,
    gravity: bool,
    visible: bool,
    display: bool,
    target_acceleration: DVec3,
    target_hardware: Option<HardwareFixture>,
    target_q: DQuat,
    target_w: DVec3,
}
impl Flight {
    fn new(
        position: DVec3,
        target: DVec3,
        velocity: DVec3,
        target_velocity: DVec3,
        gravity: bool,
    ) -> Self {
        let cat = guidance_catalogue();
        let mut blueprint = starter(EXAMPLE_CONTROLLER.to_vec());

        let design = blueprint.compile(&cat).unwrap();
        let hardware = fixture(&design, &cat);
        Self {
            pilot: Pilot::default(),
            origins: Vec::new(),
            cat,
            design,
            hardware,
            thrust_factor: 1.,
            min_separation: f64::INFINITY,
            min_radius: f64::INFINITY,
            p: position,
            v: velocity,
            target,
            target_v: target_velocity,
            q: DQuat::IDENTITY,
            w: DVec3::ZERO,
            imu: AccelerometerState {
                time_s: Some(0.),
                ..default()
            },
            time: 0.,
            gravity,
            visible: true,
            display: false,
            target_acceleration: DVec3::ZERO,
            target_hardware: None,
            target_q: DQuat::IDENTITY,
            target_w: DVec3::ZERO,
        }
    }
    fn tick(&mut self, commands: Vec<Command>) -> Step {
        let (mass, inertia) = self
            .hardware
            .state()
            .mass_properties(&self.design, &self.cat);
        let contact = abi::Contact {
            id: 42,
            kind: abi::CONTACT_SHIP,
            radius_m: self.design.radius,
            position_m: (self.target - self.p).to_array(),
            velocity_m_s: (self.target_v - self.v).to_array(),
        };
        let measured = self.hardware.state().snapshot(&self.design);
        let mut hardware = Hardware::default();
        hardware.devices = self
            .design
            .device_catalogue
            .iter()
            .zip(measured)
            .map(|(descriptor, status)| {
                let capability = match descriptor.kind {
                    DeviceKind::Engine {
                        thrust_n,
                        propellant_kg_s,
                        power_w,
                        ref propellant_resource,
                    } => {
                        let resource = self
                            .cat
                            .resources
                            .iter()
                            .position(|resource| resource.id == *propellant_resource)
                            .unwrap();
                        Capability::Engine(abi::EngineSpec {
                            propellant_resource: resource as u64 + 1,
                            max_thrust_n: thrust_n,
                            propellant_units_s: propellant_kg_s
                                / self.cat.resources[resource].mass_kg,
                            max_power_w: power_w,
                        })
                    }
                    DeviceKind::Torquer { torque_nm } => Capability::Torquer(abi::TorquerSpec {
                        per_axis_limit_nm: torque_nm,
                        max_power_w: 0.,
                    }),
                    DeviceKind::Generator { power_w } => {
                        Capability::Generator(abi::GeneratorSpec {
                            max_power_w: power_w,
                            ..Default::default()
                        })
                    }
                    _ => Capability::Passive,
                };
                let sample = self.imu.at_mount(
                    DVec3::from_array(descriptor.position_m),
                    DQuat::from_array(descriptor.rotation),
                );
                let flags = u64::from(status.operational) | (u64::from(status.powered) << 1);
                Device {
                    info: abi::DeviceInfo::from(descriptor),
                    capability,
                    status: abi::DeviceStatus { flags },
                    thrust_n: match status.reading {
                        DeviceReading::Engine { thrust_n } => thrust_n,
                        _ => 0.,
                    },
                    accelerometer: abi::AccelerometerReading {
                        status: abi::DeviceStatus { flags },
                        sample_present: u64::from(sample.is_some()),
                        sample_time_s: sample.map_or(0., |sample| sample.time_s),
                        acceleration: sample.map_or([0.; 3], |sample| sample.acceleration_m_s2),
                    },
                    resource_mass_kg: 1.,
                }
            })
            .collect();
        self.origins.push(self.p);
        let sample = Sample {
            tick: abi::TickContext {
                time_s: self.time,
                dt_s: osg_model::TICK_SECONDS,
                snapshot: self.origins.len() as u64,
                interest: if self.display { abi::INTEREST_PATHS } else { 0 },
                ..Default::default()
            },
            flight: abi::FlightState {
                rotation: self.q.to_array(),
                angular_velocity: self.w.to_array(),
                velocity: self.v.to_array(),
                mass_kg: mass,
                inertia: inertia.to_cols_array(),
                radius_m: self.design.radius,
            },
            propellant_kg: self.hardware.state().inventory.available(0),
        };
        let contacts = if self.visible {
            vec![contact]
        } else {
            Vec::new()
        };
        self.pilot.observe(&sample, &hardware, &contacts);
        let results = commands
            .into_iter()
            .map(|command| {
                let (kind, payload) = command.payload();
                self.pilot.request(kind, &payload, &sample, &hardware)
            })
            .collect();
        let devices = self
            .pilot
            .control(&sample, &hardware)
            .into_iter()
            .map(|action| match action {
                Actuation::Rcs { device, value } => DeviceCommand {
                    device: DeviceHandle((device - 1) as u16),
                    setting: DeviceSetting::RcsThrust(value.thrust_n),
                },
                Actuation::Throttle { device, value } => DeviceCommand {
                    device: DeviceHandle((device - 1) as u16),
                    setting: DeviceSetting::Throttle(value.fraction),
                },
                Actuation::Torque { device, value } => DeviceCommand {
                    device: DeviceHandle((device - 1) as u16),
                    setting: DeviceSetting::TorqueNm(value.torque_nm),
                },
            })
            .collect();
        let output = Step { devices, results };
        assert!(output.valid());
        self.hardware.commands(&output.devices).unwrap();
        let engine = advance(&mut self.hardware);
        let thrust = self.q * engine.force / engine.mass;
        self.min_separation = self.min_separation.min((self.target - self.p).length());
        self.min_radius = self.min_radius.min(self.p.length());
        let body_w = self.q.inverse() * self.w;
        let alpha = self.q
            * (engine.inertia.inverse() * (engine.torque - body_w.cross(engine.inertia * body_w)));
        let g = |p: DVec3| {
            if self.gravity {
                -p.normalize() * 3.986004418e14 / p.length_squared()
            } else {
                DVec3::ZERO
            }
        };
        let old_relative = self.target - self.p;
        self.v += (thrust + g(self.p)) * osg_model::TICK_SECONDS;
        self.p += self.v * osg_model::TICK_SECONDS;
        let target_thrust = if let Some(hardware) = &mut self.target_hardware {
            let e = advance(hardware);
            let thrust = self.target_q * e.force / e.mass;
            (self.target_q, self.target_w) = rotation::integrate(
                self.target_q,
                self.target_w,
                self.target_q * e.torque,
                e.inertia,
                e.inertia.inverse(),
                osg_model::TICK_SECONDS,
            );
            thrust
        } else {
            DVec3::ZERO
        };
        self.target_v +=
            (g(self.target) + self.target_acceleration + target_thrust) * osg_model::TICK_SECONDS;
        self.target += self.target_v * osg_model::TICK_SECONDS;
        let delta = self.target - self.p - old_relative;
        let f = (-old_relative.dot(delta) / delta.length_squared().max(1e-20)).clamp(0., 1.);
        self.min_separation = self.min_separation.min((old_relative + delta * f).length());
        (self.q, self.w) = rotation::integrate(
            self.q,
            self.w,
            self.q * engine.torque,
            engine.inertia,
            engine.inertia.inverse(),
            osg_model::TICK_SECONDS,
        );
        self.time += osg_model::TICK_SECONDS;
        self.imu = AccelerometerState {
            specific_force_body: self.q.inverse() * thrust,
            angular_acceleration_body: self.q.inverse() * alpha,
            angular_velocity_body: self.q.inverse() * self.w,
            time_s: Some(self.time),
        };
        output
    }
    fn engage(&mut self) {
        for p in &mut self.design.parts {
            if let Equipment::Engine { thrust_n, .. } = &mut p.definition.equipment {
                *thrust_n *= self.thrust_factor;
            }
        }
        self.hardware = fixture(&self.design, &self.cat);
        self.tick(vec![]);
        let out = self.tick(vec![
            Command::SelectTarget(42),
            Command::EngageNavigation {
                throttle_limit: 1.,
                stand_off_m: 100.,
            },
        ]);
        assert!(out.results.iter().all(Result::is_ok), "{:?}", out.results);
    }
    fn forecast(&mut self) -> (DVec3, osg_example_controller::prediction::Forecast) {
        for _ in 0..30 {
            self.tick(vec![]);
            if let Some(forecast) = self.pilot.forecast() {
                return (
                    self.origins[forecast.snapshot as usize - 1],
                    forecast.clone(),
                );
            }
        }
        panic!("no complete forecast within 3 seconds");
    }
    fn finish(&mut self) {
        for _ in 0..100000 {
            self.tick(vec![]);
            let nav = &self.pilot.navigation;
            if nav.error().length() <= 2. && nav.u.length() <= 0.5 {
                return;
            }
            assert!(
                nav.phase.active(),
                "guidance stopped: {} at {:?}",
                nav.reason,
                nav.error()
            );
        }
        panic!(
            "no arrival: error={:?} velocity={:?}",
            self.pilot.navigation.error(),
            self.pilot.navigation.u
        );
    }
}
#[test]
fn pursuit_pass_in_free_space_and_with_lateral_or_receding_velocity() {
    for velocity in [DVec3::ZERO, DVec3::new(-100., 40., 20.)] {
        let mut f = Flight::new(
            DVec3::ZERO,
            DVec3::X * 50_000.,
            velocity,
            DVec3::ZERO,
            false,
        );
        f.engage();
        f.finish();
    }
}
#[test]
fn pursuit_pass_with_neighbour_in_low_orbit() {
    let radius = 6_170_000.;
    let speed = (3.986004418e14_f64 / radius).sqrt();
    let phase = std::f64::consts::TAU / 501.;
    let turn = DQuat::from_rotation_y(phase);
    let mut f = Flight::new(
        DVec3::Z * radius,
        turn * (DVec3::Z * radius),
        DVec3::X * speed,
        turn * (DVec3::X * speed),
        true,
    );
    f.engage();
    f.finish();
    assert!(f.min_radius > 6_160_000., "arrival entered atmosphere");
}
#[test]
fn pursuit_pass_with_nearby_ship_in_high_inclined_orbit() {
    let universe = crate::sim::orrery::Universe::init(osg_universe::example_config()).unwrap();
    let mut scenario = crate::sim::scenario::INITIAL_SCENARIO.clone();
    scenario.traffic_count = 1;
    let body = universe
        .body(universe.authored_body(scenario.body).unwrap())
        .unwrap();
    let states = scenario.fleet_states(body.radius, body.mass).unwrap();
    let (position, velocity) = states[0];
    let &(target, target_velocity) = states[1..]
        .iter()
        .min_by(|a, b| {
            a.0.distance_squared(position)
                .total_cmp(&b.0.distance_squared(position))
        })
        .unwrap();
    let mut f = Flight::new(position, target, velocity, target_velocity, true);
    f.engage();
    f.finish();
    assert!(f.min_radius > body.radius + body.atmosphere.as_ref().unwrap().height);
}
#[test]
fn guidance_waits_for_alignment_and_brakes_at_arrival() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 50_000.0,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.q = DQuat::from_rotation_arc(DVec3::NEG_Z, DVec3::NEG_X);
    f.engage();
    let output = f.tick(vec![]);
    assert!(output.devices.iter().all(|device| {
        !matches!(device.setting, DeviceSetting::Throttle(throttle) if throttle > 1e-6)
    }));
    f.finish();
    assert!(f.pilot.navigation.u.length() <= 0.5);
    f.tick(vec![Command::StopGuidance]);
    assert_eq!(f.pilot.navigation.phase, Phase::Ready);
}

#[test]
fn slow_attitude_control_and_throttle_limit_still_intercept() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 50_000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    let torquer = f.cat.parts.iter_mut().find(|p| p.id == "torquer").unwrap();
    if let Equipment::Torquer { torque_nm, .. } = &mut torquer.equipment {
        *torque_nm *= 0.1;
    }
    f.design = f.design.blueprint.compile(&f.cat).unwrap();
    f.hardware = fixture(&f.design, &f.cat);
    f.tick(vec![]);
    f.tick(vec![
        Command::SelectTarget(42),
        Command::EngageNavigation {
            throttle_limit: 0.5,
            stand_off_m: 100.,
        },
    ]);
    for _ in 0..1000 {
        let out = f.tick(vec![]);
        for d in &out.devices {
            if let DeviceSetting::Throttle(t) = d.setting {
                assert!(t <= 0.5);
            }
        }
    }
    f.finish();
}
#[test]
fn pursuit_tracks_target_maneuvers_without_reengagement() {
    for acceleration in [DVec3::X * 15., DVec3::new(8., 6., -3.)] {
        let mut f = Flight::new(
            DVec3::ZERO,
            DVec3::X * 100_000.,
            DVec3::ZERO,
            DVec3::ZERO,
            false,
        );
        f.engage();
        for tick in 0..1000 {
            f.target_acceleration = if tick < 100 {
                DVec3::ZERO
            } else if tick < 600 {
                acceleration
            } else {
                -acceleration * 0.5
            };
            let out = f.tick(vec![]);
            assert!(out.valid());
            assert!(f.pilot.navigation.phase.active());
            assert!(f.pilot.navigation.throttle <= 1.);
        }
        f.target_acceleration = DVec3::ZERO;
        f.finish();
        for _ in 0..20 {
            f.tick(vec![]);
        }
        assert_eq!(f.pilot.navigation.phase, Phase::Ready);
        assert!(f.pilot.navigation.u.length() <= 0.5);
    }
}

#[test]
fn unavoidable_overshoot_recovers_and_no_intercept_keeps_pursuing() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 1000.,
        DVec3::X * 1000.,
        DVec3::ZERO,
        false,
    );
    f.engage();
    f.finish();
    assert_eq!(f.pilot.navigation.phase, Phase::Ready);
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 1e7,
        DVec3::ZERO,
        DVec3::X * 1e5,
        false,
    );
    f.engage();
    for _ in 0..200 {
        let out = f.tick(vec![]);
        assert!(out.valid());
        assert!(f.pilot.navigation.phase.active());
    }
    assert!(
        f.pilot.delivered_thrust > 0.,
        "best effort must actually pursue"
    );
}
#[test]
fn contact_loss_pauses_until_reengagement_and_allows_manual_takeover() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 50_000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.engage();
    for _ in 0..50 {
        f.tick(vec![]);
    }

    f.visible = false;
    let out = f.tick(vec![]);
    assert_eq!(f.pilot.navigation.phase, Phase::Paused);
    assert!(!f.pilot.navigation.visible);
    assert!(
        out.devices
            .iter()
            .any(|device| matches!(device.setting, DeviceSetting::Throttle(0.)))
    );

    f.visible = true;
    f.tick(vec![]);
    assert_eq!(f.pilot.navigation.phase, Phase::Paused);
    f.tick(vec![Command::EngageNavigation {
        throttle_limit: 1.,
        stand_off_m: 100.,
    }]);
    assert!(f.pilot.navigation.phase.active());
    f.tick(vec![Command::Manual {
        throttle: 0.2,
        steering: [0., 1., 0.],
    }]);
    assert_eq!(f.pilot.navigation.phase, Phase::Ready);
}

#[test]
fn accelerometer_has_no_free_fall_signal_and_respects_mount_rotation() {
    let mut f = Flight::new(
        DVec3::Z * 6_170_000.,
        DVec3::X * 1000.,
        DVec3::X * 8000.,
        DVec3::ZERO,
        true,
    );
    f.tick(vec![]);
    assert_eq!(f.imu.specific_force_body, DVec3::ZERO);
    let sample = AccelerometerState {
        specific_force_body: DVec3::X * 2.,
        angular_velocity_body: DVec3::Z * 3.,
        angular_acceleration_body: DVec3::Y,
        time_s: Some(1.),
    };
    let at = sample
        .at_mount(
            DVec3::X,
            DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
        )
        .unwrap();
    assert!((DVec3::from_array(at.acceleration_m_s2) - DVec3::new(0., 7., -1.)).length() < 1e-10);
}

#[test]
fn reduced_thrust_and_short_encounters_intercept() {
    for (range, velocity, factor) in [
        (20000., DVec3::ZERO, 0.65),
        (1000., DVec3::ZERO, 1.),
        (50., DVec3::ZERO, 1.),
        (100., DVec3::ZERO, 1.),
        (2000., DVec3::X * 130., 1.),
    ] {
        let mut f = Flight::new(DVec3::ZERO, DVec3::X * range, velocity, DVec3::ZERO, false);
        f.thrust_factor = factor;
        f.engage();
        f.finish();
    }
}
#[test]
fn mounting_rotation_and_off_axis_thrust_are_compensated() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 2000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    let mut blueprint = f.design.blueprint.clone();
    blueprint.parts[6].attachment = Some(Attachment {
        parent: 6,
        socket: "right".into(),
        plug: "left".into(),
        roll: 0,
    });
    blueprint.parts[4].attachment.as_mut().unwrap().roll = 1;
    f.design = blueprint.compile(&f.cat).unwrap();
    f.hardware = fixture(&f.design, &f.cat);
    f.engage();
    f.finish();
}
#[test]
fn stock_firmware_flies_multiple_engines_and_rotated_torquers_without_aliases() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 2000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    let mut blueprint = f.design.blueprint.clone();
    for (id, socket, plug) in [(8, "left", "right"), (9, "right", "left")] {
        let mut engine = blueprint.parts[6].clone();
        engine.id = id;
        engine.attachment = Some(Attachment {
            parent: 6,
            socket: socket.into(),
            plug: plug.into(),
            roll: 0,
        });
        engine.alias.clear();
        blueprint.parts.push(engine);
    }
    let mut torquer = blueprint.parts[4].clone();
    torquer.id = 10;
    torquer.attachment = Some(Attachment {
        parent: 5,
        socket: "top".into(),
        plug: "bottom".into(),
        roll: 1,
    });
    torquer.alias.clear();
    blueprint.parts.push(torquer);
    for part in &mut blueprint.parts {
        part.alias.clear();
    }
    f.design = blueprint.compile(&f.cat).unwrap();
    f.hardware = fixture(&f.design, &f.cat);
    f.engage();
    f.finish();
}
#[test]
fn physics_accelerometer_subtracts_gravity_but_keeps_other_forces() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_duration(osg_model::TICK_DURATION))
        .add_systems(Update, apply_forces);
    app.world_mut()
        .resource_mut::<Time<Fixed>>()
        .advance_by(osg_model::TICK_DURATION);
    let ship = app
        .world_mut()
        .spawn((
            RigidBody,
            PreciseTransform::default(),
            MassProps {
                mass: 2.,
                ..default()
            },
            AccumulatedForce(DVec3::new(6., 0., 20.)),
            GravityAcceleration(DVec3::Z * 10.),
        ))
        .id();
    app.update();
    assert_eq!(
        app.world()
            .get::<AccelerometerState>(ship)
            .unwrap()
            .specific_force_body,
        DVec3::X * 3.
    );
    assert_eq!(
        app.world().get::<GravityAcceleration>(ship).unwrap().0,
        DVec3::ZERO
    );
}

#[test]
fn native_instrument_state_is_valid_during_a_transfer() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 50_000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.display = true;
    f.engage();
    for _ in 0..600 {
        let output = f.tick(vec![]);
        assert!(output.valid());
        assert!(f.pilot.navigation.throttle.is_finite());
    }
}

#[test]
fn intercepts_tumbling_target_at_ten_percent() {
    let universe = crate::sim::orrery::Universe::init(osg_universe::example_config()).unwrap();
    let mut scenario = crate::sim::scenario::INITIAL_SCENARIO.clone();
    scenario.traffic_count = 1;
    let body = universe
        .body(universe.authored_body(scenario.body).unwrap())
        .unwrap();
    let states = scenario.fleet_states(body.radius, body.mass).unwrap();
    let mut f = Flight::new(states[0].0, states[1].0, states[0].1, states[1].1, true);
    let mut target = fixture(&f.design, &f.cat);
    let commands = f
        .design
        .device_catalogue
        .iter()
        .filter_map(|d| {
            matches!(d.kind, DeviceKind::Engine { .. }).then_some(DeviceCommand {
                device: d.handle,
                setting: DeviceSetting::Throttle(crate::sim::scenario::TRAFFIC_CHALLENGE_THROTTLE),
            })
        })
        .collect::<Vec<_>>();
    target.commands(&commands).unwrap();
    f.target_hardware = Some(target);
    f.target_w = crate::sim::scenario::TRAFFIC_TUMBLE_BODY;
    f.engage();
    f.finish();
    assert!(f.pilot.navigation.u.length() <= 0.5);
}

#[test]
fn missed_pass_turns_back_for_another_intercept() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::new(1000., 300., 0.),
        DVec3::X * 1000.,
        DVec3::ZERO,
        false,
    );
    f.q = DQuat::from_rotation_arc(DVec3::NEG_Z, DVec3::X);
    f.engage();
    for _ in 0..30 {
        f.tick(vec![]);
    }
    assert!(f.p.x > f.target.x);
    assert!(
        f.min_separation > f.design.radius * 2.,
        "first pass must miss"
    );
    f.finish();
    assert_eq!(f.pilot.navigation.phase, Phase::Ready);
}

#[test]
fn preview_tracks_aligned_flight_and_is_cleared_on_abort() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::NEG_Z * 2000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.display = true;
    f.engage();
    let (origin, path) = f.forecast();
    let point = path
        .points
        .iter()
        .find(|point| path.epoch + point.seconds >= f.time - 1e-8)
        .unwrap();
    assert!(point.seconds > 0. && point.seconds <= 1.);
    while f.time + 1e-8 < path.epoch + point.seconds {
        f.tick(vec![]);
    }
    let predicted = origin + point.r + path.frame_velocity * point.seconds;
    assert!(
        predicted.distance(f.p) < 0.05,
        "prediction={} actual={}",
        predicted,
        f.p
    );
    f.tick(vec![Command::StopGuidance]);
    assert!(f.pilot.forecast().is_none());
}

#[test]
fn hardware_loss_cuts_thrust_during_contact_grace() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 50_000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.engage();
    f.visible = false;
    f.imu.time_s = None;
    let out = f.tick(vec![]);
    assert_eq!(f.pilot.navigation.phase, Phase::Paused);
    assert!(
        out.devices
            .iter()
            .any(|d| matches!(d.setting, DeviceSetting::Throttle(0.)))
    );
}

#[test]
fn overshoot_forecast_reports_braked_arrival() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::X * 1000.0,
        DVec3::X * 1000.0,
        DVec3::ZERO,
        false,
    );
    f.q = DQuat::from_rotation_arc(DVec3::NEG_Z, DVec3::X);
    f.display = true;
    f.engage();
    let (_, path) = f.forecast();
    assert!(path.eta.unwrap() > 2.0);
    assert!(path.points.last().unwrap().speed <= 0.5);
    assert!(path.points.last().unwrap().seconds >= path.eta.unwrap() + 9.9);
    assert!(path.points.iter().any(|p| p.speed >= 1000.0));
}

#[test]
fn target_already_at_requested_stand_off_requires_no_burn() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::NEG_Z * 100.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.engage();
    f.finish();
    assert_eq!(f.pilot.navigation.throttle, 0.);
    assert_eq!(f.pilot.navigation.stand_off, 100.);
}

#[test]
fn forecast_reaches_first_pass_and_retains_the_path_between_refreshes() {
    for range in [50_000., 1_000_000.] {
        let mut f = Flight::new(
            DVec3::ZERO,
            DVec3::NEG_Z * range,
            DVec3::ZERO,
            DVec3::ZERO,
            false,
        );
        f.display = true;
        f.engage();
        let (origin, path) = f.forecast();
        let end = path.points.last().unwrap();
        assert!(end.seconds > 30.);
        assert!(path.eta.unwrap() > 30. && path.eta.unwrap() <= end.seconds);
        assert!(path.points.iter().any(|point| point.speed > 500.0));
        assert!(path.points.iter().any(|point| {
            let predicted = origin + point.r + path.frame_velocity * point.seconds;
            predicted.distance(f.target) < range * 0.05
        }));
        assert!(path.points.len() <= abi::MAX_PATH_VERTICES as usize);
        f.tick(vec![]);
        assert!(
            !f.pilot.forecast_changed(),
            "retain the previous complete publication"
        );
    }
}

#[test]
fn complete_forecast_tracks_full_thrust_approach() {
    let mut f = Flight::new(
        DVec3::ZERO,
        DVec3::NEG_Z * 50_000.,
        DVec3::ZERO,
        DVec3::ZERO,
        false,
    );
    f.display = true;
    f.engage();
    let (origin, path) = f.forecast();
    let eta = path.eta.unwrap();
    assert!(eta > 30.);
    f.display = false;
    while f.time - path.epoch < eta {
        let t = f.time - path.epoch;
        let j = path
            .points
            .partition_point(|p| p.seconds < t)
            .clamp(1, path.points.len() - 1);
        let a = path.points[j - 1];
        let b = path.points[j];
        let p = a.r.lerp(b.r, (t - a.seconds) / (b.seconds - a.seconds));
        let predicted = origin + p + path.frame_velocity * t;
        let tolerance = (f.pilot.navigation.error().length() * 0.05).max(250.);
        assert!(
            predicted.distance(f.p) < tolerance,
            "t={} error={} tolerance={}",
            t,
            predicted.distance(f.p),
            tolerance
        );
        f.tick(vec![]);
    }
    assert!(f.pilot.navigation.u.length() <= 1.);
}

#[test]
fn default_ntr_patrol_reverses_heading_in_three_seconds() {
    let mut flight = Flight::new(DVec3::ZERO, DVec3::ZERO, DVec3::ZERO, DVec3::ZERO, false);
    flight.cat = Catalogue::builtin();
    flight.design = ntr_patrol().compile(&flight.cat).unwrap();
    flight.hardware = fixture(&flight.design, &flight.cat);
    for tick in 0..30 {
        flight.tick(if tick == 0 {
            vec![Command::AimDirection(DVec3::Z.to_array())]
        } else {
            vec![]
        });
        if (flight.q * DVec3::NEG_Z).angle_between(DVec3::Z) < 5.0_f64.to_radians()
            && flight.w.length() < 0.3
        {
            return;
        }
    }
    panic!(
        "turn after 3s: error={} deg, rate={} rad/s",
        (flight.q * DVec3::NEG_Z)
            .angle_between(DVec3::Z)
            .to_degrees(),
        flight.w.length()
    );
}
