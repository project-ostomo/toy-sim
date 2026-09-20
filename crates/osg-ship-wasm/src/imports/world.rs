use super::*;
use osg_model::{ProgramQuery, ProgramReply, wasm_world::ReplyCapacity};
use osg_ship_api::world as a;

pub(super) fn prepare(
    caller: &Caller<'_, Host>,
    mut query: ProgramQuery,
    capacity: ReplyCapacity,
    input_bytes: usize,
    output_budget: usize,
) -> CallResult<CallPlan> {
    let source = caller.data().source.as_ref().ok_or(w::ERR_UNAVAILABLE)?;
    let output_budget = source
        .query_output_bytes(&query, caller.data().display_only, capacity, output_budget)
        .map_err(world_query_error)?
        .min(output_budget);
    let base = w::CALL_GAS + words(input_bytes) + words(output_budget);
    let maximum = caller.data().gas_per_tick.saturating_sub(base);
    match &mut query {
        ProgramQuery::Tracks(query) => query.work = query.work.min(maximum),
        ProgramQuery::Continue { work, .. } => *work = (*work).min(maximum),
        _ => {}
    }
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
}

pub(super) fn execute(caller: &mut Caller<'_, Host>) -> CallResult<ProgramReply> {
    let prepared = caller
        .data_mut()
        .prepared_query
        .take()
        .expect("admitted world query");
    let source = caller.data().source.clone().ok_or(w::ERR_UNAVAILABLE)?;
    let result = source.query(
        prepared.query,
        caller.data().display_only,
        prepared.capacity,
    );
    let used = match &result {
        Ok(ProgramReply::Tracks(page)) => page.gas_used,
        _ => prepared.work,
    };
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
        |mut caller: Caller<'_, Host>, output: u32| {
            validate_array::<a::TravelReply>(&caller, output, 1)?;
            prepare(
                &caller,
                ProgramQuery::Travel,
                ReplyCapacity::default(),
                0,
                size_of::<a::TravelReply>(),
            )
        },
        {
            status((|| {
                let reply = execute(&mut caller)?;
                let value = a::TravelReply::try_from(&reply).map_err(|_| w::ERR_ARGUMENT)?;
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
        "navigation_query",
        |mut caller: Caller<'_, Host>, pointer: u32, output: u32, capacity: u32, header: u32| {
            let record: a::NavigationQuery = read(&caller, pointer)?;
            let query = ProgramQuery::try_from(&record).map_err(|_| w::ERR_ARGUMENT)?;
            if record.limit > u64::from(capacity) {
                return Err(w::ERR_BUFFER.into());
            }
            validate_array::<a::NavigationGate>(&caller, output, capacity)?;
            validate_array::<a::NavigationReply>(&caller, header, 1)?;
            prepare(
                &caller,
                query,
                ReplyCapacity {
                    records: capacity as usize,
                    ..Default::default()
                },
                size_of::<a::NavigationQuery>(),
                size_of::<a::NavigationReply>()
                    + record.limit as usize * size_of::<a::NavigationGate>(),
            )
        },
        {
            status((|| {
                let ProgramReply::Navigation { revision, gates } = execute(&mut caller)? else {
                    return Err(w::ERR_ARGUMENT.into());
                };
                if gates.len() > capacity as usize {
                    return Err(w::ERR_BUFFER.into());
                }
                let values: Vec<a::NavigationGate> = gates.iter().map(Into::into).collect();
                emit_array(&mut caller, output, &values)?;
                emit_record(
                    &mut caller,
                    header,
                    &a::NavigationReply {
                        revision,
                        count: values.len() as u64,
                    },
                )
            })())
        }
    )?;

    metered!(
        linker,
        "route_request",
        |mut caller: Caller<'_, Host>,
         pointer: u32,
         orders: u32,
         count: u32,
         header: u32,
         output: u32,
         capacity: u32,
         fuels: u32,
         fuel_capacity: u32| {
            if count as usize > osg_model::routing::MAX_ORDERS {
                return Err(w::ERR_LIMIT.into());
            }
            let record: a::RouteRequest = read(&caller, pointer)?;
            validate_array::<a::Order>(&caller, orders, count)?;
            let actions: Result<Vec<_>, _> = (0..count)
                .map(|index| {
                    let record: a::Order =
                        read(&caller, orders + index * size_of::<a::Order>() as u32)?;
                    osg_model::travel::Order::try_from(&record)
                        .map_err(|_| CallError::from(w::ERR_ARGUMENT))
                })
                .collect();
            let request = osg_model::routing::Request {
                id: record.id,
                preferences: (&record.preferences)
                    .try_into()
                    .map_err(|_| w::ERR_ARGUMENT)?,
                orders: actions?,
            };
            route_plan(
                &caller,
                ProgramQuery::RouteRequest(request),
                header,
                output,
                capacity,
                fuels,
                fuel_capacity,
                size_of::<a::RouteRequest>() + count as usize * size_of::<a::Order>(),
            )
        },
        {
            status(write_route(
                &mut caller,
                header,
                output,
                capacity,
                fuels,
                fuel_capacity,
            ))
        }
    )?;

    metered!(
        linker,
        "route_poll",
        |mut caller: Caller<'_, Host>,
         id: u64,
         header: u32,
         output: u32,
         capacity: u32,
         fuels: u32,
         fuel_capacity: u32| {
            route_plan(
                &caller,
                ProgramQuery::RoutePoll { id },
                header,
                output,
                capacity,
                fuels,
                fuel_capacity,
                0,
            )
        },
        {
            status(write_route(
                &mut caller,
                header,
                output,
                capacity,
                fuels,
                fuel_capacity,
            ))
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
    action!("travel_use_route", a::UseRoute);
    action!("travel_block", a::Block);
    action!("travel_estimate", a::Estimate);
    action!("travel_complete", a::CompleteOrder);
    action!("travel_slip", a::Slip);
    action!("travel_reserve_bay", a::ReserveBay);
    action!("travel_dock", a::Dock);
    action!("travel_undock", a::Undock);
    Ok(())
}

fn route_plan(
    caller: &Caller<'_, Host>,
    query: ProgramQuery,
    header: u32,
    orders: u32,
    capacity: u32,
    fuels: u32,
    fuel_capacity: u32,
    input_bytes: usize,
) -> CallResult<CallPlan> {
    validate_array::<a::RouteReply>(caller, header, 1)?;
    validate_array::<a::QueuedOrder>(caller, orders, capacity)?;
    validate_array::<a::FuelRequirement>(caller, fuels, fuel_capacity)?;
    let order_limit = (capacity as usize).min(osg_model::routing::MAX_ORDERS);
    let fuel_limit = (fuel_capacity as usize).min(256);
    prepare(
        caller,
        query,
        ReplyCapacity {
            records: capacity as usize,
            auxiliary: fuel_capacity as usize,
            bytes: 0,
        },
        input_bytes,
        size_of::<a::RouteReply>()
            + order_limit * size_of::<a::QueuedOrder>()
            + fuel_limit * size_of::<a::FuelRequirement>(),
    )
}

fn write_route(
    caller: &mut Caller<'_, Host>,
    header: u32,
    orders: u32,
    capacity: u32,
    fuels: u32,
    fuel_capacity: u32,
) -> CallResult {
    let reply = execute(caller)?;
    let ProgramReply::Route { id, status } = &reply else {
        return Err(w::ERR_ARGUMENT.into());
    };
    let (value, actions, resources) = osg_model::wasm_world::route_records(*id, status);
    if let ProgramReply::Route {
        status: osg_model::routing::Status::Ready { plan },
        ..
    } = &reply
    {
        if plan.orders.len() > capacity as usize
            || plan.fuel_budget.resources.len() > fuel_capacity as usize
        {
            return Err(w::ERR_BUFFER.into());
        }
        emit_array(caller, orders, &actions)?;
        emit_array(caller, fuels, &resources)?;
    }
    emit_record(caller, header, &value)
}
