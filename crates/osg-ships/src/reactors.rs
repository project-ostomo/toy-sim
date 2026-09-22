use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ReactorSpec {
    pub thermal_power_w: f64,
    pub hot_temperature_k: f64,
    pub conversion_quality: f64,
    pub core_heat_capacity_j_k: f64,
    pub heat_transfer_w_k: f64,
    pub shutdown_temperature_k: f64,
    pub meltdown_temperature_k: f64,
    pub fuel_energy_j_kg: f64,
    pub breeding_ratio: f64,
    pub decay_fraction: f64,
    pub decay_time_s: f64,
}

impl ReactorSpec {
    pub fn efficiency(self, sink_temperature_k: f64) -> f64 {
        self.conversion_quality
            * (1.0 - sink_temperature_k / self.hot_temperature_k).clamp(0.0, 1.0)
    }

    pub fn valid(self) -> bool {
        [
            self.thermal_power_w,
            self.hot_temperature_k,
            self.core_heat_capacity_j_k,
            self.heat_transfer_w_k,
            self.shutdown_temperature_k,
            self.meltdown_temperature_k,
            self.fuel_energy_j_kg,
            self.decay_time_s,
        ]
        .iter()
        .all(|value| value.is_finite() && *value > 0.0)
            && self.hot_temperature_k > 300.0
            && self.shutdown_temperature_k > self.hot_temperature_k
            && self.meltdown_temperature_k > self.shutdown_temperature_k
            && self.conversion_quality.is_finite()
            && (0.0..=1.0).contains(&self.conversion_quality)
            && self.decay_fraction.is_finite()
            && (0.0..1.0).contains(&self.decay_fraction)
            && self.breeding_ratio.is_finite()
            && (0.0..=1.2).contains(&self.breeding_ratio)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FuelProcessorSpec {
    pub throughput_kg_s: f64,
    pub power_w: f64,
    pub recovery_fraction: f64,
    pub produces_charges: bool,
}

impl FuelProcessorSpec {
    pub fn valid(self) -> bool {
        self.throughput_kg_s.is_finite()
            && self.throughput_kg_s > 0.0
            && self.power_w.is_finite()
            && self.power_w > 0.0
            && self.recovery_fraction.is_finite()
            && self.recovery_fraction > 0.0
            && self.recovery_fraction <= 1.0
    }
}
