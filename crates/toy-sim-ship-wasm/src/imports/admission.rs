use super::*;

macro_rules! metered {
    ($linker:expr, $name:expr,
     |mut $caller:ident: $caller_type:ty $(, $argument:ident: $argument_type:ty)*|
     { $($prepare:tt)* }, $body:block $(,)?) => {
        $linker.func_wrap_async(
            w::IMPORT_MODULE,
            $name,
            move |mut $caller: $caller_type, ($($argument,)*): ($($argument_type,)*)| {
                Box::new(async move {
                    loop {
                        let prepared: CallResult<CallPlan> = if $caller.data().input.is_some() {
                            (|| -> CallResult<CallPlan> { $($prepare)* })()
                        } else {
                            Err(w::ERR_UNAVAILABLE.into())
                        };
                        let cost = prepared.as_ref().map_or_else(CallError::admission_cost, |plan| plan.gas);
                        if cost > $caller.data().gas_per_tick {
                            return Ok(w::ERR_LIMIT);
                        }
                        if !crate::execution::admit(&mut $caller, cost)
                            .await
                            .map_err(wasmtime::Error::from_anyhow)?
                        {
                            continue;
                        }

                        $caller.data_mut().native_credit = 0;
                        let result: wasmtime::Result<i32> = match prepared {
                            Ok(plan) => {
                                $caller.data_mut().prepared_query = plan.query;
                                (|| $body)()
                            }
                            Err(error) => finish(Err(error)),
                        };
                        let unused = std::mem::take(&mut $caller.data_mut().native_credit);
                        assert!(unused <= cost, "native refund exceeded admitted cost");
                        crate::execution::refund(&mut $caller, unused)
                            .map_err(wasmtime::Error::from_anyhow)?;
                        return result;
                    }
                })
            },
        )
    };
}

pub(crate) struct PreparedWorldQuery {
    pub query: toy_sim_model::ProgramQuery,
    pub work: u64,
    pub capacity: usize,
}

pub(super) struct CallPlan {
    pub gas: u64,
    pub query: Option<PreparedWorldQuery>,
}

pub(super) fn words(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(8)
}

impl CallPlan {
    pub fn fixed() -> CallResult<Self> {
        Ok(Self {
            gas: w::CALL_GAS,
            query: None,
        })
    }

    pub fn record<T: Record>(
        caller: &Caller<'_, Host>,
        pointer: u32,
        bytes: u32,
    ) -> CallResult<Self> {
        Self::record_bytes(caller, pointer, bytes, size_of::<T>())
    }

    pub fn record_bytes(
        caller: &Caller<'_, Host>,
        pointer: u32,
        bytes: u32,
        expected: usize,
    ) -> CallResult<Self> {
        if bytes as usize != expected {
            return Err(w::ERR_BUFFER.into());
        }
        Self::bytes(caller, pointer, bytes, expected)
    }

    pub fn bytes(
        caller: &Caller<'_, Host>,
        pointer: u32,
        bytes: u32,
        maximum: usize,
    ) -> CallResult<Self> {
        Self::fixed()?.extra_bytes(caller, pointer, bytes, maximum)
    }

    pub fn extra_bytes(
        mut self,
        caller: &Caller<'_, Host>,
        pointer: u32,
        bytes: u32,
        maximum: usize,
    ) -> CallResult<Self> {
        if bytes as usize > maximum {
            return Err(w::ERR_LIMIT.into());
        }
        memory_range(caller, pointer, bytes)?;
        self.gas += words(bytes as usize);
        Ok(self)
    }

    pub fn work(mut self, work: u64) -> Self {
        self.gas += work;
        self
    }
}

pub(super) fn persistent_read_plan(
    caller: &Caller<'_, Host>,
    pointer: u32,
    capacity: u32,
) -> CallResult<CallPlan> {
    let length = caller.data().persistent_data.len();
    if length > capacity as usize {
        return Err(w::ERR_BUFFER.into());
    }
    CallPlan::bytes(caller, pointer, length as u32, 65536)
}

pub(super) fn world_query_plan(
    caller: &Caller<'_, Host>,
    pointer: u32,
    length: u32,
    output: u32,
    capacity: u32,
) -> CallResult<CallPlan> {
    let mut plan = CallPlan::bytes(caller, pointer, length, 65536)?
        .extra_bytes(caller, output, capacity, 65536)?;
    let input_cost = w::CALL_GAS + words(length as usize);
    let mut query: toy_sim_model::ProgramQuery =
        postcard::from_bytes(payload(caller, pointer, length)?)
            .map_err(|_| CallError::Status(w::ERR_ARGUMENT).priced(input_cost))?;
    let maximum_work = caller.data().gas_per_tick.saturating_sub(plan.gas);
    match &mut query {
        toy_sim_model::ProgramQuery::Tracks(query) => query.work = query.work.min(maximum_work),
        toy_sim_model::ProgramQuery::Continue { work, .. } => *work = (*work).min(maximum_work),
        _ => {}
    }
    let source = caller.data().source.as_ref().ok_or(w::ERR_UNAVAILABLE)?;
    let work = source
        .query_work(&query)
        .map_err(|error| world_query_error(error).priced(input_cost))?;
    if work > maximum_work {
        return Err(CallError::Status(w::ERR_LIMIT).priced(input_cost));
    }
    plan.gas += work;
    plan.query = Some(PreparedWorldQuery {
        query,
        work,
        capacity: capacity as usize,
    });
    Ok(plan)
}

pub(super) fn device_record_plan(
    caller: &Caller<'_, Host>,
    id: u64,
    kind: u64,
    pointer: u32,
    bytes: u32,
    specification: bool,
) -> CallResult<CallPlan> {
    let index = device_index(caller, id)?;
    if caller.data().catalogue[index].kind.abi_tag() != kind {
        return Err(w::ERR_UNSUPPORTED.into());
    }
    let expected = if specification {
        caller
            .data()
            .specs
            .get(index)
            .ok_or(w::ERR_UNAVAILABLE)?
            .len()
    } else {
        let state = caller
            .data()
            .input
            .as_ref()
            .unwrap()
            .devices
            .get(index)
            .ok_or(w::ERR_UNAVAILABLE)?;
        match state.reading {
            DeviceReading::Rcs { .. } => size_of::<w::RcsReading>(),
            DeviceReading::Weapon(_) => size_of::<w::WeaponReading>(),
            DeviceReading::Accelerometer { .. } => size_of::<w::AccelerometerReading>(),
            DeviceReading::Computer | DeviceReading::Storage | DeviceReading::Battery => {
                size_of::<w::DeviceStatus>()
            }
            DeviceReading::Engine { .. } => size_of::<w::EngineReading>(),
            DeviceReading::Torquer { .. } => size_of::<w::TorquerReading>(),
            DeviceReading::Generator { .. } => size_of::<w::GeneratorReading>(),
            DeviceReading::Shield { .. } => size_of::<w::ShieldReading>(),
            DeviceReading::Sensor { .. } => size_of::<w::SensorReading>(),
        }
    };
    CallPlan::record_bytes(caller, pointer, bytes, expected)
}

pub(super) fn device_write_plan(
    caller: &Caller<'_, Host>,
    setting: u64,
    pointer: u32,
    bytes: u32,
) -> CallResult<CallPlan> {
    let expected = match setting {
        w::SET_RCS => size_of::<w::RcsSetting>(),
        w::SET_WEAPON => size_of::<w::WeaponSetting>(),
        w::SET_THROTTLE => size_of::<w::ThrottleSetting>(),
        w::SET_TORQUE => size_of::<w::TorqueSetting>(),
        w::SET_GENERATOR_DEMAND => size_of::<w::GeneratorDemandSetting>(),
        w::SET_SHIELD_ENABLED | w::SET_SENSOR_ENABLED => size_of::<w::EnabledSetting>(),
        _ => return Err(w::ERR_ARGUMENT.into()),
    };
    CallPlan::record_bytes(caller, pointer, bytes, expected)
}

pub(super) fn request_read_plan(
    caller: &Caller<'_, Host>,
    index: u32,
    pointer: u32,
    bytes: u32,
) -> CallResult<CallPlan> {
    let request = caller
        .data()
        .input
        .as_ref()
        .unwrap()
        .commands
        .get(index as usize)
        .ok_or(w::ERR_ARGUMENT)?;
    CallPlan::record_bytes(caller, pointer, bytes, request.command.payload().1.len())
}

pub(super) fn scan_plan(
    caller: &Caller<'_, Host>,
    maximum: u32,
    pointer: u32,
    bytes: u32,
) -> CallResult<CallPlan> {
    if maximum > w::MAX_CONTACTS {
        return Err(w::ERR_ARGUMENT.into());
    }
    if bytes as usize != maximum as usize * size_of::<w::Contact>() {
        return Err(w::ERR_BUFFER.into());
    }
    memory_range(caller, pointer, bytes)?;
    Ok(CallPlan::fixed()?.work(u64::from(maximum) * w::SCAN_GAS_PER_OBJECT))
}

pub(super) fn weapons_plan(
    caller: &Caller<'_, Host>,
    pointer: u32,
    bytes: u32,
    rows_pointer: u32,
    count: u32,
) -> CallResult<CallPlan> {
    if count as usize > caller.data().catalogue.len().min(MAX_DEVICES) {
        return Err(w::ERR_ARGUMENT.into());
    }
    let plan = CallPlan::record::<w::WeaponsState>(caller, pointer, bytes)?;
    let length = count as usize * size_of::<w::WeaponInstrument>();
    memory_range(caller, rows_pointer, length as u32)?;
    Ok(plan.work(u64::from(count) * w::WEAPON_ROW_GAS + words(length)))
}

pub(super) fn path_plan(
    caller: &Caller<'_, Host>,
    pointer: u32,
    bytes: u32,
    vertex_pointer: u32,
    count: u32,
) -> CallResult<CallPlan> {
    if !(2..=w::MAX_PATH_VERTICES).contains(&count) {
        return Err(w::ERR_ARGUMENT.into());
    }
    let plan = CallPlan::record::<w::SpatialPath>(caller, pointer, bytes)?;
    memory_range(
        caller,
        vertex_pointer,
        count * size_of::<w::SpatialVertex>() as u32,
    )?;
    Ok(plan.work(u64::from(count) * 100))
}
