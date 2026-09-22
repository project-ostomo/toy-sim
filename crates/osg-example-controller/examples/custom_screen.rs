//! Extend the stock controller with a standalone custom screen.
//! Build for wasm32-unknown-unknown with --no-default-features.

#[cfg(target_arch = "wasm32")]
mod firmware {
    use osg_example_controller::firmware::Computer;
    use osg_ship_api::{abi, sdk};

    #[derive(Default)]
    struct Diagnostics {
        declared: bool,
        presses: u64,
        last_frame: Option<u64>,
    }

    impl Diagnostics {
        fn run(&mut self) -> Result<(), i32> {
            let tick = sdk::tick()?;

            if !self.declared {
                sdk::screen_define(&abi::ScreenDefinition {
                    id: 0,
                    width: 512,
                    height: 256,
                    title: abi::Text64::new("Custom diagnostics"),
                })?;
                self.declared = true;
            }

            for index in 0..tick.screen_event_count as u32 {
                let event = sdk::screen_event(index)?;

                if event.screen == 0 {
                    if event.kind == abi::EVENT_POINTER_PRESS
                        && event.code == 0
                        && (24. ..=240.).contains(&event.x)
                        && (100. ..=160.).contains(&event.y)
                    {
                        self.presses += 1;
                        self.last_frame = None;
                    }

                    if event.kind == abi::EVENT_KEY_PRESS && event.code == u64::from(b'R') {
                        self.presses = 0;
                        self.last_frame = None;
                    }
                }

                sdk::screen_event_ack(event.id)?;
            }

            if tick.requested_screens & 1 != 0
                && self
                    .last_frame
                    .is_none_or(|previous| tick.tick.saturating_sub(previous) >= 10)
            {
                let resources = sdk::resources()?;
                sdk::screen_begin(&abi::ScreenFrame {
                    id: 0,
                    background: 0,
                })?;
                text(24, 24, "CUSTOM FLIGHT COMPUTER SCREEN")?;
                text(
                    24,
                    55,
                    &format!("ENERGY {:.2} MJ", resources.energy_j as f64 / 1e6),
                )?;
                sdk::screen_draw(
                    0,
                    abi::DRAW_RECTANGLE,
                    &abi::ScreenRectangle {
                        color: 0x50ff78,
                        filled: 0,
                        x: 24,
                        y: 100,
                        width: 216,
                        height: 60,
                    },
                    &[],
                )?;
                text(40, 120, &format!("PRESSES {}", self.presses))?;
                text(24, 190, "Click the rectangle. R resets the count.")?;
                sdk::screen_end(0)?;
                self.last_frame = Some(tick.tick);
            }

            Ok(())
        }
    }

    fn text(x: i64, y: i64, message: &str) -> Result<(), i32> {
        sdk::screen_draw(
            0,
            abi::DRAW_TEXT,
            &abi::ScreenText {
                color: 0x50ff78,
                x,
                y,
            },
            message.as_bytes(),
        )
    }

    #[unsafe(no_mangle)]
    extern "C" fn game_version() -> u32 {
        osg_ship_api::GAME_VERSION as u32
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_tick() {
        static mut COMPUTER: Option<Computer> = None;
        let computer = unsafe { &mut *core::ptr::addr_of_mut!(COMPUTER) };
        let _ = computer.get_or_insert_with(Computer::default).run();
    }

    #[unsafe(no_mangle)]
    extern "C" fn ship_display() {
        static mut DIAGNOSTICS: Option<Diagnostics> = None;
        let diagnostics = unsafe { &mut *core::ptr::addr_of_mut!(DIAGNOSTICS) };
        let _ = diagnostics.get_or_insert_with(Diagnostics::default).run();
    }
}
