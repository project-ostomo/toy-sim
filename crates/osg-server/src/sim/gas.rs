use anyhow::{Context, Result, ensure};
use bevy::prelude::{Entity, World};
use imbl::OrdMap;
use osg_model::Id;
use osg_model::ownership::{GasAccountSnapshot, Principal};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const STARTING_GAS: u64 = 1_000_000_000_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasAccount {
    pub available: u64,
    pub spent: u64,
    pub fairness: Option<Id>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasState {
    pub accounts: OrdMap<Principal, GasAccount>,
}

impl GasState {
    pub fn ensure_account(&mut self, owner: Principal, initial_gas: u64) {
        self.accounts.entry(owner).or_insert(GasAccount {
            available: initial_gas,
            spent: 0,
            fairness: None,
        });
    }

    pub fn account(&self, owner: Principal) -> Option<GasAccountSnapshot> {
        self.accounts.get(&owner).map(|account| GasAccountSnapshot {
            owner,
            available: account.available,
            spent: account.spent,
        })
    }

    pub fn deposit(&mut self, owner: Principal, amount: u64) -> Result<()> {
        let account = self
            .accounts
            .get_mut(&owner)
            .context("gas account unavailable")?;
        account
            .available
            .checked_add(account.spent)
            .and_then(|total| total.checked_add(amount))
            .context("gas lifetime accounting overflow")?;
        account.available += amount;
        Ok(())
    }

    pub fn transfer(&mut self, from: Principal, to: Principal, amount: u64) -> Result<()> {
        ensure!(from != to && amount > 0, "invalid gas transfer");
        ensure!(
            self.accounts
                .get(&from)
                .context("source gas account unavailable")?
                .available
                >= amount,
            "insufficient gas"
        );
        self.deposit(to, amount)?;
        self.accounts.get_mut(&from).unwrap().available -= amount;
        Ok(())
    }

    pub fn debit_fair(
        &mut self,
        owner: Principal,
        requests: &[GasRequest],
    ) -> Result<Vec<(Id, u64)>> {
        let ids: BTreeSet<_> = requests.iter().map(|request| request.id).collect();
        ensure!(ids.len() == requests.len(), "duplicate gas requester");
        ensure!(
            requests.iter().all(|request| {
                request.minimum <= request.maximum && (request.maximum == 0 || request.minimum > 0)
            }),
            "invalid gas minimum"
        );

        let account = self
            .accounts
            .get_mut(&owner)
            .context("gas account unavailable")?;
        let mut remaining = account.available;

        let mut ordered = requests.to_vec();
        ordered.sort_by_key(|request| request.id);
        if let Some(cursor) = account.fairness.as_ref() {
            let start = ordered.partition_point(|request| request.id <= *cursor);
            ordered.rotate_left(start);
        }

        let mut allocations: BTreeMap<_, _> =
            requests.iter().map(|request| (request.id, 0)).collect();
        let mut admitted = Vec::new();
        let mut skipped = false;
        let mut first_admitted = None;
        for request in ordered {
            if request.maximum == 0 {
                continue;
            }
            if request.minimum > remaining {
                skipped = true;
                continue;
            }

            allocations.insert(request.id, request.minimum);
            remaining -= request.minimum;
            admitted.push((request.id, request.maximum - request.minimum));
            first_admitted.get_or_insert(request.id);
        }
        admitted.sort_by_key(|&(id, maximum)| (maximum, id));
        for (index, &(id, maximum)) in admitted.iter().enumerate() {
            let active = (admitted.len() - index) as u64;
            let share = remaining / active;
            if maximum <= share {
                *allocations.get_mut(&id).unwrap() += maximum;
                remaining -= maximum;
            } else {
                for &(id, _) in &admitted[index..] {
                    *allocations.get_mut(&id).unwrap() += share;
                }
                remaining -= share * active;
                break;
            }
        }

        if remaining > 0 {
            let mut eligible: Vec<_> = requests
                .iter()
                .filter_map(|request| {
                    let allocated = allocations[&request.id];
                    (allocated >= request.minimum && allocated < request.maximum)
                        .then_some(request.id)
                })
                .collect();
            eligible.sort_unstable();
            if let Some(cursor) = account.fairness.as_ref() {
                let start = eligible.partition_point(|id| id <= cursor);
                eligible.rotate_left(start);
            }
            for id in eligible.into_iter().take(remaining as usize) {
                *allocations.get_mut(&id).unwrap() += 1;
                account.fairness = Some(id);
            }
        }

        if skipped && let Some(id) = first_admitted {
            account.fairness = Some(id);
        }

        account.available -= allocations.values().sum::<u64>();
        Ok(allocations.into_iter().collect())
    }

    /// Called once for each local grant after execution has joined. The ECS
    /// system retains exclusive access between debit and refund.
    pub fn refund(&mut self, owner: Principal, granted: u64, used: u64) {
        assert!(used <= granted, "gas consumption exceeds grant");
        let account = self.accounts.get_mut(&owner).expect("gas payer exists");
        account.available = account
            .available
            .checked_add(granted - used)
            .expect("gas refund overflow");
        account.spent = account
            .spent
            .checked_add(used)
            .expect("gas spending overflow");
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GasRequest {
    pub id: Id,
    pub maximum: u64,
    pub minimum: u64,
}

pub fn payer(world: &World, ship: Entity) -> Result<Principal> {
    world
        .get::<super::ownership::AssetOwner>(ship)
        .map(|owner| owner.0)
        .context("computer owner unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> Principal {
        Principal::Player(Id([1; 16]))
    }
    fn request(id: u8, maximum: u64) -> GasRequest {
        GasRequest {
            id: Id([id; 16]),
            maximum,
            minimum: u64::from(maximum > 0),
        }
    }

    fn ledger(initial: u64) -> GasState {
        let mut ledger = GasState::default();
        ledger.ensure_account(owner(), initial);
        ledger
    }

    #[test]
    fn fair_allocation_caps_small_requests_and_rotates_scarce_remainders_after_restart() {
        let mut ledger = ledger(101);
        let requests = [request(3, 100), request(1, 1), request(2, 100)];
        let grants = ledger.debit_fair(owner(), &requests).unwrap();
        let allocations: Vec<_> = grants.iter().map(|(id, grant)| (*id, *grant)).collect();
        assert_eq!(
            allocations,
            vec![(Id([1; 16]), 1), (Id([2; 16]), 50), (Id([3; 16]), 50)]
        );
        for (_, grant) in grants {
            let limit = grant;
            ledger.refund(owner(), grant, limit);
        }
        let mut recipients = Vec::new();
        let mut ledger = ledger;
        for round in 0..6 {
            ledger.deposit(owner(), 1).unwrap();
            let mut requests = vec![request(1, 100), request(2, 100), request(3, 100)];
            requests.rotate_left(round % 3);
            for (id, grant) in ledger.debit_fair(owner(), &requests).unwrap() {
                let limit = grant;
                if limit > 0 {
                    recipients.push(id);
                }
                ledger.refund(owner(), grant, limit);
            }
            ledger = postcard::from_bytes(&postcard::to_stdvec(&ledger).unwrap()).unwrap();
        }
        assert_eq!(
            recipients,
            vec![
                Id([1; 16]),
                Id([2; 16]),
                Id([3; 16]),
                Id([1; 16]),
                Id([2; 16]),
                Id([3; 16])
            ]
        );
        assert!(
            ledger
                .debit_fair(owner(), &[request(1, 1), request(1, 1)])
                .is_err()
        );
    }

    #[test]
    fn atomic_calls_make_progress_and_rotate_when_equal_splits_would_stall() {
        let mut ledger = ledger(1_000_000);
        let requests: Vec<_> = (1..=3)
            .map(|id| GasRequest {
                id: Id([id; 16]),
                minimum: 700_000,
                maximum: 1_000_000,
            })
            .collect();
        let mut winners = Vec::new();
        for round in 0..6 {
            if round > 0 {
                ledger.deposit(owner(), 700_000).unwrap();
            }
            let mut round_requests = requests.clone();
            round_requests.rotate_left(round % 3);
            for (id, grant) in ledger.debit_fair(owner(), &round_requests).unwrap() {
                let used = if grant >= 700_000 {
                    winners.push(id);
                    700_000
                } else {
                    assert_eq!(grant, 0);
                    0
                };
                ledger.refund(owner(), grant, used);
            }
            assert_eq!(ledger.account(owner()).unwrap().available, 300_000);
            assert_eq!(
                ledger.account(owner()).unwrap().spent,
                (round as u64 + 1) * 700_000
            );
            ledger = postcard::from_bytes(&postcard::to_stdvec(&ledger).unwrap()).unwrap();
        }
        assert_eq!(
            winners,
            vec![
                Id([1; 16]),
                Id([2; 16]),
                Id([3; 16]),
                Id([1; 16]),
                Id([2; 16]),
                Id([3; 16])
            ]
        );

        let malformed = GasRequest {
            id: Id([9; 16]),
            maximum: 10,
            minimum: 11,
        };
        assert!(ledger.debit_fair(owner(), &[malformed]).is_err());
    }

    #[test]
    fn small_requests_do_not_pin_fairness_ahead_of_a_skipped_atomic_call() {
        let mut ledger = ledger(0);
        let requests = [
            GasRequest {
                id: Id([1; 16]),
                minimum: 700_000,
                maximum: 700_000,
            },
            GasRequest {
                id: Id([2; 16]),
                minimum: 700_000,
                maximum: 700_000,
            },
            request(3, 1),
        ];
        let mut large_winners = BTreeMap::<Id, usize>::new();
        for _ in 0..6 {
            ledger.deposit(owner(), 700_001).unwrap();
            for (id, grant) in ledger.debit_fair(owner(), &requests).unwrap() {
                let limit = grant;
                if limit == 700_000 {
                    *large_winners.entry(id).or_default() += 1;
                }
                ledger.refund(owner(), grant, limit);
            }
        }
        assert!(large_winners[&Id([1; 16])] >= 2);
        assert!(large_winners[&Id([2; 16])] >= 2);
        assert_eq!(ledger.account(owner()).unwrap().available, 0);
    }

    #[test]
    fn exhaustion_and_zero_requests_stay_bounded() {
        let mut ledger = ledger(0);
        assert!(ledger.debit_fair(owner(), &[]).unwrap().is_empty());
        let grants = ledger
            .debit_fair(owner(), &[request(1, u64::MAX), request(2, 0)])
            .unwrap();
        assert!(grants.iter().all(|(_, grant)| *grant == 0));
        drop(grants);
        assert_eq!(ledger.account(owner()).unwrap().spent, 0);
    }

    #[test]
    fn snapshot_isolation_conservation_and_invalid_transfers() {
        let mut gas = ledger(100);
        let other = Principal::Player(Id([2; 16]));
        gas.ensure_account(other, u64::MAX);
        let before = postcard::to_stdvec(&gas).unwrap();
        assert!(gas.transfer(owner(), other, 1).is_err());
        assert_eq!(postcard::to_stdvec(&gas).unwrap(), before);
        let snapshot = gas.clone();
        let grants = gas
            .debit_fair(owner(), &[request(1, 80), request(2, 20)])
            .unwrap();
        assert_eq!(gas.account(owner()).unwrap().available, 0);
        gas.refund(owner(), grants[0].1, 31);
        gas.refund(owner(), grants[1].1, 9);
        let account = gas.account(owner()).unwrap();
        assert_eq!((account.available, account.spent), (60, 40));
        assert_eq!(postcard::to_stdvec(&snapshot).unwrap(), before);
        let restored: GasState = postcard::from_bytes(&postcard::to_stdvec(&gas).unwrap()).unwrap();
        assert_eq!(restored.account(owner()), Some(account));
    }
}
