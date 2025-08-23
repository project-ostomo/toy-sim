use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::vessel::{ConsumableTanks, consumable::Consumable};

pub fn start_reactors(app: &mut App) {
    app.add_systems(FixedUpdate, run_reactors);
}

fn run_reactors(
    reactors: Query<(&mut NuclearReactor, &ChildOf)>,
    mut tanks: Query<&mut ConsumableTanks>,
    time: Res<Time>,
) {
    for (mut reactor, child_of) in reactors {
        let mut tanks = tanks.get_mut(child_of.0).unwrap();
        reactor.current_throttle += (reactor.desired_throttle - reactor.current_throttle)
            * (1.0 - (-time.delta_secs_f64() / reactor.config.throttle_lag).exp2());

        let cold_side = 300.0; // hardcode for now
        let total_efficiency =
            reactor.config.efficiency * (1.0 - cold_side / reactor.config.hot_side);

        let thermal_power = reactor.config.thermal_power * reactor.current_throttle;
        // consume fuel
        let fuel_to_consume =
            thermal_power * time.delta_secs_f64() / 8.2e13 * reactor.config.fuel_util_frac; // assume 8.2e13 J/kg of fissile
        let fissile_left = tanks.consume(
            match reactor.config.cycle {
                NuclearCycle::U235 => Consumable::Uranium235,
                NuclearCycle::Pu239 => Consumable::Plutonium239,
            },
            fuel_to_consume,
        );

        if fissile_left == 0.0 {
            reactor.current_throttle = 0.0;
            continue;
        }

        let electric_power = thermal_power * total_efficiency;
        if tanks
            .produce(
                Consumable::ElectricJoules,
                electric_power * time.delta_secs_f64(),
            )
            .is_err()
        {
            // TODO produce extra waste heat
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct NuclearReactorCfg {
    pub thermal_power: f64,
    pub hot_side: f64,
    pub efficiency: f64,
    pub fuel_util_frac: f64,
    pub cycle: NuclearCycle,
    pub throttle_lag: f64,
}

#[derive(Clone, Copy, Component)]
pub struct NuclearReactor {
    pub config: NuclearReactorCfg,
    pub current_throttle: f64,
    pub desired_throttle: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum NuclearCycle {
    U235,
    Pu239,
}
