use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::vessel::ConsumableTanks;

pub fn start_reactors(app: &mut App) {
    app.add_systems(FixedUpdate, run_reactors);
}

fn run_reactors(
    reactors: Query<(&mut NuclearReactor, &ChildOf)>,
    mut tanks: Query<&mut ConsumableTanks>,
    time: Res<Time>,
) {
    for (reactor, child_of) in reactors {
        let tanks = tanks.get_mut(child_of.0).unwrap();
        reactor.current_throttle += (reactor.desired_throttle - reactor.current_throttle)
            * (1 - (time.delta_secs_f64() / reactor.config.throttle_lag).exp2());
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
