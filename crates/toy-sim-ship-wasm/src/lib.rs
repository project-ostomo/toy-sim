//! Synchronous, metered ship imports. Faults discard staged outputs and cold-boot automatically.
mod checkpoint;
pub use checkpoint::ControllerCheckpoint;
mod imports;
mod session;
pub mod spatial;

use anyhow::{Context, Result, ensure};
use imports::imports;
pub use session::Session;
use std::{collections::HashMap, mem::size_of, sync::Arc};
use toy_sim_ship_api::abi::{self as w, Record};
use toy_sim_ships::*;

mod computer;
pub use computer::*;
use screens::*;
pub use toy_sim_model::drawing as screens;

use wasmtime::{
    Caller, Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};

pub const MEMORY_LIMIT: usize = 8 * 1024 * 1024;
pub const FUEL_PER_TICK: u64 = 1_000_000;
pub const GAS_PER_SECOND: u64 = FUEL_PER_TICK * 10;
pub const RESERVE_CAPACITY: u64 = FUEL_PER_TICK * 4;
/// Leave one tick allowance for native calls as well as the instruction limit.
pub const CALLBACK_START_GAS: u64 = FUEL_PER_TICK * 2;
pub const BOOT_GAS: u64 = FUEL_PER_TICK * 50;
pub const MAX_BOOTS_PER_TICK: usize = 64;
/// Shared immutable scene access, called only after a successful scan admission.
pub trait ScanSource: Send + Sync {
    fn scan(&self, range_m: f64, n: usize) -> Vec<SensorContact>;
    fn query(
        &self,
        _query: toy_sim_model::ProgramQuery,
        _display: bool,
    ) -> Result<toy_sim_model::ProgramReply> {
        anyhow::bail!("world service unavailable")
    }
}
struct Host {
    persistent_data: Vec<u8>,
    display_only: bool,
    working: Session,
    current: spatial::Snapshot,
    sequence: u64,
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
    gas: u64,
    last_fuel: u64,
    scan_seconds: f64,
    interest: u64,
    screens: Vec<w::ScreenDefinition>,
}
struct Machine {
    store: Store<Host>,
    tick: TypedFunc<(), ()>,
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
    machine: Option<Machine>,
    gas: u64,
    fractional_gas: f64,
    pub contacts: Vec<SensorContact>,
    pub scan_time: Option<f64>,
    /// Native publication identity; never exposed through guest memory.
    pub trajectory_revision: u64,
    pub fault: Option<String>,
    /// Native scene query time inside the most recent callback (part of run time).
    pub last_scan_seconds: f64,
    pub telemetry: Option<Observation>,
    pub screens: Vec<w::ScreenDefinition>,
    /// Client subscriptions only affect optional trajectory generation, never control behavior.
    pub instrument_interest: u64,
}
impl Controller {
    pub fn is_booting(&self) -> bool {
        self.machine.is_none()
    }
    pub fn gas_remaining(&self) -> u64 {
        self.gas
    }
    pub fn boot_progress(&self) -> f64 {
        if self.is_booting() {
            self.gas as f64 / BOOT_GAS as f64
        } else {
            1.
        }
    }
    pub fn can_run(&self) -> bool {
        !self.is_booting() && self.gas >= CALLBACK_START_GAS
    }
    pub fn memory_bytes(&self) -> usize {
        self.machine
            .as_ref()
            .and_then(|m| m.store.data().memory.map(|mem| mem.data_size(&m.store)))
            .unwrap_or(0)
    }
    /// Called once per powered physics tick, even when the script is sleeping or rebooting.
    pub fn advance(&mut self, dt: f64) {
        if !dt.is_finite() || dt <= 0. {
            return;
        }
        let cap = if self.is_booting() {
            BOOT_GAS
        } else {
            RESERVE_CAPACITY
        };
        let gain = (dt * GAS_PER_SECOND as f64 + self.fractional_gas).min(cap as f64);
        self.fractional_gas = gain.fract();
        self.gas = self.gas.saturating_add(gain.floor() as u64).min(cap);
    }
    pub fn revoke_authority(&mut self) {
        self.reboot();
    }

    pub fn reboot(&mut self) {
        self.restart_revision = self.restart_revision.wrapping_add(1);
        self.pending_requests.clear();
        self.machine = None;
        self.gas = 0;
        self.fractional_gas = 0.;
        self.state = Session::default();
        self.pending_events.clear();
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
    fn failed(&mut self, error: &anyhow::Error) {
        self.reboot();
        self.fault = Some(format!("{error:#}"));
    }
    /// Returns None while booting or accumulating an execution allowance. No saved stack exists.
    pub fn run(&mut self, input: Input) -> Result<Option<Output>> {
        self.run_with_scan(input, None)
    }
    pub fn run_with_scan(
        &mut self,
        mut input: Input,
        source: Option<Arc<dyn ScanSource>>,
    ) -> Result<Option<Output>> {
        self.last_scan_seconds = 0.;
        self.last_gas_used = 0;
        self.state.expire(input.observation.time_s);
        self.queue_input(
            std::mem::take(&mut input.commands),
            std::mem::take(&mut input.screen_events),
        )?;
        if !self.can_run() {
            return Ok(None);
        }
        input.commands = self.pending_requests.clone();
        let result = (|| -> Result<Output> {
            ensure!(
                input.commands.len() <= 256
                    && input.devices.len() <= MAX_DEVICES
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
            let machine = self.machine.as_mut().unwrap();
            let fuel = self.gas.min(FUEL_PER_TICK);
            machine.store.set_fuel(fuel)?;
            let host = machine.store.data_mut();
            host.gas = self.gas;
            host.persistent_data.clone_from(&self.persistent_data);
            host.sequence += 1;
            host.current =
                spatial::Snapshot::new(host.sequence, self.observer_origin, &input.observation);
            host.working = self.state.clone();
            host.specs = self.device_specs.clone();
            host.resources = self.resource_specs.clone();
            host.events.clone_from(&self.pending_events);
            host.event_acks.clear();
            host.drafts.clear();
            host.interest = self.instrument_interest;
            host.screens.clone_from(&self.screens);
            host.catalogue.clone_from(&self.catalogue);
            host.last_fuel = fuel;
            host.input = Some(input);
            host.scan_time = None;
            host.scan_seconds = 0.;
            host.output = Output::default();
            host.source = source;
            host.contacts.clear();
            let result = machine.tick.call(&mut machine.store, ());
            // Guest execution and native syscalls share the persistent account; instruction
            // fuel separately caps one callback. No rollback/reuse of a trapped guest stack.
            let remaining = machine.store.get_fuel()?;
            let host = machine.store.data_mut();
            host.gas = host
                .gas
                .saturating_sub(host.last_fuel.saturating_sub(remaining));
            self.last_gas_used = self.gas.saturating_sub(host.gas);
            self.gas = host.gas;
            self.last_scan_seconds = host.scan_seconds;
            let observation = host.input.take().map(|i| i.observation);
            host.source = None;
            result?;
            self.persistent_data.clone_from(&host.persistent_data);
            let output = std::mem::take(&mut host.output);

            if let Some(time) = host.scan_time.take() {
                self.scan_time = Some(time);
                self.contacts = std::mem::take(&mut host.contacts);
            }

            self.pending_requests
                .retain(|request| !output.replies.iter().any(|reply| reply.id == request.id));
            self.pending_events
                .retain(|event| !host.event_acks.contains(&event.id));
            self.state = std::mem::take(&mut host.working);
            self.trajectory_revision = self.state.spatial.revision;
            self.telemetry = observation;
            self.screens.clone_from(&self.state.screens);

            Ok(output)
        })();
        match result {
            Ok(out) => Ok(Some(out)),
            Err(e) => {
                self.failed(&e);
                Err(e)
            }
        }
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
        let mut c = Config::new();
        c.consume_fuel(true);
        c.max_wasm_stack(128 * 1024);
        let engine = Engine::new(&c)?;
        let linker = imports(&engine)?;
        Ok(Self {
            engine,
            linker,
            modules: HashMap::new(),
        })
    }
    pub fn compile(&mut self, bytes: &[u8]) -> Result<Arc<Module>> {
        ensure!(bytes.len() <= 1024 * 1024, "controller exceeds 1 MiB");
        if let Some(m) = self.modules.get(bytes) {
            return Ok(m.clone());
        }
        let m = Module::new(&self.engine, bytes)?;
        for import in m.imports() {
            ensure!(
                import.module() == w::IMPORT_MODULE && w::IMPORTS.contains(&import.name()),
                "unsupported controller import {}.{}",
                import.module(),
                import.name()
            );
        }
        // Resolve types without allocating or starting a guest instance.
        self.linker.instantiate_pre(&m)?;
        ensure!(
            m.exports().any(|e| e.name() == "memory"),
            "controller must export memory"
        );
        for (name, params, results) in [("ship_tick", 0, 0), ("ship_api_version", 0, 1)] {
            let ty = m
                .exports()
                .find(|e| e.name() == name)
                .and_then(|e| e.ty().func().cloned())
                .with_context(|| format!("missing function {name}"))?;
            ensure!(
                ty.params().len() == params && ty.results().len() == results,
                "unsupported {name} signature"
            );
        }
        for export in m.exports() {
            if let Some(mem) = export.ty().memory() {
                ensure!(
                    !mem.is_64()
                        && !mem.is_shared()
                        && mem.minimum() <= (MEMORY_LIMIT / 65536) as u64,
                    "unsupported controller memory"
                );
            }
        }
        let module = Arc::new(m);
        self.modules.insert(bytes.to_vec(), module.clone());
        Ok(module)
    }
    /// Offline editor/import preflight, including the executable ABI-version export.
    /// Simulation startup still goes through the separately metered boot path.
    pub fn validate_program(&mut self, bytes: &[u8]) -> Result<()> {
        let module = self.compile(bytes)?;
        Machine::new(&self.engine, &self.linker, &module, false)?;
        Ok(())
    }
    /// Compiles/validates the program, but leaves the computer in its initial 50-tick boot.
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
            machine: None,
            gas: 0,
            fractional_gas: 0.,
            contacts: vec![],
            scan_time: None,
            trajectory_revision: 0,
            fault: None,
            last_scan_seconds: 0.,
            telemetry: None,
            screens: vec![],
            instrument_interest: 0,
        })
    }
    /// The caller caps attempts per physics tick. The full startup charge is spent even on failure.
    pub fn boot(&self, controller: &mut Controller) -> Result<bool> {
        if !controller.is_booting() || controller.gas < BOOT_GAS {
            return Ok(false);
        }
        controller.gas -= BOOT_GAS;
        match Machine::new(
            &self.engine,
            &self.linker,
            &controller.module,
            controller.display_only,
        ) {
            Ok(machine) => {
                controller.machine = Some(machine);
                controller.fault = None;
                Ok(true)
            }
            Err(e) => {
                controller.failed(&e);
                Err(e)
            }
        }
    }
    pub fn cached_modules(&self) -> usize {
        self.modules.len()
    }

    pub fn instantiate_display(&mut self, bytes: &[u8]) -> Result<Controller> {
        let mut controller = self.instantiate(bytes)?;
        ensure!(
            controller.module.get_export("ship_display").is_some(),
            "program has no display entry point"
        );
        controller.display_only = true;
        controller.gas = BOOT_GAS;
        self.boot(&mut controller)?;
        controller.gas = CALLBACK_START_GAS;
        Ok(controller)
    }
}
impl Machine {
    fn new(
        engine: &Engine,
        linker: &Linker<Host>,
        module: &Module,
        display_only: bool,
    ) -> Result<Self> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .memories(1)
            .instances(1)
            .table_elements(4096)
            .build();
        let mut store = Store::new(
            engine,
            Host {
                persistent_data: Vec::new(),
                display_only,
                working: Session::default(),
                current: spatial::Snapshot::default(),
                sequence: 0,
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
                contacts: vec![],
                scan_time: None,
                catalogue: Arc::default(),
                gas: FUEL_PER_TICK,
                last_fuel: FUEL_PER_TICK,
                scan_seconds: 0.,
                interest: 0,
                screens: vec![],
            },
        );
        store.limiter(|h| &mut h.limits);
        store.set_fuel(FUEL_PER_TICK)?;
        let instance: Instance = linker.instantiate(&mut store, module)?;
        let version = instance
            .get_typed_func::<(), u32>(&mut store, "ship_api_version")?
            .call(&mut store, ())?;
        ensure!(
            version == w::VERSION,
            "unsupported ship controller API {version}; expected {}",
            w::VERSION
        );
        store.data_mut().memory = Some(
            instance
                .get_memory(&mut store, "memory")
                .context("missing guest memory")?,
        );
        let tick = instance.get_typed_func(
            &mut store,
            if display_only {
                "ship_display"
            } else {
                "ship_tick"
            },
        )?;
        Ok(Self { store, tick })
    }
}
fn charge(c: &mut Caller<'_, Host>, cost: u64) -> bool {
    let Ok(fuel) = c.get_fuel() else {
        return false;
    };
    let h = c.data_mut();
    h.gas = h.gas.saturating_sub(h.last_fuel.saturating_sub(fuel));
    h.last_fuel = fuel;
    if h.gas < cost {
        return false;
    }
    h.gas -= cost;
    let next = fuel.min(h.gas);
    h.last_fuel = next;
    c.set_fuel(next).is_ok()
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
