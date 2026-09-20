use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use toy_sim_model::{ProgramQuery, ProgramReply};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{
    Command, Controller, ControllerRuntime, FUEL_PER_TICK, Input, Observation, Request, ScanSource,
    SensorContact,
};

fn guest(imports: &str, declarations: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            {imports}
            (memory (export "memory") 1)
            (func (export "ship_api_version") (result i32) i32.const {version})
            {declarations}
            (func (export "ship_tick") {body}))"#,
        version = abi::VERSION,
    ))
    .unwrap()
}

fn input(tick: u64) -> Input {
    Input {
        tick,
        observation: Observation {
            time_s: tick as f64 * 0.1,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn boot(program: &[u8]) -> Controller {
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut controller = runtime.instantiate(program).unwrap();

    for tick in 0..100 {
        let slice = controller
            .run_slice(input(tick), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
        assert!(controller.last_gas_used <= FUEL_PER_TICK);
        assert!(!slice.callback_completed);

        if !controller.is_booting() {
            return controller;
        }
    }

    panic!("funded computer failed to finish booting");
}

fn words(controller: &Controller) -> Vec<u64> {
    controller
        .checkpoint()
        .persistent_data
        .chunks_exact(8)
        .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
        .collect()
}

#[test]
fn initialization_can_span_paid_slices_without_failing_preflight_validation() {
    let program = guest(
        "",
        r#"
            (global $sum (mut i64) (i64.const 0))
            (func $initialize
                (local $index i64) (local $sum i64)
                loop $work
                    local.get $index i64.const 1 i64.add local.tee $index
                    local.get $sum i64.add local.set $sum
                    local.get $index i64.const 10000 i64.lt_u br_if $work
                end
                local.get $sum global.set $sum)
            (start $initialize)
        "#,
        "global.get $sum i64.const 50005000 i64.ne if unreachable end",
    );
    let mut runtime = ControllerRuntime::new().unwrap();
    runtime.validate_program(&program).unwrap();
    let mut controller = runtime.instantiate(&program).unwrap();

    for tick in 0..50 {
        controller
            .run_slice(input(tick), None, FUEL_PER_TICK, FUEL_PER_TICK)
            .unwrap();
        assert_eq!(controller.last_gas_used, FUEL_PER_TICK);
    }

    assert_eq!(controller.boot_remaining_gas(), 0);
    assert!(controller.is_booting());
    controller
        .run_slice(input(50), None, 0, FUEL_PER_TICK)
        .unwrap();
    assert_eq!(controller.last_gas_used, 0);
    assert!(controller.is_booting());

    for tick in 51..300 {
        controller
            .run_slice(input(tick), None, 1000, FUEL_PER_TICK)
            .unwrap();
        assert!(controller.last_gas_used <= 1000);
        assert!(controller.fault.is_none());

        if !controller.is_booting() {
            assert!(tick > 61);
            assert!(
                controller
                    .run_slice(input(tick + 1), None, 1000, FUEL_PER_TICK)
                    .unwrap()
                    .callback_completed
            );
            return;
        }
    }

    panic!("funded initializer never completed");
}

#[test]
fn locals_and_callback_identity_survive_many_small_grants() {
    let program = guest(
        r#"(import "ship_v31" "persistent_write"
            (func $save (param i32 i32) (result i32)))"#,
        "(global $calls (mut i64) (i64.const 0))",
        r#"
            (local $index i64) (local $sum i64)
            global.get $calls i64.const 1 i64.add global.set $calls
            loop $work
                local.get $index i64.const 1 i64.add local.tee $index
                local.get $sum i64.add local.set $sum
                local.get $index i64.const 10000 i64.lt_u br_if $work
            end
            i32.const 0 global.get $calls i64.store
            i32.const 8 local.get $sum i64.store
            i32.const 0 i32.const 16 call $save
            i32.const 0 i32.ne if unreachable end
        "#,
    );
    let mut controller = boot(&program);
    let mut suspended = 0;

    for tick in 100..500 {
        let slice = controller
            .run_slice(input(tick), None, 1000, FUEL_PER_TICK)
            .unwrap();
        assert!(controller.last_gas_used <= 1000);
        assert!(controller.fault.is_none());

        if slice.callback_completed {
            assert!(suspended > 10);
            assert_eq!(words(&controller), [1, 50_005_000]);
            return;
        }

        assert!(controller.is_suspended());
        suspended += 1;
    }

    panic!("resumed callback never completed");
}

#[test]
fn borrowed_snapshot_survives_implicit_wait_until_it_can_be_pinned() {
    let program = guest(
        r#"
            (import "ship_v31" "tick_read"
                (func $tick (param i32 i32) (result i32)))
            (import "ship_v31" "snapshot_keep"
                (func $keep (param i64) (result i32)))
            (import "ship_v31" "snapshot_drop"
                (func $drop (param i64) (result i32)))
        "#,
        "",
        r#"
            (local $snapshot i64) (local $index i32)
            i32.const 0 i32.const 96 call $tick drop
            i32.const 8 i64.load local.set $snapshot
            loop $work
                local.get $index i32.const 1 i32.add local.tee $index
                i32.const 2000 i32.lt_u br_if $work
            end
            local.get $snapshot call $keep
            i32.const 0 i32.ne if unreachable end
            i32.const 0 i32.const 96 call $tick drop
            i32.const 8 i64.load local.get $snapshot i64.eq if unreachable end
            local.get $snapshot call $keep
            i32.const 0 i32.ne if unreachable end
            local.get $snapshot call $drop
            i32.const 0 i32.ne if unreachable end
        "#,
    );
    let mut controller = boot(&program);

    for tick in 100..200 {
        let slice = controller
            .run_slice(input(tick), None, 1000, FUEL_PER_TICK)
            .unwrap();
        assert!(controller.last_gas_used <= 1000);

        if slice.callback_completed {
            assert!(tick > 101);
            return;
        }
    }

    panic!("borrowed snapshot could not be retained across suspension");
}

struct QuerySource(Arc<AtomicUsize>);

impl ScanSource for QuerySource {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query_work(&self, _: &ProgramQuery) -> anyhow::Result<u64> {
        Ok(700)
    }

    fn query(
        &self,
        _: ProgramQuery,
        _: bool,
        _: toy_sim_model::wasm_world::ReplyCapacity,
    ) -> anyhow::Result<ProgramReply> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ProgramReply::Beacons(Vec::new()))
    }
}

#[test]
fn deferred_syscall_has_no_unfunded_effect_and_uses_the_current_scene() {
    let program = guest(
        r#"(import "ship_v31" "beacons_read"
            (func $query (param i32 i32 i32 i32 i32 i32 i32) (result i32)))"#,
        "",
        "i32.const 0 i32.const 0 i32.const 512 i32.const 1 i32.const 4096 i32.const 128 i32.const 256 call $query \
         i32.const 0 i32.lt_s if unreachable end",
    );
    let mut controller = boot(&program);
    let old_calls = Arc::new(AtomicUsize::new(0));
    let new_calls = Arc::new(AtomicUsize::new(0));
    let old_source: Arc<dyn ScanSource> = Arc::new(QuerySource(old_calls.clone()));
    let old_source_lifetime = Arc::downgrade(&old_source);

    let slice = controller
        .run_slice(input(100), Some(old_source), 200, FUEL_PER_TICK)
        .unwrap();
    assert!(!slice.callback_completed);
    assert!(controller.minimum_to_progress() > 200);
    assert!(controller.last_gas_used <= 200);
    assert_eq!(old_calls.load(Ordering::SeqCst), 0);
    assert!(
        old_source_lifetime.upgrade().is_none(),
        "suspended computer retained the previous world snapshot"
    );

    controller
        .run_slice(
            input(101),
            Some(Arc::new(QuerySource(new_calls.clone()))),
            0,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert_eq!(controller.last_gas_used, 0);
    assert_eq!(new_calls.load(Ordering::SeqCst), 0);

    let slice = controller
        .run_slice(
            input(102),
            Some(Arc::new(QuerySource(new_calls.clone()))),
            1000,
            FUEL_PER_TICK,
        )
        .unwrap();
    assert!(slice.callback_completed);
    assert!(controller.last_gas_used <= 1000);
    assert_eq!(old_calls.load(Ordering::SeqCst), 0);
    assert_eq!(new_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn malformed_typed_query_returns_argument_error_without_calling_service() {
    let program = guest(
        r#"(import "ship_v31" "destination_resolve"
            (func $query (param i32 i32) (result i32)))"#,
        r#"(data (i32.const 0) "\ff\ff\ff\ff\ff\ff\ff\ff\ff\ff")"#,
        &format!(
            "i32.const 0 i32.const 512 call $query \
             i32.const {} i32.ne if unreachable end",
            abi::ERR_ARGUMENT,
        ),
    );
    let mut controller = boot(&program);
    let first = controller
        .run_slice(input(100), None, 1000, FUEL_PER_TICK)
        .unwrap();
    assert!(first.callback_completed);
    assert!(controller.last_gas_used <= 1000);
    assert!(controller.last_gas_used >= abi::CALL_GAS);
}

#[test]
fn request_batches_stay_stable_while_observations_refresh() {
    let program = guest(
        r#"
            (import "ship_v31" "request_info"
                (func $request (param i32 i32 i32) (result i32)))
            (import "ship_v31" "request_reply"
                (func $reply (param i64 i64 i32 i32) (result i32)))
            (import "ship_v31" "tick_read"
                (func $tick (param i32 i32) (result i32)))
            (import "ship_v31" "persistent_write"
                (func $save (param i32 i32) (result i32)))
        "#,
        "",
        r#"
            (local $request_id i64) (local $first_tick i64) (local $index i32)
            i32.const 0 i32.const 0 i32.const 24 call $request
            i32.const 0 i32.ne if unreachable end
            i32.const 0 i64.load local.set $request_id
            i32.const 128 i32.const 96 call $tick drop
            i32.const 128 i64.load local.set $first_tick
            loop $work
                local.get $index i32.const 1 i32.add local.tee $index
                i32.const 2000 i32.lt_u br_if $work
            end
            i32.const 0 i32.const 0 i32.const 24 call $request
            i32.const 0 i32.ne if unreachable end
            i32.const 0 i64.load local.get $request_id i64.ne if unreachable end
            i32.const 128 i32.const 96 call $tick drop
            i32.const 128 i64.load local.get $first_tick i64.le_u if unreachable end
            i32.const 8 i32.const 184 i64.load i64.store
            i32.const 0 i32.const 16 call $save drop
            local.get $request_id i64.const 0 i32.const 0 i32.const 0 call $reply drop
        "#,
    );
    let mut controller = boot(&program);
    let mut completed = Vec::new();

    for tick in 100..300 {
        let mut input = input(tick);
        if tick == 100 || tick == 101 {
            input.commands.push(Request {
                id: if tick == 100 { 41 } else { 42 },
                command: Command::StopGuidance,
            });
        }

        let slice = controller
            .run_slice(input, None, 1000, FUEL_PER_TICK)
            .unwrap();
        assert!(controller.last_gas_used <= 1000);
        completed.extend(slice.output.replies.into_iter().map(|reply| reply.id));

        if slice.callback_completed {
            let data = words(&controller);
            assert_eq!(data[1], 1, "request count changed inside the callback");
            if completed.len() == 2 {
                assert_eq!(completed, [41, 42]);
                assert_eq!(data[0], 42);
                return;
            }
        }
    }

    panic!("queued requests were lost or callbacks did not finish");
}

#[test]
fn later_trap_keeps_prior_slice_durable_writes_but_reboots_the_computer() {
    let program = guest(
        r#"(import "ship_v31" "persistent_write"
            (func $save (param i32 i32) (result i32)))"#,
        "",
        r#"
            (local $index i32)
            i32.const 0 i64.const 41 i64.store
            i32.const 0 i32.const 8 call $save drop
            loop $work
                local.get $index i32.const 1 i32.add local.tee $index
                i32.const 2000 i32.lt_u br_if $work
            end
            i32.const 0 i64.const 99 i64.store
            i32.const 0 i32.const 8 call $save drop
            unreachable
        "#,
    );
    let mut controller = boot(&program);
    let first = controller
        .run_slice(input(100), None, 1000, FUEL_PER_TICK)
        .unwrap();
    assert!(!first.callback_completed);
    assert!(controller.last_gas_used <= 1000);
    assert_eq!(words(&controller), [41]);

    let result = controller.run_slice(input(101), None, FUEL_PER_TICK, FUEL_PER_TICK);
    assert!(result.is_err());
    assert!(controller.last_gas_used <= FUEL_PER_TICK);
    assert_eq!(words(&controller), [41]);
    assert!(controller.is_booting());
    assert!(controller.fault.is_some());
}
