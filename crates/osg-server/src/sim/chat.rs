use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use anyhow::{Result, ensure};
use bevy::prelude::*;
use osg_model::{EntityId, GalacticPosition, Id, chat::*};

use super::{identity, simulation::SimulationCounters};

const MAX_PENDING_MESSAGES: usize = 256;

#[derive(Resource, Clone, Default)]
pub struct ChatService(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    tick: u64,
    sites: BTreeMap<EntityId, Site>,
    inboxes: BTreeMap<EntityId, Inbox>,
    pending: VecDeque<(GalacticPosition, Arc<ChatMessage>)>,
}

struct Site {
    position: GalacticPosition,
    name: String,
    owner: Option<Id>,
    organization: Option<Id>,
}

#[derive(Default)]
struct Inbox {
    sequence: u64,
    messages: VecDeque<(u64, Arc<ChatMessage>)>,
    sends: VecDeque<u64>,
    requests: VecDeque<([u8; 32], u64, [u8; 32])>,
}

impl ChatService {
    pub fn send_work(&self) -> u64 {
        2_048
    }

    pub fn latest(&self, ship: EntityId) -> Result<u64> {
        let state = self.0.lock().expect("chat service poisoned");
        ensure!(
            state.sites.contains_key(&ship),
            "local transmitter unavailable"
        );
        Ok(state.inboxes.get(&ship).map_or(0, |inbox| inbox.sequence))
    }

    pub fn send(&self, ship: EntityId, scope: [u8; 32], request_id: u64, text: &str) -> Result<()> {
        ensure!(valid_text(text), "invalid local chat message");
        let mut state = self.0.lock().expect("chat service poisoned");
        let site = state
            .sites
            .get(&ship)
            .ok_or_else(|| anyhow::anyhow!("local transmitter unavailable"))?;
        let position = site.position;
        let message = Arc::new(ChatMessage {
            id: Id::new(),
            sequence: 0,
            tick: state.tick,
            calendar_unix_ms: osg_model::calendar::now_unix_ms(),
            sender_name: site.name.clone(),
            advertised_owner: site.owner,
            advertised_organization: site.organization,
            text: text.to_owned(),
        });
        let tick = state.tick;
        let digest = *blake3::hash(text.as_bytes()).as_bytes();
        let full = state.pending.len() >= MAX_PENDING_MESSAGES;
        let sender = state.inboxes.entry(ship).or_default();
        if let Some((_, _, previous)) = sender
            .requests
            .iter()
            .find(|(old_scope, old_id, _)| *old_scope == scope && *old_id == request_id)
        {
            ensure!(
                *previous == digest,
                "chat request ID reused with different text"
            );
            return Ok(());
        }
        ensure!(!full, "local chat transmission queue full");
        while sender
            .sends
            .front()
            .is_some_and(|sent| tick.saturating_sub(*sent) >= 30)
        {
            sender.sends.pop_front();
        }
        ensure!(
            sender.sends.len() < 3,
            "local chat rate limit: three messages per three seconds"
        );
        sender.sends.push_back(tick);
        sender.requests.push_back((scope, request_id, digest));
        while sender.requests.len() > MAX_HISTORY_MESSAGES {
            sender.requests.pop_front();
        }

        state.pending.push_back((position, message));
        Ok(())
    }

    pub fn flush(&self) {
        let mut state = self.0.lock().expect("chat service poisoned");
        while let Some((position, message)) = state.pending.pop_front() {
            let recipients: Vec<_> = state
                .sites
                .iter()
                .filter_map(|(id, site)| {
                    (site.position.relative_to(position).length_squared() <= LOCAL_RADIUS_M.powi(2))
                        .then_some(*id)
                })
                .collect();
            for recipient in recipients {
                let inbox = state.inboxes.entry(recipient).or_default();
                inbox.sequence = inbox
                    .sequence
                    .checked_add(1)
                    .expect("chat sequence exhausted");
                inbox.messages.push_back((inbox.sequence, message.clone()));
                while inbox.messages.len() > MAX_HISTORY_MESSAGES {
                    inbox.messages.pop_front();
                }
            }
        }
    }

    pub fn read(&self, ship: EntityId, after: u64, limit: u32) -> Result<ChatPage> {
        ensure!(
            (1..=MAX_PAGE_MESSAGES as u32).contains(&limit),
            "invalid chat page limit"
        );
        let state = self.0.lock().expect("chat service poisoned");
        ensure!(
            state.sites.contains_key(&ship),
            "local receiver unavailable"
        );
        let Some(inbox) = state.inboxes.get(&ship) else {
            ensure!(after == 0, "invalid chat cursor");
            return Ok(ChatPage::default());
        };
        ensure!(after <= inbox.sequence, "invalid chat cursor");
        let missed = inbox.messages.front().map_or(0, |(first, _)| {
            first.saturating_sub(after.saturating_add(1))
        });
        let messages: Vec<_> = inbox
            .messages
            .iter()
            .filter(|(sequence, _)| *sequence > after)
            .take(limit as usize)
            .map(|(sequence, message)| {
                let mut message = (**message).clone();
                message.sequence = *sequence;
                message
            })
            .collect();
        let next_sequence = messages.last().map_or(after, |message| message.sequence);
        Ok(ChatPage {
            messages,
            next_sequence,
            missed,
        })
    }
}

fn physical_origin(world: &World, mut entity: Entity) -> Option<GalacticPosition> {
    for _ in 0..16 {
        if world
            .get::<super::missiles::RetainedComputer>(entity)
            .is_some()
        {
            return None;
        }
        match world
            .get::<super::travel::PresenceState>(entity)
            .map(|state| &state.0)
        {
            Some(
                osg_model::travel::Presence::Destroyed
                | osg_model::travel::Presence::StoredInWreck(_),
            ) => return None,
            Some(osg_model::travel::Presence::Docked { host, .. }) => {
                entity = identity::lookup(world, *host).ok()?;
            }
            _ => return super::session::ship_pose(world, entity).map(|pose| pose.position),
        }
    }
    None
}

fn sender_name(labels: &std::collections::BTreeSet<String>) -> String {
    let mut name = String::new();
    if let Some(label) = labels.first() {
        for character in label.chars().filter(|character| !character.is_control()) {
            if name.len() + character.len_utf8() > MAX_SENDER_BYTES {
                break;
            }
            name.push(character);
        }
    }
    if name.trim().is_empty() {
        "Unidentified transmission".into()
    } else {
        name
    }
}

pub fn flush(service: Res<ChatService>) {
    service.flush();
}

pub fn refresh(world: &mut World) {
    world.resource::<ChatService>().flush();
    let sites: BTreeMap<_, _> = world
        .query_filtered::<
            (Entity, &identity::Identity, Option<&identity::Transponder>),
            With<super::vessel::ShipDesign>,
        >()
        .iter(world)
        .filter_map(|(entity, id, transponder)| {
            let position = physical_origin(world, entity)?;
            let iff = transponder.filter(|iff| iff.0.enabled).map(|iff| &iff.0);
            Some((
                id.0,
                Site {
                    position,
                    name: iff.map_or_else(
                        || "Unidentified transmission".into(),
                        |iff| sender_name(&iff.labels),
                    ),
                    owner: iff.map(|iff| iff.owner),
                    organization: iff.and_then(|iff| iff.faction),
                },
            ))
        })
        .collect();
    let service = world.resource::<ChatService>();
    let mut state = service.0.lock().expect("chat service poisoned");
    state.tick = world.resource::<SimulationCounters>().ticks;
    state.inboxes.retain(|id, _| sites.contains_key(id));
    state.sites = sites;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{precision::PreciseTransform, travel::PresenceState};
    use bevy::math::DVec3;

    fn locations(service: &ChatService, tick: u64, locations: &[(Id, f64)]) {
        service.flush();
        let mut state = service.0.lock().unwrap();
        state.tick = tick;
        state.sites.clear();
        for &(id, distance) in locations {
            let position = GalacticPosition::from_meters(DVec3::X * distance);
            state.sites.insert(
                id,
                Site {
                    position,
                    name: "Advertised name".into(),
                    owner: None,
                    organization: None,
                },
            );
        }
    }

    #[test]
    fn geometric_boundary_dark_receivers_and_arrival_history_are_private() {
        let service = ChatService::default();
        let sender = Id::new();
        let boundary = Id::new();
        let outsider = Id::new();
        locations(
            &service,
            1,
            &[
                (sender, 0.0),
                (boundary, LOCAL_RADIUS_M),
                (outsider, LOCAL_RADIUS_M + 1.0),
            ],
        );
        service.send(sender, [1; 32], 1, "At the boundary").unwrap();
        service.flush();
        let received = service.read(boundary, 0, 32).unwrap();
        assert_eq!(received.messages.len(), 1);
        assert_eq!(received.messages[0].advertised_owner, None);
        assert_eq!(service.read(outsider, 0, 32).unwrap().messages.len(), 0);

        locations(
            &service,
            2,
            &[(sender, 0.0), (boundary, LOCAL_RADIUS_M), (outsider, 0.0)],
        );
        assert!(service.read(outsider, 0, 32).unwrap().messages.is_empty());
        service.send(sender, [1; 32], 2, "New arrival").unwrap();
        service.flush();
        let arrived = service.read(outsider, 0, 32).unwrap();
        assert_eq!(arrived.messages.len(), 1);
        assert_eq!(arrived.messages[0].text, "New arrival");
        assert_eq!(arrived.messages[0].sequence, 1);
        let sender_page = service.read(sender, 0, 32).unwrap();
        assert_eq!(sender_page.messages[1].id, arrived.messages[0].id);
        assert_eq!(sender_page.messages[1].sequence, 2);
    }

    #[test]
    fn rate_limit_dedup_and_overflow_count_are_bounded_per_ship() {
        let service = ChatService::default();
        let sender = Id::new();
        locations(&service, 0, &[(sender, 0.0)]);
        for id in 0..3 {
            service.send(sender, [1; 32], id, "Hello").unwrap();
        }
        service.send(sender, [1; 32], 0, "Hello").unwrap();
        assert!(service.send(sender, [1; 32], 0, "Changed").is_err());
        assert!(
            service
                .send(sender, [2; 32], 0, "Other computer scope")
                .is_err()
        );
        service.flush();
        assert_eq!(service.latest(sender).unwrap(), 3);
        for id in 3..140 {
            locations(&service, id * 30, &[(sender, 0.0)]);
            service.send(sender, [1; 32], id, "Hello").unwrap();
        }
        service.flush();
        let page = service.read(sender, 0, 32).unwrap();
        assert_eq!(page.missed, 12);
        assert_eq!(page.messages[0].sequence, 13);
        assert_eq!(page.messages.len(), 32);
        let next = service.read(sender, page.next_sequence, 32).unwrap();
        assert_eq!(next.missed, 0);
        assert_eq!(next.messages[0].sequence, 45);
        assert!(service.read(sender, 141, 32).is_err());
    }

    #[test]
    fn pending_messages_use_admission_positions_and_full_queue_does_not_spend_sender_quota() {
        let service = ChatService::default();
        let sender = Id::new();
        let receiver = Id::new();
        locations(
            &service,
            0,
            &[(sender, 0.0), (receiver, LOCAL_RADIUS_M + 1.0)],
        );
        service.send(sender, [1; 32], 1, "Before crossing").unwrap();
        locations(
            &service,
            1,
            &[(sender, 0.0), (receiver, LOCAL_RADIUS_M - 1.0)],
        );
        assert!(service.read(receiver, 0, 32).unwrap().messages.is_empty());
        service
            .send(sender, [1; 32], 2, "Inside at admission")
            .unwrap();
        locations(
            &service,
            2,
            &[
                (sender, LOCAL_RADIUS_M * 3.0),
                (receiver, LOCAL_RADIUS_M + 1.0),
            ],
        );
        assert_eq!(
            service.read(receiver, 0, 32).unwrap().messages[0].text,
            "Inside at admission"
        );

        let service = ChatService::default();
        let ships: Vec<_> = (0..=MAX_PENDING_MESSAGES)
            .map(|index| (Id::new(), index as f64 * LOCAL_RADIUS_M * 2.0))
            .collect();
        locations(&service, 0, &ships);
        for &(ship, _) in ships.iter().take(MAX_PENDING_MESSAGES) {
            service.send(ship, [1; 32], 1, "Queued").unwrap();
        }
        let blocked = ships[MAX_PENDING_MESSAGES].0;
        assert!(service.send(blocked, [1; 32], 1, "Capacity retry").is_err());
        service.flush();
        for request in 1..=3 {
            service
                .send(blocked, [1; 32], request, "Capacity retry")
                .unwrap();
        }
        service.flush();
        assert_eq!(service.latest(blocked).unwrap(), 3);
    }

    #[test]
    fn docked_transmitter_uses_host_and_destroyed_carrier_has_no_origin() {
        let mut world = World::new();
        world.init_resource::<identity::IdentityIndex>();
        let host_id = Id::new();
        let host_position = GalacticPosition::from_meters(DVec3::X * LOCAL_RADIUS_M);
        let host = world
            .spawn(PreciseTransform {
                translation_um: host_position,
                ..Default::default()
            })
            .id();
        identity::register(&mut world, host, host_id);
        let ship = world
            .spawn((
                PreciseTransform::default(),
                PresenceState(osg_model::travel::Presence::Docked {
                    host: host_id,
                    bay: 0,
                }),
            ))
            .id();
        assert_eq!(physical_origin(&world, ship), Some(host_position));
        world
            .entity_mut(host)
            .insert(PresenceState(osg_model::travel::Presence::Destroyed));
        assert_eq!(physical_origin(&world, ship), None);
        world
            .entity_mut(host)
            .insert(PresenceState(osg_model::travel::Presence::Space));
        world
            .entity_mut(ship)
            .insert(super::super::missiles::RetainedComputer);
        assert_eq!(physical_origin(&world, ship), None);
    }

    #[test]
    fn destroyed_ships_and_wreck_contents_lose_real_chat_endpoints() {
        let owner = Id::new();
        let mut app = crate::sim::provision(&[owner], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let (ship, id) = world
            .query_filtered::<(Entity, &identity::Identity), With<super::super::vessel::ControlledVessel>>()
            .single(world)
            .map(|(entity, id)| (entity, id.0))
            .unwrap();
        refresh(world);
        world.resource::<ChatService>().latest(id).unwrap();

        super::super::travel::destroy(world, ship);
        assert!(
            world
                .get::<super::super::missiles::RetainedComputer>(ship)
                .is_none()
        );
        refresh(world);
        let service = world.resource::<ChatService>();
        assert!(service.latest(id).is_err());
        assert!(
            service
                .send(id, [0; 32], 1, "No transmitter remains")
                .is_err()
        );
        assert!(service.read(id, 0, 4).is_err());

        world
            .entity_mut(ship)
            .insert(PresenceState(osg_model::travel::Presence::StoredInWreck(
                Id::new(),
            )));
        refresh(world);
        assert!(world.resource::<ChatService>().latest(id).is_err());
    }

    #[test]
    fn docked_ship_receives_at_host_position_despite_its_stale_stored_transform() {
        let owner = Id::new();
        let peer = Id::new();
        let mut app = crate::sim::provision(&[owner, peer], None, None).unwrap();
        let world = app.world_mut();
        let ships: BTreeMap<_, _> = world
            .query::<(Entity, &identity::Identity, &identity::Control)>()
            .iter(world)
            .map(|(entity, id, control)| (control.account, (entity, id.0)))
            .collect();
        let (receiver, receiver_id) = ships[&owner];
        let (sender, sender_id) = ships[&peer];
        let host = world
            .query_filtered::<Entity, With<super::super::travel::DockingBays>>()
            .iter(world)
            .next()
            .unwrap();
        let host_id = world.get::<identity::Identity>(host).unwrap().0;
        let host_position = GalacticPosition::from_meters(DVec3::X * LOCAL_RADIUS_M * 3.0);
        world
            .get_mut::<PreciseTransform>(host)
            .unwrap()
            .translation_um = host_position;
        world
            .get_mut::<PreciseTransform>(receiver)
            .unwrap()
            .translation_um = GalacticPosition::ZERO;
        world
            .get_mut::<PreciseTransform>(sender)
            .unwrap()
            .translation_um = host_position.offset_by(DVec3::X * LOCAL_RADIUS_M);
        world.entity_mut(receiver).insert((
            PresenceState(osg_model::travel::Presence::Docked {
                host: host_id,
                bay: 0,
            }),
            super::super::travel::Dormant,
        ));
        refresh(world);
        let service = world.resource::<ChatService>();
        service
            .send(sender_id, [1; 32], 1, "Dockside receiver")
            .unwrap();
        service.flush();
        assert_eq!(
            service.read(receiver_id, 0, 32).unwrap().messages[0].text,
            "Dockside receiver"
        );
        service
            .send(receiver_id, [2; 32], 1, "Dockside transmitter")
            .unwrap();
        service.flush();
        assert_eq!(
            service.read(sender_id, 0, 32).unwrap().messages[1].text,
            "Dockside transmitter"
        );
    }
}
