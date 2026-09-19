use super::{
    physics::{AccumulatedForce, AccumulatedTorque, MassProps},
    precision::PreciseTransform,
    vessel::{ShipCatalogue, ShipDesign, ShipSoftware},
};
use bevy::{ecs::query::QueryData, math::DVec3, prelude::*};
use toy_sim_ships::*;

pub(crate) mod cooling;
pub(crate) mod devices;
pub mod propulsion;
pub(crate) mod reactors;
pub mod utilities;
use devices::{Demand, Generator, Shield};

#[derive(Component)]
pub struct ShipInventory(pub Inventory);

#[derive(Component)]
pub struct Hull(pub f64);

#[derive(Component)]
pub struct ShipThermal(pub thermal::ThermalState);

#[derive(Component)]
pub struct Avionics(pub DeviceState);

#[derive(Component)]
pub struct DeviceSettings(pub Vec<Option<DeviceSetting>>);

#[derive(Component)]
pub struct HardwareClock(pub u64);

#[derive(Component)]
pub struct SensorRange(pub f64);

#[derive(Component, Clone, Copy)]
pub struct SensorOverride {
    pub range_m: f64,
    pub occlusion: bool,
}

#[derive(Component, Default)]
pub struct PartDevices(pub Vec<Entity>);

#[derive(Component)]
pub struct InstalledPart {
    pub ship: Entity,
    pub index: usize,
}

#[derive(Component)]
pub struct ActiveDevice;

#[derive(Component)]
pub struct Device(pub DeviceState);

#[derive(Component)]
pub struct Weapon(pub weapons::WeaponState);

#[derive(Component)]
pub struct PendingHardwareReset(pub ShipState);

#[derive(Clone, Copy)]
struct DeviceOutput {
    actual: f64,
    thrust_n: [f64; 3],
    powered: bool,
    power: DevicePower,
}

impl Default for DeviceOutput {
    fn default() -> Self {
        Self {
            actual: 0.0,
            thrust_n: [0.0; 3],
            powered: true,
            power: DevicePower::default(),
        }
    }
}

#[derive(Component)]
pub(crate) struct DeviceOutputs(Vec<DeviceOutput>);

#[derive(Component, Default)]
pub(crate) struct DormantThermalElapsed(pub f64);

#[derive(Component, Default, Clone, Copy)]
pub struct DevicePower {
    pub requested_w: f64,
    pub supplied_w: f64,
    pub recovered_w: f64,
}

#[derive(Component, Default, Clone, Copy)]
pub struct PowerFlow {
    pub generated_w: f64,
    pub requested_w: f64,
    pub supplied_w: f64,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwareSystems {
    Initialize,
    Run,
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct HardwareWrite {
    pub inventory: &'static mut ShipInventory,
    pub hull: &'static mut Hull,
    pub thermal: &'static mut ShipThermal,
    pub avionics: &'static mut Avionics,
    pub settings: &'static mut DeviceSettings,
    pub clock: &'static mut HardwareClock,
    pub range: &'static mut SensorRange,
    pub parts: &'static PartDevices,
}

pub fn bundle(d: &CompiledShipDesign, state: ShipState) -> impl Bundle + use<> {
    (
        ShipInventory(state.inventory.clone()),
        Hull(state.hull),
        ShipThermal(state.thermal),
        Avionics(state.avionics.clone()),
        DeviceSettings(state.settings.clone()),
        HardwareClock(state.tick),
        SensorRange(state.sensor_range),
        PartDevices::default(),
        PendingHardwareReset(state),
        PowerFlow::default(),
        (
            propulsion::ActuatorOutput::default(),
            propulsion::InstalledRatings(propulsion::telemetry(d, None)),
        ),
        DeviceOutputs(vec![DeviceOutput::default(); d.parts.len()]),
        DormantThermalElapsed::default(),
    )
}

pub fn install(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (initialize, apply_impacts)
            .chain()
            .in_set(HardwareSystems::Initialize)
            .before(super::vessel::run),
    )
    .add_systems(
        FixedUpdate,
        (
            (begin, reset_weapons),
            generators,
            reactors::dock_heat_transfer,
            reactors::generate,
            avionics,
            device_systems(),
            utilities::run,
            utilities::service_docked,
            cooling::run,
            power_totals,
            publish_mass,
            dormant_thermal,
            sensor_overrides,
        )
            .chain()
            .in_set(HardwareSystems::Run)
            .after(super::vessel::run)
            .in_set(super::simulation::SimulationSystems::PrepareBodies),
    );
}

pub(crate) fn initialize(
    mut commands: Commands,
    ships: Query<(
        Entity,
        &ShipDesign,
        &PendingHardwareReset,
        &PartDevices,
        Has<super::travel::Dormant>,
        Option<&utilities::Crew>,
        Option<&super::travel::DockingBays>,
    )>,
) {
    for (ship, d, reset, old, dormant, crew, bays) in &ships {
        for part in &old.0 {
            commands.entity(*part).despawn();
        }
        utilities::install_ship(&mut commands, ship, &d.0, crew, bays);
        let slip_power: f64 =
            d.0.parts
                .iter()
                .filter_map(|part| match part.definition.equipment {
                    Equipment::Utility {
                        utility: toy_sim_ships::utilities::UtilityDef::SlipDrive { power_w },
                    } => Some(power_w),
                    _ => None,
                })
                .sum();
        if slip_power > 0. {
            commands.entity(ship).insert(super::travel::SlipDrive {
                power_w: slip_power,
                ..Default::default()
            });
        } else {
            commands.entity(ship).remove::<super::travel::SlipDrive>();
        }
        let state = &reset.0;
        let mut parts = Vec::with_capacity(d.0.parts.len());
        for (index, device) in state.devices.iter().enumerate() {
            let mut part = commands.spawn((
                InstalledPart { ship, index },
                Device(device.clone()),
                ChildOf(ship),
                DevicePower::default(),
            ));
            if !dormant {
                part.insert(ActiveDevice);
            }
            devices::install(&mut part, &d.0.parts[index].definition.equipment);
            if let Equipment::Utility { utility } = &d.0.parts[index].definition.equipment {
                part.insert(utilities::Utility(utility.clone()));
            }
            cooling::install(&mut part, &d.0.parts[index].definition.equipment);
            reactors::install(&mut part, &d.0.parts[index].definition.equipment);
            if let Some(w) = d.0.part_weapons[index] {
                part.insert(Weapon(state.weapons[w].clone()));
            }
            parts.push(part.id());
        }
        commands
            .entity(ship)
            .insert((
                ShipInventory(state.inventory.clone()),
                Hull(state.hull),
                ShipThermal(state.thermal),
                Avionics(state.avionics.clone()),
                DeviceSettings(state.settings.clone()),
                HardwareClock(state.tick),
                SensorRange(state.sensor_range),
                PartDevices(parts),
                (
                    propulsion::ActuatorOutput::default(),
                    propulsion::InstalledRatings(propulsion::telemetry(&d.0, None)),
                ),
                DeviceOutputs(vec![DeviceOutput::default(); d.0.parts.len()]),
                DormantThermalElapsed::default(),
            ))
            .remove::<PendingHardwareReset>();
    }
}

impl HardwareWriteItem<'_, '_> {
    pub fn computer_running(&self, _d: &CompiledShipDesign) -> bool {
        self.hull.0 > 0. && self.avionics.0.operational && self.avionics.0.powered
    }
    pub fn reset_commands(&mut self, d: &CompiledShipDesign) {
        self.settings.0 = default_settings(d);
    }
    pub fn apply_commands(
        &mut self,
        d: &CompiledShipDesign,
        commands: &[DeviceCommand],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(commands.len() <= MAX_DEVICES, "too many device commands");
        for command in commands {
            let descriptor = d
                .device_catalogue
                .get(command.device.0 as usize)
                .ok_or_else(|| anyhow::anyhow!("unknown device handle"))?;
            anyhow::ensure!(
                command.setting.finite() && command.setting.supports(&descriptor.kind),
                "invalid device setting"
            );
        }
        for command in commands {
            self.settings.0[command.device.0 as usize] = Some(command.setting.clone());
        }
        Ok(())
    }
    pub fn mass_properties(
        &self,
        d: &CompiledShipDesign,
        cat: &Catalogue,
    ) -> (f64, bevy::math::DMat3) {
        let m = d.dry_mass
            + self.inventory.0.mass(cat)
            + self.thermal.0.shield_reserve_kg()
            + self.thermal.0.shield_deployed_kg;
        (m, d.inertia * (m / d.dry_mass))
    }
    pub fn resources(&self, d: &CompiledShipDesign) -> toy_sim_ship_api::abi::ShipResources {
        resources(d, self.hull.0, &self.thermal.0, self.inventory.0.energy_j)
    }
    pub fn snapshot(
        &self,
        d: &CompiledShipDesign,
        parts: &Query<(&InstalledPart, &Device, Option<&Weapon>)>,
    ) -> Vec<DeviceStatus> {
        let mut state = ShipState {
            inventory: Inventory {
                tank_capacities_m3: Vec::new(),
                quantities: Vec::new(),
                cargo: Vec::new(),
                energy_j: 0,
            },
            weapons: vec![weapons::WeaponState::default(); d.weapon_parts.len()],
            devices: vec![DeviceState::default(); d.parts.len()],
            avionics: DeviceState::default(),
            hull: 0.0,
            thermal: thermal::ThermalState::default(),
            settings: Vec::new(),
            tick: 0,
            sensor_range: 0.0,
        };
        state.inventory = self.inventory.0.clone();
        state.hull = self.hull.0;
        state.thermal = self.thermal.0;
        state.avionics = self.avionics.0.clone();
        state.sensor_range = self.range.0;
        for entity in &self.parts.0 {
            if let Ok((part, device, weapon)) = parts.get(*entity) {
                state.devices[part.index] = device.0.clone();
                if let Some(weapon) = weapon {
                    state.weapons[d.part_weapons[part.index].unwrap()] = weapon.0.clone();
                }
            }
        }
        state.snapshot(d)
    }
}

pub fn resources(
    d: &CompiledShipDesign,
    hull: f64,
    t: &thermal::ThermalState,
    energy: u64,
) -> toy_sim_ship_api::abi::ShipResources {
    toy_sim_ship_api::abi::ShipResources {
        hull_hp: hull,
        hull_max_hp: d.hull,
        hull_heat_j: t.hull_energy_j,
        hull_heat_capacity_j: d.hull_heat_capacity_j,
        shield_temperature_k: t.shield_temperature(d.into()),
        shield_reserve_kg: t.shield_reserve_kg(),
        shield_reserve_capacity_kg: d.shield_reserve_capacity_kg,
        shield_strength: t.shield_strength(d.into()),
        energy_j: energy,
        shield_state: t.shield_state,
    }
}

pub fn snapshot(world: &World, ship: Entity) -> Option<ShipState> {
    let d = &world.get::<ShipDesign>(ship)?.0;
    let mut state = ShipState {
        inventory: Inventory {
            tank_capacities_m3: Vec::new(),
            quantities: Vec::new(),
            cargo: Vec::new(),
            energy_j: 0,
        },
        weapons: vec![weapons::WeaponState::default(); d.weapon_parts.len()],
        devices: vec![DeviceState::default(); d.parts.len()],
        avionics: DeviceState::default(),
        hull: 0.0,
        thermal: thermal::ThermalState::default(),
        settings: Vec::new(),
        tick: 0,
        sensor_range: 0.0,
    };
    state.inventory = world.get::<ShipInventory>(ship)?.0.clone();
    state.hull = world.get::<Hull>(ship)?.0;
    state.thermal = world.get::<ShipThermal>(ship)?.0;
    state.avionics = world.get::<Avionics>(ship)?.0.clone();
    state.settings = world.get::<DeviceSettings>(ship)?.0.clone();
    state.tick = world.get::<HardwareClock>(ship)?.0;
    state.sensor_range = world.get::<SensorRange>(ship)?.0;
    for entity in &world.get::<PartDevices>(ship)?.0 {
        let part = world.get::<InstalledPart>(*entity)?;
        state.devices[part.index] = world.get::<Device>(*entity)?.0.clone();
        if let Some(weapon) = world.get::<Weapon>(*entity) {
            state.weapons[d.part_weapons[part.index]?] = weapon.0.clone();
        }
    }
    Some(state)
}

fn apply_impacts(mut ships: Query<(&ShipDesign, &mut Hull, &mut ShipThermal, &mut ShipSoftware)>) {
    ships
        .par_iter_mut()
        .for_each(|(design, mut hull, mut thermal, mut software)| {
            thermal.0.deposit(
                &mut hull.0,
                design.0.as_ref().into(),
                false,
                std::mem::take(&mut software.hull_energy_j),
            );
            thermal.0.deposit(
                &mut hull.0,
                design.0.as_ref().into(),
                true,
                std::mem::take(&mut software.shield_energy_j),
            );
        });
}

fn begin(
    mut ships: Query<
        (&ShipDesign, HardwareWrite, &mut DormantThermalElapsed),
        Without<super::travel::Dormant>,
    >,
) {
    ships
        .par_iter_mut()
        .for_each(|(design, mut hardware, mut dormant_elapsed)| {
            hardware.clock.0 += 1;
            hardware.range.0 = 0.0;
            hardware.thermal.0.shield_powered = false;
            hardware.thermal.0.shield_enabled = false;
            dormant_elapsed.0 = 0.0;
            if !hardware.computer_running(&design.0) {
                hardware.reset_commands(&design.0);
            }
            if hardware.hull.0 <= 0.0 {
                hardware.avionics.0.powered = false;
            }
        });
}

fn reset_weapons(
    ships: Query<(), Without<super::travel::Dormant>>,
    mut parts: Query<(&InstalledPart, &mut Weapon), With<ActiveDevice>>,
) {
    parts.par_iter_mut().for_each(|(installed, mut weapon)| {
        if !ships.contains(installed.ship) {
            return;
        }
        weapon.0.previous_yaw_rad = weapon.0.yaw_rad;
        weapon.0.previous_pitch_rad = weapon.0.pitch_rad;
        weapon.0.powered = false;
        weapon.0.command = None;
    });
}

pub(crate) fn avionics(
    time: Res<Time<Fixed>>,
    mut ships: Query<
        (&ShipDesign, HardwareWrite, &mut DeviceOutputs),
        Without<super::travel::Dormant>,
    >,
    parts: Query<&Device>,
) {
    let dt = time.delta_secs_f64();
    ships.par_iter_mut().for_each(|(d, mut h, mut outputs)| {
        h.avionics.0.powered = false;
        if h.avionics.0.operational && h.hull.0 > 0. {
            for (index, part) in d.0.parts.iter().enumerate() {
                let Equipment::Utility {
                    utility: toy_sim_ships::utilities::UtilityDef::Command { power_w },
                } = part.definition.equipment
                else {
                    continue;
                };
                if !parts
                    .get(h.parts.0[index])
                    .is_ok_and(|device| device.0.operational)
                {
                    continue;
                }
                let fraction = spend(&mut h.inventory.0, [0., 0., power_w * dt]);
                let output = &mut outputs.0[index];
                output.power.requested_w = power_w;
                output.power.supplied_w = power_w * fraction;
                output.powered = fraction >= 1. - 1e-9;
                h.avionics.0.powered |= output.powered;
                h.thermal.0.add_waste_heat(power_w * fraction * dt, dt);
            }
        }
        if !h.avionics.0.powered {
            h.reset_commands(&d.0);
            return;
        }
        if matches!(
            h.settings.0[d.0.avionics_handles[2].0 as usize],
            Some(DeviceSetting::SensorEnabled(true))
        ) && spend(&mut h.inventory.0, [0., 0., SENSOR_POWER_W * dt]) >= 1. - 1e-9
        {
            h.range.0 = SENSOR_RANGE_M;
        }
    });
}

pub(crate) fn generators(
    time: Res<Time<Fixed>>,
    mut ships: Query<
        (&ShipDesign, HardwareWrite, &mut DeviceOutputs),
        Without<super::travel::Dormant>,
    >,
    parts: Query<(&Generator, &Device)>,
) {
    let dt = time.delta_secs_f64();
    ships
        .par_iter_mut()
        .for_each(|(design, mut hardware, mut outputs)| {
            outputs.0.fill(DeviceOutput::default());
            if hardware.hull.0 <= 0.0 {
                return;
            }
            let design = &design.0;

            for &index in &design.active_parts {
                let Ok((generator, device)) = parts.get(hardware.parts.0[index]) else {
                    continue;
                };
                if !device.0.operational {
                    continue;
                }
                let throttle = match hardware.settings.0[design.part_devices[index].unwrap()] {
                    Some(DeviceSetting::GeneratorDemand(value)) => value.clamp(0.0, 1.0),
                    _ => 0.0,
                };
                let output = &mut outputs.0[index];
                output.power.requested_w = generator.power_w * throttle;
                let wanted = (output.power.requested_w * dt).min(
                    design
                        .battery_j
                        .saturating_sub(hardware.inventory.0.energy_j) as f64,
                );
                if wanted <= 0.0 {
                    continue;
                }

                let fuel = generator.fuel_kg_s * wanted / generator.power_w;
                let fraction = spend(&mut hardware.inventory.0, [0.0, fuel, 0.0]);
                let generated = hardware
                    .inventory
                    .0
                    .energy_j
                    .deposit(wanted * fraction, design.battery_j)
                    as f64;
                hardware
                    .thermal
                    .0
                    .add_waste_heat(generated * (1.0 / generator.efficiency - 1.0), dt);
                output.powered = fraction > 0.0;
                output.actual = generated / dt;
                output.power.supplied_w = output.actual;
            }
        });
}

pub(crate) fn device_systems()
-> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (
        (
            devices::prepare_engines,
            devices::prepare_micropulse_engines,
            devices::prepare_thermal_engines,
            devices::prepare_rcs,
            devices::prepare_torquers,
            devices::prepare_shields,
        ),
        actuate,
        devices::thermal_engine_decay,
        devices::prepare_weapons,
        reactors::process,
        publish_devices,
    )
        .chain()
}

pub(crate) fn actuate(
    time: Res<Time<Fixed>>,
    catalogue: Res<ShipCatalogue>,
    mut ships: Query<
        (
            &ShipDesign,
            HardwareWrite,
            &mut DeviceOutputs,
            &PreciseTransform,
            &mut AccumulatedForce,
            &mut AccumulatedTorque,
            &mut propulsion::ActuatorOutput,
        ),
        Without<super::travel::Dormant>,
    >,
    parts: Query<(&Demand, &Device, Has<Shield>)>,
) {
    let dt = time.delta_secs_f64();
    ships.par_iter_mut().for_each(
        |(design, mut hardware, mut outputs, pose, mut force, mut torque, mut measured)| {
            *measured = propulsion::ActuatorOutput::default();
            if hardware.hull.0 <= 0.0 {
                return;
            }
            let mut local_force = DVec3::ZERO;
            let mut local_torque = DVec3::ZERO;
            let mut any_shield_enabled = false;
            let mut all_shields_powered = true;

            for &index in &design.0.active_parts {
                let Ok((demand, device, shield)) = parts.get(hardware.parts.0[index]) else {
                    continue;
                };
                if !device.0.operational {
                    if shield {
                        all_shields_powered = false;
                    }
                    continue;
                }
                let mut supplied_energy = 0;
                let fraction = if demand.enabled {
                    let mut fraction = 1.0_f64;
                    for (resource, requested) in
                        [demand.resource_input, demand.secondary_resource_input]
                            .into_iter()
                            .flatten()
                    {
                        let available = hardware.inventory.0.available(resource);
                        fraction = fraction.min(if requested > 0.0 {
                            (available / requested).min(1.0)
                        } else {
                            f64::from(available > 0.0)
                        });
                    }
                    for (available, requested) in [
                        hardware.inventory.0.available(0),
                        hardware.inventory.0.available(1),
                        hardware.inventory.0.energy_j as f64,
                    ]
                    .into_iter()
                    .zip(demand.inputs)
                    {
                        if requested > 0.0 {
                            fraction = fraction.min((available / requested).clamp(0.0, 1.0));
                        }
                    }
                    if let Some((resource, quantity)) = demand.resource_output {
                        let room = hardware.inventory.0.tank_room(resource, &catalogue.0);
                        if quantity > 0.0 {
                            fraction = fraction.min(room as f64 / quantity);
                        }
                    }
                    let mut converted_units = 0;
                    for (slot, input) in [demand.resource_input, demand.secondary_resource_input]
                        .into_iter()
                        .enumerate()
                    {
                        if let Some((resource, requested)) = input {
                            let before = hardware.inventory.0.quantities[resource];
                            hardware.inventory.0.consume(resource, requested * fraction);
                            if slot == 1 {
                                converted_units =
                                    before - hardware.inventory.0.quantities[resource];
                            }
                        }
                    }
                    hardware.inventory.0.consume(0, demand.inputs[0] * fraction);
                    hardware.inventory.0.consume(1, demand.inputs[1] * fraction);
                    supplied_energy = hardware
                        .inventory
                        .0
                        .energy_j
                        .withdraw(demand.inputs[2] * fraction);
                    if let Some((resource, _)) = demand.resource_output {
                        hardware
                            .inventory
                            .0
                            .insert_consumable(resource, converted_units, &catalogue.0)
                            .expect("reserved output tank space");
                    }
                    fraction
                } else {
                    0.0
                };
                let output = &mut outputs.0[index];
                output.power.requested_w = demand.inputs[2] / dt;
                output.power.supplied_w = supplied_energy as f64 / dt;
                let recovered = hardware
                    .inventory
                    .0
                    .energy_j
                    .deposit(demand.generated_energy_j * fraction, design.0.battery_j)
                    as f64;
                output.power.recovered_w = recovered / dt;
                output.actual = demand.actual * fraction;
                output.thrust_n = (demand.thrust * fraction).to_array();
                output.powered = demand.enabled
                    && if demand.requires_full_supply {
                        fraction >= 1.0 - 1e-9
                    } else {
                        fraction > 0.0
                    };
                local_force += demand.force * fraction;
                local_torque += demand.torque * fraction;
                if demand.waste_heat_j > 0.0 {
                    hardware
                        .thermal
                        .0
                        .add_waste_heat(demand.waste_heat_j * fraction, dt);
                }
                if shield {
                    any_shield_enabled |= demand.enabled;
                    all_shields_powered &= output.powered;
                }
            }

            hardware.thermal.0.shield_enabled = any_shield_enabled;
            hardware.thermal.0.shield_powered = any_shield_enabled && all_shields_powered;
            measured.force = local_force;
            measured.torque = local_torque;
            force.0 += pose.rotation * local_force;
            torque.0 += pose.rotation * local_torque;
        },
    );
}

fn publish_devices(
    ships: Query<(&DeviceOutputs, &Hull), Without<super::travel::Dormant>>,
    mut parts: Query<(&InstalledPart, &mut Device, &mut DevicePower), With<ActiveDevice>>,
) {
    parts
        .par_iter_mut()
        .for_each(|(installed, mut device, mut power)| {
            let Ok((outputs, hull)) = ships.get(installed.ship) else {
                return;
            };
            let output = &outputs.0[installed.index];
            if device.0.operational && hull.0 > 0.0 {
                device.0.actual = output.actual;
                device.0.thrust_n = output.thrust_n;
                device.0.powered = output.powered;
                *power = output.power;
            } else {
                device.0.actual = 0.0;
                device.0.thrust_n = [0.0; 3];
                device.0.powered = false;
                *power = DevicePower::default();
            }
        });
}

pub(crate) fn publish_mass(
    cat: Res<ShipCatalogue>,
    identities: Option<Res<super::identity::IdentityIndex>>,
    mut ships: Query<(
        Entity,
        &ShipDesign,
        HardwareWrite,
        &mut MassProps,
        Option<&mut super::travel::StoredMass>,
        Option<&super::travel::PresenceState>,
    )>,
) {
    let mut changes = Vec::new();
    for (entity, design, hardware, mut mass, stored, _) in &mut ships {
        let (own, inertia) = hardware.mass_properties(&design.0, &cat.0);
        let updated = own + stored.map_or(0., |s| s.0);
        let delta = updated - mass.mass;
        mass.mass = updated;
        mass.inertia = inertia * (updated / own);
        mass.inertia_inv = mass.inertia.inverse();
        if delta != 0. {
            changes.push((entity, delta));
        }
    }
    let Some(identities) = identities else {
        return;
    };
    for (entity, delta) in changes {
        let mut descendant = entity;
        let mut visited = Vec::new();
        while visited.len() < identities.0.len() {
            if visited.contains(&descendant) {
                break;
            }
            visited.push(descendant);
            let host_id = {
                let Ok((_, _, _, _, _, Some(presence))) = ships.get(descendant) else {
                    break;
                };
                match &presence.0 {
                    toy_sim_model::travel::Presence::Docked { host, .. }
                    | toy_sim_model::travel::Presence::StoredInWreck(host) => *host,
                    _ => break,
                }
            };
            let Some(&host) = identities.0.get(&host_id) else {
                break;
            };
            let Ok((_, _, _, mut mass, Some(mut stored), _)) = ships.get_mut(host) else {
                break;
            };
            let previous = mass.mass;
            stored.0 = (stored.0 + delta).max(0.);
            mass.mass = (mass.mass + delta).max(0.);
            if previous > 0. {
                let scale = mass.mass / previous;
                mass.inertia *= scale;
                mass.inertia_inv = mass.inertia.inverse();
            }
            descendant = host;
        }
    }
}

pub fn add_travel_heat(world: &mut World, ship: Entity, energy_j: f64, dt: f64) {
    if let Some(mut thermal) = world.get_mut::<ShipThermal>(ship) {
        thermal.0.add_waste_heat(energy_j, dt);
    }
}

pub fn shutdown(world: &mut World, ship: Entity) {
    let settings = world
        .get::<ShipDesign>(ship)
        .map(|d| default_settings(&d.0));
    if let (Some(settings), Some(mut target)) = (settings, world.get_mut::<DeviceSettings>(ship)) {
        target.0 = settings;
    }
    if let Some(mut avionics) = world.get_mut::<Avionics>(ship) {
        avionics.0.powered = false;
    }
    if let Some(mut sensor) = world.get_mut::<SensorRange>(ship) {
        sensor.0 = 0.0;
    }
    if let Some(mut flow) = world.get_mut::<PowerFlow>(ship) {
        *flow = PowerFlow::default();
    }
    let parts = world
        .get::<PartDevices>(ship)
        .map(|parts| parts.0.clone())
        .unwrap_or_default();
    for entity in parts {
        world.entity_mut(entity).remove::<ActiveDevice>();
        if let Some(mut device) = world.get_mut::<Device>(entity) {
            device.0.actual = 0.0;
            device.0.thrust_n = [0.0; 3];
            device.0.powered = false;
        }
        if let Some(mut weapon) = world.get_mut::<Weapon>(entity) {
            weapon.0.powered = false;
            weapon.0.command = None;
        }
        if let Some(mut power) = world.get_mut::<DevicePower>(entity) {
            *power = DevicePower::default();
        }
    }
    if let Some(mut t) = world.get_mut::<ShipThermal>(ship) {
        t.0.shield_powered = false;
        t.0.shield_enabled = false;
        t.0.shield_state = toy_sim_ship_api::abi::SHIELD_OFF;
    }
}

fn spend(inventory: &mut Inventory, demand: [f64; 3]) -> f64 {
    let available = [
        inventory.available(0),
        inventory.available(1),
        inventory.energy_j as f64,
    ];
    let mut fraction = 1f64;
    for i in 0..3 {
        if demand[i] > 0. {
            fraction = fraction.min((available[i] / demand[i]).clamp(0., 1.));
        }
    }
    inventory.consume(0, demand[0] * fraction);
    inventory.consume(1, demand[1] * fraction);
    inventory.energy_j.withdraw(demand[2] * fraction);
    fraction
}

fn default_settings(d: &CompiledShipDesign) -> Vec<Option<DeviceSetting>> {
    d.device_catalogue
        .iter()
        .map(|device| match device.kind {
            DeviceKind::Engine { .. } => Some(DeviceSetting::Throttle(0.)),
            DeviceKind::Torquer { .. } => Some(DeviceSetting::TorqueNm([0.; 3])),
            DeviceKind::Rcs { .. } => Some(DeviceSetting::RcsThrust([0.; 3])),
            DeviceKind::Generator { .. } => Some(DeviceSetting::GeneratorDemand(1.)),
            DeviceKind::Shield { .. } => Some(DeviceSetting::ShieldEnabled(true)),
            DeviceKind::Sensor { .. } => Some(DeviceSetting::SensorEnabled(
                d.blueprint.avionics.sensor_enabled,
            )),
            _ => None,
        })
        .collect()
}

fn power_totals(
    mut ships: Query<
        (
            &ShipDesign,
            &PartDevices,
            &Avionics,
            &SensorRange,
            &mut PowerFlow,
            &mut super::displays::DisplayEnvironment,
        ),
        Without<super::travel::Dormant>,
    >,
    parts: Query<&DevicePower>,
) {
    ships.par_iter_mut().for_each(
        |(design, installed, avionics, sensor, mut flow, mut display)| {
            *flow = PowerFlow::default();
            display.powered = avionics.0.operational && avionics.0.powered;

            for (index, part) in installed.0.iter().enumerate() {
                let Ok(power) = parts.get(*part) else {
                    continue;
                };
                flow.generated_w += power.recovered_w;
                if matches!(
                    design.0.parts[index].definition.equipment,
                    Equipment::Generator { .. }
                ) {
                    flow.generated_w += power.supplied_w;
                } else {
                    flow.requested_w += power.requested_w;
                    flow.supplied_w += power.supplied_w;
                }
            }

            if sensor.0 > 0.0 {
                flow.requested_w += SENSOR_POWER_W;
                flow.supplied_w += SENSOR_POWER_W;
            }
        },
    );
}

fn dormant_thermal(
    time: Res<Time<Fixed>>,
    mut ships: Query<
        (
            &ShipDesign,
            &mut Hull,
            &mut ShipThermal,
            &mut DormantThermalElapsed,
            Option<&super::travel::PresenceState>,
        ),
        With<super::travel::Dormant>,
    >,
) {
    ships
        .par_iter_mut()
        .for_each(|(design, mut hull, mut thermal, mut elapsed, presence)| {
            if presence
                .is_some_and(|p| matches!(p.0, toy_sim_model::travel::Presence::Docked { .. }))
            {
                return;
            }
            elapsed.0 += time.delta_secs_f64();
            if elapsed.0 < 1.0 - 1e-9 {
                return;
            }
            let duration = std::mem::take(&mut elapsed.0);
            thermal
                .0
                .advance(&mut hull.0, design.0.as_ref().into(), duration);
        });
}

#[cfg(test)]
mod tests;

fn sensor_overrides(
    mut ships: Query<
        (
            &SensorOverride,
            &mut SensorRange,
            &mut super::sensors::Sensor,
        ),
        Without<super::travel::Dormant>,
    >,
) {
    for (override_, mut range, mut sensor) in &mut ships {
        range.0 = override_.range_m;
        sensor.range_m = override_.range_m;
        sensor.occlusion = override_.occlusion;
    }
}

#[cfg(test)]
pub(crate) mod fixtures;

pub fn wake(world: &mut World, ship: Entity) {
    let parts = world
        .get::<PartDevices>(ship)
        .map(|parts| parts.0.clone())
        .unwrap_or_default();
    for entity in parts {
        world.entity_mut(entity).insert(ActiveDevice);
    }
}
