use crate::*;
use crate::{DeviceCommand, DeviceKind, DeviceReading, DeviceSetting, DeviceStatus};
use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
#[derive(Clone, Debug)]
pub struct Inventory {
    pub quantities: Vec<f64>,
    pub energy_j: f64,
}
impl Inventory {
    pub fn empty(cat: &Catalogue) -> Self {
        Self {
            quantities: vec![0.; cat.resources.len()],
            energy_j: 0.,
        }
    }
    pub fn volume(&self, cat: &Catalogue) -> f64 {
        self.quantities
            .iter()
            .zip(&cat.resources)
            .map(|(&q, r)| q * r.volume_m3)
            .sum()
    }
    pub fn mass(&self, cat: &Catalogue) -> f64 {
        self.quantities
            .iter()
            .zip(&cat.resources)
            .map(|(&q, r)| q * r.mass_kg)
            .sum()
    }
    pub fn insert(
        &mut self,
        resource: usize,
        amount: f64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(
            amount.is_finite() && amount >= 0. && capacity.is_finite() && capacity >= 0.,
            "invalid inventory amount/capacity"
        );
        let r = cat.resources.get(resource).context("unknown resource")?;
        ensure!(
            self.volume(cat) + amount * r.volume_m3 <= capacity,
            "inventory capacity exceeded"
        );
        let q = self
            .quantities
            .get_mut(resource)
            .context("unknown resource")?;
        ensure!(
            amount.is_finite() && amount >= 0. && (*q + amount).is_finite(),
            "invalid resource amount"
        );
        *q += amount;
        Ok(())
    }
    pub fn transfer(
        &mut self,
        to: &mut Self,
        resource: usize,
        amount: f64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(
            *self.quantities.get(resource).context("unknown resource")? >= amount,
            "insufficient inventory"
        );
        to.insert(resource, amount, capacity, cat)?;
        self.quantities[resource] -= amount;
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct DeviceState {
    pub operational: bool,
    pub powered: bool,
    pub actual: f64,
    pub thrust_n: [f64; 3],
}
impl Default for DeviceState {
    fn default() -> Self {
        Self {
            operational: true,
            powered: true,
            actual: 0.,
            thrust_n: [0.; 3],
        }
    }
}
#[derive(Clone, Debug)]
pub struct ShipState {
    pub inventory: Inventory,
    pub weapons: Vec<crate::weapons::WeaponState>,
    pub devices: Vec<DeviceState>,
    pub avionics: DeviceState,
    pub hull: f64,
    pub thermal: crate::thermal::ThermalState,
    pub settings: Vec<Option<DeviceSetting>>,
    pub tick: u64,
    pub sensor_range: f64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct HardwareOutput {
    pub force: DVec3,
    pub torque: DVec3,
    pub mass: f64,
    pub inertia: DMat3,
}
impl ShipState {
    pub fn new(design: &CompiledShipDesign, cat: &Catalogue) -> Self {
        Self {
            inventory: Inventory::empty(cat),
            weapons: vec![crate::weapons::WeaponState::default(); design.weapon_parts.len()],
            devices: vec![DeviceState::default(); design.parts.len()],
            avionics: DeviceState::default(),
            hull: design.hull,
            thermal: crate::thermal::ThermalState::new(design.into()),
            settings: default_settings(design),
            tick: 0,
            sensor_range: 0.,
        }
    }
    pub fn test_loadout(&mut self, d: &CompiledShipDesign, cat: &Catalogue) {
        self.inventory.energy_j = d.battery_j;
        let fuel = (d.capacity_m3 * 10.).min(10.);
        let _ = self.inventory.insert(1, fuel, d.capacity_m3, cat);
        for spec in &d.weapon_specs {
            let resource = spec.ammunition_resource as usize - 1;
            let wanted: f64 = if spec.projectile_mass_kg < 1.0 {
                1000.0
            } else {
                12.0
            };
            let available = ((d.capacity_m3 - self.inventory.volume(cat)).max(0.0)
                / cat.resources[resource].volume_m3)
                .floor();
            let _ = self
                .inventory
                .insert(resource, wanted.min(available), d.capacity_m3, cat);
        }
        let propellant =
            (d.capacity_m3 - self.inventory.volume(cat)).max(0.) / cat.resources[0].volume_m3;
        let _ = self.inventory.insert(0, propellant, d.capacity_m3, cat);
    }
    pub fn computer_running(&self, _d: &CompiledShipDesign) -> bool {
        self.hull > 0. && self.avionics.operational && self.avionics.powered
    }
    /// Validate the entire command buffer before applying any of it.
    pub fn apply_commands(
        &mut self,
        d: &CompiledShipDesign,
        commands: &[DeviceCommand],
    ) -> Result<()> {
        ensure!(
            commands.len() <= crate::MAX_DEVICES,
            "too many device commands"
        );
        for command in commands {
            let descriptor = d
                .device_catalogue
                .get(usize::from(command.device.0))
                .context("unknown device handle")?;
            ensure!(
                command.setting.finite() && command.setting.supports(&descriptor.kind),
                "invalid setting for device {}",
                descriptor.alias
            );
        }
        for command in commands {
            self.settings[usize::from(command.device.0)] = Some(command.setting.clone());
        }
        Ok(())
    }
    pub fn reset_commands(&mut self, d: &CompiledShipDesign) {
        self.settings = default_settings(d);
        for weapon in &mut self.weapons {
            weapon.command = None;
        }
    }
    pub fn snapshot(&self, d: &CompiledShipDesign) -> Vec<DeviceStatus> {
        let states: Vec<_> = d
            .device_catalogue
            .iter()
            .zip(&d.device_sources)
            .map(|(descriptor, source)| {
                let state = match source {
                    DeviceSource::Part(part) => &self.devices[*part],
                    DeviceSource::Avionics => &self.avionics,
                };
                let actual = if state.operational && state.powered {
                    state.actual
                } else {
                    0.
                };
                let reading = match descriptor.kind {
                    DeviceKind::Accelerometer => DeviceReading::Accelerometer { sample: None },
                    DeviceKind::Computer => DeviceReading::Computer,
                    DeviceKind::Rcs { .. } => DeviceReading::Rcs {
                        thrust_n: if state.operational && state.powered {
                            state.thrust_n
                        } else {
                            [0.; 3]
                        },
                    },
                    DeviceKind::Weapon => {
                        let part = d.part_for_device(descriptor.handle).unwrap();
                        let index = d.part_weapons[part].unwrap();
                        let weapon = &self.weapons[index];
                        let spec = &d.weapon_specs[index];
                        use toy_sim_ship_api::abi;

                        let ammo = self.inventory.quantities[spec.ammunition_resource as usize - 1];
                        let mut flags = weapon.inhibit_flags
                            & (abi::WEAPON_BLOCKED | abi::WEAPON_TRAVEL | abi::WEAPON_POINTING);
                        for (blocked, flag) in [
                            (!state.operational, abi::WEAPON_UNAVAILABLE),
                            (ammo < 1.0, abi::WEAPON_AMMO),
                            (
                                self.inventory.quantities[0]
                                    < crate::weapons::shot_propellant_kg(spec)
                                        + if spec.ammunition_resource == 1 {
                                            1.0
                                        } else {
                                            0.0
                                        },
                                abi::WEAPON_PROPELLANT,
                            ),
                            (
                                self.inventory.energy_j < crate::weapons::shot_energy(spec),
                                abi::WEAPON_ENERGY,
                            ),
                            (
                                weapon.next_fire_s > weapon.advanced_s + 1e-8,
                                abi::WEAPON_COOLDOWN,
                            ),
                        ] {
                            if blocked {
                                flags |= flag;
                            }
                        }
                        DeviceReading::Weapon(toy_sim_ship_api::abi::WeaponReading {
                            status: Default::default(),
                            inhibit_flags: flags,
                            ammunition_units: self.inventory.quantities
                                [spec.ammunition_resource as usize - 1]
                                .floor() as u64,
                            shots_fired: weapon.shots_fired,
                            battery_energy_j: self.inventory.energy_j,
                            shot_energy_j: crate::weapons::shot_energy(spec),
                            yaw_rad: weapon.yaw_rad,
                            pitch_rad: weapon.pitch_rad,
                            next_fire_s: weapon.next_fire_s,
                        })
                    }
                    DeviceKind::Storage { .. } => DeviceReading::Storage,
                    DeviceKind::Battery { .. } => DeviceReading::Battery,
                    DeviceKind::Engine { .. } => DeviceReading::Engine { thrust_n: actual },
                    DeviceKind::Torquer { .. } => DeviceReading::Torquer { torque_nm: actual },
                    DeviceKind::Generator { .. } => DeviceReading::Generator { power_w: actual },
                    DeviceKind::Shield { .. } => DeviceReading::Shield {
                        state: self.thermal.shield_state,
                        temperature_k: self.shield_temperature(d),
                        reserve_kg: self.thermal.shield_reserve_kg,
                        reserve_capacity_kg: d.shield_reserve_capacity_kg,
                        strength: self.thermal.shield_strength(d.into()),
                        ablation_kg_s: self.thermal.ablation_kg_s,
                        radiated_power_w: crate::thermal::radiation(
                            self.shield_temperature(d),
                            self.thermal.radiator_area(d.into()),
                        ),
                        power_w: actual,
                    },
                    DeviceKind::Sensor { .. } => DeviceReading::Sensor {
                        range_m: if state.operational && state.powered {
                            self.sensor_range
                        } else {
                            0.
                        },
                    },
                };
                DeviceStatus {
                    operational: state.operational,
                    powered: state.powered
                        && (!matches!(descriptor.kind, DeviceKind::Sensor { .. })
                            || self.sensor_range > 0.),
                    reading,
                }
            })
            .collect();
        states
    }

    pub fn mass_properties(&self, d: &CompiledShipDesign, cat: &Catalogue) -> (f64, DMat3) {
        let m = d.dry_mass
            + self.inventory.mass(cat)
            + self.thermal.shield_reserve_kg
            + self.thermal.shield_deployed_kg;
        (m, d.inertia * (m / d.dry_mass))
    }
    fn step_avionics(&mut self, d: &CompiledShipDesign, dt: f64) {
        self.avionics.powered = self.avionics.operational
            && self.hull > 0.
            && spend(&mut self.inventory, [0., 0., AVIONICS_POWER_W * dt]) >= 1. - 1e-9;
        if !self.avionics.powered {
            self.reset_commands(d);
            return;
        }
        let enabled = matches!(
            self.settings[d.avionics_handles[2].0 as usize],
            Some(DeviceSetting::SensorEnabled(true))
        );
        if enabled && spend(&mut self.inventory, [0., 0., SENSOR_POWER_W * dt]) >= 1. - 1e-9 {
            self.sensor_range = SENSOR_RANGE_M;
        }
    }
    pub fn step(&mut self, d: &CompiledShipDesign, cat: &Catalogue, dt: f64) -> HardwareOutput {
        assert!(dt.is_finite() && dt > 0. && dt <= 1.);
        self.tick += 1;
        self.sensor_range = 0.;
        for weapon in &mut self.weapons {
            weapon.previous_yaw_rad = weapon.yaw_rad;
            weapon.previous_pitch_rad = weapon.pitch_rad;
            weapon.powered = false;
            weapon.command = None;
        }
        self.thermal.shield_powered = false;
        self.thermal.shield_enabled = false;
        if !self.computer_running(d) {
            self.reset_commands(d);
        }
        if self.hull <= 0. {
            for s in &mut self.devices {
                s.actual = 0.;
                s.powered = false;
            }
            self.avionics.powered = false;
            self.reset_commands(d);
            let (m, i) = self.mass_properties(d, cat);
            return HardwareOutput {
                mass: m,
                inertia: i,
                ..Default::default()
            };
        }
        let mut output = HardwareOutput::default();
        let mut avionics_updated = false;
        for &i in &d.active_parts {
            if !avionics_updated
                && !matches!(d.parts[i].definition.equipment, Equipment::Generator { .. })
            {
                self.step_avionics(d, dt);
                avionics_updated = true;
            }
            let p = &d.parts[i];
            let setting = self.settings[d.part_devices[i].expect("active part has device")].clone();
            let throttle = match setting {
                Some(DeviceSetting::Throttle(v) | DeviceSetting::GeneratorDemand(v)) => {
                    v.clamp(0., 1.)
                }
                _ => 0.,
            };
            let enabled = matches!(
                setting,
                Some(DeviceSetting::SensorEnabled(true) | DeviceSetting::ShieldEnabled(true))
            );
            let state = &mut self.devices[i];
            state.actual = 0.;
            state.thrust_n = [0.; 3];
            state.powered = state.operational;
            if !state.operational {
                continue;
            }
            match p.definition.equipment {
                Equipment::Weapon { .. } => {
                    let index = d.part_weapons[i].unwrap();
                    let weapon = &mut self.weapons[index];
                    weapon.powered = true;
                    // The application supplies the absolute epoch before collision integration.
                    weapon.command = match setting {
                        Some(DeviceSetting::Weapon(value)) => Some(crate::weapons::WeaponCommand {
                            setting: value,
                            epoch_s: 0.0,
                        }),
                        _ => None,
                    };
                }

                Equipment::Generator {
                    power_w,
                    fuel_kg_s,
                    efficiency,
                } => {
                    let wanted = (power_w * dt * throttle)
                        .min((d.battery_j - self.inventory.energy_j).max(0.));
                    if wanted > 0. {
                        let fuel = fuel_kg_s * wanted / power_w;
                        let fraction = spend(&mut self.inventory, [0., fuel, 0.]);
                        state.powered = fraction > 0.;
                        let generated = wanted * fraction;
                        self.inventory.energy_j += generated;
                        self.thermal
                            .add_waste_heat(generated * (1.0 / efficiency - 1.0), dt);
                        state.actual = generated / dt;
                    }
                }
                Equipment::Engine {
                    thrust_n,
                    propellant_kg_s,
                    power_w,
                    ..
                } => {
                    let f = spend(
                        &mut self.inventory,
                        [propellant_kg_s * dt * throttle, 0., power_w * dt * throttle],
                    );
                    state.powered = throttle == 0. || f > 0.;
                    state.actual = thrust_n * throttle * f;
                    let force = p.rotation * DVec3::NEG_Z * state.actual;
                    output.force += force;
                    output.torque += (p.centre - d.centre).cross(force);
                }
                Equipment::Rcs {
                    thrust_n,
                    propellant_kg_s,
                    power_w,
                } => {
                    let demand = match setting {
                        Some(DeviceSetting::RcsThrust(value)) => {
                            DVec3::from_array(value) / thrust_n
                        }
                        _ => DVec3::ZERO,
                    }
                    .clamp(DVec3::NEG_ONE, DVec3::ONE);
                    // Each signed axis selects one of its two opposing nozzles.
                    let total = demand.abs().element_sum();
                    let fraction = spend(
                        &mut self.inventory,
                        [propellant_kg_s * dt * total, 0.0, power_w * dt * total],
                    );
                    let local_force = demand * (thrust_n * fraction);
                    state.powered = total == 0.0 || fraction > 0.0;
                    state.actual = local_force.length();
                    state.thrust_n = local_force.to_array();
                    let force = p.rotation * local_force;
                    output.force += force;
                    output.torque += (p.centre - d.centre).cross(force);
                }
                Equipment::Torquer { torque_nm, power_w } => {
                    let v = match setting {
                        Some(DeviceSetting::TorqueNm(value)) => {
                            DVec3::from_array(value) / torque_nm
                        }
                        _ => DVec3::ZERO,
                    }
                    .clamp(DVec3::NEG_ONE, DVec3::ONE);
                    let f = spend(
                        &mut self.inventory,
                        [0., 0., power_w * dt * v.abs().max_element()],
                    );
                    state.powered = v == DVec3::ZERO || f > 0.;
                    output.torque += p.rotation * v * torque_nm * f;
                    state.actual = torque_nm * f * v.length();
                }
                Equipment::Shield { power_w, .. } => {
                    self.thermal.shield_enabled |= enabled;
                    let fraction = if enabled {
                        spend(&mut self.inventory, [0.0, 0.0, power_w * dt])
                    } else {
                        0.0
                    };
                    state.powered = enabled && fraction >= 1.0 - 1e-9;
                    state.actual = fraction * power_w;
                    self.thermal.add_waste_heat(state.actual * dt, dt);
                }
                _ => {}
            }
        }
        self.thermal.shield_powered = self.thermal.shield_enabled
            && d.active_parts
                .iter()
                .copied()
                .filter(|&i| matches!(d.parts[i].definition.equipment, Equipment::Shield { .. }))
                .all(|i| self.devices[i].operational && self.devices[i].powered);

        if !avionics_updated {
            self.step_avionics(d, dt);
        }
        if !self.computer_running(d) {
            self.reset_commands(d);
        }
        let (m, inertia) = self.mass_properties(d, cat);
        output.mass = m;
        output.inertia = inertia;
        output
    }
}
/// Allocate each operation's inputs together; output scales with actual supply.
fn spend(inventory: &mut Inventory, demand: [f64; 3]) -> f64 {
    let available = [
        inventory.quantities[0],
        inventory.quantities[1],
        inventory.energy_j,
    ];
    let mut fraction = 1f64;
    for i in 0..3 {
        if demand[i] > 0. {
            fraction = fraction.min((available[i] / demand[i]).clamp(0., 1.));
        }
    }
    inventory.quantities[0] = (inventory.quantities[0] - demand[0] * fraction).max(0.);
    inventory.quantities[1] = (inventory.quantities[1] - demand[1] * fraction).max(0.);
    inventory.energy_j = (inventory.energy_j - demand[2] * fraction).max(0.);
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
