use toy_sim_ship_api::abi;

const REFERENCE_SLICE: u64 = 1_000_000;
const FLIGHT_ALLOWANCE: u64 = 700_000;
const PLANNING_ALLOWANCE: u64 = 300_000;
const PUBLICATION_ALLOWANCE: u64 = 230_000;

fn headroom(budget: abi::BudgetInfo, allowance: u64) -> u64 {
    let granted = budget
        .gas_limit
        .min(budget.gas_per_tick)
        .min(REFERENCE_SLICE);
    granted.saturating_mul(allowance) / REFERENCE_SLICE
}

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
        let affordable = budget
            .gas_remaining
            .saturating_sub(headroom(budget, FLIGHT_ALLOWANCE))
            / abi::SCAN_GAS_PER_OBJECT;
        self.requested
            .min(affordable.min(u64::from(abi::MAX_CONTACTS)) as usize)
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
    budget.gas_remaining > headroom(budget, PUBLICATION_ALLOWANCE)
}

pub fn planning_allowed(budget: abi::BudgetInfo) -> bool {
    budget.gas_remaining > headroom(budget, PLANNING_ALLOWANCE)
}

pub fn allocation_allowed(budget: abi::BudgetInfo, handles: usize) -> bool {
    let allowance = 60_000_u64.saturating_add((handles as u64).saturating_mul(180));
    budget.gas_remaining > headroom(budget, allowance)
}

pub fn navigation_limit(budget: abi::BudgetInfo, maximum: u16) -> u16 {
    let affordable = budget
        .gas_remaining
        .saturating_sub(headroom(budget, PLANNING_ALLOWANCE))
        .saturating_sub(abi::NAVIGATION_GAS_BASE)
        / abi::NAVIGATION_GAS_PER_GATE;
    affordable.min(u64::from(maximum)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(granted: u64, gas_remaining: u64) -> abi::BudgetInfo {
        abi::BudgetInfo {
            gas_remaining,
            gas_limit: granted,
            gas_per_tick: REFERENCE_SLICE,
        }
    }

    #[test]
    fn scans_progress_each_fresh_slice_while_leaving_flight_headroom() {
        for granted in [REFERENCE_SLICE, 500_000, 250_000, 50_000] {
            let slice = budget(granted, granted);
            let mut window = ScanWindow::default();
            let mut largest = 0;
            for _ in 0..100 {
                let limit = window.limit(slice);
                largest = largest.max(limit);
                assert!((1..=256).contains(&limit));
                assert!(
                    limit as u64 * abi::SCAN_GAS_PER_OBJECT + headroom(slice, FLIGHT_ALLOWANCE)
                        <= slice.gas_remaining
                );
                window.observed(limit, limit);
            }
            let affordable =
                (granted - headroom(slice, FLIGHT_ALLOWANCE)) / abi::SCAN_GAS_PER_OBJECT;
            assert_eq!(largest, affordable.min(256) as usize);
            window.observed(largest, 1);
            assert_eq!(window.limit(slice), affordable.min(32) as usize);
        }
    }

    #[test]
    fn optional_work_uses_the_current_grant_and_yields_for_control_and_publication() {
        for granted in [REFERENCE_SLICE, 250_000, 50_000] {
            let slice = budget(granted, granted);
            let planning = headroom(slice, PLANNING_ALLOWANCE);
            let publication = headroom(slice, PUBLICATION_ALLOWANCE);
            let allocation = headroom(slice, 60_000 + 32 * 180);
            assert!(planning_allowed(slice));
            assert!(forecast_allowed(slice));
            assert!(!planning_allowed(budget(granted, planning)));
            assert!(planning_allowed(budget(granted, planning + 1)));
            assert!(!forecast_allowed(budget(granted, publication)));
            assert!(forecast_allowed(budget(granted, publication + 1)));
            assert!(!allocation_allowed(budget(granted, allocation), 32));
            assert!(allocation_allowed(budget(granted, allocation + 1), 32));
            assert_eq!(
                ScanWindow::default().limit(budget(granted, headroom(slice, FLIGHT_ALLOWANCE))),
                0
            );
        }
        assert!(!forecast_allowed(budget(0, 0)));
        assert!(!planning_allowed(budget(0, 0)));
        assert!(!allocation_allowed(budget(0, 0), 32));
    }

    #[test]
    fn catalogue_pages_fit_the_available_slice_and_do_not_need_accumulated_gas() {
        for granted in [REFERENCE_SLICE, 250_000, 50_000] {
            let slice = budget(granted, granted);
            let limit = navigation_limit(slice, 96);
            assert!((1..=96).contains(&limit));
            assert!(
                abi::NAVIGATION_GAS_BASE
                    + u64::from(limit) * abi::NAVIGATION_GAS_PER_GATE
                    + headroom(slice, PLANNING_ALLOWANCE)
                    <= slice.gas_remaining
            );
            assert_eq!(
                navigation_limit(budget(granted, headroom(slice, PLANNING_ALLOWANCE)), 96),
                0
            );
            assert_eq!(navigation_limit(slice, 1), 1);
        }
        assert_eq!(navigation_limit(budget(0, 0), 96), 0);
    }
}
