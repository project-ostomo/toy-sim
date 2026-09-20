use super::*;
use crate::spatial::{Marker, Path, Snapshot};

fn metadata(
    caller: &Caller<'_, Host>,
    meta: w::SpatialMeta,
    maximum_role: u64,
) -> CallResult<bool> {
    let active = lease_active(caller, meta.valid_until_s)?;

    if meta.id == 0 || meta.role > maximum_role || meta.label.as_str().is_none() {
        return Err(w::ERR_ARGUMENT.into());
    }

    Ok(active)
}

fn frame(
    caller: &Caller<'_, Host>,
    frame: w::SpatialFrame,
    marker: bool,
    active: bool,
) -> CallResult<Option<Snapshot>> {
    finite(&frame.origin_velocity_m_s)?;

    match frame.kind {
        w::FRAME_SHIP | w::FRAME_SHIP_BODY if frame.reference == 0 => {}
        w::FRAME_SNAPSHOT | w::FRAME_CONTACT if frame.reference != 0 => {}
        w::FRAME_PATH if marker && frame.reference != 0 => {}
        _ => return Err(w::ERR_ARGUMENT.into()),
    }

    if frame.kind != w::FRAME_SNAPSHOT && frame.origin_velocity_m_s != [0.; 3] {
        return Err(w::ERR_ARGUMENT.into());
    }

    if !active {
        return Ok(None);
    }

    let host = caller.data();

    if frame.kind == w::FRAME_SNAPSHOT {
        return host
            .working
            .snapshot(frame.reference, host.current, host.borrowed_snapshot)
            .map(Some)
            .ok_or_else(|| w::ERR_HANDLE.into());
    }

    match frame.kind {
        w::FRAME_SHIP | w::FRAME_SHIP_BODY if frame.reference == 0 => Ok(None),
        w::FRAME_CONTACT => {
            let track = host
                .working
                .spatial
                .tracks
                .get(&frame.reference)
                .ok_or(w::ERR_UNAVAILABLE)?;

            if track.at(host.current.epoch).is_none() {
                return Err(w::ERR_UNAVAILABLE.into());
            }

            Ok(None)
        }
        w::FRAME_PATH if marker => {
            let path = host
                .working
                .spatial
                .paths
                .get(&frame.reference)
                .ok_or(w::ERR_UNAVAILABLE)?;

            if path.header.kind != w::PATH_TIMED || !path.active(host.current.epoch) {
                return Err(w::ERR_UNAVAILABLE.into());
            }

            Ok(None)
        }
        _ => Err(w::ERR_ARGUMENT.into()),
    }
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "spatial_marker_put",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::SpatialMarker>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let record: w::SpatialMarker = input(&mut caller, pointer, bytes)?;
                let active = metadata(&caller, record.meta, w::MARKER_EVENT)?;
                finite(&record.offset_m)?;
                finite(&[record.time_s])?;
                let snapshot = frame(&caller, record.frame, true, active)?;

                match record.time_mode {
                    w::TIME_CURRENT if record.time_s == 0. => {}
                    w::TIME_FIXED
                        if matches!(record.frame.kind, w::FRAME_SNAPSHOT | w::FRAME_PATH)
                            && record.time_s >= 0. => {}
                    _ => return Err(w::ERR_ARGUMENT.into()),
                }

                if snapshot.is_some_and(|snapshot| {
                    record.time_mode == w::TIME_FIXED && record.time_s < snapshot.epoch
                }) {
                    return Err(w::ERR_ARGUMENT.into());
                }

                if !active {
                    return Ok(());
                }

                let spatial = &caller.data().working.spatial;

                if spatial.paths.contains_key(&record.meta.id) {
                    return Err(w::ERR_ARGUMENT.into());
                }

                if !spatial.markers.contains_key(&record.meta.id)
                    && spatial.markers.len() >= w::MAX_MARKERS as usize
                {
                    return Err(w::ERR_LIMIT.into());
                }

                let path_revision = if record.frame.kind == w::FRAME_PATH {
                    let path = &spatial.paths[&record.frame.reference];

                    if record.time_mode == w::TIME_FIXED && path.at(record.time_s).is_none() {
                        return Err(w::ERR_ARGUMENT.into());
                    }

                    Some(path.revision)
                } else {
                    None
                };
                let published_at = caller.data().current.epoch;
                let spatial = &mut caller.data_mut().working.spatial;
                spatial.revision = spatial.revision.wrapping_add(1);
                spatial.markers.insert(
                    record.meta.id,
                    Marker {
                        record,
                        snapshot,
                        path_revision,
                        published_at,
                    },
                );
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "spatial_path_put",
        |mut caller: Caller<'_, Host>,
         pointer: u32,
         bytes: u32,
         vertex_pointer: u32,
         count: u32| { path_plan(&caller, pointer, bytes, vertex_pointer, count) },
        {
            status((|| {
                if !(2..=w::MAX_PATH_VERTICES).contains(&count) {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let header: w::SpatialPath = input(&mut caller, pointer, bytes)?;
                let active = metadata(&caller, header.meta, w::PATH_ROUTE)?;
                let snapshot = frame(&caller, header.frame, false, active)?;

                if header.kind > w::PATH_TIMED
                    || (header.kind == w::PATH_TIMED && header.frame.kind != w::FRAME_SNAPSHOT)
                {
                    return Err(w::ERR_ARGUMENT.into());
                }

                match header.meta.role {
                    w::PATH_OWN_FORECAST
                        if header.kind == w::PATH_TIMED && header.subject_contact == 0 => {}
                    w::PATH_CONTACT_FORECAST
                        if header.kind == w::PATH_TIMED && header.subject_contact != 0 => {}
                    w::PATH_REFERENCE | w::PATH_ROUTE if header.subject_contact == 0 => {}
                    _ => return Err(w::ERR_ARGUMENT.into()),
                }

                let vertex_bytes = count * size_of::<w::SpatialVertex>() as u32;
                memory_range(&caller, vertex_pointer, vertex_bytes)?;
                let vertices: Vec<_> = payload(&caller, vertex_pointer, vertex_bytes)?
                    .chunks_exact(size_of::<w::SpatialVertex>())
                    .map(|bytes| w::SpatialVertex::read(bytes).unwrap())
                    .collect();

                for vertex in &vertices {
                    finite(&vertex.position_m)?;
                    finite(&[vertex.time_s])?;

                    if header.kind == w::PATH_POLYLINE {
                        if vertex.time_s != 0. {
                            return Err(w::ERR_ARGUMENT.into());
                        }
                    } else if vertex.time_s < 0.
                        || snapshot.is_some_and(|snapshot| vertex.time_s < snapshot.epoch)
                    {
                        return Err(w::ERR_ARGUMENT.into());
                    }
                }

                if header.kind == w::PATH_TIMED
                    && !vertices
                        .windows(2)
                        .all(|pair| pair[0].time_s < pair[1].time_s)
                {
                    return Err(w::ERR_ARGUMENT.into());
                }

                if !active {
                    return Ok(());
                }

                let spatial = &caller.data().working.spatial;

                if spatial.markers.contains_key(&header.meta.id) {
                    return Err(w::ERR_ARGUMENT.into());
                }

                if !spatial.paths.contains_key(&header.meta.id)
                    && spatial.paths.len() >= w::MAX_PATHS as usize
                {
                    return Err(w::ERR_LIMIT.into());
                }

                let other_vertices: usize = spatial
                    .paths
                    .iter()
                    .filter(|(id, _)| **id != header.meta.id)
                    .map(|(_, path)| path.vertices.len())
                    .sum();

                if other_vertices + count as usize > w::MAX_TOTAL_VERTICES as usize {
                    return Err(w::ERR_LIMIT.into());
                }

                let published_at = caller.data().current.epoch;
                let spatial = &mut caller.data_mut().working.spatial;
                spatial.revision = spatial.revision.wrapping_add(1);
                spatial.paths.insert(
                    header.meta.id,
                    Path {
                        header,
                        snapshot,
                        vertices: vertices.into(),
                        revision: spatial.revision,
                        published_at,
                    },
                );
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "spatial_remove",
        |mut caller: Caller<'_, Host>, id: u64| { CallPlan::fixed() },
        {
            status((|| {
                if id == 0 {
                    return Err(w::ERR_ARGUMENT.into());
                }

                caller.data_mut().working.spatial.remove(id);
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "spatial_clear",
        |mut caller: Caller<'_, Host>| { CallPlan::fixed() },
        {
            status((|| {
                caller.data_mut().working.spatial.clear();
                Ok(())
            })())
        },
    )?;

    Ok(())
}
