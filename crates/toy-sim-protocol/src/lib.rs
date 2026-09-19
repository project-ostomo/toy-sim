mod chat;
mod industry;
pub mod local_space;
pub mod navigation;
mod presentation;
pub mod routing;
use anyhow::{Context, Result, bail, ensure};
pub use industry::validate_snapshot_content as validate_industry_snapshot_content;
pub use presentation::validate_catalogue;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use toy_sim_model::*;

pub const VERSION: u16 = 26;
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
            section(&mut body, 9, &frame.society)?;
            section(&mut body, 10, &frame.calendar_unix_ms)?;
            section(&mut body, 11, &frame.optical)?;
            section(&mut body, 12, &frame.industry)?;
            section(&mut body, 13, &frame.chat)?;
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
    let known = if kind == 1 { 13 } else { 1 };
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
                chat: read(&sections, 13)?,
                industry: read(&sections, 12)?,
                optical: read(&sections, 11)?,
                calendar_unix_ms: read(&sections, 10)?,
                society: read(&sections, 9)?,
                presentation: read(&sections, 8)?,
                world: clock.world,
                sequence: clock.sequence,
                tick: clock.tick,
                sim_time_ns: clock.sim_time_ns,
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

pub fn validate_order(order: &travel::Order) -> Result<()> {
    let destination = match order {
        travel::Order::TravelTo(destination)
        | travel::Order::Sublight(destination)
        | travel::Order::Slip { destination } => Some(destination),
        travel::Order::Guidance(guidance) => {
            ensure!(
                guidance.range_m.is_finite() && (0. ..=1e12).contains(&guidance.range_m),
                "invalid guidance range"
            );
            match &guidance.target {
                travel::Target::Destination(destination) => Some(destination),
                travel::Target::Contact(_) => None,
                travel::Target::Direction(direction) => {
                    let length_squared = direction.iter().map(|n| n * n).sum::<f64>();
                    ensure!(
                        guidance.mode == travel::GuidanceMode::Align
                            && direction.iter().all(|n| n.is_finite())
                            && length_squared.is_finite()
                            && length_squared > 1e-12,
                        "invalid alignment direction"
                    );
                    None
                }
            }
        }
        _ => None,
    };
    if let Some(destination) = destination {
        validate_destination(destination)?;
    }
    Ok(())
}

pub fn validate_destination(destination: &travel::Destination) -> Result<()> {
    if let travel::Destination::Galactic(position)
    | travel::Destination::Relative {
        offset: position, ..
    } = destination
    {
        ensure!(position_valid(*position), "invalid destination");
    }
    Ok(())
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
    if let Some(update) = &frame.chat {
        chat::validate_update(update)?;
        ensure!(
            update.unavailable
                || frame
                    .views
                    .iter()
                    .any(|view| view.id == update.view && view.revision == update.view_revision),
            "chat view unavailable"
        );
    }
    if let Some(snapshot) = &frame.industry {
        industry::validate_snapshot(snapshot)?;
    }
    ensure!(frame.society.valid(), "invalid society snapshot");
    presentation::validate(&frame.presentation)?;
    for system in frame
        .presentation
        .celestial_systems
        .iter()
        .chain(&frame.presentation.navigation.ephemerides)
    {
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
    ensure!(
        frame.optical.len() <= optical::MAX_OPTICAL_OBSERVATIONS,
        "optical observation limit"
    );
    let known_contacts: BTreeSet<_> = frame
        .tracks
        .iter()
        .flat_map(|(group, tracks)| tracks.iter().map(move |track| (*group, track.id)))
        .collect();
    let mut optical_ids = BTreeSet::new();
    for observation in &frame.optical {
        ensure!(
            optical_ids.insert((observation.view, observation.id))
                && frame.views.iter().any(|view| view.id == observation.view),
            "duplicate optical observation or unknown view"
        );
        ensure!(
            pose_valid(&observation.pose)
                && observation.radius_m.is_finite()
                && observation.radius_m > 0.
                && observation.luminosity_w.is_finite()
                && observation.luminosity_w >= 0.,
            "invalid optical observation"
        );
        if let Some(contact) = observation.contact {
            ensure!(
                known_contacts.contains(&(contact.group, contact.track)),
                "optical contact references unknown track"
            );
        }
        presentation::validate_visual(&observation.visual)?;
    }

    ensure!(
        frame.rate.is_finite() && frame.rate > 0. && frame.rate <= 100.,
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
                ship.hull_heat_j,
                ship.shield_temperature_k,
                ship.coolant_reserve_kg,
                ship.radius_m
            ]
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.),
            "invalid ship resources"
        );
        ensure!(
            ship.travel.orders.len() <= 256 && ship.travel.order <= ship.travel.orders.len(),
            "travel state exceeds limit"
        );
        ensure!(
            ship.travel.planning.as_ref().is_none_or(|progress| {
                ship.travel.status == travel::Status::Planning
                    && progress
                        .total
                        .is_none_or(|total| progress.completed <= total)
            }),
            "invalid planning progress"
        );
        ensure!(
            ship.travel.preferences.valid()
                && ship
                    .travel
                    .fuel_budget
                    .as_ref()
                    .is_none_or(|budget| budget.valid())
                && ship.travel.orders.iter().all(|stage| stage
                    .estimated_propellant_kg
                    .is_none_or(|kg| kg.is_finite() && kg >= 0.)),
            "invalid travel estimates"
        );
        for stage in &ship.travel.orders {
            validate_order(&stage.action)?;
        }
        if let travel::Status::Blocked(reason) = &ship.travel.status {
            ensure!(reason.len() <= 1024, "travel error exceeds limit");
        }
    }
    for event in &frame.events {
        ensure!(
            event.kind.len() <= 64 && event.position.is_none_or(position_valid),
            "invalid event"
        );
    }
    for result in &frame.results {
        if let Some(Reply::Route { id, status }) = &result.reply {
            ensure!(*id != 0, "invalid route result id");
            routing::validate_status(status)?;
        }
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
            Action::RouteRequest { request, .. } => routing::validate_request(request)?,
            Action::RoutePoll { id, .. } => ensure!(*id != 0, "invalid route request id"),
            Action::ChatSubscribe(subscription) => ensure!(
                subscription.revision > 0,
                "invalid chat subscription revision"
            ),
            Action::ChatSend {
                subscription_revision,
                text,
            } => ensure!(
                *subscription_revision > 0 && toy_sim_model::chat::valid_text(text),
                "invalid chat message"
            ),
            Action::Industry(command) => industry::validate_command(command)?,
            Action::IndustrySubscribe(subscription) => {
                industry::validate_subscription(subscription)?
            }
            Action::Society(ownership::SocietyCommand::CreateOrganization { name }) => {
                ensure!(
                    !name.trim().is_empty()
                        && name.len() <= 128
                        && !name.chars().any(char::is_control),
                    "invalid organization name"
                );
            }
            Action::Society(ownership::SocietyCommand::SetAssetAccess { policy, .. }) => {
                ensure!(policy.valid(), "invalid asset access policy");
            }
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
                    rate.is_finite() && *rate > 0. && *rate <= 100.,
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
            Action::Ship { command, .. } => validate_ship_command(command)?,
            _ => {}
        }
    }
    Ok(())
}

pub fn validate_ship_command(command: &ShipCommand) -> Result<()> {
    match command {
        ShipCommand::UseRoute { id, .. } => ensure!(*id != 0, "invalid route request id"),
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
        ShipCommand::MarkTarget {
            maximum_flight_time_s,
            ..
        } => {
            ensure!(
                maximum_flight_time_s.is_finite() && (0.01..=60.).contains(maximum_flight_time_s),
                "invalid weapon flight time"
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
        ShipCommand::SetThrottle(value) => {
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(value),
                "invalid throttle"
            );
        }
        ShipCommand::SetTravel {
            orders,
            preferences,
            ..
        } => {
            ensure!(preferences.valid(), "invalid planning preference");
            ensure!(orders.len() <= 256, "too many waypoints");
            for order in orders {
                validate_order(order)?;
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
                command: ShipCommand::Flight(FlightCommand::AimDirection([f64::NAN, 0., 0.])),
            },
        ));
        assert!(encode(&Message::Input(input)).is_err());
    }

    fn empty_frame() -> Frame {
        Frame {
            chat: None,
            industry: None,
            optical: Vec::new(),
            calendar_unix_ms: 0,
            society: Default::default(),
            presentation: PresentationFrame::default(),
            world: Id([1; 16]),
            sequence: 1,
            tick: 0,
            sim_time_ns: 0,
            rate: 1.,
            views: Vec::new(),
            tracks: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
        }
    }

    #[test]
    fn running_clock_and_debug_capabilities_roundtrip() {
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
        assert!(encode(&Message::State(frame.clone())).is_ok());
        for rate in [0., -1., f64::NAN, f64::INFINITY, 101.] {
            frame.rate = rate;
            assert!(encode(&Message::State(frame.clone())).is_err());
        }
        assert_eq!(
            DebugCommand::SetRate(2.).capability(),
            DebugCapability::Clock
        );
        assert_eq!(DebugCommand::Reset.capability(), DebugCapability::Reset);
    }

    #[test]
    fn industry_interest_and_commands_roundtrip_in_required_section() {
        use toy_sim_model::industry::{
            CargoItem, HangarSubscription, IndustryCommand, IndustrySnapshot, IndustrySubscription,
        };

        let subscription = IndustrySubscription {
            revision: 3,
            directory: true,
            hangar: Some(HangarSubscription {
                ship: Id([2; 16]),
                after: Some(Id([7; 16])),
            }),
            inventories: vec![Id([2; 16])],
            catalogue: true,
            ..Default::default()
        };
        let message = Message::Input(InputFrame {
            world: Id([1; 16]),
            sequence: 4,
            actions: vec![
                (Id([3; 16]), Action::IndustrySubscribe(subscription)),
                (
                    Id([4; 16]),
                    Action::Industry(IndustryCommand::Transfer {
                        source: Id([2; 16]),
                        target: Id([5; 16]),
                        item: CargoItem::Part("laser_turret".into()),
                        quantity: 2,
                    }),
                ),
                (
                    Id([6; 16]),
                    Action::Industry(IndustryCommand::UnloadProduct {
                        source: Id([2; 16]),
                        target: Id([2; 16]),
                        resource: "spent_fuel".into(),
                        quantity: 10,
                    }),
                ),
            ],
        });
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);

        let mut frame = empty_frame();
        frame.industry = Some(IndustrySnapshot {
            subscription_revision: 3,
            error: Some("One inventory exceeds the subscription byte limit.".into()),
            ..Default::default()
        });
        let message = Message::State(frame);
        let mut encoded = encode(&message).unwrap();
        assert_eq!(decode(&encoded).unwrap(), message);
        encoded[4..6].copy_from_slice(&(VERSION - 1).to_le_bytes());
        assert!(decode(&encoded).is_err());
    }

    #[test]
    fn local_chat_roundtrips_and_requires_its_version_23_section() {
        let input = Message::Input(InputFrame {
            world: Id([1; 16]),
            sequence: 1,
            actions: vec![
                (
                    Id([2; 16]),
                    Action::ChatSubscribe(toy_sim_model::chat::ChatSubscription {
                        revision: 1,
                        view: 1,
                    }),
                ),
                (
                    Id([3; 16]),
                    Action::ChatSend {
                        subscription_revision: 1,
                        text: "Hello, local".into(),
                    },
                ),
            ],
        });
        assert_eq!(decode(&encode(&input).unwrap()).unwrap(), input);
        let mut frame = empty_frame();
        frame.chat = Some(toy_sim_model::chat::ChatUpdate {
            subscription_revision: 1,
            view: 1,
            view_revision: 2,
            unavailable: true,
            page: Default::default(),
        });
        let message = Message::State(frame);
        let wire = encode(&message).unwrap();
        assert_eq!(decode(&wire).unwrap(), message);
        let mut old = wire.clone();
        old[4..6].copy_from_slice(&22_u16.to_le_bytes());
        assert!(decode(&old).is_err());
        let mut missing = wire[..HEADER_SIZE].to_vec();
        let mut offset = HEADER_SIZE;
        while offset < wire.len() {
            let id = u16::from_le_bytes(wire[offset..offset + 2].try_into().unwrap());
            let length =
                u32::from_le_bytes(wire[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if id != 13 {
                missing.extend_from_slice(&wire[offset..offset + 8 + length]);
            }
            offset += 8 + length;
        }
        let size = (missing.len() - HEADER_SIZE) as u32;
        missing[8..12].copy_from_slice(&size.to_le_bytes());
        assert!(decode(&missing).is_err());
    }

    #[test]
    fn calendar_roundtrips_beyond_signed_nanosecond_range() {
        let mut frame = empty_frame();
        frame.calendar_unix_ms = toy_sim_model::calendar::from_real_unix_ms(1_789_689_600_000);
        assert!(frame.calendar_unix_ms > i64::MAX / 1_000_000);
        let first = Message::State(frame.clone());
        assert_eq!(decode(&encode(&first).unwrap()).unwrap(), first);

        frame.sequence += 1;
        frame.tick += 10;
        frame.sim_time_ns += 1_000_000_000;
        frame.calendar_unix_ms += 1000;
        let second = Message::State(frame);
        let Message::State(decoded) = decode(&encode(&second).unwrap()).unwrap() else {
            unreachable!()
        };
        assert_eq!(decoded.rate, 1.);
        assert_eq!(decoded.sim_time_ns, 1_000_000_000);
        assert_eq!(
            toy_sim_model::calendar::format_utc(decoded.calendar_unix_ms),
            "Fri 2426-09-18 00:00:01 UTC"
        );
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
        frame.optical.push(optical::OpticalObservation {
            view: 1,
            id: Id([2; 16]),
            spatial_instance: Id([3; 16]),
            known_entity: None,
            contact: None,
            pose: Pose::default(),
            radius_m: 5.,
            luminosity_w: 100.,
            appearance: None,
            visual: ShipVisual {
                engines: Vec::new(),
                turrets: Vec::new(),
                shield: None,
            },
        });
        assert!(encode(&Message::State(frame)).is_err());
    }

    #[test]
    fn anonymous_optical_observations_roundtrip_without_radio_tracks() {
        let mut frame = empty_frame();
        frame.views.push(ViewState {
            focused_ship: None,
            origin: GalacticPosition::ZERO,
            id: 7,
            revision: 1,
            group: Id([4; 16]),
            tracks: Vec::new(),
            completion: Completion::Complete,
        });
        let observation = optical::OpticalObservation {
            view: 7,
            id: Id([5; 16]),
            spatial_instance: Id([6; 16]),
            known_entity: None,
            contact: None,
            pose: Pose::default(),
            radius_m: 10.,
            luminosity_w: 42.,
            appearance: Some([7; 32]),
            visual: ShipVisual {
                engines: Vec::new(),
                turrets: Vec::new(),
                shield: None,
            },
        };
        frame.optical.push(observation.clone());
        let message = Message::State(frame.clone());
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);

        frame.optical.push(observation);
        assert!(validate_frame(&frame).is_err());
        frame.optical.pop();
        frame.optical[0].luminosity_w = f64::NAN;
        assert!(validate_frame(&frame).is_err());
        frame.optical[0].luminosity_w = 0.;
        frame.optical[0].contact = Some(ContactRef {
            group: Id([4; 16]),
            track: Id([8; 16]),
        });
        assert!(validate_frame(&frame).is_err());
        frame.optical[0].contact = None;
        frame.optical[0].visual.shield = Some(ShieldVisual {
            temperature_k: 1000.,
            coverage: 1.1,
        });
        assert!(validate_frame(&frame).is_err());
    }

    #[test]
    fn debug_and_flight_commands_validate_numeric_limits() {
        let Message::Input(mut input) = message() else {
            unreachable!()
        };
        input
            .actions
            .push((Id::new(), Action::Debug(DebugCommand::SetRate(2.))));
        assert!(encode(&Message::Input(input.clone())).is_ok());
        for rate in [0., -0., -1., f64::NAN, f64::INFINITY, 101.] {
            input.actions[0].1 = Action::Debug(DebugCommand::SetRate(rate));
            assert!(encode(&Message::Input(input.clone())).is_err());
        }
        input.actions[0].1 = Action::Ship {
            ship: Id::new(),
            authority_revision: 0,
            command: ShipCommand::Flight(FlightCommand::AimDirection([0.; 3])),
        };
        assert!(encode(&Message::Input(input)).is_err());
    }

    #[test]
    fn direction_guidance_only_accepts_finite_alignment_vectors() {
        let Message::Input(mut input) = message() else {
            unreachable!()
        };
        for (direction, mode, valid) in [
            ([0., 1., 0.], travel::GuidanceMode::Align, true),
            ([0.; 3], travel::GuidanceMode::Align, false),
            ([f64::NAN, 0., 1.], travel::GuidanceMode::Align, false),
            ([f64::MAX; 3], travel::GuidanceMode::Align, false),
            ([0., 1., 0.], travel::GuidanceMode::KeepRange, false),
        ] {
            input.actions = vec![(
                Id::new(),
                Action::Ship {
                    ship: Id::new(),
                    authority_revision: 0,
                    command: ShipCommand::SetTravel {
                        preferences: Default::default(),
                        engage: true,
                        expected_revision: 0,
                        orders: vec![travel::Order::Guidance(travel::Guidance {
                            mode,
                            target: travel::Target::Direction(direction),
                            range_m: 0.,
                        })],
                    },
                },
            )];
            assert_eq!(encode(&Message::Input(input.clone())).is_ok(), valid);
        }
    }

    #[test]
    fn slip_destinations_roundtrip_and_validate_in_inputs_and_snapshots() {
        use travel::{Axes, Destination, Order, Reference};

        let beacon = Id([3; 16]);
        let offset = GalacticPosition {
            x: 12_000_000,
            y: -9_000_000,
            z: 3_000_000,
        };
        let invalid_offset = GalacticPosition {
            x: i128::MIN,
            ..GalacticPosition::ZERO
        };
        let destinations = [
            (Destination::Beacon(beacon), true),
            (Destination::Galactic(offset), true),
            (
                Destination::Relative {
                    reference: Reference::Beacon(beacon),
                    offset,
                    axes: Axes::Galactic,
                },
                true,
            ),
            (
                Destination::Relative {
                    reference: Reference::Celestial(Id([4; 16])),
                    offset,
                    axes: Axes::BodyFixed,
                },
                true,
            ),
            (Destination::Galactic(invalid_offset), false),
            (
                Destination::Relative {
                    reference: Reference::Beacon(beacon),
                    offset: invalid_offset,
                    axes: Axes::Galactic,
                },
                false,
            ),
        ];

        for (destination, valid) in destinations {
            let order = Order::Slip { destination };
            let input = Message::Input(InputFrame {
                world: Id([1; 16]),
                sequence: 1,
                actions: vec![(
                    Id([2; 16]),
                    Action::Ship {
                        ship: Id([5; 16]),
                        authority_revision: 1,
                        command: ShipCommand::SetTravel {
                            preferences: Default::default(),
                            engage: true,
                            expected_revision: 0,
                            orders: vec![order.clone()],
                        },
                    },
                )],
            });
            let mut frame = empty_frame();
            frame.ships.push(ShipTelemetry {
                can_control: true,
                appearance: None,
                radius_m: 10.,
                dock_services: Default::default(),
                spatial_instance: Id([6; 16]),
                info_group: InfoGroupKey([0; 32]),
                iff: IffIdentity {
                    owner: Id([7; 16]),
                    faction: None,
                    labels: Default::default(),
                    enabled: true,
                    range_m: 1e8,
                },
                ship: Id([5; 16]),
                authority_revision: 1,
                presence: travel::Presence::Space,
                pose: Some(Pose::default()),
                battery_j: 0,
                hull_heat_j: 0.,
                shield_temperature_k: 0.,
                coolant_reserve_kg: 0.,
                travel: travel::TravelState {
                    orders: vec![order.into()],
                    ..Default::default()
                },
            });

            for message in [input, Message::State(frame)] {
                let encoded = encode(&message);
                assert_eq!(encoded.is_ok(), valid, "{message:?}");
                if let Ok(bytes) = encoded {
                    assert_eq!(decode(&bytes).unwrap(), message);
                }
            }
        }
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

    #[test]
    fn navigation_ephemerides_require_real_views_and_bounded_unique_references() {
        let mut frame = empty_frame();
        let reference = CelestialSystemRef {
            view: 7,
            system: Id([2; 16]),
            definition: [3; 32],
            epoch_mjd_utc: 60_000.,
            sim_time_origin_ns: 0,
        };
        std::sync::Arc::make_mut(&mut frame.presentation.navigation)
            .ephemerides
            .push(reference.clone());
        assert!(validate_frame(&frame).is_err());

        frame.views = [7, 8]
            .into_iter()
            .map(|id| ViewState {
                focused_ship: None,
                origin: GalacticPosition::ZERO,
                id,
                revision: 1,
                group: Id([4; 16]),
                tracks: Vec::new(),
                completion: Completion::Complete,
            })
            .collect();
        let message = Message::State(frame.clone());
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);

        std::sync::Arc::make_mut(&mut frame.presentation.navigation)
            .ephemerides
            .push(reference.clone());
        assert!(validate_frame(&frame).is_err());

        let navigation = std::sync::Arc::make_mut(&mut frame.presentation.navigation);
        navigation.ephemerides[1].view = 8;
        assert!(validate_frame(&frame).is_ok());

        let navigation = std::sync::Arc::make_mut(&mut frame.presentation.navigation);
        navigation.ephemerides[1].epoch_mjd_utc = f64::NAN;
        assert!(validate_frame(&frame).is_err());

        let navigation = std::sync::Arc::make_mut(&mut frame.presentation.navigation);
        navigation.ephemerides = (0..256_u128)
            .map(|index| CelestialSystemRef {
                system: Id(index.to_le_bytes()),
                ..reference.clone()
            })
            .collect();
        navigation.ephemerides.push(CelestialSystemRef {
            view: 8,
            ..reference.clone()
        });
        assert!(validate_frame(&frame).is_ok());

        std::sync::Arc::make_mut(&mut frame.presentation.navigation)
            .ephemerides
            .push(CelestialSystemRef {
                system: Id(256_u128.to_le_bytes()),
                ..reference
            });
        assert!(validate_frame(&frame).is_err());
    }
}
