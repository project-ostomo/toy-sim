use bevy::prelude::*;

pub mod reactor;
pub mod thruster;
pub mod torquer;

#[derive(Component)]
pub struct Module;

pub fn start_modules(app: &mut App) {
    app.add_plugins((
        reactor::start_reactors,
        thruster::start_thrusters,
        torquer::start_torquers,
    ));
}
