use super::*;
use bevy::ecs::system::RunSystemOnce;
use std::{sync::Arc, time::Duration};

pub(crate) struct HardwareFixture {
    pub app: App,
    pub ship: Entity,
    pub design: Arc<CompiledShipDesign>,
}

pub(crate) struct Wrench {
    pub force: DVec3,
    pub torque: DVec3,
}

impl HardwareFixture {
    pub fn new(blueprint: ShipBlueprint) -> Self {
        let mut app = App::new();
        app.add_plugins(bevy::app::TaskPoolPlugin::default());
        let catalogue = Catalogue::builtin();
        let design = Arc::new(blueprint.compile(&catalogue).unwrap());
        app.insert_resource(ShipCatalogue(catalogue))
            .insert_resource(super::super::vessel::WasmRuntime::default())
            .insert_resource(Time::<Fixed>::from_hz(10.0));
        let ship = super::super::vessel::spawn_ship(
            app.world_mut(),
            design.clone(),
            PreciseTransform::default(),
            DVec3::ZERO,
            "Hardware fixture".into(),
        )
        .unwrap();
        app.world_mut().run_system_once(initialize).unwrap();
        app.world_mut().get_mut::<Avionics>(ship).unwrap().0.powered =
            design.parts.iter().any(|part| {
                matches!(
                    part.definition.equipment,
                    Equipment::Utility {
                        utility: osg_ships::utilities::UtilityDef::Command { .. }
                    }
                )
            });
        install(&mut app);
        Self { app, ship, design }
    }

    pub fn standard() -> Self {
        Self::new(starter(EXAMPLE_CONTROLLER.to_vec()))
    }

    pub fn state(&self) -> ShipState {
        snapshot(self.app.world(), self.ship).unwrap()
    }

    pub fn commands(&mut self, commands: &[DeviceCommand]) -> anyhow::Result<()> {
        let world = self.app.world_mut();
        let mut query = world.query::<HardwareWrite>();
        query
            .get_mut(world, self.ship)
            .unwrap()
            .apply_commands(&self.design, commands)
    }

    pub fn device(&self, alias: &str) -> DeviceHandle {
        self.design
            .device_catalogue
            .iter()
            .find(|d| d.alias == alias)
            .unwrap()
            .handle
    }

    pub fn set_inventory(&mut self, change: impl FnOnce(&mut Inventory)) {
        change(
            &mut self
                .app
                .world_mut()
                .get_mut::<ShipInventory>(self.ship)
                .unwrap()
                .0,
        );
    }

    pub fn set_operational(&mut self, handle: DeviceHandle, operational: bool) {
        let world = self.app.world_mut();
        if let Some(index) = self.design.part_for_device(handle) {
            let entity = world.get::<PartDevices>(self.ship).unwrap().0[index];
            world.get_mut::<Device>(entity).unwrap().0.operational = operational;
        } else {
            world.get_mut::<Avionics>(self.ship).unwrap().0.operational = operational;
        }
    }

    pub fn advance(&mut self) -> Wrench {
        let world = self.app.world_mut();
        world.get_mut::<AccumulatedForce>(self.ship).unwrap().0 = DVec3::ZERO;
        world.get_mut::<AccumulatedTorque>(self.ship).unwrap().0 = DVec3::ZERO;
        world
            .resource_mut::<Time<Fixed>>()
            .advance_by(Duration::from_millis(100));
        world.run_schedule(FixedUpdate);
        Wrench {
            force: world.get::<AccumulatedForce>(self.ship).unwrap().0,
            torque: world.get::<AccumulatedTorque>(self.ship).unwrap().0,
        }
    }
}

#[test]
fn command_module_powers_standard_avionics_and_logical_devices() {
    let mut fixture = HardwareFixture::standard();
    let design = fixture.design.clone();
    assert!(matches!(
        ShipBlueprint::default().firmware,
        Firmware::Standard
    ));
    assert_eq!(design.parts.len(), 9);
    let physical: f64 = design.parts.iter().map(|p| p.definition.mass_kg).sum();
    assert_eq!(design.dry_mass, physical + 71.0);
    for handle in design.avionics_handles {
        assert!(design.part_for_device(handle).is_none());
        assert_eq!(design.device_catalogue[handle.0 as usize].part_id, 0);
    }
    fixture.set_inventory(|inventory| inventory.quantities[1] = 0);
    let before = fixture.state().inventory.energy_j;
    fixture.advance();
    assert!(before - fixture.state().inventory.energy_j == 100_300);
    fixture
        .commands(&[DeviceCommand {
            device: design.avionics_handles[2],
            setting: DeviceSetting::SensorEnabled(false),
        }])
        .unwrap();
    let before = fixture.state().inventory.energy_j;
    fixture.advance();
    let state = fixture.state();
    assert!(before - state.inventory.energy_j == 100_200);
    assert_eq!(state.sensor_range, 0.0);
    fixture.set_operational(design.avionics_handles[0], false);
    fixture.advance();
    let state = fixture.state();
    assert!(!state.computer_running(&design));
    assert!(
        state
            .snapshot(&design)
            .iter()
            .zip(&design.device_sources)
            .all(|(status, source)| {
                !matches!(source, DeviceSource::Avionics) || !status.operational
            })
    );
}

#[test]
fn last_fraction_of_propellant_scales_thrust_and_energy_together() {
    let mut idle = HardwareFixture::standard();
    let idle_energy = idle.state().inventory.energy_j;
    idle.advance();
    let auxiliary_energy = idle_energy - idle.state().inventory.energy_j;

    let mut fixture = HardwareFixture::standard();
    let engine = &fixture.design.device_catalogue[fixture.device("main_engine").0 as usize];
    let DeviceKind::Engine {
        thrust_n,
        propellant_kg_s,
        power_w,
        ..
    } = engine.kind
    else {
        unreachable!()
    };
    let part = fixture
        .app
        .world()
        .get::<PartDevices>(fixture.ship)
        .unwrap()
        .0[fixture
        .design
        .part_for_device(fixture.device("main_engine"))
        .unwrap()];
    fixture
        .app
        .world_mut()
        .get_mut::<devices::Engine>(part)
        .unwrap()
        .propellant_kg_s = propellant_kg_s * 20.0;
    let fraction = 1.0 / (propellant_kg_s * 20.0 * 0.1);
    let expected_thrust = thrust_n * fraction;
    let expected_energy = power_w * fraction * 0.1;
    fixture.set_inventory(|inventory| inventory.quantities[0] = 1);
    let energy = fixture.state().inventory.energy_j;
    fixture
        .commands(&[DeviceCommand {
            device: fixture.device("main_engine"),
            setting: DeviceSetting::Throttle(1.0),
        }])
        .unwrap();
    let wrench = fixture.advance();
    assert!((wrench.force.z + expected_thrust).abs() < 1e-8);
    assert!(wrench.torque.length() < 1e-8);
    let state = fixture.state();
    assert_eq!(state.inventory.quantities[0], 0);
    assert!(
        ((energy - state.inventory.energy_j) as f64 - auxiliary_energy as f64 - expected_energy)
            .abs()
            <= 2.0
    );
    assert_eq!(state.sensor_range, 100_000_000.0);
    let mass = fixture.app.world().get::<MassProps>(fixture.ship).unwrap();
    assert!(mass.inertia.abs_diff_eq(
        fixture.design.inertia * (mass.mass / fixture.design.dry_mass),
        1e-8
    ));
}

#[test]
fn off_axis_engine_generates_lever_arm_torque() {
    let mut blueprint = starter(EXAMPLE_CONTROLLER.to_vec());
    blueprint.parts[6].attachment = Some(Attachment {
        parent: 6,
        socket: "right".into(),
        plug: "left".into(),
        roll: 0,
    });
    let mut fixture = HardwareFixture::new(blueprint);
    fixture
        .commands(&[DeviceCommand {
            device: fixture.device("main_engine"),
            setting: DeviceSetting::Throttle(1.0),
        }])
        .unwrap();
    let wrench = fixture.advance();
    assert!(wrench.torque.y.abs() > 1000.0);
    assert_eq!(
        wrench.torque,
        (fixture.design.parts[6].centre - fixture.design.centre).cross(wrench.force)
    );
}

#[test]
fn computer_power_loss_and_failure_neutralize_outputs() {
    for failed in [false, true] {
        let mut fixture = HardwareFixture::standard();
        let capacity = fixture.design.battery_j;
        fixture.set_inventory(|inventory| {
            inventory.energy_j = if failed { capacity } else { 5 };
            inventory.quantities[1] = 0;
        });
        fixture.set_operational(fixture.design.avionics_handles[0], !failed);
        let engine = fixture.device("main_engine");
        fixture
            .commands(&[DeviceCommand {
                device: engine,
                setting: DeviceSetting::Throttle(1.0),
            }])
            .unwrap();
        assert_eq!(fixture.advance().force, DVec3::ZERO);
        let state = fixture.state();
        assert!(!state.computer_running(&fixture.design));
        assert_eq!(
            state.settings[engine.0 as usize],
            Some(DeviceSetting::Throttle(0.0))
        );
    }
}

#[test]
fn device_handles_are_dense_and_commands_are_checked_atomically() {
    let mut fixture = HardwareFixture::standard();
    assert_eq!(
        fixture.design.device_catalogue.len(),
        fixture.design.parts.len()
    );
    for (index, descriptor) in fixture.design.device_catalogue.iter().enumerate() {
        assert_eq!(descriptor.handle.0 as usize, index);
    }
    let engine = fixture.device("main_engine");
    let radar = fixture.device("radar");
    let before = fixture.state().settings;
    for bad in [
        DeviceCommand {
            device: radar,
            setting: DeviceSetting::Throttle(1.0),
        },
        DeviceCommand {
            device: DeviceHandle(u16::MAX),
            setting: DeviceSetting::Throttle(1.0),
        },
        DeviceCommand {
            device: engine,
            setting: DeviceSetting::Throttle(f64::NAN),
        },
    ] {
        assert!(
            fixture
                .commands(&[
                    DeviceCommand {
                        device: engine,
                        setting: DeviceSetting::Throttle(0.5)
                    },
                    bad,
                ])
                .is_err()
        );
        assert_eq!(fixture.state().settings, before);
    }
    fixture
        .commands(&[DeviceCommand {
            device: engine,
            setting: DeviceSetting::Throttle(0.5),
        }])
        .unwrap();
    assert!(fixture.advance().force.z < -9000.0);
    assert!(fixture.advance().force.z < -9000.0);
    fixture.set_operational(engine, false);
    assert_eq!(fixture.advance().force, DVec3::ZERO);
    assert!(!fixture.state().snapshot(&fixture.design)[engine.0 as usize].operational);
}

#[test]
fn sensor_reading_tracks_power_and_enable_setting() {
    let mut fixture = HardwareFixture::standard();
    let radar = fixture.design.avionics_handles[2];
    fixture.advance();
    assert!(
        matches!(fixture.state().snapshot(&fixture.design)[radar.0 as usize].reading,
        DeviceReading::Sensor { range_m } if range_m > 0.0)
    );
    fixture
        .commands(&[DeviceCommand {
            device: radar,
            setting: DeviceSetting::SensorEnabled(false),
        }])
        .unwrap();
    fixture.advance();
    assert_eq!(fixture.state().sensor_range, 0.0);
    fixture.set_operational(radar, false);
    assert!(!fixture.state().snapshot(&fixture.design)[radar.0 as usize].operational);
}

#[test]
fn rcs_scales_all_axes_when_fuel_runs_out_and_applies_mount_torque() {
    let mut fixture = HardwareFixture::new(armed_starter());
    fixture.set_inventory(|inventory| inventory.quantities[0] = 1);
    let device = fixture
        .design
        .device_catalogue
        .iter()
        .find(|device| matches!(device.kind, DeviceKind::Rcs { .. }))
        .unwrap()
        .handle;
    let entity = fixture
        .app
        .world()
        .get::<PartDevices>(fixture.ship)
        .unwrap()
        .0[fixture.design.part_for_device(device).unwrap()];
    fixture
        .app
        .world_mut()
        .get_mut::<devices::ReactionControl>(entity)
        .unwrap()
        .propellant_kg_s *= 20.0;
    fixture
        .commands(&[DeviceCommand {
            device,
            setting: DeviceSetting::RcsThrust([2000.0, -2000.0, 0.0]),
        }])
        .unwrap();
    let part = &fixture.design.parts[fixture.design.part_for_device(device).unwrap()];
    let expected = part.rotation * DVec3::new(1000.0, -1000.0, 0.0);
    let torque = (part.centre - fixture.design.centre).cross(expected);
    let wrench = fixture.advance();
    assert!(wrench.force.distance(expected) < 1e-8);
    assert!(wrench.torque.distance(torque) < 1e-8);
    assert_eq!(fixture.state().inventory.quantities[0], 0);
    let wrench = fixture.advance();
    assert_eq!(wrench.force, DVec3::ZERO);
    assert_eq!(wrench.torque, DVec3::ZERO);
}

#[test]
fn every_shield_generator_must_be_powered_for_the_combined_field() {
    let mut blueprint = starter(EXAMPLE_CONTROLLER.to_vec());
    let mut second = blueprint
        .parts
        .iter()
        .find(|p| p.prototype == "shield")
        .unwrap()
        .clone();
    second.id = blueprint.parts.iter().map(|part| part.id).max().unwrap() + 1;
    second.alias = "second_shield".into();
    second.attachment = Some(Attachment {
        parent: 4,
        socket: "right".into(),
        plug: "left".into(),
        roll: 0,
    });
    blueprint.parts.push(second);
    let mut fixture = HardwareFixture::new(blueprint);
    fixture.advance();
    assert!(fixture.state().thermal.shield_powered);
    fixture.set_operational(fixture.device("second_shield"), false);
    fixture.advance();
    let mut state = fixture.state();
    assert!(!state.thermal.shield_powered);
    state.shield_activation(&fixture.design, true);
    assert!(!state.shield_active());
}

#[test]
fn failed_command_module_stops_computer_and_clears_thrust_commands() {
    let mut fixture = HardwareFixture::standard();
    let command_index = fixture
        .design
        .parts
        .iter()
        .position(|part| {
            matches!(
                part.definition.equipment,
                Equipment::Utility {
                    utility: osg_ships::utilities::UtilityDef::Command { .. }
                }
            )
        })
        .unwrap();
    fixture.advance();
    assert!(fixture.state().computer_running(&fixture.design));
    let engine = fixture.device("main_engine");
    fixture
        .commands(&[DeviceCommand {
            device: engine,
            setting: DeviceSetting::Throttle(1.0),
        }])
        .unwrap();
    let part = fixture
        .app
        .world()
        .get::<PartDevices>(fixture.ship)
        .unwrap()
        .0[command_index];
    fixture
        .app
        .world_mut()
        .get_mut::<Device>(part)
        .unwrap()
        .0
        .operational = false;
    assert_eq!(fixture.advance().force, DVec3::ZERO);
    let state = fixture.state();
    assert!(!state.computer_running(&fixture.design));
    assert_eq!(
        state.settings[engine.0 as usize],
        Some(DeviceSetting::Throttle(0.0))
    );
}
