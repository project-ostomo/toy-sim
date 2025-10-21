mod aero_env;

pub use aero_env::*;
mod aero_model;
pub use aero_model::*;

use bevy::prelude::*;

use crate::GameState;
use crate::physics::aerodynamics::aero_model::calc_aerodynamics;

pub(super) fn run_aero(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (update_aero_env, calc_aerodynamics)
            .chain()
            .run_if(in_state(GameState::Game)),
    );
}
