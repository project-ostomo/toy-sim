use bevy::{
    light::atmosphere::{Falloff, PhaseFunction, ScatteringMedium, ScatteringTerm},
    math::curve::{FunctionCurve, Interval},
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

#[cfg(test)]
mod tests {

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
