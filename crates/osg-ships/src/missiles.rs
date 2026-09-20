use serde::{Deserialize, Serialize};

use crate::{ShipBlueprint, Tank};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissileLauncherDef {
    pub cycle_interval_s: f64,
    pub ejection_speed_m_s: f64,
    pub maximum_range_m: f64,
    pub power_w: f64,
}

impl MissileLauncherDef {
    pub fn valid(&self) -> bool {
        [
            self.cycle_interval_s,
            self.ejection_speed_m_s,
            self.maximum_range_m,
            self.power_w,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.)
            && self.cycle_interval_s >= 0.1
    }
}

pub const AMMUNITION: &str = "interceptor_missile";
pub const PROPELLANT: &str = "rocket_propellant";
pub const BODY_PART: &str = "interceptor_body";
pub const LAUNCHER_PART: &str = "missile_launcher_2m";
pub const FUEL_KG: u64 = 240;
pub const THRUST_N: f64 = 25_000.;
pub const EXHAUST_M_S: f64 = 3_000.;
/// Minimum design endurance under continuous seeker and attitude control loads.
/// Actual shutdown follows the battery's remaining energy.
pub const GUIDANCE_ENDURANCE_S: f64 = 900.;
pub const SENSOR_RANGE_M: f64 = 2_000_000.;
pub const TURN_RATE_RAD_S: f64 = 3.;
pub const BATTERY_J: u64 = 20_000_000;
pub const MAGAZINE_ROUNDS: u64 = 12;

pub fn blueprint() -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: "Kite kinetic interceptor".into(),
        ..Default::default()
    };
    ship.attach(BODY_PART, 0, "", "", 0);
    ship.attach("interceptor_engine", 1, "aft", "fore", 0);
    let command = ship.attach("interceptor_command", 1, "fore", "aft", 0);
    ship.attach("interceptor_seeker", command, "fore", "aft", 0);
    ship.attach("interceptor_torquer", 1, "top", "bottom", 0);
    ship.attach("interceptor_torquer", 1, "bottom", "top", 0);
    ship.parts[1].alias = "main_engine".into();
    ship.parts[4].alias = "attitude_dorsal".into();
    ship.parts[5].alias = "attitude_ventral".into();
    ship.parts[0].tanks.push(Tank {
        resource: PROPELLANT.into(),
        volume_m3: 0.15,
        initial_fill: 1.,
    });
    ship
}

fn load_magazine(ship: &mut ShipBlueprint, part_id: u64) {
    ship.parts
        .iter_mut()
        .find(|part| part.id == part_id)
        .expect("installed launcher")
        .tanks
        .push(Tank {
            resource: AMMUNITION.into(),
            volume_m3: 6.,
            initial_fill: 1.,
        });
}

pub fn missile_patrol() -> ShipBlueprint {
    let mut ship = crate::expedition_patrol();
    ship.name = "Shrike missile patrol".into();
    let launchers: Vec<_> = ship
        .parts
        .iter_mut()
        .filter(|part| part.prototype == "laser_pulse_2m")
        .map(|part| {
            part.prototype = LAUNCHER_PART.into();
            part.id
        })
        .collect();
    for launcher in launchers {
        load_magazine(&mut ship, launcher);
    }
    ship
}

pub fn missile_defense_station() -> ShipBlueprint {
    let mut ship = ShipBlueprint {
        name: "Kestrel installation defense battery".into(),
        ..Default::default()
    };
    ship.attach("station_core_32m", 0, "", "", 0);
    let beacon = ship.attach("directory_transmitter_48m", 1, "aft", "fore", 0);
    let reactor = ship.attach("reactor_hot_4m", 1, "left", "right", 0);
    ship.attach("radiator_32m", reactor, "left", "right", 0);
    let battery = ship.attach("battery_2m", 1, "right", "left", 0);
    ship.attach("command_2m", battery, "right", "left", 0);
    for (parent, socket, plug) in [
        (1, "top", "bottom"),
        (1, "bottom", "top"),
        (beacon, "left", "right"),
        (beacon, "right", "left"),
    ] {
        let launcher = ship.attach(LAUNCHER_PART, parent, socket, plug, 0);
        load_magazine(&mut ship, launcher);
    }
    ship.parts[0].tanks = vec![
        Tank {
            resource: "reactor_fuel".into(),
            volume_m3: 2.,
            initial_fill: 1.,
        },
        Tank {
            resource: "spent_fuel".into(),
            volume_m3: 2.,
            initial_fill: 0.,
        },
    ];
    ship
}
