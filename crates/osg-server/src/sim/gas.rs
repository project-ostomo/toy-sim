use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::Id;
use osg_model::ownership::{GasAccountSnapshot, Principal};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

pub const STARTING_GAS: u64 = 1_000_000_000_000_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GasLedgerSnapshot {
    pub accounts: BTreeMap<Principal, GasAccountSnapshot>,
    pub fairness: BTreeMap<Principal, Id>,
}

#[derive(Clone, Copy, Debug)]
pub struct GasRequest {
    pub id: Id,
    pub maximum: u64,
    pub minimum: u64,
}

#[derive(Default)]
struct LedgerState {
    accounts: BTreeMap<Principal, GasAccountSnapshot>,
    fairness: BTreeMap<Principal, Id>,
    reservations: BTreeMap<u64, (Principal, u64)>,
    next_reservation: u64,
}

#[derive(Resource, Clone, Default)]
pub struct GasLedger(Arc<Mutex<LedgerState>>);

impl GasLedger {
    pub fn ensure_account(&self, owner: Principal, initial_gas: u64) {
        self.0
            .lock()
            .unwrap()
            .accounts
            .entry(owner)
            .or_insert(GasAccountSnapshot {
                owner,
                available: initial_gas,
                reserved: 0,
                spent: 0,
            });
    }

    pub fn account(&self, owner: Principal) -> Option<GasAccountSnapshot> {
        self.0.lock().unwrap().accounts.get(&owner).copied()
    }

    pub fn deposit(&self, owner: Principal, amount: u64) -> Result<()> {
        let mut ledger = self.0.lock().unwrap();
        let account = ledger
            .accounts
            .get_mut(&owner)
            .context("gas account unavailable")?;
        let available = account
            .available
            .checked_add(amount)
            .context("gas balance overflow")?;
        ensure!(
            available
                .checked_add(account.reserved)
                .and_then(|n| n.checked_add(account.spent))
                .is_some(),
            "gas lifetime accounting overflow"
        );

        account.available = available;
        Ok(())
    }

    pub fn transfer(&self, from: Principal, to: Principal, amount: u64) -> Result<()> {
        ensure!(from != to && amount > 0, "invalid gas transfer");
        let mut ledger = self.0.lock().unwrap();
        let source = ledger
            .accounts
            .get(&from)
            .context("source gas account unavailable")?;
        ensure!(source.available >= amount, "insufficient gas");
        let target = ledger
            .accounts
            .get(&to)
            .context("recipient gas account unavailable")?;
        let available = target
            .available
            .checked_add(amount)
            .context("gas balance overflow")?;
        ensure!(
            available
                .checked_add(target.reserved)
                .and_then(|n| n.checked_add(target.spent))
                .is_some(),
            "gas lifetime accounting overflow"
        );
        ledger.accounts.get_mut(&from).unwrap().available -= amount;
        ledger.accounts.get_mut(&to).unwrap().available = available;
        Ok(())
    }

    pub fn reserve(&self, owner: Principal, maximum: u64) -> Result<GasReservation> {
        let mut ledger = self.0.lock().unwrap();
        let account = ledger
            .accounts
            .get(&owner)
            .context("gas account unavailable")?;
        ensure!(account.available >= maximum, "insufficient global gas");
        ensure!(
            ledger.next_reservation < u64::MAX,
            "gas reservation sequence exhausted"
        );
        Ok(self.reserve_locked(&mut ledger, owner, maximum))
    }

    pub fn reserve_fair(
        &self,
        owner: Principal,
        requests: &[GasRequest],
    ) -> Result<Vec<(Id, GasReservation)>> {
        let ids: BTreeSet<_> = requests.iter().map(|request| request.id).collect();
        ensure!(ids.len() == requests.len(), "duplicate gas requester");
        ensure!(
            requests.iter().all(|request| {
                request.minimum <= request.maximum && (request.maximum == 0 || request.minimum > 0)
            }),
            "invalid gas minimum"
        );

        let mut ledger = self.0.lock().unwrap();
        let mut remaining = ledger
            .accounts
            .get(&owner)
            .context("gas account unavailable")?
            .available;
        ensure!(
            ledger
                .next_reservation
                .checked_add(requests.len() as u64)
                .is_some(),
            "gas reservation sequence exhausted"
        );

        let mut ordered = requests.to_vec();
        ordered.sort_by_key(|request| request.id);
        if let Some(cursor) = ledger.fairness.get(&owner) {
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
            if let Some(cursor) = ledger.fairness.get(&owner) {
                let start = eligible.partition_point(|id| id <= cursor);
                eligible.rotate_left(start);
            }
            for id in eligible.into_iter().take(remaining as usize) {
                *allocations.get_mut(&id).unwrap() += 1;
                ledger.fairness.insert(owner, id);
            }
        }

        if skipped && let Some(id) = first_admitted {
            ledger.fairness.insert(owner, id);
        }

        Ok(allocations
            .into_iter()
            .map(|(id, amount)| (id, self.reserve_locked(&mut ledger, owner, amount)))
            .collect())
    }

    fn reserve_locked(
        &self,
        ledger: &mut LedgerState,
        owner: Principal,
        limit: u64,
    ) -> GasReservation {
        let id = ledger.next_reservation;
        ledger.next_reservation += 1;

        let account = ledger.accounts.get_mut(&owner).unwrap();
        account.available -= limit;
        account.reserved = account
            .reserved
            .checked_add(limit)
            .expect("gas reserve overflow");
        ledger.reservations.insert(id, (owner, limit));

        GasReservation {
            ledger: self.clone(),
            id: Some(id),
            limit,
            used: 0,
        }
    }

    pub fn snapshot(&self) -> Result<GasLedgerSnapshot> {
        let ledger = self.0.lock().unwrap();
        ensure!(
            ledger.reservations.is_empty(),
            "gas checkpoint requires settled reservations"
        );
        ensure!(
            ledger
                .accounts
                .values()
                .all(|account| account.reserved == 0),
            "unsettled gas balance"
        );
        Ok(GasLedgerSnapshot {
            accounts: ledger.accounts.clone(),
            fairness: ledger.fairness.clone(),
        })
    }

    pub fn from_snapshot(snapshot: GasLedgerSnapshot) -> Result<Self> {
        for (owner, account) in &snapshot.accounts {
            ensure!(*owner == account.owner, "gas account owner mismatch");
            ensure!(
                account.reserved == 0,
                "persisted gas reservations are not settled"
            );
            ensure!(
                account.available.checked_add(account.spent).is_some(),
                "gas lifetime accounting overflow"
            );
        }
        ensure!(
            snapshot
                .fairness
                .keys()
                .all(|owner| snapshot.accounts.contains_key(owner)),
            "unknown gas fairness account"
        );
        Ok(Self(Arc::new(Mutex::new(LedgerState {
            accounts: snapshot.accounts,
            fairness: snapshot.fairness,
            ..Default::default()
        }))))
    }
}

pub struct GasReservation {
    ledger: GasLedger,
    id: Option<u64>,
    limit: u64,
    used: u64,
}

impl GasReservation {
    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn record_used(&mut self, used: u64) -> Result<()> {
        ensure!(
            used >= self.used && used <= self.limit,
            "invalid gas consumption"
        );
        self.used = used;
        Ok(())
    }

    pub fn settle(mut self, used: u64) -> Result<()> {
        self.record_used(used)?;
        self.finish();
        Ok(())
    }

    fn finish(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let mut ledger = self.ledger.0.lock().unwrap();
        let (owner, reserved) = ledger
            .reservations
            .remove(&id)
            .expect("gas reservation unavailable");
        let account = ledger.accounts.get_mut(&owner).unwrap();
        account.reserved -= reserved;
        account.available = account
            .available
            .checked_add(reserved - self.used)
            .expect("gas refund overflow");
        account.spent = account
            .spent
            .checked_add(self.used)
            .expect("gas spending overflow");
    }
}

impl Drop for GasReservation {
    fn drop(&mut self) {
        self.finish();
    }
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

    fn ledger(initial: u64) -> GasLedger {
        let ledger = GasLedger::default();
        ledger.ensure_account(owner(), initial);
        ledger
    }

    #[test]
    fn conservation_partial_use_zero_use_and_overdraft() {
        let ledger = ledger(100);
        let first = ledger.reserve(owner(), 80).unwrap();
        assert!(ledger.reserve(owner(), 21).is_err());
        let second = ledger.reserve(owner(), 20).unwrap();
        assert_eq!(ledger.account(owner()).unwrap().available, 0);
        first.settle(31).unwrap();
        second.settle(9).unwrap();
        drop(ledger.reserve(owner(), 60).unwrap());
        let account = ledger.account(owner()).unwrap();
        assert_eq!(
            (account.available, account.reserved, account.spent),
            (60, 0, 40)
        );
        ledger.ensure_account(owner(), 9999);
        assert_eq!(ledger.account(owner()).unwrap(), account);
    }

    #[test]
    fn recorded_consumption_survives_unwinding_and_invalid_settlement() {
        let ledger = ledger(100);
        let other = ledger.clone();
        let result = std::panic::catch_unwind(move || {
            let mut reservation = other.reserve(owner(), 80).unwrap();
            reservation.record_used(27).unwrap();
            panic!("callback failed after metered work");
        });
        assert!(result.is_err());
        assert_eq!(ledger.account(owner()).unwrap().available, 73);
        let mut reservation = ledger.reserve(owner(), 60).unwrap();
        reservation.record_used(13).unwrap();
        assert!(reservation.record_used(12).is_err());
        assert!(reservation.settle(61).is_err());
        assert_eq!(ledger.account(owner()).unwrap().spent, 40);
    }

    #[test]
    fn checkpoint_refuses_inflight_consumption_including_zero_reservations() {
        let ledger = ledger(100);
        let mut reservation = ledger.reserve(owner(), 80).unwrap();
        reservation.record_used(35).unwrap();
        assert!(ledger.snapshot().is_err());
        reservation.settle(35).unwrap();
        let zero = ledger.reserve(owner(), 0).unwrap();
        assert!(ledger.snapshot().is_err());
        drop(zero);
        let restored = GasLedger::from_snapshot(ledger.snapshot().unwrap()).unwrap();
        assert_eq!(restored.account(owner()), ledger.account(owner()));
    }

    #[test]
    fn concurrent_reservations_cannot_overspend_and_settle_exactly() {
        let ledger = ledger(1000);
        let barrier = Arc::new(std::sync::Barrier::new(32));
        let workers: Vec<_> = (0..32)
            .map(|_| {
                let ledger = ledger.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let reservation = ledger.reserve(owner(), 100).ok();
                    barrier.wait();
                    reservation
                        .map(|reservation| reservation.settle(70).unwrap())
                        .is_some()
                })
            })
            .collect();
        let granted = workers
            .into_iter()
            .filter_map(|worker| worker.join().unwrap().then_some(()))
            .count();
        assert_eq!(granted, 10);
        let account = ledger.account(owner()).unwrap();
        assert_eq!(
            (account.available, account.reserved, account.spent),
            (300, 0, 700)
        );
    }

    #[test]
    fn fair_allocation_caps_small_requests_and_rotates_scarce_remainders_after_restart() {
        let ledger = ledger(101);
        let requests = [request(3, 100), request(1, 1), request(2, 100)];
        let grants = ledger.reserve_fair(owner(), &requests).unwrap();
        let allocations: Vec<_> = grants
            .iter()
            .map(|(id, grant)| (*id, grant.limit()))
            .collect();
        assert_eq!(
            allocations,
            vec![(Id([1; 16]), 1), (Id([2; 16]), 50), (Id([3; 16]), 50)]
        );
        for (_, grant) in grants {
            let limit = grant.limit();
            grant.settle(limit).unwrap();
        }
        let mut recipients = Vec::new();
        let mut ledger = ledger;
        for round in 0..6 {
            ledger.deposit(owner(), 1).unwrap();
            let mut requests = vec![request(1, 100), request(2, 100), request(3, 100)];
            requests.rotate_left(round % 3);
            for (id, grant) in ledger.reserve_fair(owner(), &requests).unwrap() {
                let limit = grant.limit();
                if limit > 0 {
                    recipients.push(id);
                }
                grant.settle(limit).unwrap();
            }
            ledger = GasLedger::from_snapshot(ledger.snapshot().unwrap()).unwrap();
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
                .reserve_fair(owner(), &[request(1, 1), request(1, 1)])
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
            for (id, grant) in ledger.reserve_fair(owner(), &round_requests).unwrap() {
                let used = if grant.limit() >= 700_000 {
                    winners.push(id);
                    700_000
                } else {
                    assert_eq!(grant.limit(), 0);
                    0
                };
                grant.settle(used).unwrap();
            }
            assert_eq!(ledger.account(owner()).unwrap().available, 300_000);
            assert_eq!(
                ledger.account(owner()).unwrap().spent,
                (round as u64 + 1) * 700_000
            );
            ledger = GasLedger::from_snapshot(ledger.snapshot().unwrap()).unwrap();
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
        assert!(ledger.reserve_fair(owner(), &[malformed]).is_err());
    }

    #[test]
    fn small_requests_do_not_pin_fairness_ahead_of_a_skipped_atomic_call() {
        let ledger = ledger(0);
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
            for (id, grant) in ledger.reserve_fair(owner(), &requests).unwrap() {
                let limit = grant.limit();
                if limit == 700_000 {
                    *large_winners.entry(id).or_default() += 1;
                }
                grant.settle(limit).unwrap();
            }
        }
        assert!(large_winners[&Id([1; 16])] >= 2);
        assert!(large_winners[&Id([2; 16])] >= 2);
        assert_eq!(ledger.account(owner()).unwrap().available, 0);
    }

    #[test]
    fn exhaustion_and_zero_requests_stay_bounded() {
        let ledger = ledger(0);
        assert!(ledger.reserve(owner(), 1).is_err());
        assert!(ledger.reserve_fair(owner(), &[]).unwrap().is_empty());
        let grants = ledger
            .reserve_fair(owner(), &[request(1, u64::MAX), request(2, 0)])
            .unwrap();
        assert!(grants.iter().all(|(_, grant)| grant.limit() == 0));
        drop(grants);
        assert_eq!(ledger.account(owner()).unwrap().spent, 0);
    }

    #[test]
    fn corrupt_restore_and_lifetime_overflow_are_rejected() {
        let ledger = ledger(u64::MAX);
        let reservation = ledger.reserve(owner(), 10).unwrap();
        assert!(ledger.deposit(owner(), 1).is_err());
        reservation.settle(5).unwrap();
        assert!(ledger.deposit(owner(), 1).is_err());
        let mut snapshot = ledger.snapshot().unwrap();
        snapshot.accounts.get_mut(&owner()).unwrap().reserved = 1;
        assert!(GasLedger::from_snapshot(snapshot).is_err());
        let mut snapshot = ledger.snapshot().unwrap();
        snapshot.accounts.get_mut(&owner()).unwrap().owner = Principal::Organization(Id([1; 16]));
        assert!(GasLedger::from_snapshot(snapshot).is_err());
    }

    #[test]
    fn payer_follows_actual_ownership_without_iff_or_control_aliases() {
        let mut world = World::new();
        let player = Id([1; 16]);
        let organization = Principal::Organization(player);
        let ship = world
            .spawn((
                super::super::ownership::AssetOwner(owner()),
                super::super::identity::Control {
                    account: Id([2; 16]),
                    revision: 1,
                },
            ))
            .id();
        assert_eq!(payer(&world, ship).unwrap(), owner());
        world
            .get_mut::<super::super::ownership::AssetOwner>(ship)
            .unwrap()
            .0 = organization;
        assert_eq!(payer(&world, ship).unwrap(), organization);
        let ledger = ledger(100);
        ledger.ensure_account(organization, 300);
        ledger
            .reserve(organization, 30)
            .unwrap()
            .settle(20)
            .unwrap();
        assert_eq!(ledger.account(owner()).unwrap().available, 100);
        assert_eq!(ledger.account(organization).unwrap().available, 280);
    }
}
