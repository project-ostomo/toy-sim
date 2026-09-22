use super::world::{emit_array, emit_record, execute, prepare, validate_array};
use super::*;
use osg_model::{Id, ProgramQuery, ProgramReply, wasm_beacons, wasm_world::ReplyCapacity};
use osg_ship_api::beacons as a;

fn output_plan<T: Record, P: Record>(
    caller: &Caller<'_, Host>,
    query: ProgramQuery,
    input_bytes: usize,
    output: u32,
    capacity: u32,
    arena: u32,
    arena_capacity: u32,
    page: u32,
) -> CallResult<CallPlan> {
    validate_array::<T>(caller, output, capacity)?;
    validate_array::<P>(caller, page, 1)?;
    memory_range(caller, arena, arena_capacity)?;
    prepare(
        caller,
        query,
        ReplyCapacity {
            records: capacity as usize,
            auxiliary: 0,
            bytes: arena_capacity as usize,
        },
        input_bytes,
        capacity as usize * size_of::<T>() + arena_capacity as usize + size_of::<P>(),
    )
}

fn beacons(caller: &mut Caller<'_, Host>, output: u32, arena: u32, page: u32) -> CallResult {
    let capacity = caller
        .data()
        .prepared_query
        .as_ref()
        .expect("admitted query")
        .capacity;
    let ProgramReply::Beacons(result) = execute(caller)? else {
        return Err(w::ERR_ARGUMENT.into());
    };
    let mut bytes = Vec::new();
    if result.len() > capacity.records
        || result
            .iter()
            .map(wasm_beacons::beacon_arena_bytes)
            .sum::<usize>()
            > capacity.bytes
    {
        return Err(w::ERR_BUFFER.into());
    }
    let records: Vec<_> = result
        .iter()
        .map(|beacon| wasm_beacons::encode_beacon(beacon, &mut bytes))
        .collect();
    let header = a::BeaconPage {
        count: records.len() as u32,
        arena_bytes: bytes.len() as u32,
    };
    emit_array(caller, output, &records)?;
    super::world::emit_bytes(caller, arena, &bytes)?;
    emit_record(caller, page, &header)
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "beacons_read",
        |mut caller: Caller<'_, Host>,
         after: u32,
         has_after: u32,
         output: u32,
         capacity: u32,
         arena: u32,
         arena_capacity: u32,
         page: u32| {
            if has_after > 1 || capacity == 0 || capacity > 256 {
                return Err(w::ERR_ARGUMENT.into());
            }
            let after = if has_after != 0 {
                Some(Id(payload(&caller, after, 16)?.try_into().unwrap()))
            } else {
                None
            };
            output_plan::<a::Beacon, a::BeaconPage>(
                &caller,
                ProgramQuery::Beacons {
                    after,
                    limit: capacity as u16,
                },
                16 * has_after as usize,
                output,
                capacity,
                arena,
                arena_capacity,
                page,
            )
        },
        { status(beacons(&mut caller, output, arena, page)) }
    )?;

    metered!(
        linker,
        "beacon_read",
        |mut caller: Caller<'_, Host>,
         id: u32,
         output: u32,
         arena: u32,
         arena_capacity: u32,
         page: u32| {
            let id = Id(payload(&caller, id, 16)?.try_into().unwrap());
            output_plan::<a::Beacon, a::BeaconPage>(
                &caller,
                ProgramQuery::Beacon(id),
                16,
                output,
                1,
                arena,
                arena_capacity,
                page,
            )
        },
        { status(beacons(&mut caller, output, arena, page)) }
    )?;
    Ok(())
}
