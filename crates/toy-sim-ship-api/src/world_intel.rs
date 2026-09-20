use crate::abi::{Record, Text, private};
use crate::world::{Pose, Position};

macro_rules! record {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name { $(pub $field: $ty,)* }
        impl private::Sealed for $name {}
        impl Record for $name {}
    };
}

record!(Tag { kind: u64, id: [u8; 16], text: Text<64> });
record!(TrackQuery {
    flags: u64,
    track: [u8; 16],
    centre: Position,
    radius_m: f64,
    max_age_ticks: u64,
    work: u64,
    limit: u32,
    all_count: u32,
    any_count: u32,
    exclude_count: u32,
});
record!(Track {
    spatial_instance: [u8; 16],
    id: [u8; 16],
    entity: [u8; 16],
    pose: Pose,
    position_sigma_m: f64,
    velocity_sigma_m_s: f64,
    observed_tick: u64,
    estimate_tick: u64,
    radius_m: f64,
    appearance: [u8; 32],
    flags: u32,
    provenance: u32,
    tags_offset: u32,
    tags_count: u32,
});
record!(TrackPage {
    revision: u64,
    gas_used: u64,
    continuation: [u8; 16],
    count: u32,
    completion: u32,
    has_continuation: u32,
    arena_bytes: u32,
});
record!(Bay {
    index: u64,
    pose: Pose
});
record!(Beacon {
    entity: [u8; 16],
    pose: Pose,
    radius_m: f64,
    owner: [u8; 16],
    faction: [u8; 16],
    gate_exit: [u8; 16],
    range_m: f64,
    exclusion_m: f64,
    flags: u32,
    labels_offset: u32,
    labels_count: u32,
    bays_offset: u32,
    bays_count: u32,
    reserved: u32,
});
record!(BeaconPage {
    count: u32,
    arena_bytes: u32
});

#[cfg(target_arch = "wasm32")]
pub mod raw {
    use super::*;
    #[link(wasm_import_module = "ship_v31")]
    unsafe extern "C" {
        pub fn intel_tracks(
            query: *const TrackQuery,
            tags: *const Tag,
            output: *mut Track,
            capacity: u32,
            arena: *mut u8,
            arena_capacity: u32,
            page: *mut TrackPage,
        ) -> i32;
        pub fn intel_continue(
            cursor: *const u8,
            work: u64,
            output: *mut Track,
            capacity: u32,
            arena: *mut u8,
            arena_capacity: u32,
            page: *mut TrackPage,
        ) -> i32;
        pub fn beacons_read(
            after: *const u8,
            has_after: u32,
            output: *mut Beacon,
            capacity: u32,
            arena: *mut u8,
            arena_capacity: u32,
            page: *mut BeaconPage,
        ) -> i32;
        pub fn beacon_read(
            id: *const u8,
            output: *mut Beacon,
            arena: *mut u8,
            arena_capacity: u32,
            page: *mut BeaconPage,
        ) -> i32;
    }
}

#[cfg(target_arch = "wasm32")]
pub fn tracks(
    query: &TrackQuery,
    tags: &[Tag],
    output: &mut [Track],
    arena: &mut [u8],
) -> Result<TrackPage, i32> {
    let count = query
        .all_count
        .checked_add(query.any_count)
        .and_then(|n| n.checked_add(query.exclude_count));
    if count != Some(tags.len() as u32) {
        return Err(crate::abi::ERR_ARGUMENT);
    }
    let mut page = TrackPage::default();
    crate::sdk::check(unsafe {
        raw::intel_tracks(
            query,
            tags.as_ptr(),
            output.as_mut_ptr(),
            output.len() as u32,
            arena.as_mut_ptr(),
            arena.len() as u32,
            &mut page,
        )
    })?;
    Ok(page)
}

#[cfg(target_arch = "wasm32")]
pub fn continue_tracks(
    cursor: &[u8; 16],
    work: u64,
    output: &mut [Track],
    arena: &mut [u8],
) -> Result<TrackPage, i32> {
    let mut page = TrackPage::default();
    crate::sdk::check(unsafe {
        raw::intel_continue(
            cursor.as_ptr(),
            work,
            output.as_mut_ptr(),
            output.len() as u32,
            arena.as_mut_ptr(),
            arena.len() as u32,
            &mut page,
        )
    })?;
    Ok(page)
}

#[cfg(target_arch = "wasm32")]
pub fn beacons(
    after: Option<&[u8; 16]>,
    output: &mut [Beacon],
    arena: &mut [u8],
) -> Result<BeaconPage, i32> {
    let mut page = BeaconPage::default();
    crate::sdk::check(unsafe {
        raw::beacons_read(
            after.map_or(core::ptr::null(), |id| id.as_ptr()),
            u32::from(after.is_some()),
            output.as_mut_ptr(),
            output.len() as u32,
            arena.as_mut_ptr(),
            arena.len() as u32,
            &mut page,
        )
    })?;
    Ok(page)
}

#[cfg(target_arch = "wasm32")]
pub fn beacon(id: &[u8; 16], output: &mut Beacon, arena: &mut [u8]) -> Result<BeaconPage, i32> {
    let mut page = BeaconPage::default();
    crate::sdk::check(unsafe {
        raw::beacon_read(
            id.as_ptr(),
            output,
            arena.as_mut_ptr(),
            arena.len() as u32,
            &mut page,
        )
    })?;
    Ok(page)
}
