use super::*;
use bevy::ecs::system::RunSystemOnce;
use std::{sync::Arc, time::Duration};

fn ship() -> (World, Entity, Arc<CompiledShipDesign>) {
    ship_with_catalogue(Catalogue::builtin())
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
    world.get_mut::<ShipInventory>(ship).unwrap().0.energy_j = 0.0;
    let initial_fuel = world.get::<ShipInventory>(ship).unwrap().0.quantities[1];
    let initial_heat = world
        .get::<ShipThermal>(ship)
        .unwrap()
        .0
        .pending_waste_heat_j;
    world.run_system_once(generators).unwrap();
    let inventory = &world.get::<ShipInventory>(ship).unwrap().0;
    let generated = (power * 0.1 * 0.5).min(design.battery_j);
    assert!((inventory.energy_j - generated).abs() <= generated * 1e-10);
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
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, energy);
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
    world.entity_mut(ship).insert(super::super::travel::Dormant);
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
                electric_fraction: 0.0005,
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
    inventory.0.energy_j = 0.0;
    (world, ship, design, charge_index, engine)
}

#[test]
fn micropulse_charges_supply_thrust_heat_and_recovered_power_without_propellant_or_electric_input()
{
    let (mut world, ship, design, charge, engine) = micropulse_ship();
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
    assert!((inventory.energy_j - (consumed_kg * 3e9 * 0.0005).min(design.battery_j)).abs() < 1e-6);
    assert!((world.get::<AccumulatedForce>(ship).unwrap().0.length() - 500_000.0).abs() < 1e-6);
    assert!(
        (world
            .get::<ShipThermal>(ship)
            .unwrap()
            .0
            .pending_waste_heat_j
            - consumed_kg * 3e9 * 0.001)
            .abs()
            < 1e-6
    );
    assert_eq!(world.get::<Device>(engine).unwrap().0.actual, 500_000.0);
    assert_eq!(world.get::<DevicePower>(engine).unwrap().supplied_w, 0.0);
    assert!(world.get::<DevicePower>(engine).unwrap().recovered_w > 0.0);
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
        design.battery_j
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
            - resource_mass * 3e9 * 0.001)
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
                world.entity_mut(ship).insert(super::super::travel::Dormant);
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
        assert!(design.battery_j > 0.0);
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
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, 0.0);
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
    assert_eq!(world.get::<ShipInventory>(ship).unwrap().0.energy_j, 0.0);
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
    inventory.0.energy_j = 0.0;
    (world, ship, design, hydrogen, fuel, spent, engine)
}

#[test]
fn thermal_engine_shortage_scales_both_inputs_and_retains_spent_fuel() {
    let (mut world, ship, _, hydrogen, fuel, spent, engine) = thermal_ship();
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
    assert_eq!(inventory.energy_j, 0.0);
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
    assert!((prompt_heat + delayed + jet_energy - full_energy * 0.25).abs() < 1e-6);
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
    world.entity_mut(ship).insert(super::super::travel::Dormant);
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
