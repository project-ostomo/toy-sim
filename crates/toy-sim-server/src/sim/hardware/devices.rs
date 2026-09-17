use super::*;

#[derive(Component)]
pub struct Generator {
    pub power_w: f64,
    pub fuel_kg_s: f64,
    pub efficiency: f64,
}

#[derive(Component)]
pub struct Engine {
    pub thrust_n: f64,
    pub propellant_kg_s: f64,
    pub power_w: f64,
}

#[derive(Component)]
pub struct ReactionControl {
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
            thrust_n,
            propellant_kg_s,
            power_w,
            ..
        } => {
            part.insert((
                Engine {
                    thrust_n,
                    propellant_kg_s,
                    power_w,
                },
                Demand::default(),
            ));
        }
        Equipment::Rcs {
            thrust_n,
            propellant_kg_s,
            power_w,
        } => {
            part.insert((
                ReactionControl {
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
                inputs: [
                    engine.propellant_kg_s * dt * throttle,
                    0.0,
                    engine.power_w * dt * throttle,
                ],
                force,
                torque: (part.centre - d.centre).cross(force),
                actual,
                enabled: true,
                ..default()
            };
        });
}

pub(crate) fn prepare_rcs(
    time: Res<Time<Fixed>>,
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
                inputs: [
                    rcs.propellant_kg_s * dt * total,
                    0.0,
                    rcs.power_w * dt * total,
                ],
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
