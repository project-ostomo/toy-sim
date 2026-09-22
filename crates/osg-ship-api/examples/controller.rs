//! Minimal firmware exercising persistent guest state and host publications.
#![cfg_attr(target_arch = "wasm32", no_std)]
#![cfg_attr(target_arch = "wasm32", no_main)]

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
mod controller {
    use osg_ship_api::{abi, sdk};

    static mut CALLBACKS: u64 = 0;

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        core::arch::wasm32::unreachable()
    }

    #[unsafe(no_mangle)]
    extern "C" fn game_version() -> u32 {
        osg_ship_api::GAME_VERSION as u32
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_tick() {
        let Ok(tick) = sdk::tick() else {
            return;
        };
        let flight: Result<abi::FlightState, _> =
            sdk::read(|p, n| unsafe { abi::raw::flight_read(p, n) });
        let Ok(flight) = flight else {
            return;
        };

        // Guest callbacks execute serially on one WebAssembly instance.
        let first = unsafe {
            CALLBACKS += 1;
            CALLBACKS == 1
        };
        let setting = abi::ThrottleSetting {
            fraction: if first { 0.25 } else { 0.5 },
        };
        let _ = sdk::write(&setting, |p, n| unsafe {
            abi::raw::device_write(1, abi::SET_THROTTLE, p, n)
        });
        let attitude = abi::AttitudeState {
            valid_until_s: tick.time_s + 1.,
            mode: abi::ATTITUDE_MANUAL,
            ..Default::default()
        };
        let _ = sdk::write(&attitude, |p, n| unsafe {
            abi::raw::instrument_attitude_put(p, n)
        });
        let path = abi::SpatialPath {
            meta: abi::SpatialMeta {
                id: 1,
                role: abi::PATH_OWN_FORECAST,
                valid_until_s: tick.time_s + 1.,
                ..Default::default()
            },
            frame: abi::SpatialFrame {
                kind: abi::FRAME_SNAPSHOT,
                reference: tick.snapshot,
                origin_velocity_m_s: flight.velocity,
                ..Default::default()
            },
            kind: abi::PATH_TIMED,
            subject_contact: 0,
        };
        let vertices = [
            abi::SpatialVertex {
                time_s: tick.time_s,
                position_m: [0.; 3],
            },
            abi::SpatialVertex {
                time_s: tick.time_s + 60.,
                position_m: [0., 42., 0.],
            },
        ];
        let _ = sdk::path(&path, &vertices);
    }
}
