use toy_sim_ship_api::abi;

const FLIGHT_ALLOWANCE: u64 = 700_000;
pub const PUBLICATION_ALLOWANCE: u64 = 230_000;

#[derive(Clone, Debug)]
pub struct ScanWindow {
    requested: usize,
}

impl Default for ScanWindow {
    fn default() -> Self {
        Self { requested: 32 }
    }
}

impl ScanWindow {
    pub fn limit(&self, budget: abi::BudgetInfo) -> usize {
        let refill = budget.gas_refill_per_s / 10;
        let affordable = budget
            .gas_remaining
            .saturating_sub(refill + FLIGHT_ALLOWANCE)
            / abi::SCAN_GAS_PER_OBJECT;
        self.requested
            .min(affordable as usize)
            .min(abi::MAX_CONTACTS as usize)
    }

    pub fn observed(&mut self, limit: usize, count: usize) {
        if limit == 0 {
            return;
        }
        self.requested = if count == limit {
            (limit * 2).min(abi::MAX_CONTACTS as usize)
        } else {
            (count * 2)
                .next_power_of_two()
                .max(32)
                .min(abi::MAX_CONTACTS as usize)
        };
    }
}

pub fn forecast_allowed(budget: abi::BudgetInfo) -> bool {
    budget.instruction_remaining > PUBLICATION_ALLOWANCE
        && budget.gas_remaining > budget.gas_refill_per_s / 10 + PUBLICATION_ALLOWANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(gas_remaining: u64) -> abi::BudgetInfo {
        abi::BudgetInfo {
            gas_remaining,
            instruction_remaining: 1_000_000,
            gas_capacity: 4_000_000,
            gas_refill_per_s: 10_000_000,
            instruction_limit: 1_000_000,
        }
    }

    #[test]
    fn sparse_scans_keep_small_reservations_and_dense_scans_fit_continuous_refill() {
        let mut window = ScanWindow::default();
        assert_eq!(window.limit(budget(4_000_000)), 32);
        window.observed(32, 1);
        assert_eq!(window.limit(budget(4_000_000)), 32);

        let mut gas = 4_000_000;
        let mut largest = 0;
        for _ in 0..100 {
            let limit = window.limit(budget(gas));
            largest = largest.max(limit);
            assert!((1..=256).contains(&limit));
            window.observed(limit, limit);
            gas -= limit as u64 * abi::SCAN_GAS_PER_OBJECT + FLIGHT_ALLOWANCE;
            assert!(gas >= 1_000_000);
            gas = (gas + 1_000_000).min(4_000_000);
        }
        assert_eq!(largest, 256);
        window.observed(window.limit(budget(gas)), 1);
        assert_eq!(window.limit(budget(gas)), 32);
    }

    #[test]
    fn forecast_respects_native_gas_and_preserves_next_callback_and_publication() {
        assert!(!forecast_allowed(budget(1_230_000)));
        assert!(forecast_allowed(budget(1_230_001)));
        let mut instructions = budget(4_000_000);
        instructions.instruction_remaining = PUBLICATION_ALLOWANCE;
        assert!(!forecast_allowed(instructions));
        assert_eq!(ScanWindow::default().limit(budget(1_700_000)), 0);
    }
}
