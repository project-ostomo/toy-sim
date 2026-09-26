//! Run alone: cargo test -p osg-server --release --features society-bench
//! --lib society_storage -- --ignored --nocapture --test-threads=1
use super::*;
use crate::sim::{economy::Economy, gas::GasRequest};
use osg_model::{economy::Currency, market::*, ownership::*};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::{hint::black_box, time::Instant};

struct Allocator;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            let live = LIVE.fetch_add(layout.size() as u64, Relaxed) + layout.size() as u64;
            if ENABLED.load(Relaxed) {
                ALLOCATIONS.fetch_add(1, Relaxed);
                PEAK.fetch_max(live, Relaxed);
            }
        }
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as u64, Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
}

fn measure<T>(label: &str, size: usize, operation: impl FnOnce() -> T) -> T {
    let initial = LIVE.load(Relaxed);
    PEAK.store(initial, Relaxed);
    ALLOCATIONS.store(0, Relaxed);
    ENABLED.store(true, Relaxed);
    let start = Instant::now();
    let result = black_box(operation());
    let elapsed = start.elapsed().as_nanos();
    ENABLED.store(false, Relaxed);
    println!(
        "society,{label},{size},{elapsed},{},{},{}",
        ALLOCATIONS.load(Relaxed),
        PEAK.load(Relaxed).saturating_sub(initial),
        LIVE.load(Relaxed).saturating_sub(initial)
    );
    result
}

fn principal(n: usize) -> Principal {
    Principal::Player(Id((n as u128).to_be_bytes()))
}

fn transfer(state: &mut SocietyState) {
    let mut draft = state.clone();
    draft
        .economy
        .transfer(None, principal(0), principal(1), Currency::Uec, 1, false, 0)
        .unwrap();
    *state = draft;
}

fn order(economy: &mut Economy, owner: Principal, side: Side, quantity: u64) {
    economy
        .execute_order(
            Instrument::Fx,
            None,
            Id::new(),
            owner,
            side,
            quantity,
            4_000_000,
            true,
            0,
        )
        .unwrap();
}

fn buy(state: &mut SocietyState, quantity: u64) {
    let mut draft = state.clone();
    order(&mut draft.economy, principal(0), Side::Buy, quantity);
    *state = draft;
}

fn gas(state: &mut SocietyState, requests: &[GasRequest]) {
    for (_, grant) in state.gas.debit_fair(principal(0), requests).unwrap() {
        state.gas.refund(principal(0), grant, grant / 2);
    }
}

#[test]
#[ignore = "allocation and persistent storage measurements; run alone"]
fn society_storage() {
    println!("kind,operation,size,ns,allocations,peak_extra_bytes,retained_extra_bytes");
    for rows in [1_000, 10_000, 100_000] {
        let mut state = SocietyState::default();
        state.economy = Economy::at(0);
        for n in 0..rows {
            state
                .economy
                .issue(principal(n), Currency::Uec, 1_000_000_000, 0)
                .unwrap();
            state.directory.0.players.insert(
                Id((n as u128).to_be_bytes()),
                PlayerAffiliation {
                    account: Id((n as u128).to_be_bytes()),
                    name: format!("Pilot {n}"),
                    organization: None,
                },
            );
        }
        let snapshot = measure("clone", rows, || state.clone());
        for _ in 0..5 {
            measure("transfer", rows, || transfer(&mut state));
        }
        measure("serialize", rows, || postcard::to_stdvec(&state).unwrap());
        drop(snapshot);
    }
    let mut state = SocietyState::default();
    state.economy = Economy::at(0);
    for _ in 0..1_000_000 {
        state
            .economy
            .issue(principal(0), Currency::Uec, 1, 0)
            .unwrap();
    }
    let snapshot = measure("ledger_clone", 1_000_000, || state.clone());
    for _ in 0..5 {
        measure("ledger_transfer", 1_000_000, || transfer(&mut state));
    }
    measure("ledger_serialize", 1_000_000, || {
        postcard::to_stdvec(&state).unwrap()
    });
    drop(snapshot);
    drop(state);

    for makers in [1, 10, 100] {
        let mut state = SocietyState::default();
        state.economy = Economy::at(0);
        state
            .economy
            .issue(principal(0), Currency::Uec, 1_000_000_000, 0)
            .unwrap();
        for n in 1..=makers {
            state
                .economy
                .issue(principal(n), Currency::Lat, 1_000_000, 0)
                .unwrap();
            order(&mut state.economy, principal(n), Side::Sell, 1_000_000);
        }
        measure("fill", makers, || {
            buy(&mut state, makers as u64 * 1_000_000)
        });
    }

    for computers in [1_000, 10_000, 100_000] {
        let mut state = SocietyState::default();
        state.gas.ensure_account(principal(0), 1_000_000_000_000);
        let requests: Vec<_> = (0..computers)
            .map(|n| GasRequest {
                id: Id((n as u128).to_be_bytes()),
                maximum: 1_000_000,
                minimum: 1,
            })
            .collect();
        measure("gas", computers, || gas(&mut state, &requests));
    }

    use osg_model::diplomacy::*;
    for revisions in [1_000, 10_000, 100_000] {
        let mut state = SocietyState::default();
        for n in 0..2 {
            let Principal::Player(account) = principal(n) else {
                unreachable!()
            };
            state.directory.0.players.insert(
                account,
                PlayerAffiliation {
                    account,
                    name: format!("Pilot {n}"),
                    organization: None,
                },
            );
        }
        let key = (principal(0), DeclarationCategory::Standing, principal(1));
        let declaration = |revision| Declaration {
            source: key.0,
            category: key.1,
            target: key.2,
            revision,
            standing: Standing::Friendly,
            enabled: true,
            note: "Declaration history".into(),
        };
        state
            .directory
            .0
            .diplomacy
            .declaration_history
            .insert(key, (1..=revisions as u64).map(declaration).collect());
        state
            .directory
            .0
            .diplomacy
            .declarations
            .insert(key, declaration(revisions as u64));
        measure("diplomacy", revisions, || {
            let mut draft = state.clone();
            crate::sim::diplomacy::apply(
                &mut draft.directory.0,
                Id(0_u128.to_be_bytes()),
                DiplomacyCommand::Publish(declaration(revisions as u64)),
            )
            .unwrap();
            state = draft;
        });
    }
}
