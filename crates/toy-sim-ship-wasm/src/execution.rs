use super::*;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::{Context as TaskContext, Poll, Waker},
};

pub(super) struct SliceInput {
    pub input: Input,
    pub source: Option<Arc<dyn ScanSource>>,
    pub observer_origin: spatial::Position,
    pub catalogue: Arc<[DeviceDescriptor]>,
    pub specs: Arc<[Vec<u8>]>,
    pub resources: Arc<[w::ResourceInfo]>,
    pub interest: u64,
    pub grant: u64,
    pub gas_per_tick: u64,
    pub state: Session,
}

#[derive(Default)]
pub(super) struct Exchange {
    pub resume: Option<SliceInput>,
    pub remaining: u64,
    pub minimum: u64,
    pub committed: Option<Commit>,
}

pub(super) struct Commit {
    pub persistent_data: Vec<u8>,
    pub state: Session,
    pub output: Output,
    pub event_acks: Vec<u64>,
    pub observation: Option<Observation>,
    pub memory_bytes: usize,
    pub scan_seconds: f64,
    pub scan_time: Option<f64>,
    pub contacts: Vec<SensorContact>,
}

pub(super) struct Pending {
    future: Pin<Box<dyn Future<Output = Result<Machine>> + Send>>,
}

impl Pending {
    pub fn poll(&mut self) -> Poll<Result<Machine>> {
        self.future
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop()))
    }
}

fn refresh(host: &mut Host, mut slice: SliceInput, new_callback: bool) {
    if new_callback {
        host.working = slice.state;
        host.borrowed_snapshot = None;
        host.events = std::mem::take(&mut slice.input.screen_events);
        host.drafts.clear();
        host.screens.clone_from(&host.working.screens);
    } else if let Some(previous) = &host.input {
        slice.input.commands.clone_from(&previous.commands);
        slice
            .input
            .requested_screens
            .clone_from(&previous.requested_screens);
        slice.input.dt = previous.dt;
    }
    host.observation_sequence += 1;
    host.current = spatial::Snapshot::new(
        host.observation_sequence,
        slice.observer_origin,
        &slice.input.observation,
    );
    host.working.expire(slice.input.observation.time_s);
    host.input = Some(slice.input);
    host.source = slice.source;
    host.catalogue = slice.catalogue;
    host.specs = slice.specs;
    host.resources = slice.resources;
    host.interest = slice.interest;
    host.gas_limit = slice.grant;
    host.gas_per_tick = slice.gas_per_tick;
    host.native_credit = 0;
    host.scan_seconds = 0.;
    host.scan_time = None;
}

fn commit(host: &mut Host, memory_bytes: usize) -> Commit {
    Commit {
        persistent_data: host.persistent_data.clone(),
        state: host.working.clone(),
        output: std::mem::take(&mut host.output),
        event_acks: std::mem::take(&mut host.event_acks),
        observation: host.input.as_ref().map(|input| input.observation.clone()),
        memory_bytes,
        scan_seconds: std::mem::take(&mut host.scan_seconds),
        scan_time: host.scan_time.take(),
        contacts: host.contacts.clone(),
    }
}

fn global(caller: &mut Caller<'_, Host>) -> Result<Global> {
    if let Some(global) = caller.data().remaining {
        return Ok(global);
    }
    let global = caller
        .get_export(metering::REMAINING_EXPORT)
        .and_then(|export| export.into_global())
        .context("missing private gas counter")?;
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory());
    let host = caller.data_mut();
    host.remaining = Some(global);
    host.memory = memory;
    Ok(global)
}

pub(super) fn remaining(caller: &mut Caller<'_, Host>) -> Result<u64> {
    let global = global(caller)?;
    u64::try_from(
        global
            .get(caller)
            .i64()
            .context("invalid private gas counter")?,
    )
    .context("negative private gas counter")
}

fn set_remaining(caller: &mut Caller<'_, Host>, value: u64) -> Result<()> {
    let global = global(caller)?;
    Ok(global.set(caller, wasmtime::Val::I64(i64::try_from(value)?))?)
}

pub(super) fn refund(caller: &mut Caller<'_, Host>, unused: u64) -> Result<()> {
    let remaining = remaining(caller)?;
    let refunded = remaining
        .checked_add(unused)
        .context("gas refund overflow")?;
    ensure!(
        refunded <= caller.data().gas_limit,
        "gas refund exceeds granted slice"
    );
    set_remaining(caller, refunded)
}

async fn suspend(caller: &mut Caller<'_, Host>, left: u64, required: u64) -> Result<()> {
    ensure!(
        required <= caller.data().gas_per_tick,
        "single operation requires {required} gas, exceeding computer capacity {}",
        caller.data().gas_per_tick
    );
    let memory_bytes = caller
        .data()
        .memory
        .map(|memory| memory.data_size(&*caller))
        .unwrap_or(0);
    let exchange = caller.data().exchange.clone();
    let published = commit(caller.data_mut(), memory_bytes);
    caller.data_mut().source = None;
    {
        let mut exchange = exchange.lock().unwrap();
        exchange.remaining = left;
        exchange.minimum = required;
        exchange.committed = Some(published);
    }
    let slice = poll_fn(|_| match exchange.lock().unwrap().resume.take() {
        Some(slice) => Poll::Ready(slice),
        None => Poll::Pending,
    })
    .await;
    let grant = slice.grant;
    refresh(caller.data_mut(), slice, false);
    set_remaining(caller, grant)?;
    Ok(())
}

pub(super) async fn admit(caller: &mut Caller<'_, Host>, cost: u64) -> Result<bool> {
    let left = remaining(caller)?;
    if let Some(after) = left.checked_sub(cost) {
        set_remaining(caller, after)?;
        Ok(true)
    } else {
        suspend(caller, left, cost).await?;
        Ok(false)
    }
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    linker.func_wrap_async(
        metering::MODULE,
        metering::ADMIT,
        |mut caller, (reported, required): (i64, i64)| {
            Box::new(async move {
                let result = async {
                    let required =
                        u64::try_from(required).context("invalid instruction gas cost")?;
                    if let Some(initial) = caller.data_mut().initial_remaining.take() {
                        set_remaining(&mut caller, initial)?;
                    } else {
                        ensure!(
                            remaining(&mut caller)? == u64::try_from(reported)?,
                            "invalid gas admission state"
                        );
                    }
                    loop {
                        let left = remaining(&mut caller)?;
                        if left >= required {
                            return Ok::<i64, anyhow::Error>(i64::try_from(left)?);
                        }
                        suspend(&mut caller, left, required).await?;
                    }
                }
                .await;
                result.map_err(wasmtime::Error::from_anyhow)
            })
        },
    )?;
    Ok(())
}

fn finish(store: &mut Store<Host>, successful: bool) -> Result<()> {
    let remaining = match store.data().remaining {
        Some(global) => u64::try_from(
            global
                .get(&mut *store)
                .i64()
                .context("invalid private gas counter")?,
        )?,
        None => store.data().initial_remaining.unwrap_or(0),
    };
    let exchange = store.data().exchange.clone();
    let memory_bytes = store
        .data()
        .memory
        .map(|memory| memory.data_size(&*store))
        .unwrap_or(0);
    let published = successful.then(|| commit(store.data_mut(), memory_bytes));
    store.data_mut().source = None;
    let mut exchange = exchange.lock().unwrap();
    exchange.remaining = remaining;
    exchange.minimum = 1;
    exchange.committed = published;
    Ok(())
}

pub(super) fn initialize(
    engine: Engine,
    linker: Linker<Host>,
    module: Arc<Module>,
    display_only: bool,
    exchange: Arc<Mutex<Exchange>>,
    persistent_data: Vec<u8>,
) -> Pending {
    Pending {
        future: Box::pin(async move {
            let slice = exchange
                .lock()
                .unwrap()
                .resume
                .take()
                .expect("initial slice");
            let initial = slice.grant;
            let limits = StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT)
                .memories(1)
                .instances(1)
                .table_elements(4096)
                .build();
            let mut host = Host {
                persistent_data,
                display_only,
                working: Session::default(),
                current: spatial::Snapshot::default(),
                sequence: 0,
                observation_sequence: 0,
                borrowed_snapshot: None,
                specs: Arc::default(),
                resources: Arc::default(),
                events: Vec::new(),
                event_acks: Vec::new(),
                drafts: Default::default(),
                limits,
                memory: None,
                input: None,
                output: Output::default(),
                source: None,
                contacts: Vec::new(),
                scan_time: None,
                catalogue: Arc::default(),
                gas_limit: initial,
                gas_per_tick: slice.gas_per_tick,
                native_credit: 0,
                scan_seconds: 0.,
                interest: 0,
                screens: Vec::new(),
                exchange: exchange.clone(),
                remaining: None,
                initial_remaining: Some(initial),
                prepared_query: None,
            };
            refresh(&mut host, slice, true);
            let mut store = Store::new(&engine, host);
            store.limiter(|host| &mut host.limits);
            let result = async {
                let instance: Instance = linker.instantiate_async(&mut store, &module).await?;
                let version = instance
                    .get_typed_func::<(), u32>(&mut store, "ship_api_version")?
                    .call_async(&mut store, ())
                    .await?;
                ensure!(
                    version == w::VERSION,
                    "unsupported ship controller API {version}; expected {}",
                    w::VERSION
                );
                let memory = instance
                    .get_memory(&mut store, "memory")
                    .context("missing guest memory")?;
                let remaining = instance
                    .get_global(&mut store, metering::REMAINING_EXPORT)
                    .context("missing private gas counter")?;
                store.data_mut().memory = Some(memory);
                store.data_mut().remaining = Some(remaining);
                let tick = instance.get_typed_func(
                    &mut store,
                    if display_only {
                        "ship_display"
                    } else {
                        "ship_tick"
                    },
                )?;
                Ok::<_, anyhow::Error>((tick, remaining))
            }
            .await;
            finish(&mut store, result.is_ok())?;
            let (tick, remaining) = result?;
            Ok(Machine {
                store,
                tick,
                remaining,
            })
        }),
    }
}

pub(super) fn callback(mut machine: Machine) -> Pending {
    Pending {
        future: Box::pin(async move {
            let exchange = machine.store.data().exchange.clone();
            let slice = exchange
                .lock()
                .unwrap()
                .resume
                .take()
                .expect("callback slice");
            let grant = slice.grant;
            machine.store.data_mut().sequence += 1;
            refresh(machine.store.data_mut(), slice, true);
            machine.remaining.set(
                &mut machine.store,
                wasmtime::Val::I64(i64::try_from(grant)?),
            )?;
            let result = machine.tick.call_async(&mut machine.store, ()).await;
            finish(&mut machine.store, result.is_ok())?;
            result?;
            Ok(machine)
        }),
    }
}
