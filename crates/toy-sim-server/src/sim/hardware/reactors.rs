use super::*;
use toy_sim_ships::reactors::{FuelProcessorSpec, ReactorSpec};

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
        With<super::super::travel::Dormant>,
    >,
    identities: Option<Res<super::super::identity::IdentityIndex>>,
    designs: Query<&ShipDesign>,
    mut thermal: Query<&mut ShipThermal>,
) {
    let Some(identities) = identities else {
        return;
    };
    for (ship, presence) in &docked {
        let toy_sim_model::travel::Presence::Docked { host, .. } = presence.0 else {
            commands.entity(ship).remove::<DockedSink>();
            continue;
        };
        let Some(&host) = identities.0.get(&host) else {
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

fn sink_temperature(thermal: &thermal::ThermalState, design: &CompiledShipDesign) -> f64 {
    if thermal.shield_deployed_kg > 0.0
        && matches!(
            thermal.shield_state,
            toy_sim_ship_api::abi::SHIELD_ACTIVE | toy_sim_ship_api::abi::SHIELD_DEPLETED
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
    let units = stochastic_units(amount).min(inventory.quantities[source]);
    inventory.quantities[source] -= units;
    inventory
        .insert_consumable(target, units, cat)
        .expect("reserved tank space");
    true
}

pub(crate) fn generate(
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut ships: Query<(
        &ShipDesign,
        HardwareWrite,
        &mut DeviceOutputs,
        Has<super::super::travel::Dormant>,
        Option<&DockedSink>,
    )>,
    mut reactors: Query<(&mut Reactor, &mut Device)>,
) {
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
            let headroom = (design.0.battery_j - hardware.inventory.0.energy_j).max(0.0);
            let enabled =
                !dormant && device.0.operational && hardware.hull.0 > 0.0 && !reactor.shutdown;
            let demand_j = if enabled && eta > 0.0 {
                (spec.thermal_power_w * throttle * dt).min(headroom / eta)
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
            hardware.inventory.0.energy_j += electric;
            hardware.thermal.0.add_waste_heat(removed - electric, dt);
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
        (&ShipDesign, HardwareWrite, &mut DeviceOutputs),
        Without<super::super::travel::Dormant>,
    >,
    processors: Query<(&FuelProcessor, &Device)>,
) {
    let dt = time.delta_secs_f64();
    for (design, mut hardware, mut outputs) in &mut ships {
        if hardware.hull.0 <= 0.0 {
            continue;
        }
        for &index in &design.0.active_parts {
            let Ok((processor, device)) = processors.get(hardware.parts.0[index]) else {
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
            let fuel_fraction = if spec.produces_charges {
                spec.fissile_fraction
            } else {
                1.0
            };
            let material = if spec.produces_charges {
                Some(resource(&cat.0, "repair_material"))
            } else {
                None
            };
            let amount = requested
                .min(hardware.inventory.0.available(source) / fuel_fraction)
                .min(hardware.inventory.0.energy_j / spec.power_w * spec.throughput_kg_s);
            let amount = if let Some(material) = material {
                amount.min(
                    hardware.inventory.0.available(material)
                        / (1.0 - fuel_fraction).max(f64::MIN_POSITIVE),
                )
            } else {
                amount
            };
            if amount <= 0.0 {
                continue;
            }
            let units = stochastic_units(amount);
            let fuel_units = stochastic_units(units as f64 * fuel_fraction).min(units);
            let material_units = units - fuel_units;
            let recovered = stochastic_units(units as f64 * spec.recovery_fraction).min(units);
            let discarded = units - recovered;
            let energy = units as f64 / spec.throughput_kg_s * spec.power_w;
            if units == 0
                || fuel_units > hardware.inventory.0.quantities[source]
                || material.is_some_and(|i| material_units > hardware.inventory.0.quantities[i])
                || recovered > hardware.inventory.0.tank_room(target, &cat.0)
                || discarded > hardware.inventory.0.tank_room(waste, &cat.0)
                || energy > hardware.inventory.0.energy_j
            {
                continue;
            }
            let mut next = hardware.inventory.0.clone();
            next.quantities[source] -= fuel_units;
            if let Some(material) = material {
                next.quantities[material] -= material_units;
            }
            next.insert_consumable(target, recovered, &cat.0)
                .expect("reserved tank space");
            next.insert_consumable(waste, discarded, &cat.0)
                .expect("reserved tank space");
            next.energy_j -= energy;
            hardware.inventory.0 = next;
            hardware.thermal.0.add_waste_heat(energy, dt);
            let output = &mut outputs.0[index];
            output.actual = amount / dt;
            output.powered = true;
            output.power.requested_w = spec.power_w * throttle;
            output.power.supplied_w = energy / dt;
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
            inventory.energy_j = 1e6;
        });
        fixture
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
            inventory.energy_j = 1e6;
        });
        {
            let world = fixture.app.world_mut();
            let mut q = world.query::<&mut FuelProcessor>();
            q.single_mut(world).unwrap().0.throughput_kg_s = 10.0;
        }
        let before = fixture.state();
        fixture.advance();
        let after = fixture.state();
        assert!(after.inventory.quantities[resource(&cat, "reactor_fuel")] > 0);
        assert!((before.inventory.mass(&cat) - after.inventory.mass(&cat)).abs() < 1e-9);
        assert!(after.inventory.energy_j < before.inventory.energy_j);
    }
    #[test]
    fn charge_factory_requires_material_and_conserves_mass() {
        let mut fixture = fixture("fuel_plant_8m");
        let cat = Catalogue::builtin();
        let charges = resource(&cat, "micropulse_charge");
        let fuel = resource(&cat, "reactor_fuel");
        let material = resource(&cat, "repair_material");
        fixture.advance();
        assert_eq!(fixture.state().inventory.quantities[charges], 0);
        fixture.set_inventory(|inventory| {
            inventory.quantities[material] = 1;
            inventory.energy_j = 10e6;
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
        let used_material =
            before.inventory.quantities[material] - after.inventory.quantities[material];
        assert_eq!(
            used_fuel + used_material,
            produced + after.inventory.quantities[resource(&cat, "spent_fuel")]
        );
    }
    #[test]
    fn docking_transfers_guest_heat_to_host_without_radiating_it() {
        use crate::sim::{
            identity::IdentityIndex,
            travel::{Dormant, PresenceState},
        };
        use bevy::ecs::system::RunSystemOnce;
        let mut fixture = fixture("reactor_compact_2m");
        let host_id = toy_sim_model::Id::new();
        let world = fixture.app.world_mut();
        let host = world
            .spawn((
                ShipDesign(fixture.design.clone()),
                ShipThermal(thermal::ThermalState::default()),
            ))
            .id();
        let mut index = IdentityIndex::default();
        index.0.insert(host_id, host);
        world.insert_resource(index);
        world.entity_mut(fixture.ship).insert((
            Dormant,
            PresenceState(toy_sim_model::travel::Presence::Docked {
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
