//! Temporary startup state, kept in code until a gamesave format exists.
use bevy::math::{DQuat, DVec3};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Spin and throttle used only by the maneuvering-target regression fixtures.
#[cfg(test)]
pub const TRAFFIC_TUMBLE_BODY: DVec3 = DVec3::new(0.17, 0.23, 0.11);
#[cfg(test)]
pub const TRAFFIC_CHALLENGE_THROTTLE: f64 = 0.1;

#[cfg(test)]
pub use barrage::*;

#[cfg(test)]
mod barrage {
    use super::*;

    pub const INITIAL_SLUG_COUNT: usize = 100;
    pub const INITIAL_SLUG_SPEED: f64 = 100_000.0;
    pub const INITIAL_SLUG_FINAL_SPEED: f64 = 350_000.0;
    pub const INITIAL_SLUG_MASS: f64 = 0.01;
    pub const INITIAL_SLUG_RADIUS: f64 = 0.005;

    pub fn slug_arrival_tick(index: usize) -> usize {
        let fraction = (index / 2) as f64 / (INITIAL_SLUG_COUNT / 2 - 1) as f64;
        ((10.0 + 20.0 * fraction) * osg_model::TICK_RATE_HZ).round() as usize
    }

    fn slug_speed(index: usize) -> f64 {
        let fraction = (index / 2) as f64 / (INITIAL_SLUG_COUNT / 2 - 1) as f64;
        INITIAL_SLUG_SPEED + fraction * (INITIAL_SLUG_FINAL_SPEED - INITIAL_SLUG_SPEED)
    }

    /// Fifty opposing pairs, increasing in energy to overwhelm a stock ship near
    /// the middle of the barrage. Reverse the same
    /// kick/drift used by physics, so orbital curvature does not spoil the aim.
    pub fn incoming_slugs(
        universe: &crate::sim::orrery::Universe,
        epoch: hifitime::Epoch,
        player: crate::sim::precision::GalacticPosition,
        velocity: DVec3,
        outward: DVec3,
    ) -> Vec<(crate::sim::precision::GalacticPosition, DVec3)> {
        let dt = 1.0 / osg_model::TICK_RATE_HZ;
        let last_tick = (30.0 / dt).round() as usize;
        let sources: Vec<_> = universe
            .containing_segment(player, DVec3::ZERO)
            .into_iter()
            .flat_map(|index| {
                let definition = universe.resolve_index(index).unwrap();
                definition
                    .solver
                    .iter()
                    .filter(|body| {
                        !matches!(
                            body.class_params,
                            osg_universe::orrery_cfg::BodyClass::Barycenter
                        )
                    })
                    .map(|body| (definition.body_id(&body.name).unwrap(), body.mass))
                    .collect::<Vec<_>>()
            })
            .collect();
        let gravity: Vec<Vec<_>> = (0..last_tick)
            .map(|tick| {
                let time = epoch + hifitime::Duration::from_seconds(tick as f64 * dt);
                sources
                    .iter()
                    .map(|&(reference, mass)| {
                        (
                            universe
                                .solve_position(reference, time)
                                .unwrap()
                                .relative_to(player),
                            crate::sim::physics::GRAVITATIONAL_CONSTANT * mass,
                        )
                    })
                    .collect()
            })
            .collect();

        let acceleration = |position: DVec3, sources: &[(DVec3, f64)]| {
            sources
                .iter()
                .map(|&(source, mu)| {
                    let offset = source - position;
                    offset * (mu / offset.length().powi(3))
                })
                .sum::<DVec3>()
        };
        let mut position = DVec3::ZERO;
        let mut velocity = velocity;
        let mut coasting = vec![(position, velocity)];
        for sources in &gravity {
            velocity += acceleration(position, sources) * dt;
            position += velocity * dt;
            coasting.push((position, velocity));
        }

        (0..INITIAL_SLUG_COUNT)
            .map(|index| {
                let tick = slug_arrival_tick(index);
                let (mut position, mut velocity) = coasting[tick];
                // Opposing pairs limit accumulated recoil before the shield fails.
                let direction = if index % 2 == 0 { 1.0 } else { -1.0 };
                velocity -= outward.normalize() * (direction * slug_speed(index));
                for sources in gravity[..tick].iter().rev() {
                    position -= velocity * dt;
                    velocity -= acceleration(position, sources) * dt;
                }
                (player.offset_by(position), velocity)
            })
            .collect()
    }
}

pub const INITIAL_SCENARIO: InitialScenario = InitialScenario {
    body: "Helion I Neris",
    exclusion_clearance_m: 1_000_000.0, // Room for nearby stations and traffic.
    orbit_seed: 42,
    camera_distance: 100.0,
    camera_yaw: 0.0,
    camera_pitch: 0.0,
    traffic_count: 0,
    // Preserve the former nearest target (Traffic 243) from the 500-ship fleet.
    traffic_orbit_start: 243,
};

/// Initial circular orbits, expressed in inertial axes relative to the chosen body.
#[derive(Clone, Debug)]
pub struct InitialScenario {
    pub body: &'static str,
    pub exclusion_clearance_m: f64,
    pub orbit_seed: u64,
    pub camera_distance: f64,
    pub camera_yaw: f64,
    pub camera_pitch: f64,
    pub traffic_count: usize,
    /// First traffic orbit in the seeded sequence; index zero belongs to the explorer.
    pub traffic_orbit_start: usize,
}

impl InitialScenario {
    pub fn nearest_traffic(states: &[(DVec3, DVec3)]) -> Option<usize> {
        let player = states.first()?.0;
        states
            .iter()
            .enumerate()
            .skip(1)
            .min_by(|(_, a), (_, b)| {
                a.0.distance_squared(player)
                    .total_cmp(&b.0.distance_squared(player))
            })
            .map(|(index, _)| index)
    }

    pub fn validate(&self, universe: &crate::sim::orrery::Universe) -> anyhow::Result<()> {
        let body = universe
            .authored_body(self.body)
            .and_then(|reference| universe.body(reference))
            .ok_or_else(|| anyhow::anyhow!("unknown starting body {}", self.body))?;
        self.relative_state(body.radius, body.mass)?;
        let atmosphere_height = body.atmosphere.as_ref().map_or(0.0, |a| a.height);
        anyhow::ensure!(
            self.orbit_dimensions(body.radius, body.mass)?.0 > body.radius + atmosphere_height,
            "starting orbit must be above the atmosphere"
        );
        anyhow::ensure!(
            self.camera_distance.is_finite()
                && self.camera_distance > 0.0
                && self.camera_yaw.is_finite()
                && self.camera_pitch.is_finite(),
            "invalid initial camera"
        );
        Ok(())
    }

    /// Explorer first, then the selected stretch of the reproducible orbital sequence.
    pub fn fleet_states(&self, radius: f64, mass: f64) -> anyhow::Result<Vec<(DVec3, DVec3)>> {
        let (r, speed) = self.orbit_dimensions(radius, mass)?;
        anyhow::ensure!(
            self.traffic_orbit_start > 0,
            "traffic cannot reuse the explorer's orbit"
        );
        let mut rng = ChaCha8Rng::seed_from_u64(self.orbit_seed);
        let mut states = vec![sample_orbit(r, speed, &mut rng)];
        for _ in 1..self.traffic_orbit_start {
            sample_orbit(r, speed, &mut rng);
        }
        states.extend((0..self.traffic_count).map(|_| sample_orbit(r, speed, &mut rng)));
        Ok(states)
    }

    pub fn sunlit_state(
        &self,
        universe: &crate::sim::orrery::Universe,
        epoch: hifitime::Epoch,
    ) -> anyhow::Result<(DVec3, DVec3)> {
        let reference = universe
            .authored_body(self.body)
            .ok_or_else(|| anyhow::anyhow!("starting body unavailable"))?;
        let body = universe.body(reference).expect("starting body");
        let system = universe.resolve(reference.system)?;
        let star = system.body_id(&system.star_name).expect("system star");
        let planet_position = universe
            .solve_position(reference, epoch)
            .ok_or_else(|| anyhow::anyhow!("starting planet ephemeris unavailable"))?;
        let star_position = universe
            .solve_position(star, epoch)
            .ok_or_else(|| anyhow::anyhow!("starting star ephemeris unavailable"))?;
        let outward = star_position.relative_to(planet_position).normalize();
        anyhow::ensure!(
            outward.is_finite(),
            "starting planet coincides with its star"
        );

        let (radius, speed) = self.orbit_dimensions(body.radius, body.mass)?;
        let (_, seeded_velocity) = self.relative_state(body.radius, body.mass)?;
        let mut tangent = seeded_velocity - outward * seeded_velocity.dot(outward);
        if tangent.length_squared() < speed * speed * 1e-12 {
            let axis = if outward.x.abs() < 0.9 {
                DVec3::X
            } else {
                DVec3::Y
            };
            tangent = outward.cross(axis);
        }
        Ok((outward * radius, tangent.normalize() * speed))
    }

    pub fn relative_state(&self, radius: f64, mass: f64) -> anyhow::Result<(DVec3, DVec3)> {
        let (r, speed) = self.orbit_dimensions(radius, mass)?;
        Ok(sample_orbit(
            r,
            speed,
            &mut ChaCha8Rng::seed_from_u64(self.orbit_seed),
        ))
    }

    fn orbit_dimensions(&self, radius: f64, mass: f64) -> anyhow::Result<(f64, f64)> {
        let r = osg_model::travel::slip::exclusion_radius_m(mass) + self.exclusion_clearance_m;
        anyhow::ensure!(
            self.exclusion_clearance_m > 0.0
                && r.is_finite()
                && r > radius
                && mass.is_finite()
                && mass > 0.0,
            "invalid starting orbit"
        );
        Ok((
            r,
            (crate::sim::physics::GRAVITATIONAL_CONSTANT * mass / r).sqrt(),
        ))
    }
}

fn sample_orbit(radius: f64, speed: f64, rng: &mut ChaCha8Rng) -> (DVec3, DVec3) {
    // Uniform unit quaternion: randomize both the plane and phase without
    // concentrating orbital normals near the poles (as uniform inclination would).
    let u = rng.random::<f64>();
    let a = rng.random::<f64>() * std::f64::consts::TAU;
    let b = rng.random::<f64>() * std::f64::consts::TAU;
    let rotation = DQuat::from_xyzw(
        (1.0 - u).sqrt() * a.sin(),
        (1.0 - u).sqrt() * a.cos(),
        u.sqrt() * b.sin(),
        u.sqrt() * b.cos(),
    );
    (
        rotation * (DVec3::Z * radius),
        rotation * (DVec3::X * speed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_orbit_stays_on_the_dayside_with_circular_tangential_velocity() {
        let universe = crate::sim::orrery::Universe::init(osg_universe::example_config()).unwrap();
        let scenario = &INITIAL_SCENARIO;
        let reference = universe.authored_body(scenario.body).unwrap();
        let body = universe.body(reference).unwrap();
        let system = universe.resolve(reference.system).unwrap();
        let star_name = &system.star_name;
        let star_reference = system.body_id(star_name).unwrap();
        for days in [0., 1., 1234.] {
            let epoch = hifitime::Epoch::from_mjd_utc(days);
            let (offset, velocity) = scenario.sunlit_state(&universe, epoch).unwrap();
            let planet = universe.solve_position(reference, epoch).unwrap();
            let star = universe.solve_position(star_reference, epoch).unwrap();
            let ship = planet.offset_by(offset);
            let sunward = star.relative_to(planet).normalize();
            let radius = osg_model::travel::slip::exclusion_radius_m(body.mass)
                + scenario.exclusion_clearance_m;
            let expected_speed =
                (crate::sim::physics::GRAVITATIONAL_CONSTANT * body.mass / radius).sqrt();

            assert!((offset.length() - radius).abs() < 1e-6);
            assert!((velocity.length() - expected_speed).abs() < 1e-9);
            assert!(offset.normalize().dot(sunward) > 1. - 1e-12);
            assert!(offset.normalize().dot(velocity.normalize()).abs() < 1e-12);
            for occluder in system.solver.iter().filter(|body| &body.name != star_name) {
                let center = system.solver.solve_position(&occluder.name, epoch).unwrap();
                assert!(
                    !crate::sim::spatial::sphere_blocks(
                        star.relative_to(ship),
                        center.relative_to(ship),
                        occluder.radius,
                    ),
                    "{} eclipses the initial ship at day {days}",
                    occluder.name
                );
            }
            assert_eq!(
                scenario.sunlit_state(&universe, epoch).unwrap(),
                (offset, velocity)
            );
        }
    }
}
