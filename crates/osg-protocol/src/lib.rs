pub mod industry;
pub mod navigation;

use anyhow::{Result, ensure};
pub use industry::{decode_blueprint_upload_ack, encode_blueprint_upload_ack};
use osg_model::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_FRAME: usize = 8 * 1024 * 1024;
pub const MAX_INPUT: usize = 64 * 1024;
pub const HEADER_SIZE: usize = 4;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Message {
    State(Frame),
    Input(InputFrame),
    Session {
        world: Id,
        universe: UniverseDescriptor,
    },
}

pub fn payload_length(header: &[u8]) -> Result<usize> {
    let length = u32::from_be_bytes(header.try_into()?) as usize;
    ensure!(length <= MAX_FRAME, "application message exceeds limit");
    Ok(length)
}

pub fn encode(message: &Message) -> Result<Vec<u8>> {
    if let Message::Input(input) = message {
        validate_input(input)?;
    }
    let body = postcard::to_allocvec(message)?;
    let maximum = if matches!(message, Message::Input(_)) {
        MAX_INPUT
    } else {
        MAX_FRAME
    };
    ensure!(body.len() <= maximum, "application message exceeds limit");
    let mut bytes = Vec::with_capacity(HEADER_SIZE + body.len());
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Message> {
    ensure!(bytes.len() >= HEADER_SIZE, "truncated application header");
    let length = payload_length(&bytes[..HEADER_SIZE])?;
    ensure!(
        bytes.len() == HEADER_SIZE + length,
        "incorrect application message length"
    );
    let message = postcard::from_bytes(&bytes[HEADER_SIZE..])?;
    if let Message::Input(input) = &message {
        ensure!(length <= MAX_INPUT, "input message exceeds limit");
        validate_input(input)?;
    }
    Ok(message)
}

fn position_valid(position: GalacticPosition) -> bool {
    [position.x, position.y, position.z]
        .iter()
        .all(|x| x.unsigned_abs() <= 1_u128 << 110)
}

pub fn validate_guidance(guidance: &travel::Guidance) -> Result<()> {
    ensure!(
        guidance.range_m.is_finite() && (0. ..=1e12).contains(&guidance.range_m),
        "invalid guidance range"
    );
    let destination = match &guidance.target {
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
    };
    if let Some(destination) = destination {
        validate_destination(destination)?;
    }
    Ok(())
}

pub fn validate_itinerary_entry(entry: &travel::ItineraryEntry) -> Result<()> {
    ensure!(
        !entry.label.trim().is_empty() && entry.label.len() <= 256,
        "invalid directive label"
    );
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

pub fn validate_input(input: &InputFrame) -> Result<()> {
    ensure!(input.actions.len() <= 256, "too many actions");
    let mut ids = BTreeSet::new();
    for (id, action) in &input.actions {
        ensure!(ids.insert(*id), "duplicate command id");
        match action {
            Action::ChatSubscribe(subscription) => ensure!(
                subscription.revision > 0,
                "invalid chat subscription revision"
            ),
            Action::ChatSend {
                subscription_revision,
                text,
            } => ensure!(
                *subscription_revision > 0 && osg_model::chat::valid_text(text),
                "invalid chat message"
            ),
            Action::Subscribe(_) => {}
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

pub fn validate_iff(iff: &IffIdentity) -> Result<()> {
    ensure!(
        iff.labels.len() <= 16
            && iff.labels.iter().all(|label| {
                !label.is_empty() && label.len() <= 64 && !label.chars().any(char::is_control)
            }),
        "invalid IFF labels"
    );
    Ok(())
}

pub fn validate_ship_command(command: &ShipCommand) -> Result<()> {
    match command {
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
        ShipCommand::SetIff(iff) => validate_iff(iff)?,
        ShipCommand::SetThrottle(value) => {
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(value),
                "invalid throttle"
            );
        }
        ShipCommand::SetItinerary {
            itinerary,
            preferences,
            ..
        } => {
            ensure!(preferences.valid(), "invalid planning preference");
            ensure!(
                itinerary.len() <= osg_model::travel::MAX_DIRECTIVES,
                "too many directives"
            );
        }
        ShipCommand::SetGuidance(Some(guidance)) => validate_guidance(guidance)?,
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
                    command: ShipCommand::SetGuidance(Some(travel::Guidance {
                        mode,
                        target: travel::Target::Direction(direction),
                        range_m: 0.,
                    })),
                },
            )];
            assert_eq!(encode(&Message::Input(input.clone())).is_ok(), valid);
        }
    }

    #[test]
    fn messages_roundtrip_and_truncated_frames_fail() {
        let message = Message::Session {
            world: Id([2; 16]),
            universe: UniverseDescriptor {
                fingerprint: [3; 32],
                epoch_mjd_utc: 60_000.,
                sim_time_origin_ns: 123,
            },
        };
        let bytes = encode(&message).unwrap();
        assert_eq!(decode(&bytes).unwrap(), message);
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
    }

    #[test]
    fn untrusted_input_is_validated_after_deserialization() {
        let message = Message::Input(InputFrame {
            world: Id([1; 16]),
            sequence: 9,
            actions: vec![(Id([2; 16]), Action::Debug(DebugCommand::SetRate(f64::NAN)))],
        });
        assert!(encode(&message).is_err());
        let body = postcard::to_allocvec(&message).unwrap();
        let mut wire = (body.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(&body);
        assert!(decode(&wire).is_err());
    }
}
