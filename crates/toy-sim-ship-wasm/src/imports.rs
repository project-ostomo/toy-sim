mod drawing;
mod instruments;
mod publications;

use super::*;

pub(super) struct ScreenDraft {
    frame: ScreenImage,
    payload_bytes: usize,
}

type CallResult<T = ()> = std::result::Result<T, i32>;

fn enter(caller: &mut Caller<'_, Host>) -> CallResult {
    pay(caller, w::CALL_GAS)?;

    if caller.data().input.is_none() {
        return Err(w::ERR_UNAVAILABLE);
    }

    Ok(())
}

fn pay(caller: &mut Caller<'_, Host>, gas: u64) -> CallResult {
    if charge(caller, gas) {
        Ok(())
    } else {
        Err(w::ERR_GAS)
    }
}

fn status(result: CallResult) -> i32 {
    result.map_or_else(|error| error, |()| 0)
}

fn input<T: Record>(caller: &mut Caller<'_, Host>, pointer: u32, bytes: u32) -> CallResult<T> {
    if bytes as usize != size_of::<T>() {
        return Err(w::ERR_BUFFER);
    }

    let value = read(caller, pointer, bytes).ok_or(w::ERR_BUFFER)?;
    pay(caller, u64::from(bytes).div_ceil(8))?;
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
        return Err(w::ERR_BUFFER);
    }

    let target = range(caller, pointer, bytes).ok_or(w::ERR_BUFFER)?;
    pay(caller, u64::from(bytes).div_ceil(8))?;
    let memory = caller.data().memory.ok_or(w::ERR_UNAVAILABLE)?;
    memory.data_mut(caller)[target].copy_from_slice(value);
    Ok(())
}

fn payload<'a>(caller: &'a Caller<'_, Host>, pointer: u32, bytes: u32) -> CallResult<&'a [u8]> {
    let source = range(caller, pointer, bytes).ok_or(w::ERR_BUFFER)?;
    let memory = caller.data().memory.ok_or(w::ERR_UNAVAILABLE)?;
    Ok(&memory.data(caller)[source])
}

fn device_index(caller: &Caller<'_, Host>, id: u64) -> CallResult<usize> {
    let index = id
        .checked_sub(1)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(w::ERR_HANDLE)?;

    if index >= caller.data().catalogue.len() {
        return Err(w::ERR_HANDLE);
    }

    Ok(index)
}

fn finite(values: &[f64]) -> CallResult {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(w::ERR_ARGUMENT)
    }
}

fn fraction(value: f64) -> CallResult {
    if value.is_finite() && (0. ..=1.).contains(&value) {
        Ok(())
    } else {
        Err(w::ERR_ARGUMENT)
    }
}

fn lease(caller: &Caller<'_, Host>, until: f64) -> CallResult {
    if until.is_finite() && until > caller.data().current.epoch {
        Ok(())
    } else {
        Err(w::ERR_ARGUMENT)
    }
}

pub(super) fn imports(engine: &Engine) -> Result<Linker<Host>> {
    let mut linker = Linker::new(engine);
    context(&mut linker)?;
    hardware(&mut linker)?;
    sensors(&mut linker)?;
    requests(&mut linker)?;
    instruments::register(&mut linker)?;
    publications::register(&mut linker)?;
    drawing::register(&mut linker)?;
    Ok(linker)
}

fn context(linker: &mut Linker<Host>) -> Result<()> {
    linker.func_wrap(
        w::IMPORT_MODULE,
        "tick_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "budget_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;

                if bytes as usize != size_of::<w::BudgetInfo>()
                    || range(&caller, pointer, bytes).is_none()
                {
                    return Err(w::ERR_BUFFER);
                }

                pay(&mut caller, u64::from(bytes) / 8)?;
                let value = w::BudgetInfo {
                    gas_remaining: caller.data().gas,
                    instruction_remaining: caller.get_fuel().map_err(|_| w::ERR_UNAVAILABLE)?,
                    gas_capacity: RESERVE_CAPACITY,
                    gas_refill_per_s: GAS_PER_SECOND,
                    instruction_limit: FUEL_PER_TICK,
                };
                status_to_result(put(&mut caller, pointer, bytes, &value))
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "flight_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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

    linker.func_wrap(
        w::IMPORT_MODULE,
        "ship_resources_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let observation = &caller.data().input.as_ref().unwrap().observation;
                let value = observation.resources;
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "tick_set_interval",
        |mut caller: Caller<'_, Host>, seconds: f64| {
            status((|| {
                enter(&mut caller)?;

                if !seconds.is_finite() || seconds < 0. {
                    return Err(w::ERR_ARGUMENT);
                }

                caller.data_mut().output.tick_interval_seconds = Some(seconds);
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "snapshot_keep",
        |mut caller: Caller<'_, Host>, id: u64| {
            status((|| {
                enter(&mut caller)?;
                let host = caller.data();
                let snapshot = host
                    .working
                    .snapshot(id, host.current)
                    .ok_or(w::ERR_HANDLE)?;

                if !host.working.pins.contains_key(&id)
                    && host.working.pins.len() >= w::MAX_SNAPSHOTS as usize
                {
                    return Err(w::ERR_LIMIT);
                }

                pay(&mut caller, 100)?;
                caller.data_mut().working.pins.insert(id, snapshot);
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "snapshot_drop",
        |mut caller: Caller<'_, Host>, id: u64| {
            status((|| {
                enter(&mut caller)?;
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

fn status_to_result(value: i32) -> CallResult {
    if value < 0 { Err(value) } else { Ok(()) }
}

fn hardware(linker: &mut Linker<Host>) -> Result<()> {
    linker.func_wrap(
        w::IMPORT_MODULE,
        "device_info",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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

    linker.func_wrap(
        w::IMPORT_MODULE,
        "device_group_read",
        |mut caller: Caller<'_, Host>, id: u64, group: u32, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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

    for name in ["device_spec", "device_read"] {
        linker.func_wrap(
            w::IMPORT_MODULE,
            name,
            move |mut caller: Caller<'_, Host>, id: u64, kind: u64, pointer: u32, bytes: u32| {
                status((|| {
                    enter(&mut caller)?;
                    let index = device_index(&caller, id)?;
                    let device = &caller.data().catalogue[index];

                    if device.kind.abi_tag() != kind {
                        return Err(w::ERR_UNSUPPORTED);
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

    linker.func_wrap(
        w::IMPORT_MODULE,
        "device_write",
        |mut caller: Caller<'_, Host>, id: u64, setting: u64, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let index = device_index(&caller, id)?;
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
                        if !toy_sim_ships::weapons::valid_setting(&value)
                            || value.valid_until_s < host.current.epoch
                            || value.valid_until_s > host.current.epoch + dt + 1e-6
                        {
                            return Err(w::ERR_ARGUMENT);
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
                            return Err(w::ERR_ARGUMENT);
                        }

                        if setting == w::SET_SHIELD_ENABLED {
                            DeviceSetting::ShieldEnabled(value.enabled != 0)
                        } else {
                            DeviceSetting::SensorEnabled(value.enabled != 0)
                        }
                    }
                    _ => return Err(w::ERR_ARGUMENT),
                };

                if !setting.supports(&caller.data().catalogue[index].kind) {
                    return Err(w::ERR_UNSUPPORTED);
                }

                let commands = &mut caller.data_mut().output.devices;
                commands.retain(|command| usize::from(command.device.0) != index);
                commands.push(DeviceCommand {
                    device: DeviceHandle(index as u16),
                    setting,
                });
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "resource_info",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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

    linker.func_wrap(
        w::IMPORT_MODULE,
        "resource_read",
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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
    linker.func_wrap(
        w::IMPORT_MODULE,
        "sensor_scan",
        |mut caller: Caller<'_, Host>, sensor: u64, maximum: u32, pointer: u32, bytes: u32| {
            (|| -> CallResult<i32> {
                enter(&mut caller)?;

                if maximum > w::MAX_CONTACTS {
                    return Err(w::ERR_ARGUMENT);
                }

                if bytes != maximum * size_of::<w::Contact>() as u32
                    || range(&caller, pointer, bytes).is_none()
                {
                    return Err(w::ERR_BUFFER);
                }

                let index = device_index(&caller, sensor)?;
                let observation = caller.data().input.as_ref().unwrap();
                let device = observation.devices.get(index).ok_or(w::ERR_UNAVAILABLE)?;
                let DeviceReading::Sensor { range_m } = device.reading else {
                    return Err(w::ERR_UNSUPPORTED);
                };

                if !device.powered || !device.operational || range_m <= 0. {
                    return Err(w::ERR_UNAVAILABLE);
                }

                pay(&mut caller, u64::from(maximum) * w::SCAN_GAS_PER_OBJECT)?;
                let start = std::time::Instant::now();
                let mut contacts = if maximum == 0 {
                    Vec::new()
                } else if let Some(source) = &caller.data().source {
                    source.scan(range_m, maximum as usize)
                } else {
                    Vec::new()
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
            })()
            .unwrap_or_else(|error| error)
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "contact_label",
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let track = caller
                    .data()
                    .working
                    .spatial
                    .tracks
                    .get(&id)
                    .ok_or(w::ERR_UNAVAILABLE)?;

                if track.latest.epoch + 2. <= caller.data().current.epoch {
                    return Err(w::ERR_UNAVAILABLE);
                }

                let value = w::Text64::new(&track.contact.name);
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    Ok(())
}

fn requests(linker: &mut Linker<Host>) -> Result<()> {
    linker.func_wrap(
        w::IMPORT_MODULE,
        "request_info",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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

    linker.func_wrap(
        w::IMPORT_MODULE,
        "request_read",
        |mut caller: Caller<'_, Host>, index: u32, expected: u64, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
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
                    return Err(w::ERR_UNSUPPORTED);
                }

                emit_bytes(&mut caller, pointer, bytes, &payload)
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "request_reply",
        |mut caller: Caller<'_, Host>, id: u64, result: u64, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;

                if result > w::REPLY_UNSUPPORTED || bytes > 256 {
                    return Err(w::ERR_ARGUMENT);
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
                    return Err(w::ERR_ARGUMENT);
                }

                pay(&mut caller, u64::from(bytes).div_ceil(8))?;
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

    linker.func_wrap(
        w::IMPORT_MODULE,
        "screen_event_read",
        |mut caller: Caller<'_, Host>, index: u32, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let value = *caller
                    .data()
                    .events
                    .get(index as usize)
                    .ok_or(w::ERR_ARGUMENT)?;
                emit(&mut caller, pointer, bytes, &value)
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "screen_event_ack",
        |mut caller: Caller<'_, Host>, id: u64| {
            status((|| {
                enter(&mut caller)?;

                if !caller.data().events.iter().any(|event| event.id == id) {
                    return Err(w::ERR_ARGUMENT);
                }

                if !caller.data().event_acks.contains(&id) {
                    caller.data_mut().event_acks.push(id);
                }

                Ok(())
            })())
        },
    )?;

    Ok(())
}
