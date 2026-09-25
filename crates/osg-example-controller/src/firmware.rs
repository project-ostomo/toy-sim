//! The bundled controller's syscall loop. Caches and scheduling belong to this firmware.
use crate::{
    Pilot,
    hardware::{Hardware, Sample},
    navigation::Phase,
};
use osg_ship_api::{abi, sdk};

const OWN_PATH: u64 = 1;
const TARGET_PATH: u64 = 2;
const TARGET_MARKER: u64 = 3;
const ARRIVAL_MARKER: u64 = 4;

#[derive(Default)]
pub struct Computer {
    pilot: Pilot,
    executor: crate::world::Executor,
    hardware: Hardware,
    weapons: crate::weapons::WeaponsController,
    scan_window: crate::budget::ScanWindow,
    console_started: bool,
    last_console_report: String,
    next_console_tick: u64,
}

impl Computer {
    pub fn run(&mut self) -> Result<abi::TickContext, i32> {
        let tick = sdk::tick()?;
        if !self.console_started {
            sdk::serial_write(
                "\x1b[2J\x1b[HSHIP COMPUTER // ONLINE\r\nDiscovering installed hardware...\r\n",
            )?;
            self.console_started = true;
        }

        if !self.hardware.discover(&tick)? {
            sdk::interval(0.)?;
            return Ok(tick);
        }

        self.hardware.measure()?;
        let mut propellant_kg = 0.;
        for resource in &self.hardware.propellants {
            propellant_kg += sdk::resource(resource.id)?.units as f64 * resource.unit_mass_kg;
        }
        let sample = Sample {
            tick,
            flight: sdk::flight()?,
            propellant_kg,
        };
        let mut contacts = [abi::Contact::default(); abi::MAX_CONTACTS as usize];
        let scan_limit = self.scan_window.limit(sdk::budget()?);
        let mut count = self
            .hardware
            .devices
            .iter()
            .find(|device| device.info.kind == abi::DEVICE_SENSOR && device.available())
            .and_then(|sensor| sdk::scan(sensor.info.id, &mut contacts[..scan_limit]).ok())
            .unwrap_or(0);
        self.scan_window.observed(scan_limit, count);
        let travel_contact = self
            .executor
            .update(tick.tick, &sample, &self.hardware, &contacts[..count])
            .ok()
            .flatten();
        let resumed_tick = sdk::tick()?;
        if resumed_tick.tick != tick.tick {
            // An unusually small gas grant can still suspend a world query.
            // Refresh sensors before applying any action from an expired sample.
            sdk::interval(0.)?;
            return Ok(resumed_tick);
        }
        if self.executor.reference_changed
            && self
                .pilot
                .navigation
                .target
                .as_ref()
                .is_some_and(|target| target.id == u64::MAX)
        {
            self.pilot.reset_navigation_reference();
        }
        if let Some(contact) = travel_contact {
            count = count.min(contacts.len() - 1);
            contacts[count] = contact;
            count += 1;
            if self
                .pilot
                .navigation
                .target
                .as_ref()
                .is_none_or(|target| target.id != u64::MAX)
                || !self.pilot.navigation.phase.active()
            {
                let _ = self
                    .pilot
                    .navigation
                    .select(u64::MAX, &contacts[..count], &sample);
                let _ = self.pilot.navigation.start(1., 0., sample.radius_m);
            }
        } else if self
            .pilot
            .navigation
            .target
            .as_ref()
            .is_some_and(|target| target.id == u64::MAX)
        {
            self.pilot.navigation.abort();
        }
        self.pilot.navigation.speed_limit = self.executor.speed_limit;
        if let Some(direction) = self.executor.aim_direction {
            let request = abi::DirectionRequest { direction };
            let _ = self.pilot.request(
                abi::REQUEST_AIM_DIRECTION,
                abi::Record::bytes(&request),
                &sample,
                &self.hardware,
            );
        }
        if let Some(rotation) = self.executor.aim_attitude {
            let _ = self
                .pilot
                .request(abi::REQUEST_HOLD_ATTITUDE, &[], &sample, &self.hardware);
            self.pilot.hold = Some(glam::DQuat::from_array(rotation));
        }
        self.weapons.observe(&sample, &contacts[..count]);
        self.pilot
            .observe(&sample, &self.hardware, &contacts[..count]);

        for index in 0..tick.request_count as u32 {
            let request = sdk::request(index)?;
            let result = self.dispatch(index, request.kind, &sample);
            let (status, message) = match &result {
                Ok(()) => (abi::REPLY_ACCEPTED, ""),
                Err(message) => (abi::REPLY_REJECTED, message.as_str()),
            };
            sdk::request_reply(request.id, status, message)?;
        }

        let actuations = self.pilot.control(&sample, &self.hardware);
        for actuation in actuations {
            actuation.apply()?;
        }

        let weapons = self.weapons.update(&sample, &self.hardware);
        for (id, setting) in &weapons.settings {
            sdk::device_write(*id, abi::SET_WEAPON, setting)?;
        }
        for marker in &weapons.markers {
            sdk::marker(marker)?;
        }
        for id in (16 + weapons.markers.len() as u64)..24 {
            sdk::remove_spatial(id)?;
        }
        sdk::weapons(&weapons.state, &weapons.rows)?;

        self.publish(&sample)?;
        // Expensive world observations are admitted only after this tick's
        // requests and actuator writes. The planner evaluates one candidate.
        let _ = self.executor.refine();
        let navigation = &self.pilot.navigation;
        let phase = match navigation.phase {
            Phase::Ready if self.executor.active => "WAIT",
            Phase::Ready => "IDLE",
            Phase::Paused => "PAUSED",
            Phase::Pursuing if navigation.pointing_error > 0.15 => "TURN",
            Phase::Pursuing if navigation.throttle < 0.01 => "COAST",
            Phase::Pursuing if navigation.acceleration.dot(navigation.u) < 0. => "BRAKE",
            Phase::Pursuing => "BURN",
        };
        let report = if navigation.phase.active() && navigation.visible {
            let distance = navigation.r.length();
            let range = if distance >= 1e6 {
                format!("{:.2} Mm", distance / 1e6)
            } else if distance >= 1e3 {
                format!("{:.2} km", distance / 1e3)
            } else {
                format!("{:.0} m", distance)
            };
            let closing = navigation.u.dot(navigation.r.normalize_or_zero());
            format!("{phase} target {range} closing {closing:.0} m/s")
        } else if navigation.phase == Phase::Paused {
            format!("{phase} {}", navigation.reason)
        } else {
            phase.to_owned()
        };
        let report = self.executor.console_status().unwrap_or(report);
        if report != self.last_console_report && tick.tick >= self.next_console_tick {
            sdk::serial_write(&format!("[{}] {}\r\n", tick.tick, report))?;
            self.last_console_report = report;
            self.next_console_tick = tick.tick.saturating_add(10);
        }
        sdk::interval(0.)?;
        Ok(tick)
    }

    fn dispatch(&mut self, index: u32, kind: u64, sample: &Sample) -> Result<(), String> {
        use abi::Record;

        let failed = |_| "Could not read request payload".to_owned();
        if matches!(
            kind,
            abi::REQUEST_THROTTLE
                | abi::REQUEST_MANUAL
                | abi::REQUEST_AIM_DIRECTION
                | abi::REQUEST_AIM_CONTACT
                | abi::REQUEST_ENGAGE_NAVIGATION
                | abi::REQUEST_STOP_GUIDANCE
                | abi::REQUEST_HOLD_ATTITUDE
        ) {
            self.executor.set_guidance(None);
        }
        match kind {
            abi::REQUEST_COMPUTER_MESSAGE => {
                let message: abi::ComputerMessage =
                    sdk::request_read(index, kind).map_err(failed)?;
                let bytes = message
                    .bytes
                    .get(..message.len as usize)
                    .ok_or("Invalid computer message")?;
                let control: crate::display::ComputerControl = postcard::from_bytes(bytes)
                    .map_err(|_| "Unknown computer message".to_owned())?;
                let osg_model::ProgramReply::Travel { presence, .. } =
                    osg_model::wasm_world::query(&osg_model::ProgramQuery::Travel)
                        .map_err(|_| "Autopilot state unavailable".to_owned())?
                else {
                    return Err("Autopilot state unavailable".into());
                };
                if !matches!(
                    presence,
                    osg_model::travel::Presence::Space | osg_model::travel::Presence::Docked { .. }
                ) {
                    return Err("Wait for arrival before changing the autopilot".into());
                }
                osg_model::wasm_world::command(control.action())
                    .map_err(|error| format!("Could not change autopilot ({error})"))?;
                self.executor.set_guidance(None);
                self.pilot.request(
                    abi::REQUEST_MANUAL,
                    abi::ManualRequest::default().bytes(),
                    sample,
                    &self.hardware,
                )?;
                self.pilot
                    .request(abi::REQUEST_HOLD_ATTITUDE, &[], sample, &self.hardware)
            }
            abi::REQUEST_SET_GUIDANCE => {
                let value: osg_ship_api::world::Guidance =
                    sdk::request_read(index, kind).map_err(failed)?;
                let guidance = osg_model::wasm_world::guidance_value(&value)
                    .map_err(|_| "Invalid guidance request".to_owned())?;
                self.executor.set_guidance(guidance);
                self.pilot.navigation.abort();
                Ok(())
            }
            abi::REQUEST_MARK_TARGET => {
                let value = sdk::request_read(index, kind).map_err(failed)?;
                self.weapons.mark_target(value, sample.tick.time_s)
            }
            abi::REQUEST_START_FIRING => self.weapons.start_firing(),
            abi::REQUEST_UNMARK_TARGET => {
                self.weapons.unmark_target();
                Ok(())
            }
            abi::REQUEST_STOP_FIRING => {
                self.weapons.stop_firing();
                Ok(())
            }
            abi::REQUEST_THROTTLE => {
                let value: abi::ThrottleRequest = sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_MANUAL => {
                let value: abi::ManualRequest = sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_AIM_DIRECTION => {
                let value: abi::DirectionRequest =
                    sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_AIM_CONTACT | abi::REQUEST_SELECT_TARGET => {
                let value: abi::ContactRequest = sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_ENGAGE_NAVIGATION => {
                let value: abi::NavigationRequest =
                    sdk::request_read(index, kind).map_err(failed)?;
                self.pilot
                    .request(kind, value.bytes(), sample, &self.hardware)
            }
            abi::REQUEST_HOLD_ATTITUDE | abi::REQUEST_STOP_GUIDANCE => {
                self.pilot.request(kind, &[], sample, &self.hardware)
            }
            _ => Err("Unsupported request".into()),
        }
    }

    fn publish(&mut self, sample: &Sample) -> Result<(), i32> {
        let pilot = &self.pilot;
        let navigation = &pilot.navigation;
        let until = sample.tick.time_s + 2.;
        let guidance = navigation.phase.active() || navigation.phase == Phase::Paused;
        sdk::attitude(&abi::AttitudeState {
            valid_until_s: until,
            mode: if guidance {
                abi::ATTITUDE_GUIDANCE
            } else if pilot.hold.is_some() {
                abi::ATTITUDE_HOLD
            } else {
                abi::ATTITUDE_MANUAL
            },
            present: if pilot.hold.is_some() {
                abi::ATTITUDE_REFERENCE
            } else {
                0
            },
            reference: pilot.hold.map_or([0.; 4], |rotation| rotation.to_array()),
            control_error: pilot.allocator.residual,
        })?;

        if pilot.forecast_changed {
            self.publish_forecast(until)?;
        }

        let forecast = pilot.prediction.result.as_ref();
        let target = navigation.target.as_ref().map_or(0, |contact| contact.id);
        let intercept = navigation.intercept_estimate();
        let arrival = intercept.map(|(seconds, _)| seconds);
        sdk::navigation(&abi::NavigationState {
            valid_until_s: until,
            status: match navigation.phase {
                Phase::Ready => abi::NAV_IDLE,
                Phase::Paused => abi::NAV_SUSPENDED,
                Phase::Pursuing => abi::NAV_ACTIVE,
            },
            target_contact: target,
            own_path: if forecast.is_some() { OWN_PATH } else { 0 },
            target_path: if forecast.is_some() { TARGET_PATH } else { 0 },
            present: if arrival.is_some() {
                abi::NAV_ARRIVAL
            } else {
                0
            } | if intercept.is_some() {
                abi::NAV_FUEL
            } else {
                0
            },
            throttle_limit: navigation.limit,
            throttle: pilot.throttle,
            // Omitted optional measurements must be zero in the instrument ABI.
            stand_off_m: 0.0,
            approach_speed_limit_m_s: 0.0,
            braking_distance_m: 0.0,
            arrival_time_s: arrival.map_or(0., |remaining| sample.tick.time_s + remaining),
            predicted_fuel_kg: intercept.map_or(0., |(_, fuel)| fuel),
            reason: abi::Text256::new(&navigation.reason),
        })?;
        match sdk::contacts(&abi::ContactsState {
            valid_until_s: until,
            selected_contact: target,
        }) {
            // A missing or unpowered sensor may have no admitted scan yet.
            Ok(()) | Err(abi::ERR_UNAVAILABLE) => {}
            Err(error) => return Err(error),
        }

        if target != 0 && sample.tick.interest & abi::INTEREST_MARKERS != 0 {
            // The host resolves only this contact's admitted sensor history.
            // No high-speed displacement correction or live entity query is needed.
            let result = sdk::marker(&abi::SpatialMarker {
                meta: abi::SpatialMeta {
                    id: TARGET_MARKER,
                    role: abi::MARKER_TARGET,
                    valid_until_s: until,
                    label: abi::Text64::new("Target"),
                },
                frame: abi::SpatialFrame {
                    kind: abi::FRAME_CONTACT,
                    reference: target,
                    ..Default::default()
                },
                ..Default::default()
            });

            if result != Err(abi::ERR_UNAVAILABLE) {
                result?;
            }
        } else {
            sdk::remove_spatial(TARGET_MARKER)?;
        }

        Ok(())
    }

    fn publish_forecast(&self, until: f64) -> Result<(), i32> {
        let Some(forecast) = &self.pilot.prediction.result else {
            for id in [OWN_PATH, TARGET_PATH, ARRIVAL_MARKER] {
                sdk::remove_spatial(id)?;
            }

            return Ok(());
        };
        let frame = abi::SpatialFrame {
            kind: abi::FRAME_SNAPSHOT,
            reference: forecast.snapshot,
            origin_velocity_m_s: forecast.frame_velocity.to_array(),
        };
        let own = abi::SpatialPath {
            meta: abi::SpatialMeta {
                id: OWN_PATH,
                role: abi::PATH_OWN_FORECAST,
                valid_until_s: until,
                label: abi::Text64::new("Planned trajectory"),
            },
            frame,
            kind: abi::PATH_TIMED,
            subject_contact: 0,
        };
        let mut vertices = [abi::SpatialVertex::default(); abi::MAX_PATH_VERTICES as usize];

        for (vertex, point) in vertices.iter_mut().zip(&forecast.points) {
            *vertex = abi::SpatialVertex {
                time_s: forecast.epoch + point.seconds,
                position_m: point.r.to_array(),
            };
        }

        sdk::path(&own, &vertices[..forecast.points.len()])?;
        let target = abi::SpatialPath {
            meta: abi::SpatialMeta {
                id: TARGET_PATH,
                role: abi::PATH_CONTACT_FORECAST,
                valid_until_s: until,
                label: abi::Text64::new("Target forecast"),
            },
            frame,
            kind: abi::PATH_TIMED,
            subject_contact: forecast.target,
        };
        let last_time = vertices[forecast.points.len() - 1].time_s;
        sdk::path(
            &target,
            &[
                abi::SpatialVertex {
                    time_s: forecast.epoch,
                    position_m: forecast.target_position.to_array(),
                },
                abi::SpatialVertex {
                    time_s: last_time,
                    position_m: forecast.target_position.to_array(),
                },
            ],
        )?;

        if let Some(eta) = forecast.eta {
            sdk::marker(&abi::SpatialMarker {
                meta: abi::SpatialMeta {
                    id: ARRIVAL_MARKER,
                    role: abi::MARKER_EVENT,
                    valid_until_s: until,
                    label: abi::Text64::new("Closest approach"),
                },
                frame: abi::SpatialFrame {
                    kind: abi::FRAME_PATH,
                    reference: OWN_PATH,
                    ..Default::default()
                },
                time_mode: abi::TIME_FIXED,
                time_s: forecast.epoch + eta,
                ..Default::default()
            })?;
        } else {
            sdk::remove_spatial(ARRIVAL_MARKER)?;
        }

        Ok(())
    }
}

#[cfg(feature = "firmware")]
#[unsafe(no_mangle)]
extern "C" fn game_version() -> u32 {
    osg_ship_api::GAME_VERSION as u32
}

#[cfg(feature = "firmware")]
#[unsafe(no_mangle)]
extern "C" fn ship_tick() {
    static mut COMPUTER: Option<Computer> = None;

    // The host enters one callback at a time and rejects shared Wasm memories.
    let computer = unsafe { &mut *core::ptr::addr_of_mut!(COMPUTER) };
    match computer.get_or_insert_with(Computer::default).run() {
        Ok(_) | Err(abi::ERR_UNAVAILABLE) => {}
        Err(error) => panic!("flight computer syscall failed: {error}"),
    }
}

#[cfg(feature = "firmware")]
#[unsafe(no_mangle)]
extern "C" fn ship_display() {
    match draw_display() {
        Ok(()) | Err(abi::ERR_UNAVAILABLE) => {}
        Err(error) => panic!("display syscall failed: {error}"),
    }
}

pub fn draw_display() -> Result<(), i32> {
    let tick = sdk::tick()?;
    for (id, title, width, height) in [(0, "Ship status", 512, 256), (1, "Autopilot", 768, 512)] {
        sdk::screen_define(&abi::ScreenDefinition {
            id,
            width,
            height,
            title: abi::Text64::new(title),
        })?;
    }
    let autopilot = if tick.requested_screens & 2 != 0 || tick.screen_event_count > 0 {
        match osg_model::wasm_world::query(&osg_model::ProgramQuery::Travel) {
            Ok(osg_model::ProgramReply::Travel { state, tick, .. }) => Some((state, tick)),
            _ => None,
        }
    } else {
        None
    };
    static mut PAGE: usize = 0;
    // Each display has its own Wasm memory and callbacks never overlap.
    let page = unsafe { &mut *core::ptr::addr_of_mut!(PAGE) };
    for index in 0..tick.screen_event_count as u32 {
        let event = sdk::screen_event(index)?;
        let change = crate::display::page_change(&event);
        *page = page.saturating_add_signed(change);
        if let Some((state, _)) = &autopilot {
            if let Some(control) = crate::display::computer_control(&event, state) {
                let bytes = postcard::to_allocvec(&control).map_err(|_| abi::ERR_ARGUMENT)?;
                sdk::computer_send(&bytes)?;
            }
        }
        sdk::screen_event_ack(event.id)?;
    }
    for slot in 0..8 {
        if tick.requested_screens & (1 << slot) == 0 {
            continue;
        }
        if slot == 1 {
            draw_autopilot(
                slot,
                autopilot.as_ref().map_or(tick.tick, |(_, tick)| *tick),
                autopilot.as_ref().map(|(state, _)| state),
                page,
            )?;
            continue;
        }
        let flight = sdk::flight()?;
        let resources = sdk::resources()?;
        sdk::screen_define(&abi::ScreenDefinition {
            id: slot,
            width: 512,
            height: 256,
            title: abi::Text64::new("Ship status"),
        })?;
        sdk::screen_begin(&abi::ScreenFrame {
            id: slot,
            background: 0x03070e,
        })?;
        let lines = [
            "SHIP STATUS".to_string(),
            format!(
                "SIMULATION {:.1} S",
                tick.tick as f64 * osg_model::TICK_SECONDS
            ),
            format!(
                "SPEED {:.2} M/S",
                glam::DVec3::from_array(flight.velocity).length()
            ),
            format!("MASS {:.0} KG", flight.mass_kg),
            format!("ENERGY {:.2} MJ", resources.energy_j as f64 / 1e6),
        ];
        for (index, line) in lines.iter().enumerate() {
            sdk::screen_draw(
                slot,
                abi::DRAW_TEXT,
                &abi::ScreenText {
                    color: 0x50ff78,
                    x: 24,
                    y: 24 + index as i64 * 36,
                },
                line.as_bytes(),
            )?;
        }
        sdk::screen_end(slot)?;
    }
    Ok(())
}

fn draw_autopilot(
    slot: u64,
    tick: u64,
    state: Option<&osg_model::travel::AutopilotState>,
    page: &mut usize,
) -> Result<(), i32> {
    sdk::screen_define(&abi::ScreenDefinition {
        id: slot,
        width: 768,
        height: 512,
        title: abi::Text64::new("Autopilot"),
    })?;
    sdk::screen_begin(&abi::ScreenFrame {
        id: slot,
        background: 0x03070e,
    })?;
    let mut lines = state.map_or_else(
        || vec!["Waiting for flight publication".into()],
        |state| crate::display::autopilot_lines(state, tick),
    );
    if lines.is_empty() {
        lines.push("No itinerary".into());
    }
    let page_count = lines.len().div_ceil(crate::display::ROWS_PER_PAGE);
    *page = (*page).min(page_count.saturating_sub(1));
    let title = state.map_or_else(
        || "AUTOPILOT".into(),
        |state| {
            format!(
                "AUTOPILOT  {}  {}",
                if state.enabled {
                    "ENGAGED"
                } else {
                    "DISENGAGED"
                },
                crate::display::phase_name(&state.status.phase)
            )
        },
    );
    display_text(slot, 18, &title, 0x50ffb0)?;
    for (index, line) in lines
        .iter()
        .skip(*page * crate::display::ROWS_PER_PAGE)
        .take(crate::display::ROWS_PER_PAGE)
        .enumerate()
    {
        display_text(
            slot,
            52 + index as i64 * 23,
            line,
            if line.starts_with("FAILURE") {
                0xff7766
            } else {
                0x50ffb0
            },
        )?;
    }
    display_text(
        slot,
        470,
        &format!("PAGE {} / {}", *page + 1, page_count),
        0x50ffb0,
    )?;
    if let Some(state) = state.filter(|state| {
        !matches!(
            state.status.phase,
            osg_model::travel::FirmwarePhase::Transit
        )
    }) {
        sdk::screen_button(slot, 0, if state.enabled { "Pause" } else { "Resume" })?;
        sdk::screen_button(slot, 1, "Clear")?;
    }
    sdk::screen_button(slot, 10, "Prev")?;
    sdk::screen_button(slot, 11, "Next")?;
    sdk::screen_end(slot)
}

fn display_text(slot: u64, y: i64, text: &str, color: u64) -> Result<(), i32> {
    sdk::screen_draw(
        slot,
        abi::DRAW_TEXT,
        &abi::ScreenText { color, x: 16, y },
        text.as_bytes(),
    )
}
