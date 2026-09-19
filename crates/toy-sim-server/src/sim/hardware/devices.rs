use super::*;

#[derive(Component)]
pub struct Generator {
    pub power_w: f64,
    pub fuel_kg_s: f64,
    pub efficiency: f64,
}

#[derive(Component)]
pub struct Engine {
    pub propellant_resource: String,
    pub propellant_energy_j_kg: f64,
    pub thrust_n: f64,
    pub propellant_kg_s: f64,
    pub power_w: f64,
}

#[derive(Component)]
pub struct ThermalEngine {
    pub thrust_n: f64,
    pub specific_impulse_s: f64,
    pub propellant_resource: String,
    pub thermal_efficiency: f64,
    pub fuel_energy_j_kg: f64,
    pub decay_energy_j: f64,
}

#[derive(Component)]
pub struct MicropulseEngine {
    pub thrust_n: f64,
    pub specific_impulse_s: f64,
    pub charge_energy_j_kg: f64,
    pub electric_fraction: f64,
    pub absorbed_heat_fraction: f64,
}

#[derive(Component)]
pub struct ReactionControl {
    pub propellant_resource: String,
    pub thrust_n: f64,
    pub propellant_kg_s: f64,
    pub power_w: f64,
}

#[derive(Component)]
pub struct Torquer {
    pub torque_nm: f64,
    pub power_w: f64,
}

#[derive(Component)]
pub struct Shield {
    pub power_w: f64,
}

#[derive(Component, Default)]
pub struct Demand {
    pub inputs: [f64; 3],
    pub resource_input: Option<(usize, f64)>,
    pub secondary_resource_input: Option<(usize, f64)>,
    pub resource_output: Option<(usize, f64)>,
    pub delayed_heat_j: f64,
    pub generated_energy_j: f64,
    pub force: DVec3,
    pub torque: DVec3,
    pub actual: f64,
    pub thrust: DVec3,
    pub waste_heat_j: f64,
    pub enabled: bool,
    pub requires_full_supply: bool,
}

pub fn install(part: &mut EntityCommands, equipment: &Equipment) {
    match *equipment {
        Equipment::Generator {
            power_w,
            fuel_kg_s,
            efficiency,
        } => {
            part.insert(Generator {
                power_w,
                fuel_kg_s,
                efficiency,
            });
        }
        Equipment::Engine {
            ref propellant_resource,
            propellant_energy_j_kg,
            thrust_n,
            propellant_kg_s,
            power_w,
            ..
        } => {
            part.insert((
                Engine {
                    propellant_resource: propellant_resource.clone(),
                    propellant_energy_j_kg,
                    thrust_n,
                    propellant_kg_s,
                    power_w,
                },
                Demand::default(),
            ));
        }
        Equipment::ThermalEngine {
            thrust_n,
            specific_impulse_s,
            ref propellant_resource,
            thermal_efficiency,
            fuel_energy_j_kg,
            ..
        } => {
            part.insert((
                ThermalEngine {
                    thrust_n,
                    specific_impulse_s,
                    propellant_resource: propellant_resource.clone(),
                    thermal_efficiency,
                    fuel_energy_j_kg,
                    decay_energy_j: 0.0,
                },
                Demand::default(),
            ));
        }
        Equipment::MicropulseEngine {
            thrust_n,
            specific_impulse_s,
            charge_energy_j_kg,
            electric_fraction,
            absorbed_heat_fraction,
            ..
        } => {
            part.insert((
                MicropulseEngine {
                    thrust_n,
                    specific_impulse_s,
                    charge_energy_j_kg,
                    electric_fraction,
                    absorbed_heat_fraction,
                },
                Demand::default(),
            ));
        }
        Equipment::Rcs {
            ref propellant_resource,
            thrust_n,
            propellant_kg_s,
            power_w,
        } => {
            part.insert((
                ReactionControl {
                    propellant_resource: propellant_resource.clone(),
                    thrust_n,
                    propellant_kg_s,
                    power_w,
                },
                Demand::default(),
            ));
        }
        Equipment::Torquer { torque_nm, power_w } => {
            part.insert((Torquer { torque_nm, power_w }, Demand::default()));
        }
        Equipment::Shield { power_w, .. } => {
            part.insert((Shield { power_w }, Demand::default()));
        }
        _ => {}
    }
}

pub(crate) fn prepare_engines(
    time: Res<Time<Fixed>>,
    catalogue: Res<ShipCatalogue>,
    ships: Query<(&ShipDesign, &DeviceSettings), Without<super::super::travel::Dormant>>,
    mut devices: Query<(&InstalledPart, &Engine, &mut Demand), With<ActiveDevice>>,
) {
    let dt = time.delta_secs_f64();
    devices
        .par_iter_mut()
        .for_each(|(installed, engine, mut demand)| {
            let Ok((design, settings)) = ships.get(installed.ship) else {
                return;
            };
            let d = &design.0;
            let part = &d.parts[installed.index];
            let throttle = match settings.0[d.part_devices[installed.index].unwrap()] {
                Some(DeviceSetting::Throttle(value)) => value.clamp(0.0, 1.0),
                _ => 0.0,
            };
            let actual = engine.thrust_n * throttle;
            let force = part.rotation * DVec3::NEG_Z * actual;

            *demand = Demand {
                inputs: [0.0, 0.0, engine.power_w * dt * throttle],
                resource_input: catalogue
                    .0
                    .resources
                    .iter()
                    .enumerate()
                    .find(|(_, r)| r.id == engine.propellant_resource)
                    .map(|(i, r)| (i, engine.propellant_kg_s * dt * throttle / r.mass_kg)),
                waste_heat_j: (engine.power_w
                    + engine.propellant_kg_s * engine.propellant_energy_j_kg
                    - 0.5 * engine.thrust_n.powi(2) / engine.propellant_kg_s)
                    .max(0.0)
                    * dt
                    * throttle,
                force,
                torque: (part.centre - d.centre).cross(force),
                actual,
                enabled: true,
                ..default()
            };
        });
}

pub(crate) fn prepare_micropulse_engines(
    time: Res<Time<Fixed>>,
    catalogue: Res<ShipCatalogue>,
    ships: Query<(&ShipDesign, &DeviceSettings), Without<super::super::travel::Dormant>>,
    mut devices: Query<(&InstalledPart, &MicropulseEngine, &mut Demand), With<ActiveDevice>>,
) {
    let dt = time.delta_secs_f64();
    let charge = catalogue
        .0
        .resources
        .iter()
        .enumerate()
        .find(|(_, resource)| resource.id == "micropulse_charge");

    devices
        .par_iter_mut()
        .for_each(|(installed, engine, mut demand)| {
            *demand = Demand::default();
            let Some((resource, charge)) = charge else {
                return;
            };
            let Ok((design, settings)) = ships.get(installed.ship) else {
                return;
            };
            let design = &design.0;
            let part = &design.parts[installed.index];
            let throttle = match settings.0[design.part_devices[installed.index].unwrap()] {
                Some(DeviceSetting::Throttle(value)) => value.clamp(0.0, 1.0),
                _ => 0.0,
            };
            let actual = engine.thrust_n * throttle;
            let charge_mass = actual / (STANDARD_GRAVITY_M_S2 * engine.specific_impulse_s) * dt;
            let released_energy = charge_mass * engine.charge_energy_j_kg;
            let force = part.rotation * DVec3::NEG_Z * actual;

            *demand = Demand {
                resource_input: Some((resource, charge_mass / charge.mass_kg)),
                generated_energy_j: released_energy * engine.electric_fraction,
                waste_heat_j: released_energy * engine.absorbed_heat_fraction,
                force,
                torque: (part.centre - design.centre).cross(force),
                actual,
                enabled: true,
                ..default()
            };
        });
}

pub(crate) fn prepare_rcs(
    time: Res<Time<Fixed>>,
    catalogue: Res<ShipCatalogue>,
    ships: Query<(&ShipDesign, &DeviceSettings), Without<super::super::travel::Dormant>>,
    mut devices: Query<(&InstalledPart, &ReactionControl, &mut Demand), With<ActiveDevice>>,
) {
    let dt = time.delta_secs_f64();
    devices
        .par_iter_mut()
        .for_each(|(installed, rcs, mut demand)| {
            let Ok((design, settings)) = ships.get(installed.ship) else {
                return;
            };
            let d = &design.0;
            let part = &d.parts[installed.index];
            let axes = match settings.0[d.part_devices[installed.index].unwrap()] {
                Some(DeviceSetting::RcsThrust(value)) => DVec3::from_array(value) / rcs.thrust_n,
                _ => DVec3::ZERO,
            }
            .clamp(DVec3::NEG_ONE, DVec3::ONE);
            let total = axes.abs().element_sum();
            let thrust = axes * rcs.thrust_n;
            let force = part.rotation * thrust;

            *demand = Demand {
                inputs: [0.0, 0.0, rcs.power_w * dt * total],
                resource_input: catalogue
                    .0
                    .resources
                    .iter()
                    .enumerate()
                    .find(|(_, r)| r.id == rcs.propellant_resource)
                    .map(|(i, r)| (i, rcs.propellant_kg_s * dt * total / r.mass_kg)),
                waste_heat_j: (rcs.power_w - 0.5 * rcs.thrust_n.powi(2) / rcs.propellant_kg_s)
                    .max(0.0)
                    * dt
                    * total,
                force,
                torque: (part.centre - d.centre).cross(force),
                actual: thrust.length(),
                thrust,
                enabled: true,
                ..default()
            };
        });
}

pub(crate) fn prepare_torquers(
    time: Res<Time<Fixed>>,
    ships: Query<(&ShipDesign, &DeviceSettings), Without<super::super::travel::Dormant>>,
    mut devices: Query<(&InstalledPart, &Torquer, &mut Demand), With<ActiveDevice>>,
) {
    let dt = time.delta_secs_f64();
    devices
        .par_iter_mut()
        .for_each(|(installed, torquer, mut demand)| {
            let Ok((design, settings)) = ships.get(installed.ship) else {
                return;
            };
            let d = &design.0;
            let axes = match settings.0[d.part_devices[installed.index].unwrap()] {
                Some(DeviceSetting::TorqueNm(value)) => {
                    DVec3::from_array(value) / torquer.torque_nm
                }
                _ => DVec3::ZERO,
            }
            .clamp(DVec3::NEG_ONE, DVec3::ONE);

            *demand = Demand {
                inputs: [0.0, 0.0, torquer.power_w * dt * axes.abs().max_element()],
                torque: d.parts[installed.index].rotation * axes * torquer.torque_nm,
                actual: torquer.torque_nm * axes.length(),
                enabled: true,
                ..default()
            };
        });
}

pub(crate) fn prepare_shields(
    time: Res<Time<Fixed>>,
    ships: Query<(&ShipDesign, &DeviceSettings), Without<super::super::travel::Dormant>>,
    mut devices: Query<(&InstalledPart, &Shield, &mut Demand), With<ActiveDevice>>,
) {
    let dt = time.delta_secs_f64();
    devices
        .par_iter_mut()
        .for_each(|(installed, shield, mut demand)| {
            let Ok((design, settings)) = ships.get(installed.ship) else {
                return;
            };
            let enabled = matches!(
                settings.0[design.0.part_devices[installed.index].unwrap()],
                Some(DeviceSetting::ShieldEnabled(true))
            );
            let power = if enabled { shield.power_w } else { 0.0 };

            *demand = Demand {
                inputs: [0.0, 0.0, power * dt],
                actual: power,
                waste_heat_j: power * dt,
                enabled,
                requires_full_supply: true,
                ..default()
            };
        });
}

pub(crate) fn prepare_weapons(
    ships: Query<(&ShipDesign, &DeviceSettings, &Hull), Without<super::super::travel::Dormant>>,
    mut weapons: Query<(&InstalledPart, &Device, &mut Weapon), With<ActiveDevice>>,
) {
    weapons
        .par_iter_mut()
        .for_each(|(installed, device, mut weapon)| {
            let Ok((design, settings, hull)) = ships.get(installed.ship) else {
                return;
            };
            weapon.0.powered = hull.0 > 0.0 && device.0.operational;
            weapon.0.command = if weapon.0.powered {
                match settings.0[design.0.part_devices[installed.index].unwrap()] {
                    Some(DeviceSetting::Weapon(setting)) => Some(weapons::WeaponCommand {
                        setting,
                        epoch_s: 0.0,
                    }),
                    _ => None,
                }
            } else {
                None
            };
        });
}

pub(crate) fn prepare_thermal_engines(
    time: Res<Time<Fixed>>,
    catalogue: Res<ShipCatalogue>,
    ships: Query<
        (&ShipDesign, &DeviceSettings, &ShipThermal),
        Without<super::super::travel::Dormant>,
    >,
    mut devices: Query<(&InstalledPart, &ThermalEngine, &mut Demand), With<ActiveDevice>>,
) {
    let dt = time.delta_secs_f64();
    devices
        .par_iter_mut()
        .for_each(|(installed, engine, mut demand)| {
            *demand = Demand::default();
            let Some((fuel_index, fuel)) = catalogue
                .0
                .resources
                .iter()
                .enumerate()
                .find(|(_, r)| r.id == "reactor_fuel")
            else {
                return;
            };
            let Some((propellant_index, propellant)) = catalogue
                .0
                .resources
                .iter()
                .enumerate()
                .find(|(_, r)| r.id == engine.propellant_resource)
            else {
                return;
            };
            let Ok((design, settings, thermal)) = ships.get(installed.ship) else {
                return;
            };
            let design = &design.0;
            let part = &design.parts[installed.index];
            let sink_temperature = if thermal.0.shield_deployed_kg > 0.0 {
                thermal.0.shield_temperature(design.as_ref().into())
            } else {
                300.0 + 1200.0 * thermal.0.hull_energy_j / design.hull_heat_capacity_j.max(1.0)
            };
            if sink_temperature >= 2800.0 {
                return;
            }
            let throttle = match settings.0[design.part_devices[installed.index].unwrap()] {
                Some(DeviceSetting::Throttle(value)) => value.clamp(0.0, 1.0),
                _ => 0.0,
            };
            let actual = engine.thrust_n * throttle;
            let exhaust_velocity = STANDARD_GRAVITY_M_S2 * engine.specific_impulse_s;
            let jet_energy = 0.5 * actual * exhaust_velocity * dt;
            let thermal_energy = jet_energy / engine.thermal_efficiency;
            let force = part.rotation * DVec3::NEG_Z * actual;
            *demand = Demand {
                resource_input: Some((
                    propellant_index,
                    actual / exhaust_velocity * dt / propellant.mass_kg,
                )),
                secondary_resource_input: Some((
                    fuel_index,
                    thermal_energy / engine.fuel_energy_j_kg / fuel.mass_kg,
                )),
                resource_output: catalogue
                    .0
                    .resources
                    .iter()
                    .position(|r| r.id == "spent_fuel")
                    .map(|index| {
                        (
                            index,
                            thermal_energy
                                / engine.fuel_energy_j_kg
                                / catalogue.0.resources[index].mass_kg,
                        )
                    }),
                waste_heat_j: thermal_energy * 0.94 - jet_energy,
                delayed_heat_j: thermal_energy * 0.06,
                force,
                torque: (part.centre - design.centre).cross(force),
                actual,
                enabled: true,
                ..default()
            };
        });
}

pub(crate) fn thermal_engine_decay(
    time: Res<Time<Fixed>>,
    mut ships: Query<(
        &DeviceOutputs,
        &mut ShipThermal,
        &Hull,
        Has<super::super::travel::Dormant>,
    )>,
    mut parts: Query<(
        &InstalledPart,
        &Demand,
        &mut ThermalEngine,
        &Device,
        Has<ActiveDevice>,
    )>,
) {
    let dt = time.delta_secs_f64();
    for (installed, demand, mut engine, device, active) in &mut parts {
        let Ok((outputs, mut thermal, hull, dormant)) = ships.get_mut(installed.ship) else {
            continue;
        };
        let output = &outputs.0[installed.index];
        let fraction = if !dormant
            && active
            && device.0.operational
            && hull.0 > 0.0
            && demand.enabled
            && demand.actual > 0.0
        {
            output.actual / demand.actual
        } else {
            0.0
        };
        let released = engine.decay_energy_j * (1.0 - (-dt / 120.0).exp());
        engine.decay_energy_j += demand.delayed_heat_j * fraction - released;
        thermal.0.add_waste_heat(released, dt);
    }
}
