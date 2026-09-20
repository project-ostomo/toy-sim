#[cfg(target_arch = "wasm32")]
mod firmware {
    use osg_example_controller::{chatter::Chatter, firmware::Computer};
    use osg_ship_api::{abi, sdk};

    #[unsafe(no_mangle)]
    extern "C" fn ship_api_version() -> u32 {
        abi::VERSION
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_tick() {
        static mut COMPUTER: Option<Computer> = None;
        static mut CHATTER: Option<Chatter> = None;
        let computer = unsafe { &mut *core::ptr::addr_of_mut!(COMPUTER) };
        match computer.get_or_insert_with(Computer::default).run() {
            Ok(_) | Err(abi::ERR_UNAVAILABLE) => {}
            Err(error) => panic!("flight computer syscall failed: {error}"),
        }
        let chatter = unsafe { &mut *core::ptr::addr_of_mut!(CHATTER) };
        let _ = chatter.get_or_insert_with(Chatter::default).run();
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_display() {
        let _ = osg_example_controller::firmware::draw_display();
    }

    #[unsafe(no_mangle)]
    extern "C" fn missile_tick(handle: u64) {
        if let Ok(observation) = sdk::missile() {
            if observation.handle == handle {
                let control = osg_example_controller::missile::guide(&observation);
                let _ = sdk::control_missile(&control);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
