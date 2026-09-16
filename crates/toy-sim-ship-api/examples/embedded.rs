//! Freestanding Rust firmware: no allocator, standard library, WASI, or serialization.
#![cfg_attr(target_arch = "wasm32", no_std)]
#![cfg_attr(target_arch = "wasm32", no_main)]

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
mod controller {
    use toy_sim_ship_api::{abi, sdk};

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        core::arch::wasm32::unreachable()
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_api_version() -> u32 {
        abi::VERSION
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_tick() {
        let Ok(tick) = sdk::tick() else {
            return;
        };
        let path = abi::SpatialPath {
            meta: abi::SpatialMeta {
                id: 1,
                role: abi::PATH_OWN_FORECAST,
                valid_until_s: tick.time_s + 1.,
                label: abi::Text64::new("Forecast"),
            },
            frame: abi::SpatialFrame {
                kind: abi::FRAME_SNAPSHOT,
                reference: tick.snapshot,
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
                time_s: tick.time_s + 10.,
                position_m: [0., 0., 100.],
            },
        ];
        let _ = sdk::path(&path, &vertices);

        for index in 0..tick.device_count as u32 {
            let device: Result<abi::DeviceInfo, _> =
                sdk::read(|p, n| unsafe { abi::raw::device_info(index, p, n) });
            let Ok(device) = device else {
                return;
            };

            if device.kind == abi::DEVICE_ENGINE {
                let setting = abi::ThrottleSetting { fraction: 0.25 };
                let _ = sdk::write(&setting, |p, n| unsafe {
                    abi::raw::device_write(device.id, abi::SET_THROTTLE, p, n)
                });
            }
        }
    }
}
