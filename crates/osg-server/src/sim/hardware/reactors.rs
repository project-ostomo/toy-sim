use super::*;
use osg_ships::StochasticRound;
use osg_ships::reactors::{FuelProcessorSpec, ReactorSpec};

#[derive(Component)]
pub struct Reactor {
    pub spec: ReactorSpec,
    pub core_energy_j: f64,
    pub decay_energy_j: f64,
    pub shutdown: bool,
}

#[derive(Component)]
pub struct FuelProcessor(pub FuelProcessorSpec);

#[derive(Component)]
pub struct DockedSink(pub f64);

pub(crate) fn dock_heat_transfer(
    mut commands: Commands,
    docked: Query<
        (Entity, &super::super::travel::PresenceState),
        With<super::super::travel::SystemsSuspended>,
    >,
    identities: Option<Res<super::super::identity::IdentityIndex>>,
    designs: Query<&ShipDesign>,
    mut thermal: Query<&mut ShipThermal>,
) {
    let _profile =
        crate::sim::diagnostics::ProfileScope::new("hardware.reactors.dock_heat_transfer");
    let Some(identities) = identities else {
        return;
    };
    for (ship, presence) in &docked {
        let osg_model::travel::Presence::Docked { host, .. } = presence.0 else {
            commands.entity(ship).remove::<DockedSink>();
            continue;
        };
        let Some(&host) = identities.entries().get(&host) else {
            continue;
        };
        let Ok(design) = designs.get(host) else {
            continue;
        };
        let Ok([mut guest, mut host_thermal]) = thermal.get_many_mut([ship, host]) else {
            continue;
        };
        let sink = sink_temperature(&host_thermal.0, &design.0);
        commands.entity(ship).insert(DockedSink(sink));
        let energy = std::mem::take(&mut guest.0.hull_energy_j)
            + std::mem::take(&mut guest.0.pending_waste_heat_j)
            + std::mem::take(&mut guest.0.shield_energy_j);
        guest.0.waste_heat_remaining_s = 0.0;
        host_thermal.0.add_hull_heat(energy);
    }
}

pub(crate) fn sink_temperature(
    thermal: &thermal::ThermalState,
    design: &CompiledShipDesign,
) -> f64 {
    if thermal.shield_deployed_kg > 0.0
        && matches!(
            thermal.shield_state,
            osg_ship_api::abi::SHIELD_ACTIVE | osg_ship_api::abi::SHIELD_DEPLETED
        )
    {
        thermal.shield_temperature(design.into())
    } else {
        300.0 + 1200.0 * thermal.hull_energy_j / design.hull_heat_capacity_j.max(1.0)
    }
}

pub fn install(part: &mut EntityCommands, equipment: &Equipment) {
    match equipment {
        Equipment::Reactor { spec } => {
            part.insert(Reactor {
                spec: *spec,
                core_energy_j: 0.0,
                decay_energy_j: 0.0,
                shutdown: false,
            });
        }
        Equipment::FuelProcessor { spec } => {
            part.insert(FuelProcessor(*spec));
        }
        _ => {}
    }
}

fn resource(cat: &Catalogue, name: &str) -> usize {
    cat.resources
        .iter()
        .position(|r| r.id == name)
        .expect("nuclear resource")
}

fn convert(
    inventory: &mut Inventory,
    cat: &Catalogue,
    source: usize,
    target: usize,
    amount: f64,
) -> bool {
    if amount <= 0.0 || inventory.available(source) < amount {
        return false;
    }
    if amount.ceil() > inventory.tank_room(target, cat) as f64 {
        return false;
    }
    let units = (amount)
        .stochastic_round()
        .min(inventory.quantities[source]);
    inventory.quantities[source] -= units;
    inventory
        .insert_consumable(target, units, cat)
        .expect("reserved tank space");
    true
}

#[derive(QueryData)]
#[query_data(mutable)]
pub(crate) struct ReactorHardware {
    inventory: &'static mut ShipInventory,
    hull: &'static mut Hull,
    thermal: &'static mut ShipThermal,
    settings: &'static DeviceSettings,
    parts: &'static PartDevices,
}

pub(crate) fn generate(
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut ships: Query<(
        &ShipDesign,
        ReactorHardware,
        &mut DeviceOutputs,
        Has<super::super::travel::SystemsSuspended>,
        Option<&DockedSink>,
    )>,
    mut reactors: Query<(&mut Reactor, &mut Device)>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("hardware.reactors.generate");
    let dt = time.delta_secs_f64();
    if reactors.is_empty() {
        return;
    }
    let fuel = resource(&cat.0, "reactor_fuel");
    let spent = resource(&cat.0, "spent_fuel");
    let fertile = resource(&cat.0, "fertile_feedstock");
    let bred = resource(&cat.0, "bred_fuel");
    for (design, mut hardware, mut outputs, dormant, docked_sink) in &mut ships {
        for &index in &design.0.active_parts {
            let Ok((mut reactor, mut device)) = reactors.get_mut(hardware.parts.0[index]) else {
                continue;
            };
            let spec = reactor.spec;
            let core_k = 300.0 + reactor.core_energy_j / spec.core_heat_capacity_j_k;
            let sink_k = if dormant {
                docked_sink.map_or_else(
                    || sink_temperature(&hardware.thermal.0, &design.0),
                    |sink| sink.0,
                )
            } else {
                sink_temperature(&hardware.thermal.0, &design.0)
            };
            if core_k >= spec.shutdown_temperature_k || sink_k >= spec.hot_temperature_k {
                reactor.shutdown = true;
            } else if core_k < spec.hot_temperature_k && sink_k < spec.hot_temperature_k * 0.9 {
                reactor.shutdown = false;
            }
            let throttle = match hardware.settings.0[design.0.part_devices[index].unwrap()] {
                Some(DeviceSetting::GeneratorDemand(value)) => value.clamp(0.0, 1.0),
                _ => 0.0,
            };
            let eta = spec.efficiency(sink_k);
            let headroom = design
                .0
                .battery_j
                .saturating_sub(hardware.inventory.0.energy_j) as f64;
            let enabled =
                !dormant && device.0.operational && hardware.hull.0 > 0.0 && !reactor.shutdown;
            let demand_j = if enabled && eta > 0.0 {
                let target_energy = (spec.hot_temperature_k - 300.0) * spec.core_heat_capacity_j_k;
                (spec.thermal_power_w * throttle * dt)
                    .min((target_energy - reactor.core_energy_j).max(0.))
            } else {
                0.0
            };
            let fuel_kg =
                (demand_j / spec.fuel_energy_j_kg).min(hardware.inventory.0.available(fuel));
            let consumed = if convert(&mut hardware.inventory.0, &cat.0, fuel, spent, fuel_kg) {
                fuel_kg
            } else {
                0.0
            };
            let released = consumed * spec.fuel_energy_j_kg;
            let decay = reactor.decay_energy_j * (1.0 - (-dt / spec.decay_time_s).exp());
            reactor.decay_energy_j =
                reactor.decay_energy_j - decay + released * spec.decay_fraction;
            reactor.core_energy_j += released * (1.0 - spec.decay_fraction) + decay;
            let available_k = 300.0 + reactor.core_energy_j / spec.core_heat_capacity_j_k;
            let exchange_fraction =
                1.0 - (-spec.heat_transfer_w_k * dt / spec.core_heat_capacity_j_k).exp();
            let removed =
                ((available_k - sink_k).max(0.0) * spec.core_heat_capacity_j_k * exchange_fraction)
                    .min(reactor.core_energy_j);
            let removed = if enabled && eta > 0. {
                removed.min(headroom / eta + spec.thermal_power_w * 0.01 * dt)
            } else {
                removed
            };
            reactor.core_energy_j -= removed;
            let conversion_temperature = (available_k
                - 0.5 * removed / spec.core_heat_capacity_j_k)
                .min(spec.hot_temperature_k);
            let actual_eta =
                spec.conversion_quality * (1.0 - sink_k / conversion_temperature).clamp(0.0, 1.0);
            let electric = if enabled {
                (removed * actual_eta).min(headroom)
            } else {
                0.0
            };
            let electric = hardware
                .inventory
                .0
                .energy_j
                .deposit(electric, design.0.battery_j) as f64;
            hardware
                .thermal
                .0
                .add_waste_heat((removed - electric).max(0.0), dt);
            if consumed > 0.0 && spec.breeding_ratio > 0.0 {
                let amount =
                    (consumed * spec.breeding_ratio).min(hardware.inventory.0.available(fertile));
                convert(&mut hardware.inventory.0, &cat.0, fertile, bred, amount);
            }
            if 300.0 + reactor.core_energy_j / spec.core_heat_capacity_j_k
                >= spec.meltdown_temperature_k
            {
                device.0.operational = false;
                reactor.shutdown = true;
                let damage_j = std::mem::take(&mut reactor.core_energy_j);
                hardware.thermal.0.add_hull_heat(damage_j);
                hardware.hull.0 = (hardware.hull.0 - damage_j / thermal::JOULES_PER_HP).max(0.0);
            }
            if !dormant {
                let output = &mut outputs.0[index];
                output.actual = electric / dt;
                output.powered = enabled
                    && hardware.inventory.0.available(fuel) > 0.0
                    && (fuel_kg <= 0.0 || consumed > 0.0);
                output.power.recovered_w = electric / dt;
            }
        }
    }
}

pub(crate) fn process(
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut ships: Query<
        (
            &ShipDesign,
            (&mut ShipInventory, &Hull, &mut ShipThermal, &PartDevices),
            &mut DeviceOutputs,
        ),
        Without<super::super::travel::SystemsSuspended>,
    >,
    processors: Query<(&FuelProcessor, &Device)>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("hardware.reactors.process");
    let dt = time.delta_secs_f64();
    for (design, (mut inventory, hull, mut thermal, parts), mut outputs) in &mut ships {
        if hull.0 <= 0.0 {
            continue;
        }
        for &index in &design.0.active_parts {
            let Ok((processor, device)) = processors.get(parts.0[index]) else {
                continue;
            };
            if !device.0.operational {
                continue;
            }
            let spec = processor.0;
            let throttle = 1.0;
            let source = resource(
                &cat.0,
                if spec.produces_charges {
                    "reactor_fuel"
                } else {
                    "bred_fuel"
                },
            );
            let target = resource(
                &cat.0,
                if spec.produces_charges {
                    "micropulse_charge"
                } else {
                    "reactor_fuel"
                },
            );
            let waste = resource(&cat.0, "spent_fuel");
            let requested = spec.throughput_kg_s * throttle * dt;
            let amount = requested
                .min(inventory.0.available(source))
                .min(inventory.0.energy_j as f64 / spec.power_w * spec.throughput_kg_s);
            if amount <= 0.0 {
                continue;
            }
            let units = (amount).stochastic_round();
            let recovered = (units as f64 * spec.recovery_fraction)
                .stochastic_round()
                .min(units);
            let discarded = units - recovered;
            let energy = units as f64 / spec.throughput_kg_s * spec.power_w;
            if units == 0
                || units > inventory.0.quantities[source]
                || recovered > inventory.0.tank_room(target, &cat.0)
                || discarded > inventory.0.tank_room(waste, &cat.0)
                || energy > inventory.0.energy_j as f64
            {
                continue;
            }
            let mut next = inventory.0.clone();
            next.quantities[source] -= units;
            next.insert_consumable(target, recovered, &cat.0)
                .expect("reserved tank space");
            next.insert_consumable(waste, discarded, &cat.0)
                .expect("reserved tank space");
            let paid = next.energy_j.withdraw(energy);
            inventory.0 = next;
            thermal.0.add_waste_heat(paid as f64, dt);
            let output = &mut outputs.0[index];
            output.actual = amount / dt;
            output.powered = true;
            output.power.requested_w = spec.power_w * throttle;
            output.power.supplied_w = paid as f64 / dt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::HardwareFixture;
    use super::*;

    fn fixture(prototype: &str) -> HardwareFixture {
        let mut blueprint = starter(EXAMPLE_CONTROLLER.to_vec());
        let part = blueprint
            .parts
            .iter_mut()
            .find(|p| p.prototype == "generator")
            .unwrap();
        let mut reactor = part.clone();
        part.prototype = "structure".into();
        part.alias.clear();
        reactor.prototype = prototype.into();
        reactor.alias.clear();
        reactor.groups.clear();
        reactor.id = 99;
        blueprint.parts.push(reactor);
        let mut parent = None;
        for part in &mut blueprint.parts {
            part.attachment = parent.map(|parent| Attachment {
                parent,
                socket: "back".into(),
                plug: "front".into(),
                roll: 0,
            });
            parent = Some(part.id);
        }
        let mut fixture = HardwareFixture::new(blueprint);
        let cat = Catalogue::builtin();
        fixture.set_inventory(|inventory| {
            inventory.tank_capacities_m3.fill(1.0);
            inventory.quantities.fill(0);
            inventory.quantities[resource(&cat, "reactor_fuel")] = 1;
            inventory.quantities[resource(&cat, "fertile_feedstock")] = 2;
            inventory.energy_j = 1000000;
        });
        fixture
    }

    #[test]
    fn hot_standby_retains_core_heat_and_supplies_a_new_load_immediately() {
        let mut fixture = fixture("reactor_compact_2m");
        let capacity = fixture.design.battery_j;
        let target_energy = {
            let world = fixture.app.world_mut();
            let mut query = world.query::<&mut Reactor>();
            let mut reactor = query.single_mut(world).unwrap();
            let energy =
                (reactor.spec.hot_temperature_k - 300.) * reactor.spec.core_heat_capacity_j_k;
            reactor.core_energy_j = energy;
            energy
        };
        for _ in 0..100 {
            fixture.set_inventory(|inventory| inventory.energy_j = capacity);
            fixture.advance();
        }
        let world = fixture.app.world_mut();
        let mut query = world.query::<&Reactor>();
        assert!(query.single(world).unwrap().core_energy_j > target_energy * 0.95);
        fixture.set_inventory(|inventory| inventory.energy_j = 0);
        fixture.advance();
        assert!(fixture.state().inventory.energy_j > 100_000);
    }

    #[test]
    fn breeder_conserves_material_and_produces_power() {
        let mut fixture = fixture("reactor_breeder_4m");
        let cat = Catalogue::builtin();
        let before = fixture.state();
        for _ in 0..2000 {
            fixture.advance();
        }
        let after = fixture.state();
        let used = before.inventory.quantities[resource(&cat, "reactor_fuel")]
            - after.inventory.quantities[resource(&cat, "reactor_fuel")];
        assert_eq!(
            after.inventory.quantities[resource(&cat, "spent_fuel")],
            used
        );
        assert!((after.inventory.mass(&cat) - before.inventory.mass(&cat)).abs() < 1e-9);
        assert_eq!(
            after.inventory.quantities[resource(&cat, "bred_fuel")]
                + after.inventory.quantities[resource(&cat, "fertile_feedstock")],
            2
        );
        assert!(after.inventory.energy_j > before.inventory.energy_j);
    }

    #[test]
    fn shutdown_retains_decay_heat_and_hot_sink_prevents_fission() {
        let mut fixture = fixture("reactor_compact_2m");
        for _ in 0..20 {
            fixture.advance();
        }
        let cat = Catalogue::builtin();
        let fuel_before = fixture.state().inventory.quantities[resource(&cat, "reactor_fuel")];
        let world = fixture.app.world_mut();
        let mut q = world.query::<&mut Reactor>();
        let mut reactor = q.single_mut(world).unwrap();
        assert!(reactor.decay_energy_j > 0.0);
        let decay_before = reactor.decay_energy_j;
        reactor.core_energy_j =
            reactor.spec.core_heat_capacity_j_k * (reactor.spec.shutdown_temperature_k - 300.0);
        fixture.advance();
        assert_eq!(
            fixture.state().inventory.quantities[resource(&cat, "reactor_fuel")],
            fuel_before
        );
        let world = fixture.app.world_mut();
        let mut q = world.query::<&Reactor>();
        assert!(q.single(world).unwrap().decay_energy_j < decay_before);
    }

    #[test]
    fn fuel_processing_cannot_recycle_exhausted_fuel_into_energy() {
        let mut fixture = fixture("fuel_processor_4m");
        let cat = Catalogue::builtin();
        fixture.set_inventory(|inventory| {
            inventory.tank_capacities_m3.fill(1.0);
            inventory.quantities.fill(0);
            inventory.quantities[resource(&cat, "spent_fuel")] = 1;
        });
        fixture.advance();
        assert_eq!(
            fixture.state().inventory.quantities[resource(&cat, "reactor_fuel")],
            0
        );
        fixture.set_inventory(|inventory| {
            inventory.quantities[resource(&cat, "bred_fuel")] = 1;
            inventory.energy_j = 1000000;
        });
        {
            let world = fixture.app.world_mut();
            let mut q = world.query::<&mut FuelProcessor>();
            q.single_mut(world).unwrap().0.throughput_kg_s = 10.0;
        }
        let before = fixture.state();
        fixture.advance();
        let after = fixture.state();
        assert_eq!(after.inventory.quantities[resource(&cat, "bred_fuel")], 0);
        assert_eq!(
            after.inventory.quantities[resource(&cat, "reactor_fuel")]
                + after.inventory.quantities[resource(&cat, "spent_fuel")]
                - before.inventory.quantities[resource(&cat, "spent_fuel")],
            1
        );
        assert!((before.inventory.mass(&cat) - after.inventory.mass(&cat)).abs() < 1e-9);
        assert!(after.inventory.energy_j < before.inventory.energy_j);
    }
    #[test]
    fn charge_factory_converts_fuel_into_charges_and_conserves_mass() {
        let mut fixture = fixture("fuel_plant_8m");
        let cat = Catalogue::builtin();
        let charges = resource(&cat, "micropulse_charge");
        let fuel = resource(&cat, "reactor_fuel");
        fixture.advance();
        assert_eq!(fixture.state().inventory.quantities[charges], 0);
        fixture.set_inventory(|inventory| {
            inventory.quantities[fuel] = 1;
            inventory.energy_j = 10000000;
        });
        {
            let world = fixture.app.world_mut();
            let mut q = world.query::<&mut FuelProcessor>();
            q.single_mut(world).unwrap().0.throughput_kg_s = 10.0;
        }
        let before = fixture.state();
        fixture.advance();
        let after = fixture.state();
        let produced = after.inventory.quantities[charges];
        assert_eq!(produced, 1);
        assert!((before.inventory.mass(&cat) - after.inventory.mass(&cat)).abs() < 1e-9);
        let used_fuel = before.inventory.quantities[fuel] - after.inventory.quantities[fuel];
        assert_eq!(
            used_fuel,
            produced + after.inventory.quantities[resource(&cat, "spent_fuel")]
                - before.inventory.quantities[resource(&cat, "spent_fuel")]
        );
    }
    #[test]
    fn docking_transfers_guest_heat_to_host_without_radiating_it() {
        use crate::sim::{
            identity::IdentityIndex,
            travel::{PresenceState, SystemsSuspended},
        };
        use bevy::ecs::system::RunSystemOnce;
        let mut fixture = fixture("reactor_compact_2m");
        let host_id = osg_model::Id::new();
        let world = fixture.app.world_mut();
        let host = world
            .spawn((
                ShipDesign(fixture.design.clone()),
                ShipThermal(thermal::ThermalState::default()),
            ))
            .id();
        world.init_resource::<IdentityIndex>();
        world
            .entity_mut(host)
            .insert(super::super::super::identity::Identity(host_id));
        world.entity_mut(fixture.ship).insert((
            SystemsSuspended,
            PresenceState(osg_model::travel::Presence::Docked {
                host: host_id,
                bay: 0,
            }),
        ));
        let mut guest = world.get_mut::<ShipThermal>(fixture.ship).unwrap();
        guest.0.hull_energy_j = 10e6;
        guest.0.pending_waste_heat_j = 2e6;
        world.run_system_once(dock_heat_transfer).unwrap();
        assert_eq!(
            world.get::<ShipThermal>(host).unwrap().0.hull_energy_j,
            12e6
        );
        assert_eq!(
            world
                .get::<ShipThermal>(fixture.ship)
                .unwrap()
                .0
                .hull_energy_j,
            0.0
        );
        assert_eq!(
            world
                .get::<ShipThermal>(fixture.ship)
                .unwrap()
                .0
                .pending_waste_heat_j,
            0.0
        );
        assert_eq!(world.get::<DockedSink>(fixture.ship).unwrap().0, 300.0);
    }
    #[test]
    fn core_overheating_causes_damage_and_permanent_device_failure() {
        let mut fixture = fixture("reactor_compact_2m");
        let world = fixture.app.world_mut();
        let mut query = world.query::<&mut Reactor>();
        let mut reactor = query.single_mut(world).unwrap();
        reactor.core_energy_j =
            reactor.spec.core_heat_capacity_j_k * reactor.spec.meltdown_temperature_k * 2.0;
        let hull_before = fixture.state().hull;
        fixture.advance();
        assert!(fixture.state().hull < hull_before);
        let world = fixture.app.world_mut();
        let mut query = world.query::<(&Reactor, &Device)>();
        let (reactor, device) = query.single(world).unwrap();
        assert!(!device.0.operational);
        assert!(reactor.shutdown);
        assert_eq!(reactor.core_energy_j, 0.0);
    }
}
