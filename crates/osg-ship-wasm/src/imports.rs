#[macro_use]
mod admission;
pub(super) use admission::PreparedWorldQuery;
use admission::*;

mod beacons;
mod drawing;
mod instruments;
mod publications;
mod services;
mod world;

use super::*;

pub(super) struct ScreenDraft {
    frame: ScreenImage,
    payload_bytes: usize,
    draw_work: usize,
}

enum CallError {
    Status(i32),
    PricedStatus { status: i32, gas: u64 },
    Memory,
}

impl CallError {
    fn admission_cost(&self) -> u64 {
        match self {
            Self::PricedStatus { gas, .. } => *gas,
            Self::Status(_) | Self::Memory => w::CALL_GAS,
        }
    }

    fn priced(self, gas: u64) -> Self {
        match self {
            Self::Status(status) | Self::PricedStatus { status, .. } => {
                Self::PricedStatus { status, gas }
            }
            Self::Memory => Self::Memory,
        }
    }
}

impl From<i32> for CallError {
    fn from(status: i32) -> Self {
        Self::Status(status)
    }
}

type CallResult<T = ()> = std::result::Result<T, CallError>;

fn finish(result: CallResult<i32>) -> wasmtime::Result<i32> {
    match result {
        Ok(value) => Ok(value),
        Err(CallError::Status(status) | CallError::PricedStatus { status, .. }) => Ok(status),
        Err(CallError::Memory) => Err(wasmtime::Error::msg("host call outside guest memory")),
    }
}

fn status(result: CallResult) -> wasmtime::Result<i32> {
    finish(result.map(|()| 0))
}

fn memory_range(
    caller: &Caller<'_, Host>,
    pointer: u32,
    bytes: u32,
) -> CallResult<std::ops::Range<usize>> {
    range(caller, pointer, bytes).ok_or(CallError::Memory)
}

fn acknowledge(host: &mut Host, id: u64) {
    if let Err(index) = host.event_acks.binary_search(&id) {
        host.event_acks.insert(index, id);
    }
}

fn clear_screen(host: &mut Host, id: u64) {
    if !host.output.cleared_screens.contains(&id) {
        host.output.cleared_screens.push(id);
    }
}

fn input<T: Record>(caller: &mut Caller<'_, Host>, pointer: u32, bytes: u32) -> CallResult<T> {
    if bytes as usize != size_of::<T>() {
        return Err(w::ERR_BUFFER.into());
    }

    let value = T::read(payload(caller, pointer, bytes)?).ok_or(w::ERR_BUFFER)?;
    Ok(value)
}

fn emit<T: Record>(
    caller: &mut Caller<'_, Host>,
    pointer: u32,
    bytes: u32,
    value: &T,
) -> CallResult {
    emit_bytes(caller, pointer, bytes, value.bytes())
}

fn emit_bytes(caller: &mut Caller<'_, Host>, pointer: u32, bytes: u32, value: &[u8]) -> CallResult {
    if bytes as usize != value.len() {
        return Err(w::ERR_BUFFER.into());
    }

    let target = memory_range(caller, pointer, bytes)?;
    let memory = caller.data().memory.ok_or(w::ERR_UNAVAILABLE)?;
    memory.data_mut(caller)[target].copy_from_slice(value);
    Ok(())
}

fn payload<'a>(caller: &'a Caller<'_, Host>, pointer: u32, bytes: u32) -> CallResult<&'a [u8]> {
    let source = memory_range(caller, pointer, bytes)?;
    let memory = caller.data().memory.ok_or(w::ERR_UNAVAILABLE)?;
    Ok(&memory.data(caller)[source])
}

fn device_index(caller: &Caller<'_, Host>, id: u64) -> CallResult<usize> {
    let index = id
        .checked_sub(1)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(w::ERR_HANDLE)?;

    if index >= caller.data().catalogue.len() {
        return Err(w::ERR_HANDLE.into());
    }

    Ok(index)
}

fn finite(values: &[f64]) -> CallResult {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(w::ERR_ARGUMENT.into())
    }
}

fn fraction(value: f64) -> CallResult {
    if value.is_finite() && (0. ..=1.).contains(&value) {
        Ok(())
    } else {
        Err(w::ERR_ARGUMENT.into())
    }
}

fn lease_active(caller: &Caller<'_, Host>, until: f64) -> CallResult<bool> {
    if until.is_finite() {
        Ok(until > caller.data().current.epoch)
    } else {
        Err(w::ERR_ARGUMENT.into())
    }
}

pub(super) fn imports(engine: &Engine) -> Result<Linker<Host>> {
    let mut linker = Linker::new(engine);
    persistent(&mut linker)?;
    services::register(&mut linker)?;
    context(&mut linker)?;
    hardware(&mut linker)?;
    sensors(&mut linker)?;
    requests(&mut linker)?;
    instruments::register(&mut linker)?;
    publications::register(&mut linker)?;
    drawing::register(&mut linker)?;
    Ok(linker)
}

fn persistent(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "persistent_read",
        |mut caller: Caller<'_, Host>, pointer: u32, capacity: u32| {
            persistent_read_plan(&caller, pointer, capacity)
        },
        {
            let length = caller.data().persistent_data.len();
            if length > capacity as usize {
                return Ok(w::ERR_BUFFER);
            }
            let target = range(&caller, pointer, length as u32)
                .ok_or_else(|| wasmtime::Error::msg("persistent read outside guest memory"))?;
            let bytes = caller.data().persistent_data.clone();
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmtime::Error::msg("guest memory unavailable"))?;
            memory.data_mut(&mut caller)[target].copy_from_slice(&bytes);
            Ok(length as i32)
        },
    )?;
    metered!(
        linker,
        "persistent_write",
        |mut caller: Caller<'_, Host>, pointer: u32, length: u32| {
            CallPlan::bytes(&caller, pointer, length, 65536)
        },
        {
            if caller.data().display_only {
                return Ok(w::ERR_UNSUPPORTED);
            }
            if length > 65536 {
                return Ok(w::ERR_LIMIT);
            }
            let source = range(&caller, pointer, length)
                .ok_or_else(|| wasmtime::Error::msg("persistent write outside guest memory"))?;
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmtime::Error::msg("guest memory unavailable"))?;
            let bytes = memory.data(&caller)[source].to_vec();
            caller.data_mut().persistent_data = bytes;
            Ok(0)
        },
    )?;
    Ok(())
}

fn context(linker: &mut Linker<Host>) -> Result<()> {
    world::register(linker)?;
    beacons::register(linker)?;
    metered!(
        linker,
        "serial_write",
        |mut caller: Caller<'_, Host>, pointer: u32, length: u32| {
            CallPlan::bytes(&caller, pointer, length, 1024)
                .map(|plan| plan.work(length as u64 * 256))
        },
        {
            status((|| {
                if caller.data().display_only {
                    return Err(w::ERR_UNAVAILABLE.into());
                }
                let bytes = payload(&caller, pointer, length)?.to_vec();
                caller.data_mut().working.serial.write(&bytes);
                Ok(())
            })())
        },
    )?;
    metered!(
        linker,
        "tick_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::TickContext>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let host = caller.data();
                let input = host.input.as_ref().unwrap();
                let value = w::TickContext {
                    tick: input.tick,
                    snapshot: host.current.id,
                    time_s: input.observation.time_s,
                    dt_s: input.dt,
                    physics_dt_s: input.physics_dt,
                    device_count: host.catalogue.len() as u64,
                    resource_count: input.observation.inventory.len() as u64,
                    request_count: input.commands.len() as u64,
                    screen_event_count: host.events.len() as u64,
                    interest: host.interest,
                    requested_screens: input
                        .requested_screens
                        .iter()
                        .fold(0, |mask, id| mask | 1 << id),
                    flags: u64::from(host.sequence == 1),
                };
                let snapshot = host.current;
                emit(&mut caller, pointer, bytes, &value)?;
                caller.data_mut().borrowed_snapshot = Some(snapshot);
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "budget_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::BudgetInfo>(&caller, pointer, bytes)
        },
        {
            status((|| {
                if bytes as usize != size_of::<w::BudgetInfo>() {
                    return Err(w::ERR_BUFFER.into());
                }
                memory_range(&caller, pointer, bytes)?;

                let value = w::BudgetInfo {
                    gas_remaining: crate::execution::remaining(&mut caller)
                        .map_err(|_| w::ERR_UNAVAILABLE)?,
                    gas_limit: caller.data().gas_limit,
                    gas_per_tick: caller.data().gas_per_tick,
                };
                status_to_result(put(&mut caller, pointer, bytes, &value))
            })())
        },
    )?;

    metered!(
        linker,
        "flight_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::FlightState>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let observation = &caller.data().input.as_ref().unwrap().observation;
                let value = w::FlightState {
                    rotation: observation.rotation,
                    angular_velocity: observation.angular_velocity,
                    velocity: observation.velocity,
                    mass_kg: observation.mass_kg,
                    inertia: observation.inertia,
                    radius_m: observation.radius_m,
                };
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "ship_resources_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ShipResources>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let observation = &caller.data().input.as_ref().unwrap().observation;
                let value = observation.resources;
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "tick_set_interval",
        |mut caller: Caller<'_, Host>, seconds: f64| { CallPlan::fixed() },
        {
            status((|| {
                if !seconds.is_finite() || seconds < 0. {
                    return Err(w::ERR_ARGUMENT.into());
                }

                caller.data_mut().output.tick_interval_seconds = Some(seconds);
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "snapshot_keep",
        |mut caller: Caller<'_, Host>, id: u64| { CallPlan::fixed().map(|plan| plan.work(100)) },
        {
            status((|| {
                let host = caller.data();
                let snapshot = host
                    .working
                    .snapshot(id, host.current, host.borrowed_snapshot)
                    .ok_or(w::ERR_HANDLE)?;

                if !host.working.pins.contains_key(&id)
                    && host.working.pins.len() >= w::MAX_SNAPSHOTS as usize
                {
                    return Err(w::ERR_LIMIT.into());
                }

                caller.data_mut().working.pins.insert(id, snapshot);
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "snapshot_drop",
        |mut caller: Caller<'_, Host>, id: u64| { CallPlan::fixed() },
        {
            status((|| {
                caller
                    .data_mut()
                    .working
                    .pins
                    .remove(&id)
                    .ok_or(w::ERR_HANDLE)?;
                Ok(())
            })())
        },
    )?;

    Ok(())
}

fn world_query_error(error: anyhow::Error) -> CallError {
    match error.downcast_ref::<WorldQueryError>() {
        Some(WorldQueryError::BufferTooSmall) => w::ERR_BUFFER.into(),
        Some(WorldQueryError::LimitExceeded) => w::ERR_LIMIT.into(),
        None => w::ERR_ARGUMENT.into(),
    }
}

fn status_to_result(value: i32) -> CallResult {
    if value < 0 { Err(value.into()) } else { Ok(()) }
}

fn hardware(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "device_info",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            CallPlan::record::<w::DeviceInfo>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let device = caller
                    .data()
                    .catalogue
                    .get(index as usize)
                    .ok_or(w::ERR_ARGUMENT)?;
                let value = w::DeviceInfo::from(device);
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "device_group_read",
        |mut caller: Caller<'_, Host>, id: u64, group: u32, pointer: u32, bytes: u32| {
            CallPlan::record::<w::Text64>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let index = device_index(&caller, id)?;
                let label = caller.data().catalogue[index]
                    .groups
                    .get(group as usize)
                    .ok_or(w::ERR_ARGUMENT)?;
                let value = w::Text64::new(label);
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    for (name, specification) in [("device_spec", true), ("device_read", false)] {
        metered!(
            linker,
            name,
            |mut caller: Caller<'_, Host>, id: u64, kind: u64, pointer: u32, bytes: u32| {
                device_record_plan(&caller, id, kind, pointer, bytes, specification)
            },
            {
                status((|| {
                    let index = device_index(&caller, id)?;
                    let device = &caller.data().catalogue[index];

                    if device.kind.abi_tag() != kind {
                        return Err(w::ERR_UNSUPPORTED.into());
                    }

                    let value = if name == "device_spec" {
                        caller
                            .data()
                            .specs
                            .get(index)
                            .cloned()
                            .ok_or(w::ERR_UNAVAILABLE)?
                    } else {
                        let state = caller
                            .data()
                            .input
                            .as_ref()
                            .unwrap()
                            .devices
                            .get(index)
                            .ok_or(w::ERR_UNAVAILABLE)?;
                        state.abi_record()
                    };
                    emit_bytes(&mut caller, pointer, bytes, &value)
                })())
            },
        )?;
    }

    metered!(
        linker,
        "device_write",
        |mut caller: Caller<'_, Host>, id: u64, setting: u64, pointer: u32, bytes: u32| {
            device_write_plan(&caller, setting, pointer, bytes)
        },
        {
            status((|| {
                if caller.data().display_only {
                    return Err(w::ERR_ARGUMENT.into());
                }
                let index = device_index(&caller, id)?;
                if caller
                    .data()
                    .input
                    .as_ref()
                    .unwrap()
                    .devices
                    .get(index)
                    .is_none()
                {
                    return Err(w::ERR_UNAVAILABLE.into());
                }
                let setting = match setting {
                    w::SET_RCS => {
                        let value: w::RcsSetting = input(&mut caller, pointer, bytes)?;
                        finite(&value.thrust_n)?;
                        DeviceSetting::RcsThrust(value.thrust_n)
                    }
                    w::SET_WEAPON => {
                        let value: w::WeaponSetting = input(&mut caller, pointer, bytes)?;
                        let host = caller.data();
                        let dt = host.input.as_ref().unwrap().physics_dt;
                        if !osg_ships::weapons::valid_setting(&value)
                            || value.valid_until_s > host.current.epoch + dt + 1e-6
                        {
                            return Err(w::ERR_ARGUMENT.into());
                        }
                        DeviceSetting::Weapon(value)
                    }
                    w::SET_THROTTLE => {
                        let value: w::ThrottleSetting = input(&mut caller, pointer, bytes)?;
                        fraction(value.fraction)?;
                        DeviceSetting::Throttle(value.fraction)
                    }
                    w::SET_TORQUE => {
                        let value: w::TorqueSetting = input(&mut caller, pointer, bytes)?;
                        finite(&value.torque_nm)?;
                        DeviceSetting::TorqueNm(value.torque_nm)
                    }
                    w::SET_GENERATOR_DEMAND => {
                        let value: w::GeneratorDemandSetting = input(&mut caller, pointer, bytes)?;
                        fraction(value.fraction)?;
                        DeviceSetting::GeneratorDemand(value.fraction)
                    }
                    w::SET_SHIELD_ENABLED | w::SET_SENSOR_ENABLED => {
                        let value: w::EnabledSetting = input(&mut caller, pointer, bytes)?;

                        if value.enabled > 1 {
                            return Err(w::ERR_ARGUMENT.into());
                        }

                        if setting == w::SET_SHIELD_ENABLED {
                            DeviceSetting::ShieldEnabled(value.enabled != 0)
                        } else {
                            DeviceSetting::SensorEnabled(value.enabled != 0)
                        }
                    }
                    _ => return Err(w::ERR_ARGUMENT.into()),
                };

                if !setting.supports(&caller.data().catalogue[index].kind) {
                    return Err(w::ERR_UNSUPPORTED.into());
                }

                if let DeviceSetting::Weapon(value) = &setting {
                    if !lease_active(&caller, value.valid_until_s)? {
                        return Ok(());
                    }
                }

                let commands = &mut caller.data_mut().output.devices;
                let command = DeviceCommand {
                    device: DeviceHandle(index as u16),
                    setting,
                };
                match commands.binary_search_by_key(&index, |command| usize::from(command.device.0))
                {
                    Ok(index) => commands[index] = command,
                    Err(index) => commands.insert(index, command),
                }
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "resource_info",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ResourceInfo>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let inventory = &caller.data().input.as_ref().unwrap().observation.inventory;
                inventory.get(index as usize).ok_or(w::ERR_ARGUMENT)?;
                let value = caller
                    .data()
                    .resources
                    .get(index as usize)
                    .copied()
                    .ok_or(w::ERR_UNAVAILABLE)?;
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "resource_read",
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ResourceAmount>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let index = id
                    .checked_sub(1)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or(w::ERR_HANDLE)?;
                let inventory = &caller.data().input.as_ref().unwrap().observation.inventory;
                let units = *inventory.get(index).ok_or(w::ERR_HANDLE)?;
                emit(&mut caller, pointer, bytes, &w::ResourceAmount { units })
            })())
        },
    )?;

    Ok(())
}

fn sensors(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "sensor_scan",
        |mut caller: Caller<'_, Host>, sensor: u64, maximum: u32, pointer: u32, bytes: u32| {
            scan_plan(&caller, maximum, pointer, bytes)
        },
        {
            finish((|| -> CallResult<i32> {
                if maximum > w::MAX_CONTACTS {
                    return Err(w::ERR_ARGUMENT.into());
                }

                if bytes != maximum * size_of::<w::Contact>() as u32 {
                    return Err(w::ERR_BUFFER.into());
                }
                memory_range(&caller, pointer, bytes)?;

                let index = device_index(&caller, sensor)?;
                let observation = caller.data().input.as_ref().unwrap();
                let device = observation.devices.get(index).ok_or(w::ERR_UNAVAILABLE)?;
                let DeviceReading::Sensor { range_m } = device.reading else {
                    return Err(w::ERR_UNSUPPORTED.into());
                };

                if !device.powered || !device.operational || range_m <= 0. {
                    return Err(w::ERR_UNAVAILABLE.into());
                }

                let start = std::time::Instant::now();
                let mut contacts = if maximum == 0 {
                    Vec::new()
                } else {
                    crate::scan_scope::with(|source| {
                        source
                            .map_or_else(Vec::new, |source| source.scan(range_m, maximum as usize))
                    })
                };
                contacts.truncate(maximum as usize);

                for (index, contact) in contacts.iter().enumerate() {
                    let record = contact.measured;
                    status_to_result(put(
                        &mut caller,
                        pointer + index as u32 * size_of::<w::Contact>() as u32,
                        size_of::<w::Contact>() as u32,
                        &record,
                    ))?;
                }

                let host = caller.data_mut();
                host.scan_seconds += start.elapsed().as_secs_f64();
                host.scan_time = Some(host.current.epoch);
                host.working.spatial.admit(&contacts, host.current);
                host.working.latest_scan = contacts.clone().into();
                host.working.latest_scan_epoch = Some(host.current.epoch);
                host.working.latest_sensor = sensor;
                host.contacts = contacts;
                Ok(host.contacts.len() as i32)
            })())
        },
    )?;

    metered!(
        linker,
        "contact_iff",
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ContactIff>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let contact =
                    crate::scan_scope::with(|source| source.and_then(|source| source.contact(id)))
                        .ok_or(w::ERR_UNAVAILABLE)?;
                let mut value = w::ContactIff::default();
                if let Some((entity, iff)) = contact.iff {
                    value.present = 1;
                    value.entity = entity.0;
                    value.owner = iff.owner.0;
                    value.faction_present = u64::from(iff.faction.is_some());
                    value.faction = iff.faction.unwrap_or_default().0;
                    value.labels_count = iff.labels.len().min(16) as u64;
                    for (output, label) in value.labels.iter_mut().zip(&iff.labels) {
                        *output = w::Text64::new(label);
                    }
                }
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "contact_label",
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32| {
            CallPlan::record::<w::Text64>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let contact =
                    crate::scan_scope::with(|source| source.and_then(|source| source.contact(id)))
                        .ok_or(w::ERR_UNAVAILABLE)?;
                let value = w::Text64::new(&contact.name);
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    Ok(())
}

fn requests(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "request_info",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            CallPlan::record::<w::RequestInfo>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let request = caller
                    .data()
                    .input
                    .as_ref()
                    .unwrap()
                    .commands
                    .get(index as usize)
                    .ok_or(w::ERR_ARGUMENT)?;
                let (kind, payload) = request.command.payload();
                let value = w::RequestInfo {
                    id: request.id,
                    kind,
                    payload_bytes: payload.len() as u64,
                };
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "request_read",
        |mut caller: Caller<'_, Host>, index: u32, expected: u64, pointer: u32, bytes: u32| {
            request_read_plan(&caller, index, pointer, bytes)
        },
        {
            status((|| {
                let request = caller
                    .data()
                    .input
                    .as_ref()
                    .unwrap()
                    .commands
                    .get(index as usize)
                    .ok_or(w::ERR_ARGUMENT)?;
                let (kind, payload) = request.command.payload();

                if expected != kind {
                    return Err(w::ERR_UNSUPPORTED.into());
                }

                emit_bytes(&mut caller, pointer, bytes, &payload)
            })())
        },
    )?;

    metered!(
        linker,
        "request_reply",
        |mut caller: Caller<'_, Host>, id: u64, result: u64, pointer: u32, bytes: u32| {
            CallPlan::bytes(&caller, pointer, bytes, 256)
        },
        {
            status((|| {
                if result > w::REPLY_UNSUPPORTED || bytes > 256 {
                    return Err(w::ERR_ARGUMENT.into());
                }

                if !caller
                    .data()
                    .input
                    .as_ref()
                    .unwrap()
                    .commands
                    .iter()
                    .any(|request| request.id == id)
                {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let message = std::str::from_utf8(payload(&caller, pointer, bytes)?)
                    .map_err(|_| w::ERR_ARGUMENT)?
                    .to_owned();
                let replies = &mut caller.data_mut().output.replies;
                replies.retain(|reply| reply.id != id);
                replies.push(RequestReply {
                    id,
                    result,
                    message,
                });
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "screen_event_read",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ScreenEvent>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let value = *caller
                    .data()
                    .events
                    .get(index as usize)
                    .ok_or(w::ERR_ARGUMENT)?;
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    metered!(
        linker,
        "screen_event_ack",
        |mut caller: Caller<'_, Host>, id: u64| { CallPlan::fixed() },
        {
            status((|| {
                if !caller.data().events.iter().any(|event| event.id == id) {
                    return Err(w::ERR_ARGUMENT.into());
                }

                acknowledge(caller.data_mut(), id);

                Ok(())
            })())
        },
    )?;

    Ok(())
}
