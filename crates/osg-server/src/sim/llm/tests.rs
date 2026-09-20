use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};

fn path() -> PathBuf {
    std::env::temp_dir().join(format!("toy-llm-test-{}/spend.sqlite", Id::new()))
}

fn caller() -> LlmCaller {
    LlmCaller {
        world: Id::new(),
        owner: Principal::Player(Id::new()),
        computer: Id::new(),
        program: [7; 32],
        display: false,
    }
}

fn request(id: u64) -> LlmRequest {
    LlmRequest {
        id,
        prompt: "Describe this test observation".into(),
        max_tokens: 128,
    }
}

fn key(request: u64) -> Key {
    Key {
        caller: caller(),
        request,
    }
}

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "LLM test timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct FakeProvider {
    calls: AtomicUsize,
    gate: tokio::sync::Semaphore,
    uncertain: bool,
}

impl FakeProvider {
    fn new(uncertain: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            gate: tokio::sync::Semaphore::new(0),
            uncertain,
        })
    }
}

impl provider::Provider for FakeProvider {
    fn complete(
        &self,
        _: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = provider::Outcome> + Send + '_>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.gate.acquire().await.unwrap().forget();
            if self.uncertain {
                provider::Outcome {
                    status: LlmStatus::Indeterminate,
                    charged: None,
                }
            } else {
                provider::Outcome {
                    status: LlmStatus::Ready {
                        text: "Observed response".into(),
                    },
                    charged: Some(15),
                }
            }
        })
    }
}

#[test]
fn durable_reservations_survive_crash_and_settlement_is_idempotent() {
    let path = path();
    let key = key(1);
    {
        let mut ledger = spend::SpendLedger::open(&path).unwrap();
        assert!(matches!(
            ledger.reserve(key, [1; 32], 60_000_000).unwrap(),
            spend::Admission::New
        ));
        assert!(matches!(
            ledger.reserve(key, [1; 32], 60_000_000).unwrap(),
            spend::Admission::Existing(LlmStatus::Pending)
        ));
        assert!(matches!(
            ledger.reserve(key, [2; 32], 1).unwrap(),
            spend::Admission::Conflicting { .. }
        ));
    }
    let mut ledger = spend::SpendLedger::open(&path).unwrap();
    assert_eq!(ledger.spending().unwrap().reserved_microdollars, 60_000_000);
    assert!(matches!(
        ledger.reserve(key, [1; 32], 60_000_000).unwrap(),
        spend::Admission::Existing(LlmStatus::Indeterminate)
    ));
    assert!(matches!(
        ledger
            .reserve(Key { request: 2, ..key }, [2; 32], 40_000_001)
            .unwrap(),
        spend::Admission::BudgetExhausted
    ));
    assert!(
        ledger
            .settle(key, Some(60_000_001), &LlmStatus::Cancelled)
            .is_err()
    );
    ledger
        .settle(key, Some(100), &LlmStatus::Cancelled)
        .unwrap();
    ledger
        .settle(key, Some(100), &LlmStatus::Cancelled)
        .unwrap();
    assert!(ledger.settle(key, Some(99), &LlmStatus::Cancelled).is_err());
    assert_eq!(
        ledger.spending().unwrap(),
        Spending {
            settled_microdollars: 100,
            reserved_microdollars: 0
        }
    );
}

#[test]
fn independent_connections_cannot_race_past_the_dollar_cap() {
    let path = path();
    spend::SpendLedger::open(&path).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut ledger = spend::SpendLedger::open(&path).unwrap();
            barrier.wait();
            matches!(
                ledger.reserve(key(1), [1; 32], 60_000_000).unwrap(),
                spend::Admission::New
            )
        }));
    }
    barrier.wait();
    assert_eq!(
        workers
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|accepted| *accepted)
            .count(),
        1
    );
    assert_eq!(
        spend::SpendLedger::open(&path)
            .unwrap()
            .spending()
            .unwrap()
            .reserved_microdollars,
        60_000_000
    );
}

#[test]
fn fresh_world_request_ids_are_independent_under_one_spending_cap() {
    let provider = FakeProvider::new(false);
    let service = LlmService::start(&path(), provider.clone()).unwrap();
    let original = caller();
    let fresh = LlmCaller {
        world: Id::new(),
        ..original
    };
    let gas = GasLedger::default();
    gas.ensure_account(original.owner, 1_000_000_000);

    assert_eq!(
        service.submit(original, request(1), &gas),
        LlmSubmission::Accepted
    );
    let mut next_world_request = request(1);
    next_world_request.prompt = "A different world's observation".into();
    assert_eq!(
        service.submit(fresh, next_world_request, &gas),
        LlmSubmission::Accepted
    );
    wait(|| provider.calls.load(Ordering::Acquire) == 2);
    provider.gate.add_permits(2);
    wait(|| {
        matches!(service.poll(original, 1), LlmStatus::Ready { .. })
            && matches!(service.poll(fresh, 1), LlmStatus::Ready { .. })
    });
    assert_eq!(service.spending().settled_microdollars, 30);
    assert_eq!(service.spending().reserved_microdollars, 0);
}

#[test]
fn asynchronous_admission_settles_gas_and_isolates_program_scopes() {
    let provider = FakeProvider::new(false);
    let path = path();
    let service = LlmService::start(&path, provider.clone()).unwrap();
    let caller = caller();
    let gas = GasLedger::default();
    gas.ensure_account(caller.owner, 1_000_000_000);
    let request = request(1);
    let quote = request.gas_quote().unwrap();
    assert_eq!(
        service.submit(caller, request.clone(), &gas),
        LlmSubmission::Accepted
    );
    assert_eq!(gas.snapshot().unwrap().accounts[&caller.owner].spent, quote);
    assert_eq!(
        service.submit(caller, request.clone(), &gas),
        LlmSubmission::AlreadyKnown
    );
    let mut changed = request.clone();
    changed.prompt.push('!');
    assert_eq!(
        service.submit(caller, changed, &gas),
        LlmSubmission::InvalidRequest
    );
    assert_eq!(gas.snapshot().unwrap().accounts[&caller.owner].spent, quote);
    for other in [
        LlmCaller {
            world: Id::new(),
            ..caller
        },
        LlmCaller {
            display: true,
            ..caller
        },
        LlmCaller {
            program: [8; 32],
            ..caller
        },
        LlmCaller {
            owner: Principal::Player(Id::new()),
            ..caller
        },
    ] {
        assert_eq!(service.poll(other, 1), LlmStatus::Unknown);
        assert!(!service.cancel(other, 1));
    }
    wait(|| provider.calls.load(Ordering::Acquire) == 1);
    provider.gate.add_permits(1);
    wait(|| matches!(service.poll(caller, 1), LlmStatus::Ready { .. }));
    assert_eq!(
        service.spending(),
        Spending {
            settled_microdollars: 15,
            reserved_microdollars: 0
        }
    );
    drop(service);
    let restarted = LlmService::start(&path, provider.clone()).unwrap();
    assert!(matches!(restarted.poll(caller, 1), LlmStatus::Ready { .. }));
    assert_eq!(
        restarted.submit(caller, request, &gas),
        LlmSubmission::AlreadyKnown
    );
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
}

#[test]
fn uncertain_provider_result_keeps_money_held_and_never_retries() {
    let provider = FakeProvider::new(true);
    let path = path();
    let service = LlmService::start(&path, provider.clone()).unwrap();
    let caller = caller();
    let gas = GasLedger::default();
    gas.ensure_account(caller.owner, 1_000_000_000);
    let request = request(1);
    let reserved = provider::reservation(&request);
    assert_eq!(
        service.submit(caller, request.clone(), &gas),
        LlmSubmission::Accepted
    );
    wait(|| provider.calls.load(Ordering::Acquire) == 1);
    provider.gate.add_permits(1);
    wait(|| service.poll(caller, 1) == LlmStatus::Indeterminate);
    assert_eq!(service.spending().reserved_microdollars, reserved);
    drop(service);
    let restarted = LlmService::start(&path, provider.clone()).unwrap();
    assert_eq!(restarted.poll(caller, 1), LlmStatus::Indeterminate);
    assert_eq!(
        restarted.submit(caller, request, &gas),
        LlmSubmission::AlreadyKnown
    );
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
}

#[test]
fn evicted_request_ids_remain_idempotent_and_preserve_the_original_result() {
    let provider = FakeProvider::new(false);
    let service = LlmService::start(&path(), provider.clone()).unwrap();
    let caller = caller();
    let gas = GasLedger::default();
    gas.ensure_account(caller.owner, 1_000_000_000);
    let original = request(1);

    assert_eq!(
        service.submit(caller, original.clone(), &gas),
        LlmSubmission::Accepted
    );
    wait(|| provider.calls.load(Ordering::Acquire) == 1);
    provider.gate.add_permits(1);
    wait(|| matches!(service.poll(caller, 1), LlmStatus::Ready { .. }));

    service.state.lock().unwrap().entries.clear();
    assert_eq!(
        service.submit(caller, original.clone(), &gas),
        LlmSubmission::Accepted
    );
    wait(|| matches!(service.poll(caller, 1), LlmStatus::Ready { .. }));
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    assert_eq!(service.spending().settled_microdollars, 15);

    service.state.lock().unwrap().entries.clear();
    let mut conflicting = original.clone();
    conflicting.prompt.push_str(" Conflicting content");
    assert_eq!(
        service.submit(caller, conflicting.clone(), &gas),
        LlmSubmission::Accepted
    );
    wait(|| matches!(service.poll(caller, 1), LlmStatus::Ready { .. }));
    assert_eq!(
        service.submit(caller, conflicting, &gas),
        LlmSubmission::InvalidRequest
    );
    assert_eq!(
        service.submit(caller, original, &gas),
        LlmSubmission::AlreadyKnown
    );
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
}

#[test]
fn restart_cache_retains_the_correct_eviction_order() {
    let path = path();
    let caller = caller();
    let mut ledger = spend::SpendLedger::open(&path).unwrap();
    for request in 1..=3 {
        let key = Key { caller, request };
        ledger.reserve(key, [request as u8; 32], 100).unwrap();
        ledger.settle(key, Some(0), &LlmStatus::Cancelled).unwrap();
    }
    drop(ledger);

    let service = LlmService::start(&path, FakeProvider::new(false)).unwrap();
    let state = service.state.lock().unwrap();
    let oldest = state
        .entries
        .iter()
        .min_by_key(|(_, entry)| entry.sequence)
        .unwrap()
        .0;
    assert_eq!(oldest.request, 1);
}

#[test]
fn cancellation_does_not_refund_an_already_dispatched_request() {
    let provider = FakeProvider::new(false);
    let service = LlmService::start(&path(), provider.clone()).unwrap();
    let caller = caller();
    let gas = GasLedger::default();
    gas.ensure_account(caller.owner, 1_000_000_000);
    assert_eq!(
        service.submit(caller, request(1), &gas),
        LlmSubmission::Accepted
    );
    wait(|| provider.calls.load(Ordering::Acquire) == 1);
    assert!(service.cancel(caller, 1));
    assert_eq!(service.poll(caller, 1), LlmStatus::Cancelled);
    assert!(service.spending().reserved_microdollars > 0);
    provider.gate.add_permits(1);
    wait(|| service.spending().settled_microdollars == 15);
    assert_eq!(service.poll(caller, 1), LlmStatus::Cancelled);
    assert!(gas.snapshot().is_ok());
}

#[test]
fn computer_and_owner_limits_do_not_charge_rejected_admission() {
    let provider = FakeProvider::new(false);
    let service = LlmService::start(&path(), provider.clone()).unwrap();
    let caller = caller();
    let gas = GasLedger::default();
    gas.ensure_account(caller.owner, 1_000_000_000);
    for id in 1..=2 {
        assert_eq!(
            service.submit(caller, request(id), &gas),
            LlmSubmission::Accepted
        );
    }
    let before = gas.account(caller.owner).unwrap();
    assert_eq!(
        service.submit(caller, request(3), &gas),
        LlmSubmission::Busy
    );
    assert_eq!(gas.account(caller.owner).unwrap(), before);
    for _ in 0..2 {
        assert_eq!(
            service.submit(
                LlmCaller {
                    computer: Id::new(),
                    ..caller
                },
                request(1),
                &gas
            ),
            LlmSubmission::Accepted
        );
    }
    assert_eq!(
        service.submit(
            LlmCaller {
                computer: Id::new(),
                ..caller
            },
            request(1),
            &gas
        ),
        LlmSubmission::Busy
    );
    let empty = LlmCaller {
        owner: Principal::Player(Id::new()),
        computer: Id::new(),
        ..caller
    };
    assert_eq!(
        service.submit(empty, request(1), &gas),
        LlmSubmission::InsufficientGas
    );
    assert_eq!(
        LlmService::from_env(false)
            .unwrap()
            .submit(caller, request(1), &gas),
        LlmSubmission::Unavailable
    );
    provider.gate.add_permits(4);
    wait(|| service.spending().settled_microdollars == 60);
}
