//! Fallible allocation-free helpers. Buffers and retained snapshot lifetimes stay explicit.
#[cfg(target_arch = "wasm32")]
use crate::abi;
use crate::abi::Record;

pub fn check(status: i32) -> Result<(), i32> {
    if status < 0 { Err(status) } else { Ok(()) }
}

pub fn read<T: Record>(call: impl FnOnce(*mut u8, u32) -> i32) -> Result<T, i32> {
    let mut value = T::default();
    check(call(
        (&mut value as *mut T).cast(),
        core::mem::size_of::<T>() as u32,
    ))?;
    Ok(value)
}

pub fn write<T: Record>(value: &T, call: impl FnOnce(*const u8, u32) -> i32) -> Result<(), i32> {
    check(call(
        value.bytes().as_ptr(),
        core::mem::size_of::<T>() as u32,
    ))
}

#[cfg(target_arch = "wasm32")]
pub fn tick() -> Result<abi::TickContext, i32> {
    read(|p, n| unsafe { abi::raw::tick_read(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn budget() -> Result<abi::BudgetInfo, i32> {
    read(|p, n| unsafe { abi::raw::budget_read(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn flight() -> Result<abi::FlightState, i32> {
    read(|p, n| unsafe { abi::raw::flight_read(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn keep_snapshot(id: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::snapshot_keep(id) })
}

#[cfg(target_arch = "wasm32")]
pub fn drop_snapshot(id: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::snapshot_drop(id) })
}

#[cfg(target_arch = "wasm32")]
pub fn marker(value: &abi::SpatialMarker) -> Result<(), i32> {
    write(value, |p, n| unsafe { abi::raw::spatial_marker_put(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn path(header: &abi::SpatialPath, vertices: &[abi::SpatialVertex]) -> Result<(), i32> {
    if vertices.len() > abi::MAX_PATH_VERTICES as usize {
        return Err(abi::ERR_LIMIT);
    }
    check(unsafe {
        abi::raw::spatial_path_put(
            header.bytes().as_ptr(),
            core::mem::size_of::<abi::SpatialPath>() as u32,
            vertices.as_ptr().cast(),
            vertices.len() as u32,
        )
    })
}

#[cfg(target_arch = "wasm32")]
pub fn resources() -> Result<abi::ShipResources, i32> {
    read(|p, n| unsafe { abi::raw::ship_resources_read(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn device(index: u32) -> Result<abi::DeviceInfo, i32> {
    read(|p, n| unsafe { abi::raw::device_info(index, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn device_group(device: u64, index: u32) -> Result<abi::Text64, i32> {
    read(|p, n| unsafe { abi::raw::device_group_read(device, index, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn resource_info(index: u32) -> Result<abi::ResourceInfo, i32> {
    read(|p, n| unsafe { abi::raw::resource_info(index, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn resource(id: u64) -> Result<abi::ResourceAmount, i32> {
    read(|p, n| unsafe { abi::raw::resource_read(id, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn contact_label(id: u64) -> Result<abi::Text64, i32> {
    read(|p, n| unsafe { abi::raw::contact_label(id, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn request(index: u32) -> Result<abi::RequestInfo, i32> {
    read(|p, n| unsafe { abi::raw::request_info(index, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_event(index: u32) -> Result<abi::ScreenEvent, i32> {
    read(|p, n| unsafe { abi::raw::screen_event_read(index, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn attitude(value: &abi::AttitudeState) -> Result<(), i32> {
    write(value, |p, n| unsafe {
        abi::raw::instrument_attitude_put(p, n)
    })
}

#[cfg(target_arch = "wasm32")]
pub fn navigation(value: &abi::NavigationState) -> Result<(), i32> {
    write(value, |p, n| unsafe {
        abi::raw::instrument_navigation_put(p, n)
    })
}

#[cfg(target_arch = "wasm32")]
pub fn contacts(value: &abi::ContactsState) -> Result<(), i32> {
    write(value, |p, n| unsafe {
        abi::raw::instrument_contacts_put(p, n)
    })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_define(value: &abi::ScreenDefinition) -> Result<(), i32> {
    write(value, |p, n| unsafe { abi::raw::screen_define(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_begin(value: &abi::ScreenFrame) -> Result<(), i32> {
    write(value, |p, n| unsafe { abi::raw::screen_begin(p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn interval(seconds: f64) -> Result<(), i32> {
    check(unsafe { abi::raw::tick_set_interval(seconds) })
}

#[cfg(target_arch = "wasm32")]
pub fn remove_spatial(id: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::spatial_remove(id) })
}

#[cfg(target_arch = "wasm32")]
pub fn clear_spatial() -> Result<(), i32> {
    check(unsafe { abi::raw::spatial_clear() })
}

#[cfg(target_arch = "wasm32")]
pub fn clear_instrument(kind: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::instrument_clear(kind) })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_remove(id: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::screen_remove(id) })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_end(id: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::screen_end(id) })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_event_ack(id: u64) -> Result<(), i32> {
    check(unsafe { abi::raw::screen_event_ack(id) })
}

#[cfg(target_arch = "wasm32")]
pub fn device_spec<T: Record>(id: u64, kind: u64) -> Result<T, i32> {
    read(|p, n| unsafe { abi::raw::device_spec(id, kind, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn device_read<T: Record>(id: u64, kind: u64) -> Result<T, i32> {
    read(|p, n| unsafe { abi::raw::device_read(id, kind, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn device_write<T: Record>(id: u64, setting: u64, value: &T) -> Result<(), i32> {
    write(value, |p, n| unsafe {
        abi::raw::device_write(id, setting, p, n)
    })
}

#[cfg(target_arch = "wasm32")]
pub fn request_read<T: Record>(index: u32, kind: u64) -> Result<T, i32> {
    read(|p, n| unsafe { abi::raw::request_read(index, kind, p, n) })
}

#[cfg(target_arch = "wasm32")]
pub fn request_reply(id: u64, result: u64, message: &str) -> Result<(), i32> {
    if message.len() > 256 {
        return Err(abi::ERR_LIMIT);
    }

    check(unsafe { abi::raw::request_reply(id, result, message.as_ptr(), message.len() as u32) })
}

/// The buffer length is the requested N; only the returned prefix is initialized with contacts.
#[cfg(target_arch = "wasm32")]
pub fn scan(sensor: u64, buffer: &mut [abi::Contact]) -> Result<usize, i32> {
    if buffer.len() > abi::MAX_CONTACTS as usize {
        return Err(abi::ERR_LIMIT);
    }

    let count = unsafe {
        abi::raw::sensor_scan(
            sensor,
            buffer.len() as u32,
            buffer.as_mut_ptr().cast(),
            core::mem::size_of_val(buffer) as u32,
        )
    };
    check(count)?;
    Ok(count as usize)
}

#[cfg(target_arch = "wasm32")]
pub fn screen_draw<T: Record>(
    screen: u64,
    kind: u64,
    parameters: &T,
    payload: &[u8],
) -> Result<(), i32> {
    if payload.len() > abi::MAX_SCREEN_PAYLOAD as usize {
        return Err(abi::ERR_LIMIT);
    }

    check(unsafe {
        abi::raw::screen_draw(
            screen,
            kind,
            parameters.bytes().as_ptr(),
            core::mem::size_of::<T>() as u32,
            payload.as_ptr(),
            payload.len() as u32,
        )
    })
}

#[cfg(target_arch = "wasm32")]
pub fn screen_button(screen: u64, key: u64, label: &str) -> Result<(), i32> {
    if label.len() > 24 {
        return Err(abi::ERR_LIMIT);
    }

    check(unsafe { abi::raw::screen_button(screen, key, label.as_ptr(), label.len() as u32) })
}

#[cfg(target_arch = "wasm32")]
pub fn weapons(state: &abi::WeaponsState, weapons: &[abi::WeaponInstrument]) -> Result<(), i32> {
    check(unsafe {
        abi::raw::instrument_weapons_put(
            state.bytes().as_ptr(),
            core::mem::size_of::<abi::WeaponsState>() as u32,
            weapons.as_ptr().cast(),
            weapons.len() as u32,
        )
    })
}
