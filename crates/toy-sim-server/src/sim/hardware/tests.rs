use super::*;
use crate::sim::physics::Velocity;
use bevy::ecs::system::RunSystemOnce;
use std::{sync::Arc, time::Duration};

fn ship() -> (World, Entity, Arc<CompiledShipDesign>) {
    ship_with_catalogue(Catalogue::builtin())
}

fn command_energy(design: &CompiledShipDesign) -> f64 {
    design
        .parts
        .iter()
        .filter_map(|part| match part.definition.equipment {
            Equipment::Utility {
                utility: toy_sim_ships::utilities::UtilityDef::Command { power_w },
            } => Some(power_w * 0.1),
            _ => None,
        })
        .sum()
}

fn ship_with_catalogue(catalogue: Catalogue) -> (World, Entity, Arc<CompiledShipDesign>) {
    bevy::tasks::ComputeTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(2).build()
    });
    let mut world = World::new();
    world.insert_resource(ShipCatalogue(catalogue));
    world.insert_resource(super::super::vessel::WasmRuntime::default());
    let mut time = Time::<Fixed>::from_hz(10.0);
    time.advance_by(Duration::from_millis(100));
    world.insert_resource(time);
    let design = Arc::new(
        toy_sim_ships::armed_starter()
            .compile(&world.resource::<ShipCatalogue>().0)
            .unwrap(),
    );
    let entity = super::super::vessel::spawn_ship(
        &mut world,
        design.clone(),
        PreciseTransform::default(),
        DVec3::ZERO,
        "Hardware regression".into(),
    )
    .unwrap();
    world.run_system_once(initialize).unwrap();
    world.get_mut::<Avionics>(entity).unwrap().0.powered = true;
    world
        .get_mut::<DeviceSettings>(entity)
        .unwrap()
        .0
        .fill(None);
    (world, entity, design)
}

#[test]
fn generators_conserve_fuel_and_account_for_conversion_heat() {
    let (mut world, ship, design) = ship();
    let (index, power, fuel_rate, efficiency) = design
        .parts
        .iter()
        .enumerate()
        .find_map(|(index, part)| match part.definition.equipment {
            Equipment::Generator {
                power_w,
                fuel_kg_s,
                efficiency,
            } => Some((index, power_w, fuel_kg_s, efficiency)),
            _ => None,
        })
        .unwrap();
    world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[index].unwrap()] =
        Some(DeviceSetting::GeneratorDemand(0.5));
    world.get_mut::<ShipInventory>(ship).unwrap().0.energy_j = 0;
    let initial_fuel = world.get::<ShipInventory>(ship).unwrap().0.quantities[1];
    let initial_heat = world
        .get::<ShipThermal>(ship)
        .unwrap()
        .0
        .pending_waste_heat_j;
    world.run_system_once(generators).unwrap();
    let inventory = &world.get::<ShipInventory>(ship).unwrap().0;
    let generated = (power * 0.1 * 0.5).min(design.battery_j as f64);
    assert!((inventory.energy_j as f64 - generated).abs() <= generated * 1e-10);
    assert!(
        ((initial_fuel - inventory.quantities[1]) as f64 - fuel_rate * generated / power).abs()
            < 1.0
    );
    let heat = world
        .get::<ShipThermal>(ship)
        .unwrap()
        .0
        .pending_waste_heat_j;
    assert!((heat - initial_heat - generated * (1.0 / efficiency - 1.0)).abs() < generated * 1e-8);
}

#[test]
fn engine_and_rcs_cannot_spend_energy_without_propellant() {
    let (mut world, ship, design) = ship();
    let mut selected = 0;
    for (index, part) in design.parts.iter().enumerate() {
        let setting = match part.definition.equipment {
            Equipment::Engine { .. } => Some(DeviceSetting::Throttle(1.0)),
            Equipment::Rcs { thrust_n, .. } => Some(DeviceSetting::RcsThrust([thrust_n; 3])),
            _ => None,
        };
        if let Some(setting) = setting {
            selected += 1;
            world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[index].unwrap()] =
                Some(setting);
        }
    }
    assert!(selected > 1);
    world.get_mut::<ShipInventory>(ship).unwrap().0.quantities[0] = 0;
    let energy = world.get::<ShipInventory>(ship).unwrap().0.energy_j;
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    assert_eq!(world.get::<AccumulatedForce>(ship).unwrap().0, DVec3::ZERO);
    assert_eq!(world.get::<AccumulatedTorque>(ship).unwrap().0, DVec3::ZERO);
    assert_eq!(
        world.get::<ShipInventory>(ship).unwrap().0.energy_j,
        energy - command_energy(&design) as u64
    );
}

#[test]
fn invalid_command_batch_does_not_apply_earlier_valid_commands() {
    let (mut world, ship, design) = ship();
    let descriptor = design
        .device_catalogue
        .iter()
        .find(|d| matches!(d.kind, DeviceKind::Engine { .. }))
        .unwrap();
    let handle = descriptor.handle;
    let mut query = world.query::<HardwareWrite>();
    let mut hardware = query.get_mut(&mut world, ship).unwrap();
    let before = hardware.settings.0.clone();
    let result = hardware.apply_commands(
        &design,
        &[
            DeviceCommand {
                device: handle,
                setting: DeviceSetting::Throttle(0.75),
            },
            DeviceCommand {
                device: handle,
                setting: DeviceSetting::Throttle(f64::NAN),
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(hardware.settings.0, before);
}

#[test]
fn shutdown_clears_powered_devices_without_consuming_inventory() {
    let (mut world, ship, design) = ship();
    for (index, part) in design.parts.iter().enumerate() {
        if matches!(part.definition.equipment, Equipment::Engine { .. }) {
            world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[index].unwrap()] =
                Some(DeviceSetting::Throttle(1.0));
        }
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    let energy = world.get::<ShipInventory>(ship).unwrap().0.energy_j;

    shutdown(&mut world, ship);
    world
        .entity_mut(ship)
        .insert(super::super::travel::SystemsSuspended);
    world.run_system_once(generators).unwrap();
    schedule.run(&mut world);

    assert!(!world.get::<Avionics>(ship).unwrap().0.powered);
    assert_eq!(world.get::<SensorRange>(ship).unwrap().0, 0.0);
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, energy);
    for entity in &world.get::<PartDevices>(ship).unwrap().0 {
        assert!(world.get::<ActiveDevice>(*entity).is_none());
        let device = &world.get::<Device>(*entity).unwrap().0;
        assert!(!device.powered);
        assert_eq!(device.actual, 0.0);
        assert_eq!(device.thrust_n, [0.0; 3]);
        assert_eq!(world.get::<DevicePower>(*entity).unwrap().supplied_w, 0.0);
    }
}

fn micropulse_ship() -> (World, Entity, Arc<CompiledShipDesign>, usize, Entity) {
    let mut catalogue = Catalogue::builtin();
    for part in &mut catalogue.parts {
        if matches!(part.equipment, Equipment::Engine { .. }) {
            part.equipment = Equipment::MicropulseEngine {
                thrust_n: 1_000_000.0,
                specific_impulse_s: 5_000.0,
                charge_energy_j_kg: 3e9,
                electric_efficiency: 0.99,
                absorbed_heat_fraction: 0.001,
                plume: None,
            };
        }
    }
    let charge_index = catalogue
        .resources
        .iter()
        .position(|r| r.id == "micropulse_charge")
        .unwrap();
    catalogue.resources[charge_index].mass_kg = 2.0;
    catalogue.parts.retain(
        |p| !matches!(p.equipment, Equipment::FuelProcessor { spec } if spec.produces_charges),
    );
    let (mut world, ship, design) = ship_with_catalogue(catalogue);
    let part_index = design
        .parts
        .iter()
        .position(|part| {
            matches!(
                part.definition.equipment,
                Equipment::MicropulseEngine { .. }
            )
        })
        .unwrap();
    world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[part_index].unwrap()] =
        Some(DeviceSetting::Throttle(0.5));
    let engine = world.get::<PartDevices>(ship).unwrap().0[part_index];
    let mut inventory = world.get_mut::<ShipInventory>(ship).unwrap();
    inventory.0.quantities.fill(0);
    inventory.0.quantities[charge_index] = 100;
    inventory.0.energy_j = 0;
    (world, ship, design, charge_index, engine)
}

#[test]
fn slip_keeps_power_and_shields_running_without_propulsion() {
    let (mut world, ship, design, _, engine) = micropulse_ship();
    let part = world.get::<InstalledPart>(engine).unwrap().index;
    world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_generators[part].unwrap()] =
        Some(DeviceSetting::GeneratorDemand(1.0));
    for (index, part) in design.parts.iter().enumerate() {
        if matches!(part.definition.equipment, Equipment::Shield { .. }) {
            world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[index].unwrap()] =
                Some(DeviceSetting::ShieldEnabled(true));
        }
    }
    let velocity = DVec3::new(10.0, 20.0, 30.0);
    world.get_mut::<Velocity>(ship).unwrap().0 = velocity;
    super::super::travel::set_dormant(
        &mut world,
        ship,
        toy_sim_model::travel::Presence::SlipTransit(toy_sim_model::Id::new()),
    );
    assert!(
        world
            .get::<super::super::travel::SystemsSuspended>(ship)
            .is_none()
    );
    assert!(world.get::<Velocity>(ship).is_none());
    assert_eq!(
        world
            .get::<super::super::travel::DormantMotion>(ship)
            .unwrap()
            .velocity,
        velocity
    );
    let mut schedule = Schedule::default();
    schedule.add_systems((begin, device_systems()).chain());
    schedule.run(&mut world);
    assert!(world.get::<DevicePower>(engine).unwrap().recovered_w > 0.0);
    assert_eq!(world.get::<Device>(engine).unwrap().0.actual, 0.0);
    assert!(world.get::<Avionics>(ship).unwrap().0.powered);
    assert_eq!(world.get::<SensorRange>(ship).unwrap().0, 0.0);
    assert!(world.get::<ShipThermal>(ship).unwrap().0.shield_powered);
    {
        let mut thermal = world.get_mut::<ShipThermal>(ship).unwrap();
        thermal.0.shield_deployed_kg = design.shield_deployed_kg;
        thermal.0.shield_energy_j = 10_000_000.0;
        thermal.0.hull_energy_j = 0.0;
    }
    world.run_system_once(transit_thermal).unwrap();
    assert!(world.get::<ShipThermal>(ship).unwrap().0.shield_energy_j < 10_000_000.0);
}

#[test]
fn micropulse_charges_supply_thrust_heat_and_recovered_power_without_propellant_or_electric_input()
{
    let (mut world, ship, design, charge, engine) = micropulse_ship();
    let part = world.get::<InstalledPart>(engine).unwrap().index;
    world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_generators[part].unwrap()] =
        Some(DeviceSetting::GeneratorDemand(0.25));
    let initial_mass = world
        .get::<ShipInventory>(ship)
        .unwrap()
        .0
        .mass(&world.resource::<ShipCatalogue>().0);
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);

    let consumed_kg = 500_000.0 / (9.80665 * 5_000.0) * 0.1;
    let resource_mass = world.resource::<ShipCatalogue>().0.resources[charge].mass_kg;
    let inventory = &world.get::<ShipInventory>(ship).unwrap().0;
    assert!((inventory.available(charge) - (100.0 - consumed_kg / resource_mass)).abs() < 1.0);
    assert_eq!(inventory.quantities[0], 0);
    assert_eq!(inventory.quantities[1], 0);
    assert!(
        (inventory.mass(&world.resource::<ShipCatalogue>().0) - initial_mass + consumed_kg).abs()
            < resource_mass
    );
    let generated = 0.005 * 1_000_000.0 * 9.80665 * 5_000.0 * 0.25 * 0.1;
    assert!((inventory.energy_j as f64 - generated + command_energy(&design)).abs() <= 1.0);
    let extracted = generated / 0.99;
    let expected_force = 500_000.0
        * (1.0 - extracted / (0.5 * consumed_kg * (9.80665_f64 * 5_000.0).powi(2))).sqrt();
    assert!(
        (world.get::<AccumulatedForce>(ship).unwrap().0.length() - expected_force).abs() < 1e-6
    );
    assert!(
        (world
            .get::<ShipThermal>(ship)
            .unwrap()
            .0
            .pending_waste_heat_j
            - consumed_kg * 3e9 * 0.001
            - extracted
            + generated
            - command_energy(&design))
        .abs()
            < 1e-6
    );
    assert!((world.get::<Device>(engine).unwrap().0.actual - expected_force).abs() < 1e-6);
    assert_eq!(world.get::<DevicePower>(engine).unwrap().supplied_w, 0.0);
    assert!(world.get::<DevicePower>(engine).unwrap().recovered_w > 0.0);
}

#[test]
fn micropulse_generator_runs_without_thrust_and_stops_when_charges_run_out() {
    let (mut world, ship, design, charge, engine) = micropulse_ship();
    let part = world.get::<InstalledPart>(engine).unwrap().index;
    let generator = design.part_generators[part].unwrap();
    world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[part].unwrap()] =
        Some(DeviceSetting::Throttle(0.0));
    world.get_mut::<DeviceSettings>(ship).unwrap().0[generator] =
        Some(DeviceSetting::GeneratorDemand(1.0));
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    assert_eq!(world.get::<AccumulatedForce>(ship).unwrap().0, DVec3::ZERO);
    assert!(world.get::<ShipInventory>(ship).unwrap().0.energy_j > 0);
    assert!(world.get::<Device>(engine).unwrap().0.generated_w > 0.0);
    assert!(world.get::<ShipInventory>(ship).unwrap().0.quantities[charge] < 100);
    let state = snapshot(&world, ship).unwrap();
    let status = state.snapshot(&design);
    assert!(
        matches!(status[generator].reading, DeviceReading::Generator { power_w } if power_w > 0.0)
    );

    world.get_mut::<ShipInventory>(ship).unwrap().0.energy_j = 0;
    world.get_mut::<ShipInventory>(ship).unwrap().0.quantities[charge] = 0;
    schedule.run(&mut world);
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, 0);
    assert_eq!(world.get::<Device>(engine).unwrap().0.generated_w, 0.0);
}

#[test]
fn micropulse_generation_supplies_a_load_larger_than_the_battery_in_one_tick() {
    let (mut world, ship, design, charge, engine) = micropulse_ship();
    let part = world.get::<InstalledPart>(engine).unwrap().index;
    let load = design.battery_j as f64 * 20.0;
    world
        .get_mut::<devices::MicropulseEngine>(engine)
        .unwrap()
        .thrust_n = load * 2.0 / (0.005 * 9.80665 * 5_000.0);
    let shield = world
        .get::<PartDevices>(ship)
        .unwrap()
        .0
        .iter()
        .copied()
        .find(|entity| world.get::<devices::Shield>(*entity).is_some())
        .unwrap();
    let shield_part = world.get::<InstalledPart>(shield).unwrap().index;
    world.get_mut::<devices::Shield>(shield).unwrap().power_w = load;
    let mut settings = world.get_mut::<DeviceSettings>(ship).unwrap();
    settings.0[design.part_devices[part].unwrap()] = Some(DeviceSetting::Throttle(0.0));
    settings.0[design.part_generators[part].unwrap()] = Some(DeviceSetting::GeneratorDemand(1.0));
    settings.0[design.part_devices[shield_part].unwrap()] =
        Some(DeviceSetting::ShieldEnabled(true));
    world.get_mut::<ShipInventory>(ship).unwrap().0.quantities[charge] = 1_000_000;
    let mut schedule = Schedule::default();
    schedule.add_systems((device_systems(), finish_electrical_tick).chain());
    schedule.run(&mut world);
    assert!(world.get::<Device>(shield).unwrap().0.powered);
    assert!((world.get::<DevicePower>(shield).unwrap().supplied_w - load).abs() <= 10.0);
    assert!(world.get::<ShipInventory>(ship).unwrap().0.energy_j <= design.battery_j);
    assert_eq!(world.get::<AccumulatedForce>(ship).unwrap().0, DVec3::ZERO);
}

#[test]
fn expedition_patrol_boots_and_supplies_avionics_without_a_reactor() {
    let mut fixture = fixtures::HardwareFixture::new(expedition_patrol());
    assert!(!fixture.design.parts.iter().any(|part| matches!(
        part.definition.equipment,
        Equipment::Reactor { .. } | Equipment::Generator { .. }
    )));
    fixture.set_inventory(|inventory| inventory.energy_j = 0);
    for _ in 0..3 {
        fixture.advance();
        assert!(fixture.state().avionics.powered);
    }
    let world = fixture.app.world();
    assert!(world.get::<PowerFlow>(fixture.ship).unwrap().generated_w > 0.0);
    assert!(fixture.state().inventory.energy_j <= fixture.design.battery_j);
}

#[test]
fn expedition_laser_power_is_available_before_physics_and_reported_after_spending() {
    for firing in [false, true] {
        let mut fixture = fixtures::HardwareFixture::new(expedition_patrol());
        let commands: Vec<_> = fixture
            .design
            .weapon_parts
            .iter()
            .map(|&part| DeviceCommand {
                device: DeviceHandle(fixture.design.part_devices[part].unwrap() as u16),
                setting: DeviceSetting::Weapon(toy_sim_ship_api::abi::WeaponSetting {
                    aim_direction: [0.0, 0.0, -1.0],
                    maximum_pointing_error_rad: 0.1,
                    valid_until_s: 100.0,
                    trigger: 1,
                    ..default()
                }),
            })
            .collect();
        fixture.commands(&commands).unwrap();
        let needed_j: f64 = fixture
            .design
            .weapon_specs
            .iter()
            .map(|spec| weapons::shot_energy(spec) * (0.1 / spec.cycle_interval_s).ceil())
            .sum();
        assert!(needed_j > fixture.design.battery_j as f64);
        fixture.advance();
        let world = fixture.app.world_mut();
        let mut inventory = world.get_mut::<ShipInventory>(fixture.ship).unwrap();
        assert!(inventory.0.energy_j as f64 >= needed_j);
        if firing {
            let paid = inventory.0.energy_j.withdraw(needed_j);
            assert!((paid as f64 - needed_j).abs() <= 1.0);
        }
        let before_heat = world
            .get::<ShipThermal>(fixture.ship)
            .unwrap()
            .0
            .pending_waste_heat_j;
        let before_fuel = world
            .get::<ShipInventory>(fixture.ship)
            .unwrap()
            .0
            .quantities
            .clone();
        world.run_system_once(finish_electrical_tick).unwrap();
        let flow = world.get::<PowerFlow>(fixture.ship).unwrap();
        if firing {
            assert!(flow.supplied_w >= needed_j * 10.0 - 10.0);
        } else {
            assert!(flow.supplied_w < needed_j);
            assert!(
                (world
                    .get::<ShipThermal>(fixture.ship)
                    .unwrap()
                    .0
                    .pending_waste_heat_j
                    - before_heat)
                    .abs()
                    < 1.0
            );
        }
        assert_eq!(
            world
                .get::<ShipInventory>(fixture.ship)
                .unwrap()
                .0
                .quantities,
            before_fuel
        );
        assert!(flow.requested_w >= needed_j * 10.0);
        assert!(
            world.get::<ShipInventory>(fixture.ship).unwrap().0.energy_j
                <= fixture.design.battery_j
        );
    }
}

#[test]
fn micropulse_charge_shortage_scales_output_and_full_battery_does_not_add_heat() {
    let (mut world, ship, design, charge, engine) = micropulse_ship();
    world
        .get_mut::<devices::MicropulseEngine>(engine)
        .unwrap()
        .thrust_n *= 4.0;
    let wanted_kg = 2_000_000.0 / (9.80665 * 5_000.0) * 0.1;
    let resource_mass = world.resource::<ShipCatalogue>().0.resources[charge].mass_kg;
    {
        let mut inventory = world.get_mut::<ShipInventory>(ship).unwrap();
        inventory.0.quantities[charge] = 1;
        inventory.0.energy_j = design.battery_j;
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    assert_eq!(
        world.get::<ShipInventory>(ship).unwrap().0.quantities[charge],
        0
    );
    assert_eq!(
        world.get::<ShipInventory>(ship).unwrap().0.energy_j,
        design.battery_j - command_energy(&design) as u64
    );
    assert!(
        (world.get::<Device>(engine).unwrap().0.actual - 2_000_000.0 * resource_mass / wanted_kg)
            .abs()
            < 1e-6
    );
    assert!(
        (world
            .get::<ShipThermal>(ship)
            .unwrap()
            .0
            .pending_waste_heat_j
            - resource_mass * 3e9 * 0.001
            - command_energy(&design))
        .abs()
            < 1e-6
    );
    assert_eq!(world.get::<DevicePower>(engine).unwrap().recovered_w, 0.0);
    schedule.run(&mut world);
    assert_eq!(world.get::<Device>(engine).unwrap().0.actual, 0.0);
}

#[test]
fn broken_disabled_and_dormant_micropulse_engines_do_not_consume_charges() {
    for mode in 0..4 {
        let (mut world, ship, design, charge, engine) = micropulse_ship();
        match mode {
            0 => world.get_mut::<Device>(engine).unwrap().0.operational = false,
            1 => world.get_mut::<DeviceSettings>(ship).unwrap().0.fill(None),
            2 => {
                shutdown(&mut world, ship);
                world
                    .entity_mut(ship)
                    .insert(super::super::travel::SystemsSuspended);
            }
            _ => world.get_mut::<Hull>(ship).unwrap().0 = 0.0,
        }
        let mut schedule = Schedule::default();
        schedule.add_systems(device_systems());
        schedule.run(&mut world);
        assert_eq!(
            world.get::<ShipInventory>(ship).unwrap().0.quantities[charge],
            100
        );
        assert_eq!(world.get::<AccumulatedForce>(ship).unwrap().0, DVec3::ZERO);
        assert_eq!(
            world
                .get::<ShipThermal>(ship)
                .unwrap()
                .0
                .pending_waste_heat_j,
            0.0
        );
        assert!(design.battery_j > 0);
    }
}

#[test]
fn idle_micropulse_engine_reports_charge_availability_without_consuming_resources() {
    let (mut world, ship, _, charge, engine) = micropulse_ship();
    world.get_mut::<DeviceSettings>(ship).unwrap().0.fill(None);
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    assert!(world.get::<Device>(engine).unwrap().0.powered);
    assert_eq!(world.get::<Device>(engine).unwrap().0.actual, 0.0);
    assert_eq!(
        world.get::<ShipInventory>(ship).unwrap().0.quantities[charge],
        100
    );
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, 0);
    assert_eq!(
        world
            .get::<ShipThermal>(ship)
            .unwrap()
            .0
            .pending_waste_heat_j,
        0.0
    );
    assert_eq!(world.get::<AccumulatedForce>(ship).unwrap().0, DVec3::ZERO);

    world.get_mut::<ShipInventory>(ship).unwrap().0.quantities[charge] = 0;
    schedule.run(&mut world);
    assert!(!world.get::<Device>(engine).unwrap().0.powered);
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, 0);
    assert_eq!(
        world
            .get::<ShipThermal>(ship)
            .unwrap()
            .0
            .pending_waste_heat_j,
        0.0
    );
}

fn thermal_ship() -> (
    World,
    Entity,
    Arc<CompiledShipDesign>,
    usize,
    usize,
    usize,
    Entity,
) {
    let mut catalogue = Catalogue::builtin();
    catalogue
        .parts
        .iter_mut()
        .find(|p| p.id == "engine")
        .unwrap()
        .equipment = Equipment::ThermalEngine {
        thrust_n: 1_000_000.0,
        specific_impulse_s: 900.0,
        propellant_resource: "hydrogen".into(),
        thermal_efficiency: 0.9,
        fuel_energy_j_kg: 6e13,
        plume: None,
    };
    let resource = |id| catalogue.resources.iter().position(|r| r.id == id).unwrap();
    let hydrogen = resource("hydrogen");
    let fuel = resource("reactor_fuel");
    let spent = resource("spent_fuel");
    let (mut world, ship, design) = ship_with_catalogue(catalogue);
    let index = design
        .parts
        .iter()
        .position(|p| matches!(p.definition.equipment, Equipment::ThermalEngine { .. }))
        .unwrap();
    world.get_mut::<DeviceSettings>(ship).unwrap().0[design.part_devices[index].unwrap()] =
        Some(DeviceSetting::Throttle(1.0));
    let engine = world.get::<PartDevices>(ship).unwrap().0[index];
    let mut inventory = world.get_mut::<ShipInventory>(ship).unwrap();
    inventory.0.quantities.fill(0);
    inventory.0.quantities[hydrogen] = 100;
    inventory.0.tank_capacities_m3[hydrogen] = 2.0;
    inventory.0.quantities[fuel] = 1;
    inventory.0.tank_capacities_m3[spent] = 1.0;
    inventory.0.energy_j = design.battery_j;
    (world, ship, design, hydrogen, fuel, spent, engine)
}

#[test]
fn thermal_engine_shortage_scales_both_inputs_and_retains_spent_fuel() {
    let (mut world, ship, design, hydrogen, fuel, spent, engine) = thermal_ship();
    let exhaust_velocity = 900.0 * STANDARD_GRAVITY_M_S2;
    let full_energy = 0.5 * 1_000_000.0 * exhaust_velocity * 0.1 / 0.9;
    world
        .get_mut::<devices::ThermalEngine>(engine)
        .unwrap()
        .fuel_energy_j_kg = full_energy / 4.0;
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    let inventory = &world.get::<ShipInventory>(ship).unwrap().0;
    assert_eq!(inventory.quantities[fuel], 0);
    assert_eq!(inventory.quantities[spent], 1);
    assert!(
        (inventory.available(hydrogen) - (100.0 - 250_000.0 / exhaust_velocity * 0.1)).abs() < 1.0
    );
    assert_eq!(
        inventory.energy_j,
        design.battery_j - command_energy(&design) as u64
    );
    assert!((world.get::<Device>(engine).unwrap().0.actual - 250_000.0).abs() < 1e-8);
    let delayed = world
        .get::<devices::ThermalEngine>(engine)
        .unwrap()
        .decay_energy_j;
    assert!((delayed - full_energy * 0.25 * 0.06).abs() < 1e-6);
    let prompt_heat = world
        .get::<ShipThermal>(ship)
        .unwrap()
        .0
        .pending_waste_heat_j;
    let jet_energy = full_energy * 0.25 * 0.9;
    assert!(
        (prompt_heat + delayed + jet_energy - full_energy * 0.25 - command_energy(&design)).abs()
            < 1e-6
    );
}

#[test]
fn thermal_engine_decay_continues_when_docked_without_new_fission() {
    let (mut world, ship, _, _, fuel, _, engine) = thermal_ship();
    let mut schedule = Schedule::default();
    schedule.add_systems(device_systems());
    schedule.run(&mut world);
    let before = world
        .get::<devices::ThermalEngine>(engine)
        .unwrap()
        .decay_energy_j;
    assert!(before > 0.0);
    let fuel_before = world.get::<ShipInventory>(ship).unwrap().0.quantities[fuel];
    world
        .entity_mut(ship)
        .insert(super::super::travel::SystemsSuspended);
    schedule.run(&mut world);
    assert_eq!(
        world.get::<ShipInventory>(ship).unwrap().0.quantities[fuel],
        fuel_before
    );
    let after = world
        .get::<devices::ThermalEngine>(engine)
        .unwrap()
        .decay_energy_j;
    assert!((after - before * (-0.1_f64 / 120.0).exp()).abs() < 1e-6);
}

#[test]
fn thermal_engine_stops_when_cooling_sink_is_hot() {
    for shield_available in [false, true] {
        let (mut world, ship, design, hydrogen, fuel, _, engine) = thermal_ship();
        let mut thermal = world.get_mut::<ShipThermal>(ship).unwrap();
        if shield_available {
            thermal.0.shield_state = toy_sim_ship_api::abi::SHIELD_ACTIVE;
            thermal.0.shield_energy_j =
                thermal.0.shield_deployed_kg * toy_sim_ships::thermal::SPECIFIC_HEAT * 3000.0;
        } else {
            thermal.0.shield_deployed_kg = 0.0;
            thermal.0.hull_energy_j = design.hull_heat_capacity_j * 3.0;
        }
        let before = world
            .get::<ShipInventory>(ship)
            .unwrap()
            .0
            .quantities
            .clone();
        let mut schedule = Schedule::default();
        schedule.add_systems(device_systems());
        schedule.run(&mut world);
        let inventory = &world.get::<ShipInventory>(ship).unwrap().0;
        assert_eq!(inventory.quantities[hydrogen], before[hydrogen]);
        assert_eq!(inventory.quantities[fuel], before[fuel]);
        assert_eq!(world.get::<Device>(engine).unwrap().0.actual, 0.0);
    }
}

#[test]
fn partial_device_power_cannot_overdraw_a_depleted_battery() {
    let (mut world, ship, design) = ship();
    let index = design.active_parts[0];
    let part = world.get::<PartDevices>(ship).unwrap().0[index];
    world.entity_mut(part).insert(Demand {
        inputs: [0.0, 0.0, 300.7],
        actual: 1.0,
        enabled: true,
        ..Default::default()
    });
    world.get_mut::<ShipInventory>(ship).unwrap().0.energy_j = 1;

    world.run_system_once(actuate).unwrap();

    let energy = world.get::<ShipInventory>(ship).unwrap().0.energy_j;
    assert_eq!(energy, 0);
    let output = &world.get::<DeviceOutputs>(ship).unwrap().0[index];
    assert!((output.actual - 1.0 / 300.7).abs() < 1e-12);
    assert!((output.power.supplied_w * 0.1 - 1.0).abs() < 1e-12);
}
