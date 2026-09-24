use anyhow::{Result, ensure};
use bevy::prelude::*;
use osg_model::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::commands::{authorize as ship_authority, observe};
use super::identity::{self, Account, Control, Identity, WorldEpoch};
use super::simulation::SimulationCounters;
use super::vessel::SoftwareDiagnostics;

mod cache;
pub(super) use cache::maintain as maintain_cache;
pub use cache::prepare_publication;
mod chat;
mod optical;

#[derive(Resource)]
pub struct Clock {
    pub rate: f64,
    pub reset_requested: bool,
    pub inspect: bool,
    pub requests: VecDeque<DebugCommand>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            rate: 1.,
            reset_requested: false,
            inspect: false,
            requests: VecDeque::new(),
        }
    }
}

#[derive(Resource, Default)]
pub struct Events(pub VecDeque<osg_model::Event>);

#[derive(Component)]
pub struct Session {
    pub account: AccountId,
    pub views: BTreeMap<u64, ViewSubscription>,
    pub screens: BTreeMap<(EntityId, u8), u8>,
    pub instruments: BTreeSet<EntityId>,
    sequence: u64,
    input_sequence: Option<u64>,
    sent_event: u64,
    results: VecDeque<CommandResult>,
    seen: BTreeSet<Id>,
    optical: optical::OpticalSession,
    chat: chat::ChatSession,
}

pub fn connect(
    world: &mut World,
    account: AccountId,
    _uploads: crate::blueprint_uploads::BlueprintUploads,
) -> Result<Entity> {
    let owner = identity::lookup(world, account)?;
    ensure!(world.get::<Account>(owner).is_some(), "account unavailable");
    let sent_event = world
        .resource::<Events>()
        .0
        .back()
        .map_or(0, |event| event.sequence);
    Ok(world
        .spawn(Session {
            account,
            views: BTreeMap::new(),
            screens: BTreeMap::new(),
            instruments: BTreeSet::new(),
            sequence: 0,
            input_sequence: None,
            sent_event,
            results: VecDeque::new(),
            seen: BTreeSet::new(),
            optical: optical::OpticalSession::default(),
            chat: chat::ChatSession::default(),
        })
        .id())
}

pub fn disconnect(world: &mut World, session: Entity) {
    world.despawn(session);
}

pub fn prune_events(world: &mut World) {
    let _profile = super::diagnostics::ProfileScope::new("session.prune_events");
    let published = world
        .query::<&Session>()
        .iter(world)
        .map(|session| session.sent_event)
        .min()
        .unwrap_or_else(|| {
            world
                .resource::<Events>()
                .0
                .back()
                .map_or(0, |event| event.sequence)
        });
    let mut events = world.resource_mut::<Events>();
    while events.0.len() > 1
        && events
            .0
            .front()
            .is_some_and(|event| event.sequence <= published)
    {
        events.0.pop_front();
    }
    super::combat::prune(world, published);
}

pub fn input(world: &mut World, entity: Entity, input: InputFrame) -> Result<()> {
    let mut session = world
        .entity_mut(entity)
        .take::<Session>()
        .ok_or_else(|| anyhow::anyhow!("session unavailable"))?;
    let result = session.input(world, input);
    world.entity_mut(entity).insert(session);
    result
}

/// Publish after navigation publication, display updates, and `prepare_publication`,
/// with no intervening changes to simulation or authorization data.
pub fn frame(world: &mut World, entity: Entity) -> Result<Frame> {
    let _profile = super::diagnostics::ProfileScope::new("session.frame");
    let mut session = world
        .entity_mut(entity)
        .take::<Session>()
        .ok_or_else(|| anyhow::anyhow!("session unavailable"))?;
    let result = session.frame(world);
    world.entity_mut(entity).insert(session);
    result
}

impl Session {
    fn detail_capacity(&self, ship: Id, replaced_view: Option<u64>) -> Result<()> {
        let selected: BTreeSet<_> = self
            .views
            .iter()
            .filter(|(id, _)| Some(**id) != replaced_view)
            .filter_map(|(_, view)| view.focused_ship)
            .chain(self.screens.keys().map(|(ship, _)| *ship))
            .chain(self.instruments.iter().copied())
            .collect();
        ensure!(
            selected.contains(&ship) || selected.len() < 8,
            "detailed ship subscription limit"
        );
        Ok(())
    }

    fn input(&mut self, world: &mut World, input: InputFrame) -> Result<()> {
        osg_protocol::validate_input(&input)?;
        if input.world != world.resource::<WorldEpoch>().0 {
            return Ok(());
        }
        ensure!(
            self.input_sequence
                .is_none_or(|previous| input.sequence > previous),
            "replayed input frame"
        );
        self.input_sequence = Some(input.sequence);
        for (id, action) in input.actions {
            if self.seen.contains(&id) {
                continue;
            }
            ensure!(
                self.seen.len() < 65536,
                "session command history exhausted; reconnect"
            );
            ensure!(self.results.len() < 4096, "command results backlogged");
            self.seen.insert(id);
            let error = self
                .apply(world, action)
                .err()
                .map(|error| error.to_string());
            self.results.push_back(CommandResult { id, error });
        }
        Ok(())
    }

    fn apply(&mut self, world: &mut World, action: Action) -> Result<()> {
        match action {
            Action::ChatSubscribe(subscription) => {
                world.run_system_cached(super::chat::refresh)?;
                self.chat
                    .subscribe(world, self.account, &self.views, subscription)?;
            }
            Action::ChatUnsubscribe => self.chat.unsubscribe(),
            Action::ChatSend {
                subscription_revision,
                text,
            } => {
                world.run_system_cached(super::chat::refresh)?;
                self.chat.send(
                    world,
                    self.account,
                    &self.views,
                    subscription_revision,
                    &text,
                )?;
            }
            Action::Subscribe(view) => {
                ensure!(
                    self.views.contains_key(&view.id) || self.views.len() < 8,
                    "view limit"
                );
                ensure!(
                    self.views
                        .get(&view.id)
                        .is_none_or(|old| view.revision > old.revision),
                    "stale view revision"
                );
                if let Some(ship) = view.focused_ship {
                    self.detail_capacity(ship, Some(view.id))?;
                    observe(world, self.account, ship)?;
                }
                self.views.insert(view.id, view);
            }
            Action::Unsubscribe(id) => {
                self.views.remove(&id);
            }
            Action::InstrumentSubscribe { ship } => {
                self.detail_capacity(ship, None)?;
                observe(world, self.account, ship)?;
                ensure!(
                    self.instruments.contains(&ship) || self.instruments.len() < 8,
                    "instrument limit"
                );
                self.instruments.insert(ship);
            }
            Action::InstrumentUnsubscribe { ship } => {
                self.instruments.remove(&ship);
            }
            Action::ScreenSubscribe { ship, slot, hz } => {
                self.detail_capacity(ship, None)?;
                observe(world, self.account, ship)?;
                ensure!(
                    self.screens.contains_key(&(ship, slot)) || self.screens.len() < 8,
                    "display limit"
                );
                self.screens.insert((ship, slot), hz);
            }
            Action::ScreenUnsubscribe { ship, slot } => {
                self.screens.remove(&(ship, slot));
            }
            Action::Debug(command) => {
                let account = identity::lookup(world, self.account)?;
                ensure!(
                    world
                        .get::<Account>(account)
                        .is_some_and(|account| account.debug),
                    "debug access denied"
                );
                let mut clock = world.resource_mut::<Clock>();
                match command {
                    DebugCommand::SetRate(rate) => clock.rate = rate,
                    DebugCommand::Reset => clock.reset_requested = true,
                    DebugCommand::Inspect(enabled) => clock.inspect = enabled,
                    command => {
                        ensure!(clock.requests.len() < 256, "debug queue full");
                        clock.requests.push_back(command);
                    }
                }
            }
            Action::Ship {
                ship,
                authority_revision,
                command,
            } => {
                super::commands::authorize(
                    world,
                    self.account,
                    ship,
                    Some(authority_revision),
                    super::commands::permission(&command),
                )?;
                if let ShipCommand::ScreenInput { slot, .. } = &command {
                    ensure!(
                        self.screens.contains_key(&(ship, *slot)),
                        "display is not subscribed"
                    );
                }
                super::commands::execute(world, self.account, ship, authority_revision, command)?;
            }
        }
        Ok(())
    }
}

impl Session {
    fn frame(&mut self, world: &mut World) -> Result<Frame> {
        let readings = world
            .resource::<super::presentation::DeviceReadings>()
            .clone();
        let profile = super::diagnostics::ProfileScope::new("session.contacts_and_events");
        self.views.retain(|_, view| {
            view.focused_ship
                .is_none_or(|ship| observe(world, self.account, ship).is_ok())
        });
        self.screens
            .retain(|(ship, _), _| observe(world, self.account, *ship).is_ok());
        self.instruments
            .retain(|ship| observe(world, self.account, *ship).is_ok());
        let tick = world.resource::<SimulationCounters>().ticks;
        let focused: BTreeSet<_> = self
            .views
            .values()
            .filter_map(|view| view.focused_ship)
            .chain(self.instruments.iter().copied())
            .chain(self.screens.keys().map(|(ship, _)| *ship))
            .collect();
        let mut contacts: BTreeMap<EntityId, BTreeMap<u64, SensorObservation>> = BTreeMap::new();
        for id in &focused {
            if let Ok(entity) = observe(world, self.account, *id) {
                contacts.insert(*id, super::sensors::observe(world, entity).contacts.clone());
            }
        }
        let views: Vec<_> = self
            .views
            .values()
            .map(|view| ViewState {
                id: view.id,
                revision: view.revision,
                focused_ship: view.focused_ship,
                origin: view
                    .focused_ship
                    .and_then(|id| identity::lookup(world, id).ok())
                    .and_then(|entity| ship_pose(world, entity))
                    .map_or(GalacticPosition::ZERO, |pose| pose.position),
            })
            .collect();
        let visible: BTreeSet<_> = contacts
            .values()
            .flat_map(|contacts| contacts.values().filter_map(|contact| contact.entity))
            .collect();
        let history = &world.resource::<Events>().0;
        let mut events: Vec<_> = history
            .iter()
            .rev()
            .take_while(|event| event.sequence > self.sent_event)
            .filter(|event| {
                event.subject.is_some_and(|id| {
                    visible.contains(&id) || observe(world, self.account, id).is_ok()
                })
            })
            .cloned()
            .collect();
        events.reverse();
        let published_event = history
            .back()
            .map_or(self.sent_event, |event| event.sequence);
        self.sequence += 1;
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.observable_ships");
        let owned = observable_ships(world, self.account, &focused);
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.telemetry");
        let ships = owned
            .iter()
            .filter_map(|(_, entity)| super::commands::ship_telemetry(world, *entity, self.account))
            .collect();
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.slip");
        let mut presentation = PresentationFrame::default();
        presentation.slip = super::slip_effects::observe(world, self.account, &views);
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.navigation");
        presentation.navigation = super::infrastructure::navigation_snapshot(
            world,
            &views,
            &owned.iter().map(|(_, entity)| *entity).collect::<Vec<_>>(),
        );
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.ship_presentation");
        presentation.ships = owned
            .iter()
            .filter(|(id, _)| focused.contains(id))
            .filter_map(|(id, entity)| {
                super::presentation::ship(world, *entity, self.instruments.contains(id), &readings)
            })
            .collect();
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.optical_combat");
        let (optical, optically_visible) =
            self.optical
                .observe(world, self.account, &views, &contacts, &readings);
        presentation.combat = super::combat::for_session(
            world,
            &self.optical.references(),
            &optically_visible,
            &self.optical.previous_entities,
            self.sent_event,
        );
        self.optical.previous_entities = optically_visible;
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.diagnostics");
        let owner = identity::lookup(world, self.account)?;
        if world
            .get::<Account>(owner)
            .is_some_and(|account| account.debug)
        {
            presentation.capabilities = vec![
                DebugCapability::Clock,
                DebugCapability::Reset,
                DebugCapability::Relocate,
                DebugCapability::Recover,
                DebugCapability::InjectHeat,
                DebugCapability::Inspect,
                DebugCapability::ConfigureSensor,
            ];
            if world.resource::<Clock>().inspect {
                let mut active_ships = 0;
                let mut dormant_ships = 0;
                let mut timings = super::vessel::ShipStepTimings::default();
                for (dormant, software) in world.query_filtered::<(Has<super::travel::Dormant>, Option<&SoftwareDiagnostics>), With<super::vessel::Vessel>>().iter(world) {
                    if dormant {
                        dormant_ships += 1;
                    } else {
                        active_ships += 1;
                        if let Some(software) = software {
                            timings.prepare += software.timings.prepare;
                            timings.callback += software.timings.callback;
                            timings.scan += software.timings.scan;
                            timings.publish += software.timings.publish;
                            timings.hardware += software.timings.hardware;
                        }
                    }
                }
                let mut systems = vec![
                    ("ship preparation".into(), timings.prepare * 1000.),
                    ("ship WASM callbacks".into(), timings.callback * 1000.),
                    ("ship sensor queries".into(), timings.scan * 1000.),
                    ("ship publication".into(), timings.publish * 1000.),
                    ("ship hardware".into(), timings.hardware * 1000.),
                ];
                if let Some(collision) =
                    world.get_resource::<super::physics::collision::CollisionStats>()
                {
                    systems.extend([
                        ("collision indexing".into(), collision.index_seconds * 1000.),
                        ("collision queries".into(), collision.query_seconds * 1000.),
                        ("collision solving".into(), collision.solve_seconds * 1000.),
                        ("collision total".into(), collision.total_seconds * 1000.),
                    ]);
                }
                presentation.diagnostics = Some(Diagnostics {
                    collision: world
                        .get_resource::<super::physics::collision::CollisionStats>()
                        .map(|stats| CollisionDiagnostics {
                            bodies: stats.bodies as u64,
                            candidates: stats.candidates,
                            contact_pairs: stats.contact_pairs,
                            impacts: stats.impacts,
                            dissipated_j: stats.dissipated_j,
                        }),
                    entity_count: world.entities().len() as u64,
                    active_ships,
                    dormant_ships,
                    tick_duration_ms: world
                        .get_resource::<super::simulation::TickMetrics>()
                        .map_or(0., |metrics| metrics.last_duration_ms),
                    systems,
                });
            }
        }
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.screens");
        let screens = self
            .screens
            .keys()
            .map(|(ship, slot)| {
                let entity = identity::lookup(world, *ship).unwrap();
                super::displays::frame(world, entity, *slot).unwrap_or(ScreenUpdate {
                    ship: *ship,
                    slot: *slot,
                    revision: world.get::<Control>(entity).unwrap().revision,
                    tick,
                    frame: None,
                    error: Some("Display unavailable".into()),
                })
            })
            .collect();
        self.sent_event = published_event;
        drop(profile);
        let profile = super::diagnostics::ProfileScope::new("session.chat");
        let chat = self.chat.frame(world, self.account, &self.views)?;
        drop(profile);
        Ok(Frame {
            chat,
            optical,
            calendar_unix_ms: osg_model::calendar::now_unix_ms(),
            presentation,
            world: world.resource::<WorldEpoch>().0,
            sequence: self.sequence,
            tick,
            sim_time_ns: world
                .resource::<Time<Fixed>>()
                .elapsed()
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            rate: world.resource::<Clock>().rate,
            views,
            contacts: contacts
                .into_iter()
                .map(|(observer, contacts)| (observer, contacts.into_values().collect()))
                .collect(),
            ships,
            screens,
            events,
            results: self.results.drain(..).collect(),
        })
    }
}

pub fn ship_pose(world: &World, entity: Entity) -> Option<Pose> {
    resolve_ship_pose(
        entity,
        |entity| {
            Some((
                world.get::<super::precision::PreciseTransform>(entity)?,
                world.get::<super::physics::Velocity>(entity),
                world.get::<super::physics::AngularVelocity>(entity),
                world.get::<super::travel::PresenceState>(entity),
                world.get::<super::travel::Transit>(entity),
                world.get::<super::travel::DockingBays>(entity),
            ))
        },
        |id| identity::lookup(world, id).ok(),
    )
}

pub(crate) type ShipPoseParts<'a> = (
    &'a super::precision::PreciseTransform,
    Option<&'a super::physics::Velocity>,
    Option<&'a super::physics::AngularVelocity>,
    Option<&'a super::travel::PresenceState>,
    Option<&'a super::travel::Transit>,
    Option<&'a super::travel::DockingBays>,
);

pub(crate) fn resolve_ship_pose<'a>(
    entity: Entity,
    parts: impl Fn(Entity) -> Option<ShipPoseParts<'a>>,
    lookup: impl Fn(Id) -> Option<Entity>,
) -> Option<Pose> {
    let mut current = entity;
    let mut visited = Vec::new();
    let mut bays = Vec::new();
    let mut pose = loop {
        if visited.contains(&current) {
            return None;
        }
        let (transform, velocity, angular, presence, transit, _) = parts(current)?;
        if let Some(super::travel::PresenceState(travel::Presence::Docked { host, bay })) = presence
        {
            visited.push(current);
            current = lookup(*host)?;
            bays.push(parts(current)?.5?.0.get(*bay as usize)?);
        } else {
            break transit.map_or_else(
                || super::identity::pose(transform, velocity, angular),
                |transit| transit.pose(transform.rotation.to_array()),
            );
        }
    };
    for bay in bays.into_iter().rev() {
        pose = super::travel::bay_pose(&pose, bay);
    }
    Some(pose)
}

fn observable_ships(
    world: &mut World,
    account: AccountId,
    focused: &BTreeSet<Id>,
) -> Vec<(Id, Entity)> {
    let cached = cache::get(world, account);
    let mut ships: Vec<_> = cached
        .ships
        .iter()
        .map(|&(id, entity, can_control)| {
            let living = world
                .get::<super::travel::PresenceState>(entity)
                .is_none_or(|presence| {
                    !matches!(
                        presence.0,
                        travel::Presence::Destroyed | travel::Presence::StoredInWreck(_)
                    )
                });
            let controllable = living && can_control;
            ((!focused.contains(&id), !controllable, id), entity)
        })
        .collect();

    ships.sort_unstable_by_key(|(priority, _)| *priority);
    ships.truncate(64);
    ships
        .into_iter()
        .map(|((_, _, id), entity)| (id, entity))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::identity::Transponder;
    use super::*;

    fn fixture() -> (World, Entity, Id, Id, Entity) {
        let mut world = World::new();
        let account = Id::new();
        identity::initialize(&mut world, &[account]);
        world.init_resource::<Clock>();
        world.init_resource::<Events>();
        world.init_resource::<SimulationCounters>();
        let ship_id = Id::new();
        let ship = world
            .spawn((
                Control {
                    account,
                    revision: 1,
                },
                super::super::ownership::AssetOwner(ownership::Principal::Player(account)),
                Transponder(IffIdentity {
                    owner: account,
                    faction: None,
                    labels: BTreeSet::new(),
                    enabled: true,
                }),
            ))
            .id();
        identity::register(&mut world, ship, ship_id).unwrap();
        let session = connect(
            &mut world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        (world, session, account, ship_id, ship)
    }

    fn batch(world: &World, sequence: u64, id: Id, action: Action) -> InputFrame {
        InputFrame {
            world: world.resource::<WorldEpoch>().0,
            sequence,
            actions: vec![(id, action)],
        }
    }

    #[test]
    fn publication_keeps_controllable_living_ship_ahead_of_more_than_64_view_only_assets() {
        let (mut world, _, account, _, _) = fixture();
        let owner = Id::new();
        identity::add_account(&mut world, owner, false);
        let mut last_view_only = Id([0; 16]);
        for index in 1..=80 {
            let id = Id([index; 16]);
            let entity = world
                .spawn((
                    super::super::vessel::Vessel {
                        vessel_name: "View-only station".into(),
                    },
                    Control {
                        account: owner,
                        revision: 1,
                    },
                    super::super::ownership::AssetOwner(ownership::Principal::Player(owner)),
                    super::super::ownership::AssetAccess(ownership::AccessPolicy {
                        public: BTreeSet::from([ownership::Permission::View]),
                        grants: Vec::new(),
                    }),
                ))
                .id();
            identity::register(&mut world, entity, id).unwrap();
            last_view_only = id;
        }
        for (id, presence) in [
            (Id([90; 16]), travel::Presence::Destroyed),
            (Id([91; 16]), travel::Presence::StoredInWreck(Id([99; 16]))),
            (Id([250; 16]), travel::Presence::Space),
        ] {
            let entity = world
                .spawn((
                    super::super::vessel::Vessel {
                        vessel_name: "Player hull".into(),
                    },
                    Control {
                        account,
                        revision: 1,
                    },
                    super::super::ownership::AssetOwner(ownership::Principal::Player(account)),
                    super::super::travel::PresenceState(presence),
                ))
                .id();
            identity::register(&mut world, entity, id).unwrap();
        }

        prepare_publication(&mut world);
        let ships = observable_ships(&mut world, account, &BTreeSet::new());
        assert_eq!(ships.len(), 64);
        assert_eq!(ships[0].0, Id([250; 16]));
        assert!(!ships.iter().any(|(id, _)| *id == last_view_only));
        assert!(
            !ships
                .iter()
                .any(|(id, _)| *id == Id([90; 16]) || *id == Id([91; 16]))
        );

        let ships = observable_ships(&mut world, account, &BTreeSet::from([last_view_only]));
        assert_eq!(ships.len(), 64);
        assert_eq!(ships[0].0, last_view_only);
        assert_eq!(ships[1].0, Id([250; 16]));
    }

    #[test]
    fn telemetry_control_fact_is_per_account_and_tracks_acl_changes() {
        let owner = Id::new();
        let viewer = Id::new();
        let mut app = super::super::provision(&[owner, viewer], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let ship = world
            .query::<(Entity, &super::super::ownership::AssetOwner)>()
            .iter(world)
            .find(|(_, asset)| asset.0 == ownership::Principal::Player(owner))
            .unwrap()
            .0;
        world
            .entity_mut(ship)
            .insert(super::super::ownership::AssetAccess(
                ownership::AccessPolicy {
                    public: BTreeSet::from([ownership::Permission::View]),
                    grants: Vec::new(),
                },
            ));
        let own = super::super::commands::ship_telemetry(world, ship, owner).unwrap();
        let shared = super::super::commands::ship_telemetry(world, ship, viewer).unwrap();
        assert!(own.can_control);
        assert!(!shared.can_control);
        assert_eq!(own.authority_revision, shared.authority_revision);

        world
            .get_mut::<super::super::ownership::AssetAccess>(ship)
            .unwrap()
            .0
            .public
            .insert(ownership::Permission::Control);
        assert!(
            super::super::commands::ship_telemetry(world, ship, viewer)
                .unwrap()
                .can_control
        );
        world
            .get_mut::<super::super::ownership::AssetAccess>(ship)
            .unwrap()
            .0
            .public
            .remove(&ownership::Permission::Control);
        assert!(
            !super::super::commands::ship_telemetry(world, ship, viewer)
                .unwrap()
                .can_control
        );
    }

    #[test]
    fn permissions_admit_only_their_command_classes_and_enforce_revision() {
        let (mut world, _, owner, ship_id, ship) = fixture();
        let delegate = Id::new();
        identity::add_account(&mut world, delegate, false);
        let connection = connect(
            &mut world,
            delegate,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        let grant = |permission| ownership::AccessPolicy {
            public: BTreeSet::from([permission]),
            grants: Vec::new(),
        };
        super::super::ownership::set_access(
            &mut world,
            owner,
            ship,
            grant(ownership::Permission::Configure),
        )
        .unwrap();
        let mut session = world.entity_mut(connection).take::<Session>().unwrap();
        let action = |revision, command| Action::Ship {
            ship: ship_id,
            authority_revision: revision,
            command,
        };
        assert!(
            session
                .apply(
                    &mut world,
                    action(1, ShipCommand::SetTransponderEnabled(false))
                )
                .is_ok()
        );
        assert!(!world.get::<Transponder>(ship).unwrap().0.enabled);
        let error = session
            .apply(&mut world, action(1, ShipCommand::SetThrottle(1.)))
            .unwrap_err();
        assert!(error.to_string().contains("Control access denied"));
        assert!(
            session
                .apply(
                    &mut world,
                    action(0, ShipCommand::SetTransponderEnabled(true))
                )
                .is_err()
        );
        assert!(
            session
                .apply(&mut world, Action::InstrumentSubscribe { ship: ship_id })
                .is_err()
        );

        super::super::ownership::set_access(
            &mut world,
            owner,
            ship,
            grant(ownership::Permission::Control),
        )
        .unwrap();
        assert!(
            session
                .apply(
                    &mut world,
                    action(1, ShipCommand::SetTransponderEnabled(true))
                )
                .is_err()
        );
        assert!(
            session
                .apply(&mut world, Action::InstrumentSubscribe { ship: ship_id })
                .is_ok()
        );
        assert!(
            ship_authority(
                &world,
                delegate,
                ship_id,
                Some(1),
                super::super::commands::permission(&ShipCommand::SetThrottle(1.))
            )
            .is_ok()
        );

        super::super::ownership::set_access(
            &mut world,
            owner,
            ship,
            grant(ownership::Permission::View),
        )
        .unwrap();
        assert!(
            session
                .apply(&mut world, Action::InstrumentSubscribe { ship: ship_id })
                .is_ok()
        );
        assert!(
            session
                .apply(&mut world, action(1, ShipCommand::SetThrottle(1.)))
                .is_err()
        );
        super::super::ownership::set_access(&mut world, owner, ship, Default::default()).unwrap();
        assert!(observe(&world, delegate, ship_id).is_err());
    }

    #[test]
    fn publications_deliver_events_and_results_once_without_client_acknowledgements() {
        let account = Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let first = connect(
            world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        let second = connect(
            world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        let ship = world
            .query::<(&Identity, &Control)>()
            .iter(world)
            .find(|(_, control)| control.account == account)
            .unwrap()
            .0
            .0;
        let command = Id::new();
        input(
            world,
            first,
            batch(world, 1, command, Action::Debug(DebugCommand::SetRate(2.))),
        )
        .unwrap();
        world
            .resource_mut::<Events>()
            .0
            .extend((1..=9000).map(|sequence| osg_model::Event {
                sequence,
                tick: 0,
                subject: Some(ship),
                kind: "test".into(),
                position: None,
            }));
        prepare_publication(world);
        let one = frame(world, first).unwrap();
        assert_eq!(one.events.len(), 9000);
        assert!(
            one.events
                .windows(2)
                .all(|events| events[0].sequence < events[1].sequence)
        );
        assert_eq!(one.results.len(), 1);
        assert_eq!(one.results[0].id, command);
        prune_events(world);
        assert_eq!(world.resource::<Events>().0.len(), 9000);
        let two = frame(world, second).unwrap();
        assert_eq!(two.events.len(), 9000);
        assert!(two.results.is_empty());
        prune_events(world);
        assert_eq!(world.resource::<Events>().0.len(), 1);
        let next = frame(world, first).unwrap();
        assert!(next.events.is_empty());
        assert!(next.results.is_empty());
        let input_frame = batch(
            world,
            2,
            Id::new(),
            Action::Debug(DebugCommand::SetRate(2.)),
        );
        input(world, first, input_frame).unwrap();
        prepare_publication(world);
        assert_eq!(frame(world, first).unwrap().results.len(), 1);
    }

    #[test]
    fn views_require_access_to_the_focused_ship() {
        let (mut world, session, _, ship_id, ship) = fixture();
        super::super::ownership::capture_control(&mut world, ship, Id::new()).unwrap();
        let command = batch(
            &world,
            1,
            Id::new(),
            Action::Subscribe(ViewSubscription {
                id: 1,
                revision: 1,
                focused_ship: Some(ship_id),
            }),
        );
        input(&mut world, session, command).unwrap();
        assert!(world.get::<Session>(session).unwrap().views.is_empty());
        assert!(
            world
                .get::<Session>(session)
                .unwrap()
                .results
                .back()
                .unwrap()
                .error
                .is_some()
        );
    }

    #[test]
    fn capture_revokes_commands_without_rewriting_iff() {
        let (mut world, session, account, ship_id, ship) = fixture();
        super::super::ownership::capture_control(&mut world, ship, Id::new()).unwrap();
        let input_frame = batch(
            &world,
            1,
            Id::new(),
            Action::Ship {
                ship: ship_id,
                authority_revision: 1,
                command: ShipCommand::SetTransponderEnabled(false),
            },
        );
        input(&mut world, session, input_frame).unwrap();
        assert!(world.get::<Transponder>(ship).unwrap().0.enabled);
        assert_eq!(world.get::<Transponder>(ship).unwrap().0.owner, account);
        assert!(
            world.get::<Session>(session).unwrap().results[0]
                .error
                .is_some()
        );
    }

    #[test]
    fn client_cannot_grant_itself_debug_clock_control() {
        let (mut world, session, account, _, _) = fixture();
        let command = batch(
            &world,
            1,
            Id::new(),
            Action::Debug(DebugCommand::SetRate(2.)),
        );
        input(&mut world, session, command).unwrap();
        assert_eq!(world.resource::<Clock>().rate, 1.);
        assert!(
            world.get::<Session>(session).unwrap().results[0]
                .error
                .is_some()
        );
        let owner = identity::lookup(&world, account).unwrap();
        world.get_mut::<Account>(owner).unwrap().debug = true;
        let command = batch(
            &world,
            2,
            Id::new(),
            Action::Debug(DebugCommand::SetRate(2.)),
        );
        input(&mut world, session, command).unwrap();
        assert_eq!(world.resource::<Clock>().rate, 2.);
    }

    #[test]
    fn repeated_commands_are_idempotent_and_frames_reject_replay() {
        let (mut world, session, account, _, _) = fixture();
        let owner = identity::lookup(&world, account).unwrap();
        world.get_mut::<Account>(owner).unwrap().debug = true;
        let id = Id::new();
        let first = batch(&world, 1, id, Action::Debug(DebugCommand::SetRate(2.)));
        input(&mut world, session, first.clone()).unwrap();
        assert!(input(&mut world, session, first).is_err());
        let second = batch(&world, 2, id, Action::Debug(DebugCommand::SetRate(3.)));
        input(&mut world, session, second).unwrap();
        assert_eq!(world.resource::<Clock>().rate, 2.);
        assert_eq!(world.get::<Session>(session).unwrap().results.len(), 1);
    }
}
