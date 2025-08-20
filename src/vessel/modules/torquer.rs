use bevy::{math::DVec3, prelude::*};

use crate::{physics::AccumulatedTorque, precision::PreciseTransform};

pub fn start_torquers(app: &mut App) {
    app.add_systems(FixedUpdate, (apply_torquers, magic_torquers));
}

#[derive(Component, Default)]
/// A "torquer", representing something that directly outputs torque (reaction wheels, etc).
pub struct Torquer {
    /// A vector indicating throttle position in all three axes.
    pub throttle: DVec3,
    /// Actual torque produced
    pub torque: DVec3,
    /// Offset of the torquer
    pub offset: DVec3,
}

fn apply_torquers(
    torquers: Query<(&Torquer, &ChildOf)>,
    mut vessels: Query<(&PreciseTransform, &mut AccumulatedTorque)>,
) {
    for (torquer, child_of) in torquers {
        if let Ok((ptf, mut torque)) = vessels.get_mut(child_of.0) {
            torque.0 += ptf.rotation.mul_vec3(torquer.torque);
        }
    }
}

/// A "magic torquer" that produces torque out of nothing.
#[derive(Component)]
#[require(Torquer)]
pub struct MagicTorquer {
    pub torque: f64,
}

fn magic_torquers(query: Query<(&mut Torquer, &MagicTorquer)>) {
    for (mut torquer, magic) in query {
        torquer.torque = torquer.throttle * magic.torque;
    }
}
