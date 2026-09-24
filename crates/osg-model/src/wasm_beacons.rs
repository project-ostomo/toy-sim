use crate::*;
use osg_ship_api::{abi::Record, beacons as w};
use std::mem::size_of;

pub fn beacon_arena_bytes(beacon: &Beacon) -> usize {
    beacon.iff.labels.len() * size_of::<osg_ship_api::abi::Text64>()
        + beacon.bays.len() * size_of::<w::Bay>()
}

pub fn reply_fits(reply: &ProgramReply, capacity: wasm_world::ReplyCapacity) -> bool {
    let (records, auxiliary, bytes) = match reply {
        ProgramReply::Beacons(beacons) => (
            beacons.len(),
            0,
            beacons.iter().map(beacon_arena_bytes).sum(),
        ),
        ProgramReply::Orrery(bodies) => (bodies.len(), 0, 0),
        ProgramReply::Route {
            status: routing::Status::Ready { plan },
            ..
        } => (plan.itinerary.len(), plan.fuel_budget.resources.len(), 0),
        ProgramReply::Travel {
            state, location, ..
        } => (
            state.itinerary.len(),
            state
                .fuel_budget
                .as_ref()
                .map_or(0, |budget| budget.resources.len()),
            location.hierarchy.len() * size_of::<osg_ship_api::world::CelestialRef>(),
        ),
        ProgramReply::SlipEligibilityBatch(results) => (results.len(), 0, 0),
        _ => (0, 0, 0),
    };
    records <= capacity.records && auxiliary <= capacity.auxiliary && bytes <= capacity.bytes
}

fn append<T: Record>(arena: &mut Vec<u8>, record: &T) {
    arena.extend_from_slice(record.bytes());
}

pub fn encode_beacon(beacon: &Beacon, arena: &mut Vec<u8>) -> w::Beacon {
    let labels_offset = arena.len() as u32;
    for label in &beacon.iff.labels {
        append(arena, &osg_ship_api::abi::Text64::new(label));
    }
    let bays_offset = arena.len() as u32;
    for (index, pose) in &beacon.bays {
        append(
            arena,
            &w::Bay {
                index: *index as u64,
                pose: pose.into(),
            },
        );
    }
    w::Beacon {
        entity: beacon.entity.0,
        system: beacon.system.unwrap_or_default().0,
        pose: (&beacon.pose).into(),
        radius_m: beacon.radius_m,
        owner: beacon.iff.owner.0,
        faction: beacon.iff.faction.unwrap_or(Id([0; 16])).0,
        flags: u32::from(beacon.iff.enabled)
            | (u32::from(beacon.iff.faction.is_some()) << 1)
            | (u32::from(beacon.system.is_some()) << 2),
        labels_offset,
        labels_count: beacon.iff.labels.len() as u32,
        bays_offset,
        bays_count: beacon.bays.len() as u32,
        reserved: 0,
    }
}

fn arena_records<T: Record>(arena: &[u8], offset: u32, count: u32) -> Result<Vec<T>, ()> {
    let start = offset as usize;
    let length = (count as usize).checked_mul(size_of::<T>()).ok_or(())?;
    if !start.is_multiple_of(8) {
        return Err(());
    }
    arena
        .get(start..start.checked_add(length).ok_or(())?)
        .ok_or(())?
        .chunks_exact(size_of::<T>())
        .map(|bytes| T::read(bytes).ok_or(()))
        .collect()
}

pub fn decode_beacon(record: &w::Beacon, arena: &[u8]) -> Result<Beacon, ()> {
    Ok(Beacon {
        entity: Id(record.entity),
        system: (record.flags & 4 != 0).then_some(Id(record.system)),
        pose: (&record.pose).into(),
        radius_m: record.radius_m,
        iff: IffIdentity {
            owner: Id(record.owner),
            faction: (record.flags & 2 != 0).then_some(Id(record.faction)),
            enabled: record.flags & 1 != 0,
            labels: arena_records::<osg_ship_api::abi::Text64>(
                arena,
                record.labels_offset,
                record.labels_count,
            )?
            .iter()
            .map(|tag| tag.as_str().map(str::to_owned).ok_or(()))
            .collect::<Result<_, _>>()?,
        },
        bays: arena_records::<w::Bay>(arena, record.bays_offset, record.bays_count)?
            .iter()
            .map(|bay| Ok((bay.index.try_into().map_err(|_| ())?, (&bay.pose).into())))
            .collect::<Result<_, ()>>()?,
    })
}

#[cfg(target_arch = "wasm32")]
pub fn query(request: &ProgramQuery) -> Result<ProgramReply, i32> {
    use osg_ship_api::{abi::ERR_ARGUMENT, sdk::check};
    match request {
        ProgramQuery::Beacon(_) | ProgramQuery::Beacons { .. } => {
            let capacity = match request {
                ProgramQuery::Beacons { limit, .. } => *limit as usize,
                _ => 1,
            };
            let mut output = vec![w::Beacon::default(); capacity];
            let mut arena = vec![
                0u8;
                capacity
                    * (32 * size_of::<w::Bay>()
                        + 32 * size_of::<osg_ship_api::abi::Text64>())
            ];
            let mut page = w::BeaconPage::default();
            let status = match request {
                ProgramQuery::Beacon(id) => unsafe {
                    w::raw::beacon_read(
                        id.0.as_ptr(),
                        output.as_mut_ptr(),
                        arena.as_mut_ptr(),
                        arena.len() as u32,
                        &mut page,
                    )
                },
                ProgramQuery::Beacons { after, .. } => unsafe {
                    w::raw::beacons_read(
                        after.as_ref().map_or(std::ptr::null(), |id| id.0.as_ptr()),
                        u32::from(after.is_some()),
                        output.as_mut_ptr(),
                        capacity as u32,
                        arena.as_mut_ptr(),
                        arena.len() as u32,
                        &mut page,
                    )
                },
                _ => unreachable!(),
            };
            check(status)?;
            let arena = arena.get(..page.arena_bytes as usize).ok_or(ERR_ARGUMENT)?;
            let records = output.get(..page.count as usize).ok_or(ERR_ARGUMENT)?;
            Ok(ProgramReply::Beacons(
                records
                    .iter()
                    .map(|record| decode_beacon(record, arena))
                    .collect::<Result<_, _>>()
                    .map_err(|_| ERR_ARGUMENT)?,
            ))
        }
        _ => Err(ERR_ARGUMENT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_arena_exceeds_old_buffer_without_losing_bays_or_labels() {
        let beacon = Beacon {
            system: Some(Id([7; 16])),
            radius_m: 2000.,
            entity: Id([1; 16]),
            pose: Pose::default(),
            iff: IffIdentity {
                owner: Id([2; 16]),
                faction: Some(Id([3; 16])),
                labels: BTreeSet::from(["Anchorage".to_owned(), "station".to_owned()]),
                enabled: true,
            },
            bays: (0..1024)
                .map(|id| {
                    (
                        id,
                        Pose {
                            velocity: [id as f64, 0., 0.],
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        };
        let mut arena = Vec::new();
        let record = encode_beacon(&beacon, &mut arena);
        assert!(arena.len() > 65536);
        assert_eq!(arena.len(), beacon_arena_bytes(&beacon));
        let decoded = decode_beacon(&record, &arena).unwrap();
        assert_eq!(decoded.bays, beacon.bays);
        assert_eq!(decoded.iff, beacon.iff);
        assert!(decode_beacon(&record, &arena[..arena.len() - 1]).is_err());
    }
}
