use super::*;
use std::{
    future::Future,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};
use wasmtime::{Config, Engine, Linker, Module, Store};

#[derive(Default)]
struct Tick {
    epoch: AtomicU64,
    remaining: AtomicU64,
    calls: AtomicU64,
    effects: AtomicU64,
}

#[test]
fn jit_keeps_recursive_locals_and_bulk_side_effects_across_admission() -> anyhow::Result<()> {
    let wasm = wat::parse_str(
        r#"(module
        (memory (export "memory") 1)
        (func $recursive (param $n i32) (result i64) (local $sum i64)
            local.get $n i32.eqz
            if (result i64) i64.const 7
            else
                local.get $n i64.extend_i32_u local.set $sum
                local.get $n i32.const 1 i32.sub call $recursive
                local.get $sum i64.add
            end)
        (func $initialize i32.const 0 i32.const 123 i32.store i32.const 10 call $recursive drop)
        (start $initialize)
        (func (export "run") (result i64)
            i32.const 0 i32.const 42 i32.const 4096 memory.fill
            i32.const 100 call $recursive)
    )"#,
    )?;
    let transformed = instrument(&wasm)?;
    let config = Config::new();
    let engine = Engine::new(&config)?;
    let module = Module::new(&engine, transformed)?;
    let gate = Arc::new(Tick::default());
    let mut linker = Linker::<Arc<Tick>>::new(&engine);
    linker.func_wrap_async(
        "__private_meter",
        "admit",
        |caller, (remaining, cost): (i64, i64)| {
            Box::new(async move {
                if cost > 128 {
                    return Err(wasmtime::Error::msg(
                        "single operation exceeds tick admission",
                    ));
                }
                let gate = caller.data().clone();
                gate.remaining.store(remaining as u64, Ordering::SeqCst);
                gate.calls.fetch_add(1, Ordering::SeqCst);
                let waiting = gate.epoch.load(Ordering::SeqCst);
                std::future::poll_fn(|_| {
                    if gate.epoch.load(Ordering::SeqCst) > waiting {
                        Poll::Ready(128_i64)
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                Ok::<i64, wasmtime::Error>(128)
            })
        },
    )?;
    let mut store = Store::new(&engine, gate.clone());
    let (instance, start_ticks) = drive(linker.instantiate_async(&mut store, &module), &gate)?;
    assert!(start_ticks > 0);
    assert_eq!(
        instance
            .get_memory(&mut store, "memory")
            .unwrap()
            .data(&store)[0],
        123
    );
    let run = instance.get_typed_func::<(), i64>(&mut store, "run")?;
    let (value, ticks) = drive(run.call_async(&mut store, ()), &gate)?;
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    assert!(memory.data(&store)[..4096].iter().all(|byte| *byte == 42));
    assert_eq!(value, 5057);
    assert!(ticks > 2);
    eprintln!("resumed recursive JIT after {ticks} prepaid tick boundaries");
    Ok(())
}

#[test]
fn real_firmware_reencodes_and_validates() -> anyhow::Result<()> {
    let original = osg_ships::EXAMPLE_CONTROLLER;
    let transformed = instrument(original)?;
    let engine = Engine::default();
    let start = std::time::Instant::now();
    Module::new(&engine, &transformed)?;
    eprintln!(
        "stock firmware {} -> {} bytes, compile {:.2} ms",
        original.len(),
        transformed.len(),
        start.elapsed().as_secs_f64() * 1000.
    );
    Ok(())
}

fn drive<T>(
    future: impl Future<Output = wasmtime::Result<T>>,
    gate: &Tick,
) -> anyhow::Result<(T, usize)> {
    let mut call = pin!(future);
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(&waker);
    let mut ticks = 0;
    loop {
        match call.as_mut().poll(&mut context) {
            Poll::Ready(result) => return Ok((result?, ticks)),
            Poll::Pending => {
                assert!(gate.remaining.load(Ordering::SeqCst) < 128);
                ticks += 1;
                assert!(ticks < 1000);
                gate.epoch.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

#[test]
fn structured_branches_tables_refs_and_tail_calls_preserve_indices() -> anyhow::Result<()> {
    let bytes = wat::parse_str(
        r#"(module
        (type $t (func (param i32) (result i32)))
        (table 2 funcref)
        (elem (i32.const 0) $plus)
        (func $plus (type $t) local.get 0 i32.const 1 i32.add)
        (func $tail (type $t) local.get 0 return_call $plus)
        (func $count (param $n i32) (result i32)
            block $done
                loop $again
                    local.get $n i32.eqz br_if $done
                    local.get $n i32.const 1 i32.sub local.set $n
                    i32.const 0 br_table $again $done
                end
            end
            i32.const 41 call $tail)
        (func (export "run") (result i32)
            i32.const 1 ref.func $plus table.set
            i32.const 100 call $count drop
            i32.const 41 i32.const 1 call_indirect (type $t)))"#,
    )?;
    let engine = Engine::default();
    let module = Module::new(&engine, instrument(&bytes)?)?;
    let mut store = Store::new(&engine, ());
    let mut linker = Linker::new(&engine);
    linker.func_wrap("__private_meter", "admit", |_: i64, _: i64| -> i64 {
        1_000_000
    })?;
    let instance = linker.instantiate(&mut store, &module)?;
    assert_eq!(
        instance
            .get_typed_func::<(), i32>(&mut store, "run")?
            .call(&mut store, ())?,
        42
    );
    Ok(())
}

#[test]
fn forged_meter_import_and_global_export_are_rejected() -> anyhow::Result<()> {
    let forged = wat::parse_str(
        r#"(module (import "__private_meter" "admit" (func (param i64 i64) (result i64))) (func))"#,
    )?;
    assert!(instrument(&forged).is_err());
    let forged = wat::parse_str(
        r#"(module (global (export "__private_remaining") (mut i64) (i64.const 0)) (func))"#,
    )?;
    assert!(instrument(&forged).is_err());
    for forged_index in [
        r#"(module (func (export "run") call 1))"#,
        r#"(module (func (export "run") i64.const 999 global.set 0))"#,
    ] {
        assert!(instrument(&wat::parse_str(forged_index)?).is_err());
    }
    Ok(())
}

#[test]
fn host_effect_is_prepaid_once_and_caller_is_safely_held_across_await() -> anyhow::Result<()> {
    let bytes = wat::parse_str(
        r#"(module
        (import "host" "effect" (func $effect))
        (func (export "run") (result i64) call $effect i64.const 9))"#,
    )?;
    let engine = Engine::default();
    let module = Module::new(&engine, instrument(&bytes)?)?;
    let gate = Arc::new(Tick::default());
    let mut linker = Linker::<Arc<Tick>>::new(&engine);
    linker.func_wrap("__private_meter", "admit", |_: i64, _: i64| -> i64 {
        panic!("unexpected instruction shortage")
    })?;
    linker.func_wrap_async("host", "effect", |mut caller, (): ()| {
        Box::new(async move {
            let global = caller
                .get_export("__private_remaining")
                .unwrap()
                .into_global()
                .unwrap();
            let remaining = global.get(&mut caller).i64().unwrap();
            let gate = caller.data().clone();
            gate.remaining.store(remaining as u64, Ordering::SeqCst);
            assert!(remaining < 60);
            let waiting = gate.epoch.load(Ordering::SeqCst);
            std::future::poll_fn(|_| {
                if gate.epoch.load(Ordering::SeqCst) > waiting {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            global.set(&mut caller, wasmtime::Val::I64(128 - 60))?;
            gate.effects.fetch_add(1, Ordering::SeqCst);
            Ok::<(), wasmtime::Error>(())
        })
    })?;
    let mut store = Store::new(&engine, gate.clone());
    let (instance, _) = drive(linker.instantiate_async(&mut store, &module), &gate)?;
    instance
        .get_global(&mut store, REMAINING_EXPORT)
        .unwrap()
        .set(&mut store, wasmtime::Val::I64(50))?;
    let run = instance.get_typed_func::<(), i64>(&mut store, "run")?;
    {
        let mut call = pin!(run.call_async(&mut store, ()));
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(&waker);
        assert!(call.as_mut().poll(&mut context).is_pending());
        assert_eq!(gate.effects.load(Ordering::SeqCst), 0);
        gate.epoch.fetch_add(1, Ordering::SeqCst);
        assert!(matches!(
            call.as_mut().poll(&mut context),
            Poll::Ready(Ok(9))
        ));
    }
    assert_eq!(gate.effects.load(Ordering::SeqCst), 1);
    let remaining = instance
        .get_global(&mut store, "__private_remaining")
        .unwrap()
        .get(&mut store)
        .i64()
        .unwrap();
    assert_eq!(remaining, 67);
    Ok(())
}

fn synchronous_module(
    source: &str,
    grant: i64,
) -> anyhow::Result<(Store<usize>, wasmtime::Instance)> {
    let engine = Engine::default();
    let module = Module::new(&engine, instrument(&wat::parse_str(source)?)?)?;
    let mut linker = Linker::new(&engine);
    linker.func_wrap(
        MODULE,
        ADMIT,
        move |mut caller: wasmtime::Caller<'_, usize>,
              remaining: i64,
              cost: i64|
              -> wasmtime::Result<i64> {
            if cost > grant || cost <= 0 || remaining < 0 || remaining >= cost {
                return Err(wasmtime::Error::msg("invalid or oversized admission"));
            }
            *caller.data_mut() += 1;
            Ok(grant)
        },
    )?;
    let mut store = Store::new(&engine, 0);
    let instance = linker.instantiate(&mut store, &module)?;
    Ok((store, instance))
}

#[test]
fn long_segments_and_loop_back_edges_have_bounded_admissions() -> anyhow::Result<()> {
    let source = format!(
        r#"(module (func (export "run") (result i32) {} i32.const 7))"#,
        "i32.const 1 drop ".repeat(10_000)
    );
    let (mut store, instance) = synchronous_module(&source, MAX_SEGMENT_OPERATORS as i64)?;
    assert_eq!(
        instance
            .get_typed_func::<(), i32>(&mut store, "run")?
            .call(&mut store, ())?,
        7
    );
    assert_eq!(*store.data(), 20_001_usize.div_ceil(MAX_SEGMENT_OPERATORS));

    let (mut store, instance) = synchronous_module(
        r#"(module
        (func (export "run") (result i32) (local $n i32)
            i32.const 10_000 local.set $n
            loop $again
                local.get $n i32.const 1 i32.sub local.tee $n br_if $again
            end
            local.get $n))"#,
        MAX_SEGMENT_OPERATORS as i64,
    )?;
    assert_eq!(
        instance
            .get_typed_func::<(), i32>(&mut store, "run")?
            .call(&mut store, ())?,
        0
    );
    assert!(*store.data() > 700);
    Ok(())
}

#[test]
fn unreachable_code_preserves_validation_without_charging_dead_work() -> anyhow::Result<()> {
    let (mut store, instance) = synchronous_module(
        r#"(module
        (func (export "run") (result i32)
            i32.const 7 return
            loop $dead br $dead end unreachable))"#,
        64,
    )?;
    assert_eq!(
        instance
            .get_typed_func::<(), i32>(&mut store, "run")?
            .call(&mut store, ())?,
        7
    );
    let remaining = instance
        .get_global(&mut store, REMAINING_EXPORT)
        .unwrap()
        .get(&mut store)
        .i64()
        .unwrap();
    assert_eq!(remaining, 62);
    assert_eq!(*store.data(), 1);
    Ok(())
}

#[test]
fn entire_memory_bulk_operations_fit_a_tick_and_are_prepaid() -> anyhow::Result<()> {
    let (mut store, instance) = synchronous_module(
        r#"(module
        (memory (export "memory") 128)
        (func (export "run")
            i32.const 0 i32.const 42 i32.const 8388608 memory.fill
            i32.const 0 i32.const 0 i32.const 8388608 memory.copy))"#,
        1_000_000,
    )?;
    instance
        .get_typed_func::<(), ()>(&mut store, "run")?
        .call(&mut store, ())?;
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    assert!(memory.data(&store).iter().all(|byte| *byte == 42));
    let remaining = instance
        .get_global(&mut store, REMAINING_EXPORT)
        .unwrap()
        .get(&mut store)
        .i64()
        .unwrap();
    assert_eq!(
        remaining,
        1_000_000 - 6 - 2 * (8_388_608 / MEMORY_BYTES_PER_GAS + 1)
    );
    assert_eq!(*store.data(), 1);
    Ok(())
}

#[test]
fn unsupported_execution_proposals_are_rejected_explicitly() -> anyhow::Result<()> {
    for source in [
        "(module (memory i64 1) (func))",
        "(module (memory 1 1 shared) (func))",
        "(module (type (struct (field i32))) (func))",
        "(module (func v128.const i32x4 0 0 0 0 drop))",
        "(module (memory 1) (memory 1) (func))",
    ] {
        assert!(instrument(&wat::parse_str(source)?).is_err(), "{source}");
    }
    Ok(())
}

#[test]
fn original_globals_keep_their_indices_and_only_host_sees_new_budget() -> anyhow::Result<()> {
    let (mut store, instance) = synchronous_module(
        r#"(module
        (global $value (mut i64) (i64.const 10))
        (func (export "run") (result i64)
            global.get $value i64.const 5 i64.add global.set $value global.get $value))"#,
        64,
    )?;
    let run = instance.get_typed_func::<(), i64>(&mut store, "run")?;
    assert_eq!(run.call(&mut store, ())?, 15);
    assert_eq!(run.call(&mut store, ())?, 20);
    let remaining = instance
        .get_global(&mut store, REMAINING_EXPORT)
        .unwrap()
        .get(&mut store)
        .i64()
        .unwrap();
    assert_eq!(remaining, 54);
    Ok(())
}

#[test]
fn missing_sections_and_imported_globals_are_handled_without_aliasing() -> anyhow::Result<()> {
    let (mut store, instance) = synchronous_module("(module (func))", 64)?;
    let remaining = instance
        .get_global(&mut store, REMAINING_EXPORT)
        .unwrap()
        .get(&mut store)
        .i64()
        .unwrap();
    assert_eq!(remaining, 0);

    let engine = Engine::default();
    let bytes = wat::parse_str(
        r#"(module
        (import "host" "value" (global $value (mut i64)))
        (func (export "run") (result i64)
            global.get $value i64.const 5 i64.add global.set $value global.get $value))"#,
    )?;
    let module = Module::new(&engine, instrument(&bytes)?)?;
    let mut store = Store::new(&engine, ());
    let value = wasmtime::Global::new(
        &mut store,
        wasmtime::GlobalType::new(wasmtime::ValType::I64, wasmtime::Mutability::Var),
        wasmtime::Val::I64(10),
    )?;
    let mut linker = Linker::new(&engine);
    linker.define(&store, "host", "value", value)?;
    linker.func_wrap(MODULE, ADMIT, |_: i64, _: i64| 64_i64)?;
    let instance = linker.instantiate(&mut store, &module)?;
    assert_eq!(
        instance
            .get_typed_func::<(), i64>(&mut store, "run")?
            .call(&mut store, ())?,
        15
    );
    assert_eq!(value.get(&mut store).i64(), Some(15));
    let remaining = instance
        .get_global(&mut store, REMAINING_EXPORT)
        .unwrap()
        .get(&mut store)
        .i64()
        .unwrap();
    assert_eq!(remaining, 59);
    Ok(())
}
