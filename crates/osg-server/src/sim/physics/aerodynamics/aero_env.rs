use bevy::{math::DVec3, prelude::*};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::sim::{
    orrery::{Celestial, Universe},
    physics::{Velocity, WithinSoi, sim_time},
    precision::{PreciseTransform, ToMicrometersExt},
};

#[derive(Component, Default, Serialize, Deserialize)]
pub struct AeroEnv {
    pub planet: SmolStr,
    pub planet_rel: PreciseTransform,
    pub altitude: f64,
    pub pressure: f64,
    pub density: f64,
    pub temperature: f64,
    pub speed_of_sound: f64,
    pub airspeed: DVec3,
}

pub(super) fn update_aero_env(
    orrery: Res<Universe>,
    mut objects: Query<(
        &PreciseTransform,
        &Velocity,
        Option<&WithinSoi>,
        &mut AeroEnv,
    )>,
    bodies: Query<(
        Entity,
        &Celestial,
        &PreciseTransform,
        Option<&crate::sim::orrery::activity::CelestialState>,
    )>,
    time: Res<Time>,
) {
    // Fixed time names the end of this step; force inputs still describe its start.
    let epoch = sim_time(&time) - hifitime::Duration::from_seconds(time.delta_secs_f64());
    for (ptf, velocity, soi, mut env) in &mut objects {
        // Atmosphere membership is geometric, independent of gravitational SOI.
        let mut selected = soi.and_then(|s| bodies.get(s.0).ok());
        let mut density = 0.0;
        for candidate @ (_, celestial, body_tf, state) in &bodies {
            let body = state.map_or_else(|| orrery.get_body(&celestial.0).unwrap(), |s| &s.body);
            if let Some(atmosphere) = &body.atmosphere {
                let altitude = (ptf.translation_um - body_tf.translation_um)
                    .to_meters_64()
                    .length()
                    - body.radius;
                let candidate_density = atmosphere.density(altitude);
                if candidate_density > density {
                    density = candidate_density;
                    selected = Some(candidate);
                }
            }
        }
        // Reset gas properties each tick so exiting an atmosphere cannot retain drag.
        *env = AeroEnv {
            airspeed: velocity.0,
            ..default()
        };
        let Some((_, celestial, body_tf, state)) = selected else {
            continue;
        };
        let body = state.map_or_else(|| orrery.get_body(&celestial.0).unwrap(), |s| &s.body);
        let relative = (ptf.translation_um - body_tf.translation_um).to_meters_64();
        env.planet = celestial.0.clone();
        env.altitude = relative.length() - body.radius;
        env.airspeed = velocity.0
            - state.map_or_else(
                || {
                    orrery
                        .atmospheric_velocity_at_point(&celestial.0, ptf.translation_um, epoch)
                        .unwrap_or(DVec3::ZERO)
                },
                |s| {
                    let spin = if body.rotation.rotation_period != 0.0 {
                        (body_tf.rotation * DVec3::Z).cross(relative) * std::f64::consts::TAU
                            / body.rotation.rotation_period
                    } else {
                        DVec3::ZERO
                    };
                    s.velocity + spin
                },
            );
        let inverse = body_tf.rotation.inverse();
        env.planet_rel = PreciseTransform {
            translation_um: (inverse * relative).to_micrometers(),
            rotation: inverse * ptf.rotation,
        };
        if let Some(atmosphere) = &body.atmosphere {
            env.density = atmosphere.density(env.altitude);
            if env.density > 0.0 {
                env.temperature = atmosphere.temperature;
                env.pressure = env.density * atmosphere.specific_gas_constant * env.temperature;
                env.speed_of_sound = (atmosphere.heat_capacity_ratio
                    * atmosphere.specific_gas_constant
                    * env.temperature)
                    .sqrt();
            }
        }
    }
}
