use crate::*;
use crate::{DeviceCommand, DeviceKind, DeviceReading, DeviceSetting, DeviceStatus};
use anyhow::{Context, Result, ensure};
use glam::DMat3;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Inventory {
    pub quantities: Vec<u64>,
    pub cargo: Vec<u64>,
    pub packaged_parts: std::collections::BTreeMap<String, u64>,
    pub reservations: std::collections::BTreeMap<osg_model::industry::CargoItem, u64>,
    pub tank_capacities_m3: Vec<f64>,
    pub energy_j: u64,
}

impl Inventory {
    pub fn empty(cat: &Catalogue) -> Self {
        Self {
            quantities: vec![0; cat.resources.len()],
            cargo: vec![0; cat.resources.len()],
            packaged_parts: Default::default(),
            reservations: Default::default(),
            tank_capacities_m3: vec![0.; cat.resources.len()],
            energy_j: 0,
        }
    }

    pub fn for_design(design: &CompiledShipDesign, cat: &Catalogue) -> Self {
        let mut inventory = Self::empty(cat);
        let mut initial = vec![0.0; cat.resources.len()];
        for part in &design.parts {
            for tank in &part.placed.tanks {
                let index = cat
                    .resources
                    .iter()
                    .position(|r| r.id == tank.resource)
                    .expect("validated tank resource");
                let usable = tank.volume_m3 * cat.resources[index].storage.usable_fraction;
                inventory.tank_capacities_m3[index] += usable;
                initial[index] += usable * tank.initial_fill / cat.resources[index].volume_m3;
            }
        }
        inventory.quantities = initial.into_iter().map(|q| q.floor() as u64).collect();
        inventory
    }

    pub fn available(&self, resource: usize) -> f64 {
        self.quantities[resource] as f64
    }

    pub fn consume(&mut self, resource: usize, requested: f64) -> u64 {
        self.quantities[resource].withdraw(requested)
    }

    pub fn tank_room(&self, resource: usize, cat: &Catalogue) -> u64 {
        let capacity =
            (self.tank_capacities_m3[resource] / cat.resources[resource].volume_m3).floor() as u64;
        capacity.saturating_sub(self.quantities[resource])
    }

    pub fn insert_consumable(
        &mut self,
        resource: usize,
        amount: u64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(resource < self.quantities.len(), "unknown resource");
        ensure!(
            amount <= self.tank_room(resource, cat),
            "tank capacity exceeded"
        );
        self.quantities[resource] = self.quantities[resource]
            .checked_add(amount)
            .context("quantity overflow")?;
        Ok(())
    }

    pub fn cargo_volume(&self, cat: &Catalogue) -> f64 {
        self.packaged_volume(cat)
            + self
                .cargo
                .iter()
                .zip(&cat.resources)
                .map(|(&q, r)| q as f64 * r.volume_m3)
                .sum::<f64>()
    }

    pub fn volume(&self, cat: &Catalogue) -> f64 {
        self.packaged_volume(cat)
            + self
                .quantities
                .iter()
                .zip(&self.cargo)
                .zip(&cat.resources)
                .map(|((&q, &cargo), r)| (q as f64 + cargo as f64) * r.volume_m3)
                .sum::<f64>()
    }

    pub fn mass(&self, cat: &Catalogue) -> f64 {
        self.packaged_mass(cat)
            + self
                .quantities
                .iter()
                .zip(&self.cargo)
                .zip(&cat.resources)
                .map(|((&q, &cargo), r)| (q as f64 + cargo as f64) * r.mass_kg)
                .sum::<f64>()
    }

    pub fn insert_cargo(
        &mut self,
        resource: usize,
        amount: u64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        let item = osg_model::industry::CargoItem::Resource(
            cat.resources
                .get(resource)
                .context("unknown resource")?
                .id
                .clone(),
        );
        self.insert_item(&item, amount, capacity, cat)
    }

    pub fn transfer_cargo(
        &mut self,
        to: &mut Self,
        resource: usize,
        amount: u64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        let item = osg_model::industry::CargoItem::Resource(
            cat.resources
                .get(resource)
                .context("unknown resource")?
                .id
                .clone(),
        );
        self.transfer_item(to, &item, amount, capacity, cat)
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeviceState {
    pub operational: bool,
    pub powered: bool,
    pub actual: f64,
    #[serde(skip)]
    pub generated_w: f64,
    pub thrust_n: [f64; 3],
}
impl Default for DeviceState {
    fn default() -> Self {
        Self {
            operational: true,
            powered: true,
            actual: 0.,
            generated_w: 0.,
            thrust_n: [0.; 3],
        }
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    pub fn cold(design: &CompiledShipDesign, cat: &Catalogue) -> Self {
        let mut state = Self::new(design, cat);
        state.inventory.quantities.fill(0);
        state.inventory.energy_j = 0;
        state.thermal.shield_deployed_kg = 0.;
        state.thermal.shield_reserve_mg = 0;
        state.thermal.shield_energy_j = 0.;
        state
    }

    pub fn new(design: &CompiledShipDesign, cat: &Catalogue) -> Self {
        Self {
            inventory: Inventory::for_design(design, cat),
            weapons: vec![crate::weapons::WeaponState::default(); design.weapon_parts.len()],
            devices: vec![DeviceState::default(); design.parts.len()],
            avionics: DeviceState {
                powered: false,
                ..DeviceState::default()
            },
            hull: design.hull,
            thermal: crate::thermal::ThermalState::new(design.into()),
            settings: default_settings(design),
            tick: 0,
            sensor_range: 0.,
        }
    }
    pub fn test_loadout(&mut self, d: &CompiledShipDesign, _cat: &Catalogue) {
        self.inventory.energy_j = d.battery_j;
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
                    DeviceSource::Part(part) | DeviceSource::MicropulseGenerator(part) => {
                        &self.devices[*part]
                    }
                    DeviceSource::Avionics => &self.avionics,
                };
                let actual = if state.operational && state.powered {
                    if matches!(source, DeviceSource::MicropulseGenerator(_)) {
                        state.generated_w
                    } else {
                        state.actual
                    }
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
                        use osg_ship_api::abi;

                        let ammo = if spec.beam_power_w > 0.0 {
                            f64::INFINITY
                        } else {
                            self.inventory
                                .available(spec.ammunition_resource as usize - 1)
                        };
                        let mut flags = weapon.inhibit_flags
                            & (abi::WEAPON_BLOCKED | abi::WEAPON_TRAVEL | abi::WEAPON_POINTING);
                        for (blocked, flag) in [
                            (!state.operational, abi::WEAPON_UNAVAILABLE),
                            (ammo < 1.0, abi::WEAPON_AMMO),
                            (
                                self.inventory.available(0)
                                    < crate::weapons::shot_propellant_kg(spec)
                                        + if spec.ammunition_resource == 1 {
                                            1.0
                                        } else {
                                            0.0
                                        },
                                abi::WEAPON_PROPELLANT,
                            ),
                            (
                                (self.inventory.energy_j as f64)
                                    < crate::weapons::shot_energy(spec),
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
                        DeviceReading::Weapon(osg_ship_api::abi::WeaponReading {
                            status: Default::default(),
                            inhibit_flags: flags,
                            ammunition_units: ammo.floor() as u64,
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
                        reserve_kg: self.thermal.shield_reserve_kg(),
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
            + self.thermal.shield_reserve_kg()
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
