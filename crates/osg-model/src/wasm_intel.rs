use crate::*;
use osg_ship_api::{abi::Record, world_intel as w};
use std::mem::size_of;

pub fn track_arena_bytes(track: &Track) -> usize {
    track.tags.len() * size_of::<w::Tag>()
}

pub fn track_record_bytes() -> usize {
    size_of::<w::Track>()
}

pub fn track_page_bytes() -> usize {
    size_of::<w::TrackPage>()
}

pub fn beacon_arena_bytes(beacon: &Beacon) -> usize {
    beacon.iff.labels.len() * size_of::<w::Tag>() + beacon.bays.len() * size_of::<w::Bay>()
}

pub fn reply_fits(reply: &ProgramReply, capacity: wasm_world::ReplyCapacity) -> bool {
    let (records, auxiliary, bytes) = match reply {
        ProgramReply::Tracks(page) => (
            page.tracks.len(),
            0,
            page.tracks.iter().map(track_arena_bytes).sum(),
        ),
        ProgramReply::Beacons(beacons) => (
            beacons.len(),
            0,
            beacons.iter().map(beacon_arena_bytes).sum(),
        ),
        ProgramReply::Orrery(bodies) => (bodies.len(), 0, 0),
        ProgramReply::Route {
            status: routing::Status::Ready { plan },
            ..
        } => (plan.orders.len(), plan.fuel_budget.resources.len(), 0),
        _ => (0, 0, 0),
    };
    records <= capacity.records && auxiliary <= capacity.auxiliary && bytes <= capacity.bytes
}

impl From<&Tag> for w::Tag {
    fn from(tag: &Tag) -> Self {
        let mut result = Self::default();
        match tag {
            Tag::Kind(text) => {
                result.kind = 0;
                result.text = osg_ship_api::abi::Text::new(text);
            }
            Tag::IffOwner(id) => {
                result.kind = 1;
                result.id = id.0;
            }
            Tag::IffFaction(id) => {
                result.kind = 2;
                result.id = id.0;
            }
            Tag::Advertised(text) => {
                result.kind = 3;
                result.text = osg_ship_api::abi::Text::new(text);
            }
            Tag::Annotation(text) => {
                result.kind = 4;
                result.text = osg_ship_api::abi::Text::new(text);
            }
        }
        result
    }
}

impl TryFrom<&w::Tag> for Tag {
    type Error = ();
    fn try_from(tag: &w::Tag) -> Result<Self, ()> {
        let result = match tag.kind {
            0 => Self::Kind(tag.text.as_str().ok_or(())?.into()),
            1 => Self::IffOwner(Id(tag.id)),
            2 => Self::IffFaction(Id(tag.id)),
            3 => Self::Advertised(tag.text.as_str().ok_or(())?.into()),
            4 => Self::Annotation(tag.text.as_str().ok_or(())?.into()),
            _ => return Err(()),
        };
        result.valid().then_some(result).ok_or(())
    }
}

pub fn decode_query(query: &w::TrackQuery, tags: &[w::Tag]) -> Result<TrackQuery, ()> {
    let all = query.all_count as usize;
    let any = query.any_count as usize;
    let exclude = query.exclude_count as usize;
    if query.flags & !7 != 0 || all + any + exclude != tags.len() || tags.len() > 32 {
        return Err(());
    }
    Ok(TrackQuery {
        track: (query.flags & 1 != 0).then_some(Id(query.track)),
        sphere: (query.flags & 2 != 0)
            .then(|| (wasm_world::position_value(&query.centre), query.radius_m)),
        max_age_ticks: (query.flags & 4 != 0).then_some(query.max_age_ticks),
        all: tags[..all]
            .iter()
            .map(Tag::try_from)
            .collect::<Result<_, _>>()?,
        any: tags[all..all + any]
            .iter()
            .map(Tag::try_from)
            .collect::<Result<_, _>>()?,
        exclude: tags[all + any..]
            .iter()
            .map(Tag::try_from)
            .collect::<Result<_, _>>()?,
        limit: query.limit.try_into().map_err(|_| ())?,
        work: query.work,
    })
}

fn append<T: Record>(arena: &mut Vec<u8>, record: &T) {
    arena.extend_from_slice(record.bytes());
}

pub fn encode_track(track: &Track, arena: &mut Vec<u8>) -> w::Track {
    let tags_offset = arena.len() as u32;
    for tag in &track.tags {
        append(arena, &w::Tag::from(tag));
    }
    w::Track {
        spatial_instance: track.spatial_instance.0,
        id: track.id.0,
        entity: track.entity.unwrap_or(Id([0; 16])).0,
        pose: (&track.pose).into(),
        position_sigma_m: track.position_sigma_m,
        velocity_sigma_m_s: track.velocity_sigma_m_s,
        observed_tick: track.observed_tick,
        estimate_tick: track.estimate_tick,
        radius_m: track.radius_m.unwrap_or_default(),
        appearance: track.appearance.unwrap_or([0; 32]),
        flags: u32::from(track.entity.is_some())
            | (u32::from(track.radius_m.is_some()) << 1)
            | (u32::from(track.appearance.is_some()) << 2),
        provenance: match track.provenance {
            Provenance::Sensor => 0,
            Provenance::Transponder => 1,
            Provenance::GroupMember => 2,
            Provenance::Beacon => 3,
            Provenance::Extrapolated => 4,
        },
        tags_offset,
        tags_count: track.tags.len() as u32,
    }
}

pub fn encode_beacon(beacon: &Beacon, arena: &mut Vec<u8>) -> w::Beacon {
    let labels_offset = arena.len() as u32;
    for label in &beacon.iff.labels {
        append(arena, &w::Tag::from(&Tag::Advertised(label.clone())));
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
        pose: (&beacon.pose).into(),
        radius_m: beacon.radius_m,
        owner: beacon.iff.owner.0,
        faction: beacon.iff.faction.unwrap_or(Id([0; 16])).0,
        range_m: beacon.iff.range_m,
        flags: u32::from(beacon.iff.enabled) | (u32::from(beacon.iff.faction.is_some()) << 1),
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

pub fn decode_track(record: &w::Track, arena: &[u8]) -> Result<Track, ()> {
    Ok(Track {
        spatial_instance: Id(record.spatial_instance),
        id: Id(record.id),
        entity: (record.flags & 1 != 0).then_some(Id(record.entity)),
        pose: (&record.pose).into(),
        position_sigma_m: record.position_sigma_m,
        velocity_sigma_m_s: record.velocity_sigma_m_s,
        observed_tick: record.observed_tick,
        estimate_tick: record.estimate_tick,
        radius_m: (record.flags & 2 != 0).then_some(record.radius_m),
        appearance: (record.flags & 4 != 0).then_some(record.appearance),
        tags: arena_records::<w::Tag>(arena, record.tags_offset, record.tags_count)?
            .iter()
            .map(Tag::try_from)
            .collect::<Result<_, _>>()?,
        provenance: match record.provenance {
            0 => Provenance::Sensor,
            1 => Provenance::Transponder,
            2 => Provenance::GroupMember,
            3 => Provenance::Beacon,
            4 => Provenance::Extrapolated,
            _ => return Err(()),
        },
    })
}

pub fn decode_beacon(record: &w::Beacon, arena: &[u8]) -> Result<Beacon, ()> {
    Ok(Beacon {
        entity: Id(record.entity),
        pose: (&record.pose).into(),
        radius_m: record.radius_m,
        iff: IffIdentity {
            owner: Id(record.owner),
            faction: (record.flags & 2 != 0).then_some(Id(record.faction)),
            enabled: record.flags & 1 != 0,
            range_m: record.range_m,
            labels: arena_records::<w::Tag>(arena, record.labels_offset, record.labels_count)?
                .iter()
                .map(|tag| tag.text.as_str().map(str::to_owned).ok_or(()))
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
        ProgramQuery::Tracks(_) | ProgramQuery::Continue { .. } => {
            let capacity = match request {
                ProgramQuery::Tracks(query) => query.limit as usize,
                _ => 256,
            };
            let mut output = vec![w::Track::default(); capacity];
            let mut arena = vec![0u8; capacity * 32 * size_of::<w::Tag>()];
            let mut page = w::TrackPage::default();
            let status = match request {
                ProgramQuery::Tracks(query) => {
                    let tags: Vec<_> = query
                        .all
                        .iter()
                        .chain(&query.any)
                        .chain(&query.exclude)
                        .map(w::Tag::from)
                        .collect();
                    let record = w::TrackQuery {
                        flags: u64::from(query.track.is_some())
                            | (u64::from(query.sphere.is_some()) << 1)
                            | (u64::from(query.max_age_ticks.is_some()) << 2),
                        track: query.track.unwrap_or(Id([0; 16])).0,
                        centre: wasm_world::position_record(
                            &query.sphere.map_or(GalacticPosition::ZERO, |v| v.0),
                        ),
                        radius_m: query.sphere.map_or(0., |v| v.1),
                        max_age_ticks: query.max_age_ticks.unwrap_or_default(),
                        work: query.work,
                        limit: query.limit as u32,
                        all_count: query.all.len() as u32,
                        any_count: query.any.len() as u32,
                        exclude_count: query.exclude.len() as u32,
                    };
                    unsafe {
                        w::raw::intel_tracks(
                            &record,
                            tags.as_ptr(),
                            output.as_mut_ptr(),
                            capacity as u32,
                            arena.as_mut_ptr(),
                            arena.len() as u32,
                            &mut page,
                        )
                    }
                }
                ProgramQuery::Continue { cursor, work } => unsafe {
                    w::raw::intel_continue(
                        cursor.0.as_ptr(),
                        *work,
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
            let records = output.get(..page.count as usize).ok_or(ERR_ARGUMENT)?;
            let arena = arena.get(..page.arena_bytes as usize).ok_or(ERR_ARGUMENT)?;
            Ok(ProgramReply::Tracks(QueryPage {
                revision: page.revision,
                gas_used: page.gas_used,
                continuation: (page.has_continuation != 0).then_some(Id(page.continuation)),
                completion: match page.completion {
                    0 => Completion::Complete,
                    1 => Completion::ResultLimit,
                    2 => Completion::WorkLimit,
                    _ => return Err(ERR_ARGUMENT),
                },
                tracks: records
                    .iter()
                    .map(|record| decode_track(record, arena))
                    .collect::<Result<_, _>>()
                    .map_err(|_| ERR_ARGUMENT)?,
            }))
        }
        ProgramQuery::Beacon(_) | ProgramQuery::Beacons { .. } => {
            let capacity = match request {
                ProgramQuery::Beacons { limit, .. } => *limit as usize,
                _ => 1,
            };
            let mut output = vec![w::Beacon::default(); capacity];
            let mut arena =
                vec![0u8; capacity * (32 * size_of::<w::Bay>() + 32 * size_of::<w::Tag>())];
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
            radius_m: 2000.,
            entity: Id([1; 16]),
            pose: Pose::default(),
            iff: IffIdentity {
                owner: Id([2; 16]),
                faction: Some(Id([3; 16])),
                labels: BTreeSet::from(["Anchorage".to_owned(), "station".to_owned()]),
                enabled: true,
                range_m: 1e20,
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

    #[test]
    fn track_roundtrip_preserves_uncertainty_and_variable_tags() {
        let track = Track {
            spatial_instance: Id([1; 16]),
            id: Id([2; 16]),
            entity: Some(Id([3; 16])),
            pose: Pose::default(),
            position_sigma_m: 37.,
            velocity_sigma_m_s: 4.,
            observed_tick: 10,
            estimate_tick: 12,
            tags: BTreeSet::from([
                Tag::Kind("ship".into()),
                Tag::IffOwner(Id([4; 16])),
                Tag::Annotation("ally".into()),
            ]),
            provenance: Provenance::Transponder,
            radius_m: Some(12.),
            appearance: Some([7; 32]),
        };
        let mut arena = Vec::new();
        let record = encode_track(&track, &mut arena);
        assert_eq!(decode_track(&record, &arena).unwrap(), track);
        assert_eq!(arena.len(), track_arena_bytes(&track));
    }
}
