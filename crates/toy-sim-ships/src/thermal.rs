use crate::{CompiledShipDesign, ShipState, StochasticBalance, StochasticRound};
use toy_sim_ship_api::abi;

pub const BACKGROUND_K: f64 = 3.0;
pub const INITIAL_K: f64 = 300.0;
pub const SPECIFIC_HEAT: f64 = 500.0;
pub const HEAT_STORAGE_J_KG: f64 = 250_000.0;
pub const EMISSIVITY: f64 = 0.8;
pub const SIGMA: f64 = 5.670374419e-8;
pub const JOULES_PER_HP: f64 = 100_000.0;
pub const VAPORIZATION_K: f64 = 6000.0;
pub const LATENT_HEAT_J_KG: f64 = 20_000_000.0;

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ThermalState {
    pub hull_energy_j: f64,
    pub pending_waste_heat_j: f64,
    pub waste_heat_remaining_s: f64,
    pub shield_energy_j: f64,
    pub shield_deployed_kg: f64,
    pub shield_reserve_mg: u64,
    pub ablation_kg_s: f64,
    pub shield_state: u64,
    pub shield_powered: bool,
    pub shield_enabled: bool,
}

pub fn shield_radius(hull_radius: f64) -> f64 {
    hull_radius + (hull_radius * 0.1).max(0.5)
}

pub fn radiation(temperature: f64, area: f64) -> f64 {
    EMISSIVITY * SIGMA * area * (temperature.powi(4) - BACKGROUND_K.powi(4)).max(0.0)
}

/// Backward Euler with safeguarded Newton iteration. The root is monotone and
/// bracketed between the background and the previous temperature.
pub fn cooled(temperature: f64, capacity: f64, area: f64, dt: f64) -> f64 {
    if capacity <= 0.0 || dt <= 0.0 || temperature <= BACKGROUND_K {
        return temperature;
    }
    let a = EMISSIVITY * SIGMA * area * dt / capacity;
    let mut lo = BACKGROUND_K;
    let mut hi = temperature;
    let mut t = temperature;
    loop {
        let f = t - temperature + a * (t.powi(4) - BACKGROUND_K.powi(4));
        if f > 0.0 {
            hi = t;
        } else {
            lo = t;
        }
        if hi - lo <= 1e-7 * t.max(1.0) || f.abs() <= 1e-9 {
            return t;
        }
        let newton = t - f / (1.0 + 4.0 * a * t.powi(3));
        let next = if newton > lo && newton < hi {
            newton
        } else {
            (lo + hi) * 0.5
        };
        if next == t {
            return t;
        }
        t = next;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ThermalModel {
    pub hull_hp: f64,
    pub hull_heat_capacity_j: f64,
    pub hull_area: f64,
    pub shield_deployed_kg: f64,
    pub shield_reserve_capacity_kg: f64,
    pub shield_feed_kg_s: f64,
    pub shield_area: f64,
}

impl From<&CompiledShipDesign> for ThermalModel {
    fn from(d: &CompiledShipDesign) -> Self {
        Self {
            hull_hp: d.hull,
            hull_heat_capacity_j: d.hull_heat_capacity_j,
            hull_area: d.exposed_area_m2,
            shield_deployed_kg: d.shield_deployed_kg,
            shield_reserve_capacity_kg: d.shield_reserve_capacity_kg,
            shield_feed_kg_s: d.shield_feed_kg_s,
            shield_area: d.shield_radiator_area_m2,
        }
    }
}

impl ThermalState {
    pub fn new(m: ThermalModel) -> Self {
        Self {
            shield_deployed_kg: m.shield_deployed_kg,
            shield_reserve_mg: (m.shield_reserve_capacity_kg * 1e6).stochastic_round(),
            ..Self::default()
        }
    }

    pub fn add_hull_heat(&mut self, energy_j: f64) {
        assert!(energy_j.is_finite() && energy_j >= 0.0);
        self.hull_energy_j += energy_j;
    }

    pub fn add_waste_heat(&mut self, energy_j: f64, duration_s: f64) {
        assert!(energy_j.is_finite() && energy_j >= 0.0);
        assert!(duration_s.is_finite() && duration_s > 0.0);
        self.pending_waste_heat_j += energy_j;
        self.waste_heat_remaining_s = self.waste_heat_remaining_s.max(duration_s);
    }

    pub fn shield_reserve_kg(&self) -> f64 {
        self.shield_reserve_mg as f64 / 1e6
    }

    pub fn shield_temperature(&self, _m: ThermalModel) -> f64 {
        if self.shield_deployed_kg <= 0.0 {
            return 0.0;
        }
        INITIAL_K + self.shield_energy_j / (self.shield_deployed_kg * SPECIFIC_HEAT)
    }

    pub fn shield_strength(&self, m: ThermalModel) -> f64 {
        if m.shield_deployed_kg <= 0.0 {
            return 0.0;
        }
        (self.shield_deployed_kg / m.shield_deployed_kg).clamp(0.0, 1.0)
    }

    pub fn shield_operating(&self) -> bool {
        self.shield_enabled
            && self.shield_powered
            && matches!(self.shield_state, abi::SHIELD_ACTIVE | abi::SHIELD_DEPLETED)
    }

    pub fn radiator_area(&self, m: ThermalModel) -> f64 {
        if self.shield_operating() {
            m.shield_area * self.shield_strength(m)
        } else {
            0.0
        }
    }

    pub fn headroom(&self, _m: ThermalModel) -> f64 {
        let accessible_reserve = if self.shield_operating() {
            self.shield_reserve_kg()
        } else {
            0.0
        };
        ((self.shield_deployed_kg + accessible_reserve)
            * (SPECIFIC_HEAT * (VAPORIZATION_K - INITIAL_K) + LATENT_HEAT_J_KG)
            - self.shield_energy_j)
            .max(0.0)
    }

    pub fn deposit(&mut self, hull: &mut f64, m: ThermalModel, shield: bool, energy: f64) {
        assert!(energy.is_finite() && energy >= 0.0);
        if shield {
            let absorbed = energy.min(self.headroom(m));
            let overflow = energy - absorbed;
            self.hull_energy_j += overflow;
            *hull = (*hull - overflow / JOULES_PER_HP).max(0.0);
            self.shield_energy_j += absorbed;
            let sensible = SPECIFIC_HEAT * (VAPORIZATION_K - INITIAL_K);
            let escaping_energy = sensible + LATENT_HEAT_J_KG;
            if self.shield_operating() {
                let reserve_loss = ((self.shield_energy_j - self.shield_deployed_kg * sensible)
                    / escaping_energy)
                    .clamp(0.0, self.shield_reserve_kg());
                let reserve_loss = self.shield_reserve_mg.withdraw(reserve_loss * 1e6) as f64 / 1e6;
                self.shield_energy_j =
                    (self.shield_energy_j - reserve_loss * escaping_energy).max(0.0);
            }

            let loss = ((self.shield_energy_j - self.shield_deployed_kg * sensible)
                / LATENT_HEAT_J_KG)
                .clamp(0.0, self.shield_deployed_kg);
            self.shield_deployed_kg -= loss;
            self.shield_energy_j =
                (self.shield_energy_j - loss * (sensible + LATENT_HEAT_J_KG)).max(0.0);
            self.check_depleted();
        } else {
            self.hull_energy_j += energy;
            *hull = (*hull - energy / JOULES_PER_HP).max(0.0);
        }
    }

    fn check_depleted(&mut self) {
        if self.shield_deployed_kg < 1e-10 {
            self.shield_deployed_kg = 0.0;
            self.shield_energy_j = 0.0;
            if self.shield_state == abi::SHIELD_ACTIVE {
                self.shield_state = abi::SHIELD_DEPLETED;
            }
        }
    }

    pub fn advance(&mut self, hull: &mut f64, m: ThermalModel, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        let steps = (dt / 0.01).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        let mut total_loss = 0.0;
        for _ in 0..steps {
            if self.waste_heat_remaining_s > 0.0 {
                let interval = h.min(self.waste_heat_remaining_s);
                let heat = self.pending_waste_heat_j * interval / self.waste_heat_remaining_s;
                self.pending_waste_heat_j = (self.pending_waste_heat_j - heat).max(0.0);
                self.waste_heat_remaining_s = (self.waste_heat_remaining_s - interval).max(0.0);
                self.hull_energy_j += heat;
            }

            let operating = self.shield_operating();
            if operating {
                let transfer = self
                    .hull_energy_j
                    .min(400_000_000.0 * m.shield_deployed_kg * self.shield_strength(m) * h);
                self.hull_energy_j -= transfer;
                self.shield_energy_j += transfer;
            }

            let capacity = self.shield_deployed_kg * SPECIFIC_HEAT;
            if capacity > 0.0 {
                let area = self.radiator_area(m);
                let t = cooled(self.shield_temperature(m), capacity, area, h).max(INITIAL_K);
                self.shield_energy_j = (t - INITIAL_K) * capacity;
                let flux = 0.001
                    * (4500.0 / t).sqrt()
                    * (60_000.0 * (1.0 / 4500.0 - 1.0 / t))
                        .clamp(-700.0, 50.0)
                        .exp();
                let escaping_energy = SPECIFIC_HEAT * (t - INITIAL_K) + LATENT_HEAT_J_KG;
                let loss = (m.shield_area * self.shield_strength(m) * flux * h)
                    .min(self.shield_deployed_kg)
                    .min(self.shield_energy_j / escaping_energy);
                self.shield_deployed_kg -= loss;
                self.shield_energy_j = (self.shield_energy_j - loss * escaping_energy).max(0.0);
                total_loss += loss;
            }

            if operating {
                let feed = (m.shield_deployed_kg - self.shield_deployed_kg)
                    .max(0.0)
                    .min(m.shield_feed_kg_s * h)
                    .min(self.shield_reserve_kg());
                let feed = self.shield_reserve_mg.withdraw(feed * 1e6) as f64 / 1e6;
                self.shield_deployed_kg += feed;
            }
            self.check_depleted();

            let cooling = 2000.0 * m.hull_area * h;
            self.hull_energy_j = (self.hull_energy_j - cooling).max(0.0);
            let excess = (self.hull_energy_j / m.hull_heat_capacity_j.max(1.0) - 1.0).max(0.0);
            *hull = (*hull - m.hull_hp * 0.1 * excess * excess * h).max(0.0);
        }
        self.ablation_kg_s = total_loss / dt;
    }
}

impl ShipState {
    pub fn shield_temperature(&self, d: &CompiledShipDesign) -> f64 {
        self.thermal.shield_temperature(d.into())
    }

    pub fn shield_strength(&self, d: &CompiledShipDesign) -> f64 {
        self.thermal.shield_strength(d.into())
    }

    pub fn shield_headroom(&self, d: &CompiledShipDesign) -> f64 {
        self.thermal.headroom(d.into())
    }

    pub fn shield_active(&self) -> bool {
        self.thermal.shield_state == abi::SHIELD_ACTIVE
    }

    pub fn shield_activation(&mut self, d: &CompiledShipDesign, clear: bool) {
        self.thermal.shield_state = if d.shield_deployed_kg == 0.0 {
            abi::SHIELD_ABSENT
        } else if !self.thermal.shield_enabled {
            abi::SHIELD_OFF
        } else if !self.thermal.shield_powered {
            abi::SHIELD_UNPOWERED
        } else if !self.shield_active() && !clear {
            abi::SHIELD_BLOCKED
        } else if self.thermal.shield_deployed_kg == 0.0 {
            abi::SHIELD_DEPLETED
        } else {
            abi::SHIELD_ACTIVE
        };
    }

    pub fn deposit_impact(&mut self, d: &CompiledShipDesign, shield: bool, energy_j: f64) {
        self.thermal
            .deposit(&mut self.hull, d.into(), shield, energy_j);
    }

    pub fn advance_thermal(&mut self, d: &CompiledShipDesign, dt: f64) {
        self.thermal.advance(&mut self.hull, d.into(), dt);
    }

    pub fn resources(&self, d: &CompiledShipDesign) -> abi::ShipResources {
        abi::ShipResources {
            hull_hp: self.hull,
            hull_max_hp: d.hull,
            hull_heat_j: self.thermal.hull_energy_j,
            hull_heat_capacity_j: d.hull_heat_capacity_j,
            shield_temperature_k: self.shield_temperature(d),
            shield_reserve_kg: self.thermal.shield_reserve_kg(),
            shield_reserve_capacity_kg: d.shield_reserve_capacity_kg,
            shield_strength: self.shield_strength(d),
            energy_j: self.inventory.energy_j,
            shield_state: self.thermal.shield_state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ThermalModel {
        ThermalModel {
            hull_hp: 1000.0,
            hull_heat_capacity_j: 300e6,
            hull_area: 10.0,
            shield_deployed_kg: 5.0,
            shield_reserve_capacity_kg: 100.0,
            shield_feed_kg_s: 50.0,
            shield_area: 100.0,
        }
    }

    #[test]
    fn radiation_remains_stable_after_extreme_heating() {
        for t in [3.0, 300.0, 3000.0, 1e9] {
            let end = cooled(t, 1e6, 100.0, 0.1);
            assert!(end.is_finite() && end >= BACKGROUND_K && end <= t);
        }
    }

    #[test]
    fn replenishment_preserves_coverage_and_spends_reserve() {
        let m = model();
        let mut s = ThermalState::new(m);
        s.shield_enabled = true;
        s.shield_powered = true;
        s.shield_state = abi::SHIELD_ACTIVE;
        s.shield_energy_j = 5.0 * SPECIFIC_HEAT * (4500.0 - INITIAL_K);
        let initial = s.shield_energy_j;
        let mut hp = m.hull_hp;
        s.advance(&mut hp, m, 0.01);
        assert!((s.shield_deployed_kg - m.shield_deployed_kg).abs() < 1e-6);
        let remaining_mass = s.shield_deployed_kg + s.shield_reserve_kg();
        let initial_mass = m.shield_deployed_kg + m.shield_reserve_capacity_kg;
        assert!((remaining_mass + s.ablation_kg_s * 0.01 - initial_mass).abs() < 1e-10);
        assert!(s.shield_reserve_kg() < m.shield_reserve_capacity_kg);
        assert!(s.shield_energy_j < initial);
        assert_eq!(hp, m.hull_hp);
    }

    #[test]
    fn impact_capacity_is_finite_and_empty_reserve_cannot_rebuild() {
        let m = model();
        let mut s = ThermalState::new(m);
        s.shield_reserve_mg = 0;
        s.shield_enabled = true;
        s.shield_powered = true;
        s.shield_state = abi::SHIELD_ACTIVE;
        let mut hp = m.hull_hp;
        s.deposit(&mut hp, m, true, s.headroom(m));
        assert_eq!(s.shield_deployed_kg, 0.0);
        assert_eq!(s.shield_energy_j, 0.0);
        assert_eq!(s.shield_state, abi::SHIELD_DEPLETED);
        s.advance(&mut hp, m, 1.0);
        assert_eq!(s.shield_strength(m), 0.0);
        assert_eq!(hp, m.hull_hp);
    }

    #[test]
    fn impact_flushing_preserves_strength_until_the_reserve_is_empty() {
        let m = model();
        let mut s = ThermalState::new(m);
        s.shield_enabled = true;
        s.shield_powered = true;
        s.shield_state = abi::SHIELD_ACTIVE;
        let sensible = SPECIFIC_HEAT * (VAPORIZATION_K - INITIAL_K);
        let escaping_energy = sensible + LATENT_HEAT_J_KG;
        let mut hp = m.hull_hp;

        let first_energy = m.shield_deployed_kg * sensible + 20.0 * escaping_energy;
        s.deposit(&mut hp, m, true, first_energy);
        assert_eq!(s.shield_strength(m), 1.0);
        assert_eq!(s.shield_reserve_kg(), 80.0);
        assert_eq!(s.shield_energy_j + 20.0 * escaping_energy, first_energy);
        assert_eq!(hp, m.hull_hp);

        s.deposit(&mut hp, m, true, 80.0 * escaping_energy);
        assert_eq!(s.shield_reserve_kg(), 0.0);
        assert_eq!(s.shield_strength(m), 1.0);
        assert_eq!(s.headroom(m), m.shield_deployed_kg * LATENT_HEAT_J_KG);

        s.deposit(&mut hp, m, true, 2.0 * LATENT_HEAT_J_KG);
        assert_eq!(s.shield_deployed_kg, 3.0);
        assert_eq!(s.shield_strength(m), 0.6);
        s.deposit(&mut hp, m, true, s.headroom(m));
        assert_eq!(s.shield_state, abi::SHIELD_DEPLETED);
        assert_eq!(s.shield_energy_j, 0.0);
        assert_eq!(hp, m.hull_hp);
    }

    #[test]
    fn inactive_shields_have_no_radiating_area() {
        let m = model();
        let mut s = ThermalState::new(m);
        s.shield_enabled = true;
        s.shield_powered = true;
        for state in [abi::SHIELD_OFF, abi::SHIELD_UNPOWERED, abi::SHIELD_BLOCKED] {
            s.shield_state = state;
            assert_eq!(s.radiator_area(m), 0.0);
        }
        s.shield_state = abi::SHIELD_ACTIVE;
        assert_eq!(s.radiator_area(m), m.shield_area);
        s.shield_powered = false;
        assert_eq!(s.radiator_area(m), 0.0);
    }

    #[test]
    fn sustained_reactor_heat_reaches_coolant_equilibrium_across_tick_sizes() {
        let mut temperatures = Vec::new();
        for dt in [0.1, 0.01, 0.003] {
            let mut m = model();
            m.shield_area = 66.0;
            let mut s = ThermalState::new(m);
            s.shield_enabled = true;
            s.shield_powered = true;
            s.shield_state = abi::SHIELD_ACTIVE;
            let mut hp = m.hull_hp;
            for _ in 0..(5.0 / dt) as usize {
                s.add_waste_heat(450e6 * dt, dt);
                s.advance(&mut hp, m, dt);
            }
            let t = s.shield_temperature(m);
            assert!((3400.0..3600.0).contains(&t), "dt={dt}, T={t}");
            assert!((1.0 - s.shield_strength(m)) * m.shield_deployed_kg < 1e-6);
            assert_eq!(hp, m.hull_hp);
            temperatures.push(t);
        }
        assert!(
            temperatures
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max)
                - temperatures.iter().copied().fold(f64::INFINITY, f64::min)
                < 20.0
        );
    }

    #[test]
    fn excessive_shield_impacts_conserve_overflow_and_remove_coolant_mass() {
        let m = model();
        let mut s = ThermalState::new(m);
        let capacity = s.headroom(m);
        let mut hp = m.hull_hp;
        s.deposit(&mut hp, m, true, capacity + 1e6);
        assert_eq!(s.shield_deployed_kg, 0.0);
        assert_eq!(s.shield_reserve_kg(), m.shield_reserve_capacity_kg);
        assert_eq!(s.hull_energy_j, 1e6);
        assert_eq!(hp, m.hull_hp - 10.0);
    }

    #[test]
    fn heat_over_budget_causes_damage_without_a_hull_temperature() {
        let m = model();
        let mut s = ThermalState::new(m);
        let mut hp = m.hull_hp;
        s.hull_energy_j = m.hull_heat_capacity_j;
        s.advance(&mut hp, m, 1.0);
        assert_eq!(hp, m.hull_hp);
        s.hull_energy_j = 2.0 * m.hull_heat_capacity_j;
        s.advance(&mut hp, m, 1.0);
        assert!(hp < m.hull_hp);
    }
}
