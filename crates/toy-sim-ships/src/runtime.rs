use crate::*;
use crate::{DeviceCommand, DeviceKind, DeviceReading, DeviceSetting, DeviceStatus};
use anyhow::{Context, Result, ensure};
use glam::DMat3;
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
