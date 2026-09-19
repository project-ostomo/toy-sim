mod checkpoint;
mod execution;
mod imports;
mod metering;
mod session;
pub mod spatial;

pub use checkpoint::ControllerCheckpoint;
pub use session::Session;

use anyhow::{Context, Result, ensure};
use imports::imports;
use std::{
    collections::HashMap,
    mem::size_of,
    sync::{Arc, Mutex},
};
use toy_sim_ship_api::abi::{self as w, Record};
use toy_sim_ships::*;

mod computer;
pub use computer::*;
use screens::*;
pub use toy_sim_model::drawing as screens;

use wasmtime::{
    Caller, Config, Engine, Global, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};
pub const MEMORY_LIMIT: usize = 8 * 1024 * 1024;
pub const FUEL_PER_TICK: u64 = 1_000_000;
pub const GAS_PER_SECOND: u64 = FUEL_PER_TICK * 10;
pub const BOOT_GAS: u64 = FUEL_PER_TICK * 50;
pub const MAX_BOOTS_PER_TICK: usize = 64;
/// Shared immutable scene access, called only after a successful scan admission.
pub trait ScanSource: Send + Sync {
    fn scan(&self, range_m: f64, n: usize) -> Vec<SensorContact>;
    fn query_work(&self, query: &toy_sim_model::ProgramQuery) -> Result<u64> {
        Ok(query_work(query))
    }

    fn query(
        &self,
        _query: toy_sim_model::ProgramQuery,
        _display: bool,
        _reply_capacity: usize,
    ) -> Result<toy_sim_model::ProgramReply> {
        anyhow::bail!("world service unavailable")
    }
}

struct Host {
    persistent_data: Vec<u8>,
    display_only: bool,
    callback: Option<CallbackKind>,
    missile: Option<w::MissileObservation>,
    working: Session,
    current: spatial::Snapshot,
    sequence: u64,
    observation_sequence: u64,
    borrowed_snapshot: Option<spatial::Snapshot>,
    specs: Arc<[Vec<u8>]>,
    resources: Arc<[w::ResourceInfo]>,
    events: Vec<w::ScreenEvent>,
    event_acks: Vec<u64>,
    drafts: std::collections::BTreeMap<u64, imports::ScreenDraft>,
    limits: StoreLimits,
    memory: Option<Memory>,
    input: Option<Input>,
    output: Output,
    source: Option<Arc<dyn ScanSource>>,
    contacts: Vec<SensorContact>,
    scan_time: Option<f64>,
    catalogue: Arc<[DeviceDescriptor]>,
    gas_limit: u64,
    gas_per_tick: u64,
    native_credit: u64,
    scan_seconds: f64,
    interest: u64,
    screens: Vec<w::ScreenDefinition>,
    exchange: Arc<Mutex<execution::Exchange>>,
    remaining: Option<Global>,
    initial_remaining: Option<u64>,
    prepared_query: Option<imports::PreparedWorldQuery>,
}

struct Machine {
    store: Store<Host>,
    tick: TypedFunc<(), ()>,
    missile_tick: Option<TypedFunc<u64, ()>>,
    remaining: Global,
}

pub struct Controller {
    program: Arc<[u8]>,
    persistent_data: Vec<u8>,
    pub restart_revision: u64,
    pub last_gas_used: u64,
    display_only: bool,
    pub state: Session,
    pub observer_origin: spatial::Position,
    pub catalogue: Arc<[DeviceDescriptor]>,
    pub device_specs: Arc<[Vec<u8>]>,
    pub resource_specs: Arc<[w::ResourceInfo]>,
    pending_requests: Vec<Request>,
    pending_events: Vec<w::ScreenEvent>,
    module: Arc<Module>,
    engine: Engine,
    linker: Linker<Host>,
    machine: Option<Machine>,
    execution: Mutex<Option<execution::Pending>>,
    callback: Option<CallbackKind>,
    exchange: Arc<Mutex<execution::Exchange>>,
    boot_remaining: u64,
    initialized: bool,
    waiting_for_gas: bool,
    memory_bytes: usize,
    pub contacts: Vec<SensorContact>,
    pub scan_time: Option<f64>,
    pub trajectory_revision: u64,
    pub fault: Option<String>,
    pub last_scan_seconds: f64,
    pub telemetry: Option<Observation>,
    pub screens: Vec<w::ScreenDefinition>,
    pub instrument_interest: u64,
}

#[derive(Debug, Default)]
pub struct SliceOutput {
    pub output: Output,
    pub callback_completed: bool,
    pub callback: Option<CallbackKind>,
}

impl Controller {
    pub fn is_booting(&self) -> bool {
        !self.initialized
    }

    pub fn pending_callback(&self) -> Option<CallbackKind> {
        self.callback
    }

    pub fn supports_missiles(&self) -> bool {
        !self.display_only && self.module.get_export("missile_tick").is_some()
    }

    pub fn needs_instance_start(&self) -> bool {
        !self.initialized && self.execution.lock().unwrap().is_none()
    }

    pub fn boot_remaining_gas(&self) -> u64 {
        self.boot_remaining
    }

    pub fn boot_progress(&self) -> f64 {
        1. - self.boot_remaining as f64 / BOOT_GAS as f64
    }

    pub fn is_suspended(&self) -> bool {
        self.initialized && self.execution.lock().unwrap().is_some()
    }

    pub fn execution_status(&self) -> toy_sim_model::ExecutionStatus {
        if self.waiting_for_gas {
            toy_sim_model::ExecutionStatus::WaitingForGas
        } else if self.is_suspended() {
            toy_sim_model::ExecutionStatus::Suspended
        } else {
            toy_sim_model::ExecutionStatus::Ready
        }
    }

    pub fn minimum_to_progress(&self) -> u64 {
        if self.boot_remaining > 0 {
            1
        } else {
            self.exchange.lock().unwrap().minimum.max(1)
        }
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes
    }

    pub fn revoke_authority(&mut self) {
        self.reboot();
    }

    pub fn reboot(&mut self) {
        self.restart_revision = self.restart_revision.wrapping_add(1);
        self.pending_requests.clear();
        self.pending_events.clear();
        self.execution.get_mut().unwrap().take();
        self.callback = None;
        self.machine = None;
        self.exchange = Arc::default();
        self.boot_remaining = BOOT_GAS;
        self.initialized = false;
        self.waiting_for_gas = false;
        self.memory_bytes = 0;
        self.state = Session::default();
        self.contacts.clear();
        self.scan_time = None;
        self.trajectory_revision = self.trajectory_revision.wrapping_add(1);
        self.telemetry = None;
        self.screens.clear();
    }

    pub fn fail(&mut self, message: String) {
        self.reboot();
        self.fault = Some(message);
    }

    pub fn run_slice(
        &mut self,
        input: Input,
        source: Option<Arc<dyn ScanSource>>,
        grant: u64,
        gas_per_tick: u64,
    ) -> Result<SliceOutput> {
        let kind = if self.display_only {
            CallbackKind::Display
        } else {
            CallbackKind::Ship
        };
        self.run_callback_slice(kind, input, source, None, grant, gas_per_tick)
    }

    pub fn run_callback_slice(
        &mut self,
        kind: CallbackKind,
        mut input: Input,
        source: Option<Arc<dyn ScanSource>>,
        missile: Option<w::MissileObservation>,
        grant: u64,
        gas_per_tick: u64,
    ) -> Result<SliceOutput> {
        self.last_gas_used = 0;
        self.last_scan_seconds = 0.;
        ensure!(
            self.callback.is_none_or(|pending| pending == kind),
            "callback kind does not match suspended execution"
        );
        match kind {
            CallbackKind::Missile(handle) => {
                ensure!(
                    self.supports_missiles(),
                    "program has no missile_tick callback"
                );
                ensure!(
                    handle != 0 && missile.is_some_and(|value| value.handle == handle),
                    "missile observation handle does not match callback"
                );
            }
            CallbackKind::Ship => {
                ensure!(
                    !self.display_only && missile.is_none(),
                    "invalid ship callback context"
                );
            }
            CallbackKind::Display => {
                ensure!(
                    self.display_only && missile.is_none(),
                    "invalid display callback context"
                );
            }
        }
        ensure!(
            gas_per_tick > 0 && grant <= gas_per_tick,
            "invalid computer gas grant"
        );
        ensure!(
            input.devices.len() <= MAX_DEVICES
                && input
                    .requested_screens
                    .iter()
                    .all(|id| u32::from(*id) < w::MAX_SCREENS)
                && input.dt.is_finite()
                && input.dt >= 0.
                && input.physics_dt.is_finite()
                && input.physics_dt > 0.
                && input.observation.time_s.is_finite(),
            "controller input exceeds limits"
        );
        self.state.expire(input.observation.time_s);
        self.queue_input(
            std::mem::take(&mut input.commands),
            std::mem::take(&mut input.screen_events),
        )?;
        let new_callback = self.execution.get_mut().unwrap().is_none();
        if new_callback {
            input.commands.clone_from(&self.pending_requests);
            input.screen_events.clone_from(&self.pending_events);
        }
        self.waiting_for_gas = grant < self.minimum_to_progress();
        if self.waiting_for_gas {
            return Ok(SliceOutput {
                callback: self.callback,
                ..Default::default()
            });
        }

        let boot_charge = grant.min(self.boot_remaining);
        self.boot_remaining -= boot_charge;
        self.last_gas_used = boot_charge;
        let available = grant - boot_charge;
        if available == 0 {
            return Ok(SliceOutput {
                callback: self.callback,
                ..Default::default()
            });
        }
        let slice = execution::SliceInput {
            input,
            source,
            observer_origin: self.observer_origin,
            catalogue: self.catalogue.clone(),
            specs: self.device_specs.clone(),
            resources: self.resource_specs.clone(),
            interest: self.instrument_interest,
            grant: available,
            gas_per_tick,
            state: self.state.clone(),
            missile,
        };
        {
            let mut exchange = self.exchange.lock().unwrap();
            exchange.resume = Some(slice);
            exchange.remaining = available;
            exchange.minimum = 1;
            exchange.committed = None;
        }
        if new_callback {
            let pending = if self.initialized {
                self.callback = Some(kind);
                execution::callback(
                    self.machine.take().expect("ready computer has machine"),
                    kind,
                )
            } else {
                execution::initialize(
                    self.engine.clone(),
                    self.linker.clone(),
                    self.module.clone(),
                    self.display_only,
                    self.exchange.clone(),
                    self.persistent_data.clone(),
                )
            };
            *self.execution.get_mut().unwrap() = Some(pending);
        }
        let poll = self.execution.get_mut().unwrap().as_mut().unwrap().poll();
        let (remaining, commit, minimum) = {
            let mut exchange = self.exchange.lock().unwrap();
            (
                exchange.remaining,
                exchange.committed.take(),
                exchange.minimum,
            )
        };
        ensure!(
            remaining <= available,
            "computer gas accounting exceeded grant"
        );
        self.last_gas_used += available - remaining;
        let mut result = SliceOutput {
            callback: self.callback,
            ..Default::default()
        };
        if let Some(commit) = commit {
            self.persistent_data = commit.persistent_data;
            self.state = commit.state;
            self.trajectory_revision = self.state.spatial.revision;
            self.screens.clone_from(&self.state.screens);
            self.memory_bytes = commit.memory_bytes;
            self.last_scan_seconds = commit.scan_seconds;
            self.telemetry = commit.observation;
            if let Some(time) = commit.scan_time {
                self.scan_time = Some(time);
                self.contacts = commit.contacts;
            }
            self.pending_requests.retain(|request| {
                !commit
                    .output
                    .replies
                    .iter()
                    .any(|reply| reply.id == request.id)
            });
            self.pending_events
                .retain(|event| !commit.event_acks.contains(&event.id));
            result.output = commit.output;
        }
        match poll {
            std::task::Poll::Pending => {
                ensure!(
                    minimum > remaining,
                    "computer suspended despite an affordable operation"
                );
            }
            std::task::Poll::Ready(completion) => {
                self.execution.get_mut().unwrap().take();
                match completion {
                    Ok(machine) => {
                        result.callback_completed = self.initialized;
                        self.callback = None;
                        self.machine = Some(machine);
                        self.initialized = true;
                        self.fault = None;
                    }
                    Err(error) => {
                        self.fail(format!("{error:#}"));
                        return Err(error);
                    }
                }
            }
        }
        Ok(result)
    }
}

pub struct ControllerRuntime {
    engine: Engine,
    linker: Linker<Host>,
    modules: HashMap<Vec<u8>, Arc<Module>>,
}

impl Default for ControllerRuntime {
    fn default() -> Self {
        Self::new().expect("WASM runtime")
    }
}

impl ControllerRuntime {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.max_wasm_stack(128 * 1024);
        let engine = Engine::new(&config)?;
        let mut linker = imports(&engine)?;
        execution::register(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            modules: HashMap::new(),
        })
    }

    pub fn compile(&mut self, bytes: &[u8]) -> Result<Arc<Module>> {
        ensure!(bytes.len() <= 1024 * 1024, "controller exceeds 1 MiB");
        if let Some(module) = self.modules.get(bytes) {
            return Ok(module.clone());
        }
        let original = Module::new(&self.engine, bytes)?;
        validate_version(bytes)?;
        for import in original.imports() {
            ensure!(
                import.module() == w::IMPORT_MODULE && w::IMPORTS.contains(&import.name()),
                "unsupported controller import {}.{}",
                import.module(),
                import.name()
            );
        }
        self.linker.instantiate_pre(&original)?;
        ensure!(
            original.exports().any(|export| export.name() == "memory"),
            "controller must export memory"
        );
        for (name, params, results) in [("ship_tick", 0, 0), ("ship_api_version", 0, 1)] {
            let ty = original
                .exports()
                .find(|export| export.name() == name)
                .and_then(|export| export.ty().func().cloned())
                .with_context(|| format!("missing function {name}"))?;
            ensure!(
                ty.params().len() == params && ty.results().len() == results,
                "unsupported {name} signature"
            );
        }
        if let Some(export) = original.get_export("missile_tick") {
            let ty = export.func().context("missile_tick must be a function")?;
            ensure!(
                ty.params().len() == 1
                    && matches!(ty.params().next(), Some(wasmtime::ValType::I64))
                    && ty.results().len() == 0,
                "missile_tick must accept one i64 handle and return nothing"
            );
        }
        for export in original.exports() {
            if let Some(memory) = export.ty().memory() {
                ensure!(
                    !memory.is_64()
                        && !memory.is_shared()
                        && memory.minimum() <= (MEMORY_LIMIT / 65536) as u64,
                    "unsupported controller memory"
                );
            }
        }
        let instrumented = metering::instrument(bytes)?;
        let module = Arc::new(Module::new(&self.engine, instrumented)?);
        self.linker.instantiate_pre(&module)?;
        self.modules.insert(bytes.to_vec(), module.clone());
        Ok(module)
    }

    pub fn validate_program(&mut self, bytes: &[u8]) -> Result<()> {
        self.compile(bytes)?;
        Ok(())
    }

    pub fn instantiate(&mut self, bytes: &[u8]) -> Result<Controller> {
        Ok(Controller {
            program: Arc::from(bytes),
            persistent_data: Vec::new(),
            restart_revision: 0,
            last_gas_used: 0,
            display_only: false,
            state: Session::default(),
            observer_origin: [0; 3],
            catalogue: Arc::default(),
            device_specs: Arc::default(),
            resource_specs: Arc::default(),
            pending_requests: Vec::new(),
            pending_events: Vec::new(),
            module: self.compile(bytes)?,
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            machine: None,
            execution: Mutex::new(None),
            callback: None,
            exchange: Arc::default(),
            boot_remaining: BOOT_GAS,
            initialized: false,
            waiting_for_gas: false,
            memory_bytes: 0,
            contacts: Vec::new(),
            scan_time: None,
            trajectory_revision: 0,
            fault: None,
            last_scan_seconds: 0.,
            telemetry: None,
            screens: Vec::new(),
            instrument_interest: 0,
        })
    }

    pub fn cached_modules(&self) -> usize {
        self.modules.len()
    }

    pub fn instantiate_display(&mut self, bytes: &[u8]) -> Result<Controller> {
        let mut computer = self.instantiate(bytes)?;
        ensure!(
            computer.module.get_export("ship_display").is_some(),
            "program has no display entry point"
        );
        computer.display_only = true;
        Ok(computer)
    }
}

fn validate_version(bytes: &[u8]) -> Result<()> {
    use wasmparser::{ExternalKind, Operator, Parser, Payload, TypeRef};
    let mut imported_functions = 0;
    let mut version_function = None;
    let mut body_index = 0;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    if matches!(import?.ty, TypeRef::Func(_)) {
                        imported_functions += 1;
                    }
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export?;
                    if export.name == "ship_api_version" && export.kind == ExternalKind::Func {
                        version_function = Some(export.index);
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if version_function == Some(imported_functions + body_index) {
                    ensure!(
                        body.get_locals_reader()?.get_count() == 0,
                        "ship_api_version must be a literal i32 constant"
                    );
                    let mut operators = body.get_operators_reader()?;
                    let Operator::I32Const { value } = operators.read()? else {
                        anyhow::bail!("ship_api_version must be a literal i32 constant");
                    };
                    ensure!(
                        matches!(operators.read()?, Operator::End) && operators.eof(),
                        "ship_api_version must be a literal i32 constant"
                    );
                    ensure!(
                        value == w::VERSION as i32,
                        "unsupported ship controller API {value}; expected {}",
                        w::VERSION
                    );
                    return Ok(());
                }
                body_index += 1;
            }
            _ => {}
        }
    }
    anyhow::bail!("missing literal ship_api_version function")
}

fn range(c: &Caller<'_, Host>, ptr: u32, len: u32) -> Option<std::ops::Range<usize>> {
    let end = (ptr as usize).checked_add(len as usize)?;
    if end > c.data().memory?.data_size(c) {
        return None;
    }
    Some(ptr as usize..end)
}
fn read<T: Record>(c: &Caller<'_, Host>, ptr: u32, len: u32) -> Option<T> {
    let r = range(c, ptr, len)?;
    T::read(&c.data().memory?.data(c)[r])
}
fn put<T: Record>(c: &mut Caller<'_, Host>, ptr: u32, len: u32, value: &T) -> i32 {
    if len as usize != size_of::<T>() {
        return w::ERR_BUFFER;
    }
    let Some(r) = range(c, ptr, size_of::<T>() as u32) else {
        return w::ERR_BUFFER;
    };
    let mem = c.data().memory.unwrap();
    mem.data_mut(c)[r].copy_from_slice(value.bytes());
    0
}
/// Host-side callback pacing. Advance once per simulation tick, never per frame.
/// Only completed callbacks start a delay; gas refill and booting run independently.
#[derive(Clone, Debug, Default)]
pub struct CallbackSchedule {
    interval: f64,
    remaining: f64,
}
impl CallbackSchedule {
    pub fn advance(&mut self, dt: f64) {
        self.remaining = (self.remaining - dt).max(0.);
    }
    pub fn ready(&self, has_commands: bool) -> bool {
        has_commands || self.remaining <= 1e-9
    }
    pub fn completed(&mut self, interval: Option<f64>) {
        if let Some(interval) = interval {
            assert!(interval.is_finite() && interval >= 0.);
            self.interval = interval;
        }
        self.remaining = self.interval;
    }
}
