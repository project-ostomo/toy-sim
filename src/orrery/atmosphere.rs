use bevy::{
    light::atmosphere::{Falloff, PhaseFunction, ScatteringMedium, ScatteringTerm},
    math::{
        DVec3,
        curve::{FunctionCurve, Interval},
    },
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// All lengths are metres, densities kg/m³, temperatures kelvin and optical
/// coefficients m⁻¹. Omit this section entirely for an airless body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphereCfg {
    pub height: f64,
    pub surface_density: f64,
    pub scale_height: f64,
    pub temperature: f64,
    pub specific_gas_constant: f64,
    pub heat_capacity_ratio: f64,
    pub rayleigh_scattering: [f32; 3],
    pub mie_scattering: f32,
    pub mie_absorption: f32,
    pub mie_scale_height: f64,
    pub mie_asymmetry: f32,
    pub ground_albedo: [f32; 3],
}

impl AtmosphereCfg {
    pub fn validate(&self) -> anyhow::Result<()> {
        for value in [
            self.height,
            self.scale_height,
            self.temperature,
            self.specific_gas_constant,
            self.heat_capacity_ratio,
            self.mie_scale_height,
        ] {
            anyhow::ensure!(
                value.is_finite() && value > 0.0,
                "atmosphere dimensions and gas parameters must be finite and positive"
            );
        }
        anyhow::ensure!(
            self.surface_density.is_finite() && self.surface_density >= 0.0,
            "invalid surface density"
        );
        for value in self
            .rayleigh_scattering
            .into_iter()
            .chain([self.mie_scattering, self.mie_absorption])
        {
            anyhow::ensure!(
                value.is_finite() && value >= 0.0,
                "invalid scattering coefficient"
            );
        }
        anyhow::ensure!(
            self.mie_asymmetry.is_finite() && self.mie_asymmetry.abs() < 1.0,
            "invalid Mie asymmetry"
        );
        anyhow::ensure!(
            self.ground_albedo
                .iter()
                .all(|x| x.is_finite() && (0.0..=1.0).contains(x)),
            "invalid ground albedo"
        );
        Ok(())
    }

    /// A normalized exponential profile that reaches vacuum continuously at the
    /// configured outer radius. Below the surface, use the surface density.
    pub fn density(&self, altitude: f64) -> f64 {
        if altitude >= self.height {
            return 0.0;
        }
        self.surface_density * density_fraction(altitude.max(0.0), self.height, self.scale_height)
    }

    pub fn scattering_medium(&self) -> ScatteringMedium {
        let height = self.height;
        let falloff = |scale| {
            Falloff::from_curve(FunctionCurve::new(Interval::UNIT, move |p: f32| {
                density_fraction((1.0 - p as f64) * height, height, scale) as f32
            }))
        };
        ScatteringMedium::new(
            512,
            256,
            [
                ScatteringTerm {
                    scattering: Vec3::from_array(self.rayleigh_scattering),
                    absorption: Vec3::ZERO,
                    falloff: falloff(self.scale_height),
                    phase: PhaseFunction::Rayleigh,
                },
                ScatteringTerm {
                    scattering: Vec3::splat(self.mie_scattering),
                    absorption: Vec3::splat(self.mie_absorption),
                    falloff: falloff(self.mie_scale_height),
                    phase: PhaseFunction::Mie {
                        asymmetry: self.mie_asymmetry,
                    },
                },
            ],
        )
    }
}

fn density_fraction(altitude: f64, height: f64, scale: f64) -> f64 {
    let top = (-height / scale).exp();
    (((-altitude / scale).exp() - top) / -(-height / scale).exp_m1()).clamp(0.0, 1.0)
}

/// Initial circular orbit, expressed in inertial axes relative to the chosen body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioCfg {
    pub body: String,
    pub vessel: String,
    pub altitude: f64,
    pub radial_direction: [f64; 3],
    pub orbit_normal: [f64; 3],
    pub camera_distance: f64,
    pub camera_yaw: f64,
    pub camera_pitch: f64,
    #[serde(default)]
    pub traffic: Option<TrafficCfg>,
}

/// Reproducible, unpowered ships on circular orbits with random planes and phases.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficCfg {
    pub count: usize,
    pub seed: u64,
    pub min_altitude: f64,
    pub max_altitude: f64,
}

impl TrafficCfg {
    pub fn validate(&self, radius: f64, atmosphere_height: f64) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.min_altitude.is_finite()
                && self.max_altitude.is_finite()
                && self.min_altitude > atmosphere_height
                && self.max_altitude >= self.min_altitude
                && (radius + self.max_altitude).is_finite(),
            "invalid traffic orbit altitudes"
        );
        Ok(())
    }

    pub fn relative_states(&self, radius: f64, mass: f64) -> Vec<(DVec3, DVec3)> {
        use rand::{RngExt, SeedableRng};
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(self.seed);
        (0..self.count)
            .map(|_| {
                let z: f64 = rng.random_range(-1.0..1.0);
                let phase: f64 = rng.random_range(0.0..std::f64::consts::TAU);
                let radial = DVec3::new(
                    (1.0 - z * z).sqrt() * phase.cos(),
                    z,
                    (1.0 - z * z).sqrt() * phase.sin(),
                );
                let tangent = radial.any_orthonormal_vector();
                let roll: f64 = rng.random_range(0.0..std::f64::consts::TAU);
                let tangent = tangent * roll.cos() + radial.cross(tangent) * roll.sin();
                let distance = radius + rng.random_range(self.min_altitude..=self.max_altitude);
                (
                    radial * distance,
                    tangent * (crate::physics::GRAVITATIONAL_CONSTANT * mass / distance).sqrt(),
                )
            })
            .collect()
    }
}

impl ScenarioCfg {
    pub fn relative_state(&self, radius: f64, mass: f64) -> anyhow::Result<(DVec3, DVec3)> {
        let r = radius + self.altitude;
        anyhow::ensure!(
            self.altitude > 0.0 && r.is_finite() && mass.is_finite() && mass > 0.0,
            "invalid starting orbit"
        );
        let radial = DVec3::from_array(self.radial_direction)
            .try_normalize()
            .ok_or_else(|| anyhow::anyhow!("invalid orbit radial direction"))?;
        let normal = DVec3::from_array(self.orbit_normal)
            .try_normalize()
            .ok_or_else(|| anyhow::anyhow!("invalid orbit normal"))?;
        anyhow::ensure!(
            radial.dot(normal).abs() < 1e-6,
            "orbit radial direction must be perpendicular to its normal"
        );
        let tangent = normal.cross(radial).normalize();
        Ok((
            radial * r,
            tangent * (crate::physics::GRAVITATIONAL_CONSTANT * mass / r).sqrt(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_is_reproducible_and_on_circular_orbits_above_the_atmosphere() {
        let orrery = crate::orrery::Universe::init(crate::orrery::example_config()).unwrap();
        let scenario = orrery.scenario.as_ref().unwrap();
        let traffic = scenario.traffic.as_ref().unwrap();
        let body = orrery.get_body(&scenario.body).unwrap();
        let states = traffic.relative_states(body.radius, body.mass);
        assert!(states.len() >= 500);
        assert_eq!(states, traffic.relative_states(body.radius, body.mass));
        for (position, velocity) in states {
            let radius = position.length();
            assert!(radius >= body.radius + traffic.min_altitude - 1e-6);
            assert!(radius <= body.radius + traffic.max_altitude + 1e-6);
            assert!((position.normalize().dot(velocity.normalize())).abs() < 1e-12);
            let circular = crate::physics::GRAVITATIONAL_CONSTANT * body.mass / radius;
            assert!((velocity.length_squared() / circular - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn density_and_scattering_share_a_continuous_finite_atmosphere() {
        let cfg = crate::orrery::example_config();
        let atmosphere = cfg
            .bodies
            .iter()
            .find_map(|b| b.atmosphere.as_ref())
            .unwrap();
        atmosphere.validate().unwrap();
        assert_eq!(atmosphere.density(0.0), atmosphere.surface_density);
        assert_eq!(atmosphere.density(-100.0), atmosphere.surface_density);
        assert_eq!(atmosphere.density(atmosphere.height), 0.0);
        assert_eq!(atmosphere.density(atmosphere.height * 2.0), 0.0);
        let medium = atmosphere.scattering_medium();
        for fraction in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let altitude = fraction * atmosphere.height;
            let density_ratio = atmosphere.density(altitude) / atmosphere.surface_density;
            assert!(
                (medium.terms[0].falloff.sample((1.0 - fraction) as f32) as f64 - density_ratio)
                    .abs()
                    < 1e-6
            );
        }
        let mut invalid = atmosphere.clone();
        invalid.scale_height = 0.0;
        assert!(invalid.validate().is_err());
    }
}
