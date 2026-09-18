mod presentation;
use anyhow::{Context, Result, bail, ensure};
pub use presentation::validate_catalogue;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use toy_sim_model::*;

pub const VERSION: u16 = 6;
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
pub const MAX_INPUT: usize = 64 * 1024;
pub const HEADER_SIZE: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    State(Frame),
    Input(InputFrame),
}

#[derive(Serialize, Deserialize)]
struct Clock {
    world: Id,
    sequence: u64,
    tick: u64,
    sim_time_ns: u64,
    event_watermark: u64,
    rate: f64,
}

pub fn payload_length(header: &[u8]) -> Result<usize> {
    ensure!(
        header.len() == HEADER_SIZE && &header[..4] == b"TSF1",
        "invalid application header"
    );
    ensure!(
        u16::from_le_bytes(header[4..6].try_into()?) == VERSION,
        "unsupported protocol version"
    );
    let kind = u16::from_le_bytes(header[6..8].try_into()?);
    let len = u32::from_le_bytes(header[8..12].try_into()?) as usize;
    let max = match kind {
        1 => MAX_FRAME,
        2 => MAX_INPUT,
        _ => bail!("unsupported required message kind"),
    };
    ensure!(len <= max, "application message exceeds limit");
    Ok(len)
}

fn section<T: Serialize>(out: &mut Vec<u8>, id: u16, value: &T) -> Result<()> {
    let bytes = postcard::to_allocvec(value)?;
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&u32::try_from(bytes.len())?.to_le_bytes());
    out.extend_from_slice(&bytes);
    Ok(())
}

pub fn encode(message: &Message) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let kind: u16 = match message {
        Message::State(frame) => {
            validate_frame(frame)?;
            section(
                &mut body,
                1,
                &Clock {
                    world: frame.world,
                    sequence: frame.sequence,
                    tick: frame.tick,
                    sim_time_ns: frame.sim_time_ns,
                    event_watermark: frame.event_watermark,
                    rate: frame.rate,
                },
            )?;
            section(&mut body, 2, &frame.views)?;
            section(&mut body, 3, &frame.tracks)?;
            section(&mut body, 4, &frame.ships)?;
            section(&mut body, 5, &frame.screens)?;
            section(&mut body, 6, &frame.events)?;
            section(&mut body, 7, &frame.results)?;
            section(&mut body, 8, &frame.presentation)?;
            1
        }
        Message::Input(input) => {
            validate_input(input)?;
            section(&mut body, 1, input)?;
            2
        }
    };
    let mut out = Vec::with_capacity(HEADER_SIZE + body.len());
    out.extend_from_slice(b"TSF1");
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&u32::try_from(body.len())?.to_le_bytes());
    payload_length(&out)?;
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Message> {
    ensure!(bytes.len() >= HEADER_SIZE, "truncated application header");
    let length = payload_length(&bytes[..HEADER_SIZE])?;
    ensure!(
        bytes.len() == HEADER_SIZE + length,
        "incorrect application message length"
    );
    let kind = u16::from_le_bytes(bytes[6..8].try_into()?);
    let mut sections = BTreeMap::new();
    let mut body = &bytes[HEADER_SIZE..];
    let known = if kind == 1 { 8 } else { 1 };
    let mut count = 0;
    while !body.is_empty() {
        count += 1;
        ensure!(
            count <= 32 && body.len() >= 8,
            "invalid section count or header"
        );
        let id = u16::from_le_bytes(body[..2].try_into()?);
        let required = u16::from_le_bytes(body[2..4].try_into()?);
        ensure!(required <= 1, "invalid section flags");
        let len = u32::from_le_bytes(body[4..8].try_into()?) as usize;
        ensure!(len <= body.len() - 8, "truncated section");
        let value = &body[8..8 + len];
        if (1..=known).contains(&id) {
            ensure!(sections.insert(id, value).is_none(), "duplicate section");
        } else {
            ensure!(required == 0, "unsupported required section");
        }
        body = &body[8 + len..];
    }
    let message = match kind {
        1 => {
            let clock: Clock = read(&sections, 1)?;
            let frame = Frame {
                presentation: read(&sections, 8)?,
                world: clock.world,
                sequence: clock.sequence,
                tick: clock.tick,
                sim_time_ns: clock.sim_time_ns,
                event_watermark: clock.event_watermark,
                rate: clock.rate,
                views: read(&sections, 2)?,
                tracks: read(&sections, 3)?,
                ships: read(&sections, 4)?,
                screens: read(&sections, 5)?,
                events: read(&sections, 6)?,
                results: read(&sections, 7)?,
            };
            validate_frame(&frame)?;
            Message::State(frame)
        }
        2 => {
            let input = read(&sections, 1)?;
            validate_input(&input)?;
            Message::Input(input)
        }

        _ => unreachable!(),
    };
    Ok(message)
}

fn read<T: DeserializeOwned>(sections: &BTreeMap<u16, &[u8]>, id: u16) -> Result<T> {
    let bytes = sections.get(&id).context("missing required section")?;
    let (value, remaining) = postcard::take_from_bytes(bytes)?;
    ensure!(remaining.is_empty(), "trailing section data");
    Ok(value)
}

fn position_valid(position: GalacticPosition) -> bool {
    [position.x, position.y, position.z]
        .iter()
        .all(|x| x.unsigned_abs() <= 1_u128 << 110)
}

pub fn validate_query(query: &TrackQuery) -> Result<()> {
    ensure!(
        (1..=256).contains(&query.limit) && query.work <= 10_000_000,
        "invalid query budget"
    );
    ensure!(
        query.all.len() + query.any.len() + query.exclude.len() <= 32,
        "too many tags"
    );
    ensure!(
        query
            .all
            .iter()
            .chain(&query.any)
            .chain(&query.exclude)
            .all(Tag::valid),
        "invalid tags"
    );
    if let Some((position, radius)) = query.sphere {
        ensure!(
            position_valid(position) && radius.is_finite() && (0. ..=1e22).contains(&radius),
            "invalid search sphere"
        );
    }
    Ok(())
}

fn pose_valid(pose: &Pose) -> bool {
    position_valid(pose.position)
        && pose
            .velocity
            .iter()
            .chain(&pose.rotation)
            .chain(&pose.angular_velocity)
            .all(|value| value.is_finite())
        && (pose.rotation.iter().map(|value| value * value).sum::<f64>() - 1.).abs() < 1e-5
}

pub fn validate_frame(frame: &Frame) -> Result<()> {
    presentation::validate(&frame.presentation, frame.event_watermark)?;
    for system in &frame.presentation.celestial_systems {
        ensure!(
            frame.views.iter().any(|view| view.id == system.view),
            "celestial system references unknown view"
        );
    }
    let mut publication_ships = BTreeSet::new();
    for publication in &frame.presentation.ships {
        ensure!(
            publication_ships.insert(publication.ship)
                && frame.ships.iter().any(|ship| ship.ship == publication.ship),
            "private publication requires ship telemetry"
        );
    }
    let mut visual_tracks = BTreeSet::new();
    for visual in &frame.presentation.visuals {
        ensure!(
            visual_tracks.insert((visual.contact.group, visual.contact.track))
                && frame
                    .tracks
                    .get(&visual.contact.group)
                    .is_some_and(|tracks| tracks
                        .iter()
                        .any(|track| track.id == visual.contact.track)),
            "visual requires observed track"
        );
    }

    ensure!(
        frame.rate.is_finite() && frame.rate >= 0. && frame.rate <= 100.,
        "invalid clock rate"
    );
    ensure!(
        frame.views.len() <= 8
            && frame.tracks.len() <= 16
            && frame.ships.len() <= 64
            && frame.screens.len() <= 64,
        "observation limit"
    );
    ensure!(
        frame.tracks.values().map(Vec::len).sum::<usize>() <= 8192,
        "track limit"
    );
    ensure!(
        frame.events.len() <= 16384 && frame.results.len() <= 4096,
        "event limit"
    );
    let mut views = BTreeSet::new();
    for view in &frame.views {
        ensure!(
            position_valid(view.origin) && views.insert(view.id) && view.tracks.len() <= 8192,
            "duplicate or oversized view"
        );
    }
    for tracks in frame.tracks.values() {
        let mut ids = BTreeSet::new();
        for track in tracks {
            ensure!(
                ids.insert(track.id) && pose_valid(&track.pose),
                "invalid track identity or position"
            );
            ensure!(
                track.tags.len() <= 64 && track.tags.iter().all(Tag::valid),
                "invalid track tags"
            );
            ensure!(
                track
                    .pose
                    .velocity
                    .iter()
                    .chain(&track.pose.rotation)
                    .chain(&track.pose.angular_velocity)
                    .all(|x| x.is_finite()),
                "invalid track pose"
            );
            ensure!(
                track.position_sigma_m.is_finite()
                    && track.position_sigma_m >= 0.
                    && track.velocity_sigma_m_s.is_finite()
                    && track.velocity_sigma_m_s >= 0.,
                "invalid uncertainty"
            );
        }
    }
    for ship in &frame.ships {
        ensure!(
            ship.pose.as_ref().is_none_or(pose_valid),
            "invalid private pose"
        );
        ensure!(
            [
                ship.battery_j,
                ship.hull_heat_j,
                ship.shield_temperature_k,
                ship.coolant_reserve_kg
            ]
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.),
            "invalid ship resources"
        );
        ensure!(
            ship.travel.orders.len() <= 256 && ship.travel.legs.len() <= 256,
            "travel state exceeds limit"
        );
        if let travel::Status::Blocked(reason) = &ship.travel.status {
            ensure!(reason.len() <= 1024, "travel error exceeds limit");
        }
    }
    for event in &frame.events {
        ensure!(
            event.kind.len() <= 64
                && event.position.is_none_or(position_valid)
                && event.sequence <= frame.event_watermark,
            "invalid event"
        );
    }
    for result in &frame.results {
        ensure!(
            result
                .error
                .as_ref()
                .is_none_or(|error| error.len() <= 1024),
            "command error exceeds limit"
        );
    }
    for screen in &frame.screens {
        ensure!(
            screen.slot < 8 && screen.frame.as_ref().is_none_or(|frame| frame.valid()),
            "invalid drawing list"
        );
        ensure!(
            postcard::to_allocvec(screen)?.len() <= 65536,
            "display payload exceeds limit"
        );
        ensure!(
            screen
                .frame
                .as_ref()
                .is_none_or(|frame| frame.screen_id == screen.slot),
            "display slot mismatch"
        );
        ensure!(
            screen.error.as_ref().is_none_or(|s| s.len() <= 1024),
            "oversized display error"
        );
    }
    Ok(())
}

pub fn validate_input(input: &InputFrame) -> Result<()> {
    ensure!(input.actions.len() <= 256, "too many actions");
    let mut ids = BTreeSet::new();
    for (id, action) in &input.actions {
        ensure!(ids.insert(*id), "duplicate command id");
        match action {
            Action::Subscribe(view) => validate_query(&view.query)?,
            Action::ScreenSubscribe { slot, hz, .. } => ensure!(
                *slot < 8 && (1..=10).contains(hz),
                "invalid display subscription"
            ),
            Action::ScreenUnsubscribe { slot, .. } => ensure!(*slot < 8, "invalid display slot"),
            Action::Debug(command) => match command {
                DebugCommand::ConfigureSensor { range_m, .. } => ensure!(
                    range_m.is_finite() && (0. ..=1e22).contains(range_m),
                    "invalid sensor range"
                ),
                DebugCommand::SetRate(rate) => ensure!(
                    rate.is_finite() && (0. ..=100.).contains(rate),
                    "invalid clock rate"
                ),
                DebugCommand::Relocate { pose, .. } => {
                    ensure!(pose_valid(pose), "invalid debug pose")
                }
                DebugCommand::InjectHeat { joules, .. }
                | DebugCommand::InjectShieldHeat { joules, .. } => ensure!(
                    joules.is_finite() && (0. ..=1e30).contains(joules),
                    "invalid heat injection"
                ),
                _ => {}
            },
            Action::Ship { command, .. } => match command {
                ShipCommand::Flight(command) => match command {
                    FlightCommand::AimDirection(direction) => ensure!(
                        direction.iter().all(|v| v.is_finite())
                            && direction.iter().map(|v| v * v).sum::<f64>() > 1e-12,
                        "invalid aim direction"
                    ),
                    FlightCommand::EngageNavigation {
                        throttle_limit,
                        stand_off_m,
                    } => ensure!(
                        throttle_limit.is_finite()
                            && (0. ..=1.).contains(throttle_limit)
                            && stand_off_m.is_finite()
                            && (0. ..=1e22).contains(stand_off_m),
                        "invalid navigation command"
                    ),
                    _ => {}
                },
                ShipCommand::EngageWeapons {
                    maximum_flight_time_s,
                    ..
                } => {
                    ensure!(
                        maximum_flight_time_s.is_finite()
                            && (0. ..=3600.).contains(maximum_flight_time_s),
                        "invalid weapon flight time"
                    );
                }
                ShipCommand::Manual { throttle, steering } => {
                    ensure!(
                        throttle.is_finite()
                            && (0. ..=1.).contains(throttle)
                            && steering
                                .iter()
                                .all(|x| x.is_finite() && (-1. ..=1.).contains(x)),
                        "invalid manual control"
                    );
                }
                ShipCommand::SetIff(iff) => {
                    ensure!(
                        iff.labels.len() <= 16
                            && iff
                                .labels
                                .iter()
                                .all(|label| Tag::Advertised(label.clone()).valid()),
                        "invalid IFF labels"
                    );
                    ensure!(
                        iff.range_m.is_finite() && (0. ..=1e12).contains(&iff.range_m),
                        "invalid transponder range"
                    );
                }
                ShipCommand::SetTravel { orders, .. } => {
                    ensure!(orders.len() <= 256, "too many waypoints");
                    for order in orders {
                        if let travel::Order::TravelTo(destination) = order {
                            match destination {
                                travel::Destination::Galactic(p)
                                | travel::Destination::Relative { offset: p, .. } => {
                                    ensure!(position_valid(*p), "invalid destination")
                                }
                                _ => {}
                            }
                        }
                    }
                }
                ShipCommand::ScreenInput {
                    slot,
                    kind,
                    xy,
                    text,
                    ..
                } => {
                    ensure!(
                        *slot < 8
                            && *kind <= 7
                            && text.len() <= 64
                            && xy.iter().all(|x| x.is_finite() && x.abs() <= 1e6),
                        "invalid display input"
                    );
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> Message {
        Message::Input(InputFrame {
            world: Id([1; 16]),
            sequence: 9,
            acknowledged_event: 3,
            acknowledged_frame: 0,
            actions: vec![],
        })
    }

    #[test]
    fn roundtrip_and_optional_sections() {
        let mut wire = encode(&message()).unwrap();
        assert_eq!(decode(&wire).unwrap(), message());
        wire.extend_from_slice(&[99, 0, 0, 0, 2, 0, 0, 0, 7, 8]);
        let length = (wire.len() - HEADER_SIZE) as u32;
        wire[8..12].copy_from_slice(&length.to_le_bytes());
        assert_eq!(decode(&wire).unwrap(), message());
        let flag = wire.len() - 8;
        wire[flag] = 1;
        assert!(decode(&wire).is_err());
    }

    #[test]
    fn rejects_truncation_limits_and_nonfinite_input() {
        let wire = encode(&message()).unwrap();
        for length in 0..wire.len() {
            assert!(decode(&wire[..length]).is_err());
        }
        let mut header = wire[..HEADER_SIZE].to_vec();
        header[8..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(payload_length(&header).is_err());
        let Message::Input(mut input) = message() else {
            unreachable!()
        };
        input.actions.push((
            Id::new(),
            Action::Ship {
                ship: Id::new(),
                authority_revision: 0,
                command: ShipCommand::Manual {
                    throttle: f64::NAN,
                    steering: [0.; 3],
                },
            },
        ));
        assert!(encode(&Message::Input(input)).is_err());
    }

    fn empty_frame() -> Frame {
        Frame {
            presentation: PresentationFrame::default(),
            world: Id([1; 16]),
            sequence: 1,
            tick: 0,
            sim_time_ns: 0,
            event_watermark: 0,
            rate: 0.,
            views: Vec::new(),
            tracks: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
        }
    }

    #[test]
    fn paused_clock_and_debug_capabilities_roundtrip() {
        let mut frame = empty_frame();
        frame.presentation.capabilities = vec![DebugCapability::Clock, DebugCapability::Inspect];
        frame.presentation.diagnostics = Some(Diagnostics {
            active_ships: 3,
            tick_duration_ms: 4.5,
            ..Default::default()
        });
        let message = Message::State(frame.clone());
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);
        frame.sequence += 1;
        assert!(encode(&Message::State(frame)).is_ok());
        assert_eq!(DebugCommand::Step.capability(), DebugCapability::Clock);
        assert_eq!(DebugCommand::Reset.capability(), DebugCapability::Reset);
    }

    #[test]
    fn presentation_is_required_and_old_version_rejected() {
        let mut wire = encode(&Message::State(empty_frame())).unwrap();
        wire[4..6].copy_from_slice(&1_u16.to_le_bytes());
        assert!(decode(&wire).is_err());
        wire[4..6].copy_from_slice(&VERSION.to_le_bytes());
        let mut at = HEADER_SIZE;
        while u16::from_le_bytes(wire[at..at + 2].try_into().unwrap()) != 8 {
            let len = u32::from_le_bytes(wire[at + 4..at + 8].try_into().unwrap()) as usize;
            at += 8 + len;
        }
        wire.truncate(at);
        let len = (wire.len() - HEADER_SIZE) as u32;
        wire[8..12].copy_from_slice(&len.to_le_bytes());
        assert!(decode(&wire).is_err());
    }

    #[test]
    fn rejects_invalid_combat_payload_and_unobserved_visual() {
        let mut frame = empty_frame();
        frame.event_watermark = 1;
        frame.presentation.combat.push(CombatEvent {
            sequence: 1,
            sim_time_ns: 20,
            kind: CombatEventKind::Projectile {
                id: 8,
                source: None,
                start: GalacticPosition::ZERO,
                end: GalacticPosition::ZERO,
                end_time_ns: 10,
                radius_m: 1.,
            },
        });
        assert!(encode(&Message::State(frame.clone())).is_err());
        frame.presentation.combat.clear();
        frame.presentation.visuals.push(TrackVisual {
            contact: ContactRef {
                group: Id([2; 16]),
                track: Id([3; 16]),
            },
            engines: Vec::new(),
            turrets: Vec::new(),
            shield: None,
        });
        assert!(encode(&Message::State(frame)).is_err());
    }

    #[test]
    fn debug_and_flight_commands_validate_numeric_limits() {
        let Message::Input(mut input) = message() else {
            unreachable!()
        };
        input
            .actions
            .push((Id::new(), Action::Debug(DebugCommand::SetRate(0.))));
        assert!(encode(&Message::Input(input.clone())).is_ok());
        input.actions[0].1 = Action::Debug(DebugCommand::SetRate(f64::NAN));
        assert!(encode(&Message::Input(input.clone())).is_err());
        input.actions[0].1 = Action::Ship {
            ship: Id::new(),
            authority_revision: 0,
            command: ShipCommand::Flight(FlightCommand::AimDirection([0.; 3])),
        };
        assert!(encode(&Message::Input(input)).is_err());
    }

    #[test]
    fn celestial_definitions_are_scoped_to_existing_views() {
        let mut frame = empty_frame();
        frame
            .presentation
            .celestial_systems
            .push(CelestialSystemRef {
                view: 7,
                system: Id([2; 16]),
                definition: [3; 32],
                epoch_mjd_utc: 0.,
                sim_time_origin_ns: 0,
            });
        assert!(encode(&Message::State(frame.clone())).is_err());
        frame.views.push(ViewState {
            focused_ship: None,
            origin: GalacticPosition::ZERO,
            id: 7,
            revision: 1,
            group: Id([4; 16]),
            tracks: Vec::new(),
            completion: Completion::Complete,
        });
        let message = Message::State(frame.clone());
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);
        frame.presentation.celestial_systems[0].epoch_mjd_utc = f64::NAN;
        assert!(encode(&Message::State(frame)).is_err());
    }
}
