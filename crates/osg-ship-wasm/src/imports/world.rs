use super::*;
use osg_model::{ProgramQuery, ProgramReply, wasm_world::ReplyCapacity};
use osg_ship_api::world as a;

pub(super) fn prepare(
    caller: &Caller<'_, Host>,
    query: ProgramQuery,
    capacity: ReplyCapacity,
    input_bytes: usize,
    output_budget: usize,
) -> CallResult<CallPlan> {
    crate::scan_scope::with(|source| {
        let source = source.ok_or(w::ERR_UNAVAILABLE)?;
        let output_budget = source
            .query_output_bytes(&query, caller.data().display_only, capacity, output_budget)
            .map_err(world_query_error)?
            .min(output_budget);
        let base = w::CALL_GAS + words(input_bytes) + words(output_budget);
        let maximum = caller.data().gas_per_tick.saturating_sub(base);
        let work = source
            .query_work(&query)
            .map_err(|error| world_query_error(error).priced(w::CALL_GAS + words(input_bytes)))?;
        if work > maximum {
            return Err(w::ERR_LIMIT.into());
        }
        Ok(CallPlan {
            gas: base + work,
            query: Some(PreparedWorldQuery {
                query,
                work,
                capacity,
                output_budget,
            }),
        })
    })
}

pub(super) fn execute(caller: &mut Caller<'_, Host>) -> CallResult<ProgramReply> {
    let prepared = caller
        .data_mut()
        .prepared_query
        .take()
        .expect("admitted world query");
    let result = crate::scan_scope::with(|source| -> CallResult<_> {
        Ok(source.ok_or(w::ERR_UNAVAILABLE)?.query(
            prepared.query,
            caller.data().display_only,
            prepared.capacity,
        ))
    })?;
    let used = prepared.work;
    assert!(
        used <= prepared.work,
        "world service exceeded admitted work"
    );
    caller.data_mut().native_credit = prepared.work - used + words(prepared.output_budget);
    result.map_err(world_query_error)
}

pub(super) fn read<T: Record>(caller: &Caller<'_, Host>, pointer: u32) -> CallResult<T> {
    T::read(payload(caller, pointer, size_of::<T>() as u32)?).ok_or(w::ERR_ARGUMENT.into())
}

pub(super) fn validate_array<T: Record>(
    caller: &Caller<'_, Host>,
    pointer: u32,
    count: u32,
) -> CallResult {
    let bytes = count
        .checked_mul(size_of::<T>() as u32)
        .ok_or(w::ERR_BUFFER)?;
    memory_range(caller, pointer, bytes)?;
    Ok(())
}

pub(super) fn emit_record<T: Record>(
    caller: &mut Caller<'_, Host>,
    pointer: u32,
    value: &T,
) -> CallResult {
    emit(caller, pointer, size_of::<T>() as u32, value)?;
    caller.data_mut().native_credit -= words(size_of::<T>());
    Ok(())
}

pub(super) fn emit_array<T: Record>(
    caller: &mut Caller<'_, Host>,
    pointer: u32,
    values: &[T],
) -> CallResult {
    for (index, value) in values.iter().enumerate() {
        emit_record(caller, pointer + (index * size_of::<T>()) as u32, value)?;
    }
    Ok(())
}

pub(super) fn emit_bytes(caller: &mut Caller<'_, Host>, pointer: u32, bytes: &[u8]) -> CallResult {
    super::emit_bytes(caller, pointer, bytes.len() as u32, bytes)?;
    caller.data_mut().native_credit -= words(bytes.len());
    Ok(())
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "travel_read",
        |mut caller: Caller<'_, Host>,
         output: u32,
         itinerary: u32,
         capacity: u32,
         fuels: u32,
         fuel_capacity: u32,
         hierarchy: u32,
         hierarchy_capacity: u32| {
            validate_array::<a::TravelReply>(&caller, output, 1)?;
            validate_array::<a::ItineraryEntry>(&caller, itinerary, capacity)?;
            validate_array::<a::FuelRequirement>(&caller, fuels, fuel_capacity)?;
            validate_array::<a::CelestialRef>(&caller, hierarchy, hierarchy_capacity)?;
            prepare(
                &caller,
                ProgramQuery::Travel,
                ReplyCapacity {
                    records: capacity as usize,
                    auxiliary: fuel_capacity as usize,
                    bytes: hierarchy_capacity as usize * size_of::<a::CelestialRef>(),
                },
                0,
                size_of::<a::TravelReply>()
                    + (capacity as usize).min(osg_model::travel::MAX_DIRECTIVES)
                        * size_of::<a::ItineraryEntry>()
                    + (fuel_capacity as usize).min(256) * size_of::<a::FuelRequirement>()
                    + (hierarchy_capacity as usize)
                        .min(osg_model::local_space::MAX_LOCAL_OBSTACLES)
                        * size_of::<a::CelestialRef>(),
            )
        },
        {
            status((|| {
                let reply = execute(&mut caller)?;
                let value = a::TravelReply::try_from(&reply).map_err(|_| w::ERR_ARGUMENT)?;
                let ProgramReply::Travel {
                    state, location, ..
                } = &reply
                else {
                    return Err(w::ERR_ARGUMENT.into());
                };
                let entries: Vec<a::ItineraryEntry> =
                    state.itinerary.iter().map(Into::into).collect();
                let resources: Vec<a::FuelRequirement> = state
                    .fuel_budget
                    .iter()
                    .flat_map(|budget| &budget.resources)
                    .map(Into::into)
                    .collect();
                let ancestors: Vec<a::CelestialRef> =
                    location.hierarchy.iter().map(Into::into).collect();
                if entries.len() > capacity as usize
                    || resources.len() > fuel_capacity as usize
                    || ancestors.len() > hierarchy_capacity as usize
                {
                    return Err(w::ERR_BUFFER.into());
                }
                emit_array(&mut caller, itinerary, &entries)?;
                emit_array(&mut caller, fuels, &resources)?;
                emit_array(&mut caller, hierarchy, &ancestors)?;
                emit_record(&mut caller, output, &value)
            })())
        }
    )?;

    macro_rules! fixed_query {
        ($name:literal, $input:ty, $output:ty) => {
            metered!(
                linker,
                $name,
                |mut caller: Caller<'_, Host>, pointer: u32, output: u32| {
                    let record: $input = read(&caller, pointer)?;
                    let query = ProgramQuery::try_from(&record).map_err(|_| w::ERR_ARGUMENT)?;
                    validate_array::<$output>(&caller, output, 1)?;
                    prepare(
                        &caller,
                        query,
                        ReplyCapacity::default(),
                        size_of::<$input>(),
                        size_of::<$output>(),
                    )
                },
                {
                    status((|| {
                        let reply = execute(&mut caller)?;
                        let value = <$output>::try_from(&reply).map_err(|_| w::ERR_ARGUMENT)?;
                        emit_record(&mut caller, output, &value)
                    })())
                }
            )?;
        };
    }
    fixed_query!("contact_get", a::ContactRef, a::ContactReply);
    fixed_query!(
        "slip_eligibility",
        a::SlipEligibilityQuery,
        a::SlipEligibilityReply
    );
    fixed_query!("destination_resolve", a::ResolveQuery, a::Pose);

    metered!(
        linker,
        "orrery_read",
        |mut caller: Caller<'_, Host>, pointer: u32, output: u32, capacity: u32, header: u32| {
            let record: a::OrreryQuery = read(&caller, pointer)?;
            let query = ProgramQuery::from(&record);
            validate_array::<a::LocalObstacle>(&caller, output, capacity)?;
            validate_array::<a::OrreryReply>(&caller, header, 1)?;
            let limit = (capacity as usize).min(osg_model::local_space::MAX_LOCAL_OBSTACLES);
            prepare(
                &caller,
                query,
                ReplyCapacity {
                    records: capacity as usize,
                    ..Default::default()
                },
                size_of::<a::OrreryQuery>(),
                size_of::<a::OrreryReply>() + limit * size_of::<a::LocalObstacle>(),
            )
        },
        {
            status((|| {
                let ProgramReply::Orrery(obstacles) = execute(&mut caller)? else {
                    return Err(w::ERR_ARGUMENT.into());
                };
                if obstacles.len() > capacity as usize {
                    return Err(w::ERR_BUFFER.into());
                }
                let values: Vec<a::LocalObstacle> = obstacles.iter().map(Into::into).collect();
                emit_array(&mut caller, output, &values)?;
                emit_record(
                    &mut caller,
                    header,
                    &a::OrreryReply {
                        count: values.len() as u64,
                    },
                )
            })())
        }
    )?;

    metered!(
        linker,
        "orrery_system_read",
        |mut caller: Caller<'_, Host>, pointer: u32, output: u32, capacity: u32, header: u32| {
            let record: a::OrrerySystemQuery = read(&caller, pointer)?;
            let query = ProgramQuery::from(&record);
            validate_array::<a::LocalObstacle>(&caller, output, capacity)?;
            validate_array::<a::OrreryReply>(&caller, header, 1)?;
            let limit = (capacity as usize).min(osg_model::local_space::MAX_LOCAL_OBSTACLES);
            prepare(
                &caller,
                query,
                ReplyCapacity {
                    records: capacity as usize,
                    ..Default::default()
                },
                size_of::<a::OrrerySystemQuery>(),
                size_of::<a::OrreryReply>() + limit * size_of::<a::LocalObstacle>(),
            )
        },
        {
            status((|| {
                let ProgramReply::Orrery(obstacles) = execute(&mut caller)? else {
                    return Err(w::ERR_ARGUMENT.into());
                };
                if obstacles.len() > capacity as usize {
                    return Err(w::ERR_BUFFER.into());
                }
                let values: Vec<a::LocalObstacle> = obstacles.iter().map(Into::into).collect();
                emit_array(&mut caller, output, &values)?;
                emit_record(
                    &mut caller,
                    header,
                    &a::OrreryReply {
                        count: values.len() as u64,
                    },
                )
            })())
        }
    )?;

    metered!(
        linker,
        "slip_eligibility_batch",
        |mut caller: Caller<'_, Host>, pointer: u32, count: u32, output: u32| {
            if count > 256 {
                return Err(w::ERR_LIMIT.into());
            }
            validate_array::<a::SlipEligibilityQuery>(&caller, pointer, count)?;
            validate_array::<a::SlipProbeReply>(&caller, output, count)?;
            let probes = (0..count)
                .map(|index| {
                    let record: a::SlipEligibilityQuery = read(
                        &caller,
                        pointer + index * size_of::<a::SlipEligibilityQuery>() as u32,
                    )?;
                    osg_model::SlipProbe::try_from(&record)
                        .map_err(|_| CallError::from(w::ERR_ARGUMENT))
                })
                .collect::<CallResult<Vec<_>>>()?;
            prepare(
                &caller,
                ProgramQuery::SlipEligibilityBatch(probes),
                ReplyCapacity {
                    records: count as usize,
                    ..Default::default()
                },
                count as usize * size_of::<a::SlipEligibilityQuery>(),
                count as usize * size_of::<a::SlipProbeReply>(),
            )
        },
        {
            status((|| {
                let ProgramReply::SlipEligibilityBatch(results) = execute(&mut caller)? else {
                    return Err(w::ERR_ARGUMENT.into());
                };
                if results.len() != count as usize {
                    return Err(w::ERR_ARGUMENT.into());
                }
                let records: Vec<a::SlipProbeReply> = results.iter().map(Into::into).collect();
                emit_array(&mut caller, output, &records)
            })())
        }
    )?;

    macro_rules! action {
        ($name:literal, $record:ty) => {
            metered!(
                linker,
                $name,
                |mut caller: Caller<'_, Host>, pointer: u32| {
                    CallPlan::record::<$record>(&caller, pointer, size_of::<$record>() as u32)
                        .map(|plan| plan.work(1000))
                },
                {
                    status((|| {
                        if caller.data().display_only
                            || caller.data().output.world_actions.len() >= 8
                        {
                            return Err(w::ERR_UNAVAILABLE.into());
                        }
                        let record: $record = read(&caller, pointer)?;
                        let action = osg_model::ProgramAction::try_from(&record)
                            .map_err(|_| w::ERR_ARGUMENT)?;
                        caller.data_mut().output.world_actions.push(action);
                        Ok(())
                    })())
                }
            )?;
        };
    }
    action!("travel_fail", a::Fail);
    action!("travel_publish_status", a::PublishStatus);
    action!("travel_complete", a::Complete);
    action!("travel_set_autopilot", a::SetAutopilot);
    action!("travel_clear_itinerary", a::ClearItinerary);
    action!("travel_slip", a::Slip);
    action!("travel_reserve_bay", a::ReserveBay);
    action!("travel_dock", a::Dock);
    action!("travel_undock", a::Undock);
    metered!(
        linker,
        "travel_cancel_slip",
        |mut caller: Caller<'_, Host>| {
            Ok(CallPlan {
                gas: w::CALL_GAS + 1000,
                query: None,
            })
        },
        {
            status((|| {
                if caller.data().display_only || caller.data().output.world_actions.len() >= 8 {
                    return Err(w::ERR_UNAVAILABLE.into());
                }
                caller
                    .data_mut()
                    .output
                    .world_actions
                    .push(osg_model::ProgramAction::CancelSlip);
                Ok(())
            })())
        }
    )?;
    Ok(())
}
