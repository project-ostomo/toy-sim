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
}

pub fn density_fraction(altitude: f64, height: f64, scale: f64) -> f64 {
    let top = (-height / scale).exp();
    (((-altitude / scale).exp() - top) / -(-height / scale).exp_m1()).clamp(0.0, 1.0)
}
