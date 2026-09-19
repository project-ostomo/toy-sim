use anyhow::{Result, ensure};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use toy_sim_model::*;
use toy_sim_ship_wasm::Command;

use super::identity::{self, Account, Control, Identity, Membership, Transponder, WorldEpoch};
use super::intelligence::Group;
use super::simulation::SimulationCounters;
use super::vessel::ShipSoftware;

#[derive(Resource)]
pub struct Clock {
    pub rate: f64,
    pub steps: u32,
    pub reset_requested: bool,
    pub inspect: bool,
    pub requests: VecDeque<DebugCommand>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            rate: 1.,
            steps: 0,
            reset_requested: false,
            inspect: false,
            requests: VecDeque::new(),
        }
    }
}

#[derive(Resource, Default)]
pub struct Events(pub VecDeque<toy_sim_model::Event>);

#[derive(Component)]
pub struct Session {
    pub account: AccountId,
    pub groups: BTreeSet<GroupId>,
    pub views: BTreeMap<u64, ViewSubscription>,
    pub screens: BTreeMap<(EntityId, u8), u8>,
    pub instruments: BTreeSet<EntityId>,
    sequence: u64,
    input_sequence: Option<u64>,
    sent_event: u64,
    results: VecDeque<CommandResult>,
    seen: BTreeSet<Id>,
}

pub fn connect(world: &mut World, account: AccountId) -> Result<Entity> {
    let owner = identity::lookup(world, account)?;
    let group = world
        .get::<Account>(owner)
        .ok_or_else(|| anyhow::anyhow!("account unavailable"))?
        .group;
    let group = world.get::<Group>(group).unwrap().id;
    let sent_event = world
        .resource::<Events>()
        .0
        .back()
        .map_or(0, |event| event.sequence);
    Ok(world
        .spawn(Session {
            account,
            groups: BTreeSet::from([group, PUBLIC_GROUP]),
            views: BTreeMap::new(),
            screens: BTreeMap::new(),
            instruments: BTreeSet::new(),
            sequence: 0,
            input_sequence: None,
            sent_event,
            results: VecDeque::new(),
            seen: BTreeSet::new(),
        })
        .id())
}

pub fn disconnect(world: &mut World, session: Entity) {
    world.despawn(session);
}

pub fn prune_events(world: &mut World) {
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

pub fn control(world: &World, account: Id, ship: Id, revision: Option<u64>) -> Result<Entity> {
    let entity = identity::lookup(world, ship)?;
    let authority = world
        .get::<Control>(entity)
        .ok_or_else(|| anyhow::anyhow!("ship unavailable"))?;
    ensure!(authority.account == account, "control access denied");
    ensure!(
        revision.is_none_or(|revision| revision == authority.revision),
        "control authority changed"
    );
    Ok(entity)
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

pub fn frame(world: &mut World, entity: Entity) -> Result<Frame> {
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
        toy_sim_protocol::validate_input(&input)?;
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
            let (reply, error) = match self.apply(world, action) {
                Ok(reply) => (reply, None),
                Err(error) => (None, Some(error.to_string())),
            };
            self.results.push_back(CommandResult {
                reply,
                id,
                effective_tick: world.resource::<SimulationCounters>().ticks,
                error,
            });
        }
        Ok(())
    }

    fn target(&self, world: &mut World, ship: Entity, group: Id, track: Id) -> Result<u64> {
        let member = world
            .get::<Membership>(ship)
            .ok_or_else(|| anyhow::anyhow!("ship group unavailable"))?
            .0;
        ensure!(
            world
                .get::<Group>(member)
                .is_some_and(|item| item.id == group)
                && self.groups.contains(&group),
            "target group access denied"
        );
        ensure!(
            world
                .get::<Group>(member)
                .unwrap()
                .snapshot
                .tracks
                .contains_key(&track),
            "target track unavailable"
        );
        super::services::contact_handle(world, ship, group, track)
    }

    fn command(&self, world: &mut World, ship: Entity, command: Command) -> Result<()> {
        let mut software = world
            .get_mut::<ShipSoftware>(ship)
            .ok_or_else(|| anyhow::anyhow!("ship computer unavailable"))?;
        ensure!(software.inbox.len() < 255, "ship command queue full");
        software.command(command);
        Ok(())
    }

    fn apply(&mut self, world: &mut World, action: Action) -> Result<Option<Reply>> {
        match action {
            Action::JoinGroup(key) => {
                ensure!(self.groups.len() < 16, "group subscription limit");
                let entity = super::intelligence::join(world, key);
                let group = world.get::<Group>(entity).unwrap().id;
                self.groups.insert(group);
                return Ok(Some(Reply::JoinedGroup(group)));
            }
            Action::Subscribe(view) => {
                ensure!(self.groups.contains(&view.group), "group access denied");
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
                    control(world, self.account, ship, None)?;
                }
                self.views.insert(view.id, view);
            }
            Action::Unsubscribe(id) => {
                self.views.remove(&id);
            }
            Action::InstrumentSubscribe { ship } => {
                self.detail_capacity(ship, None)?;
                control(world, self.account, ship, None)?;
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
                control(world, self.account, ship, None)?;
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
                    DebugCommand::Step => {
                        ensure!(clock.rate == 0., "pause before stepping");
                        clock.steps = clock.steps.saturating_add(1).min(100);
                    }
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
                let entity = control(world, self.account, ship, Some(authority_revision))?;
                match command {
                    ShipCommand::SetTransponderEnabled(enabled) => {
                        world.get_mut::<Transponder>(entity).unwrap().0.enabled = enabled
                    }
                    ShipCommand::SetGroup(key) => {
                        let group = super::intelligence::join(world, key);
                        world.entity_mut(entity).insert(Membership(group));
                    }
                    ShipCommand::SetIff(iff) => {
                        ensure!(
                            iff.owner == self.account,
                            "IFF owner must identify current controller"
                        );
                        let owner = identity::lookup(world, self.account)?;
                        ensure!(
                            iff.faction.is_none_or(|faction| world
                                .get::<Account>(owner)
                                .unwrap()
                                .factions
                                .contains(&faction)),
                            "IFF faction access denied"
                        );
                        ensure!(iff.range_m <= 1e8, "transponder range exceeds hardware");
                        world.entity_mut(entity).insert(Transponder(iff));
                    }
                    ShipCommand::SetTravel {
                        preferences,
                        engage,
                        expected_revision,
                        orders,
                    } => {
                        ensure!(
                            !matches!(
                                world
                                    .get::<super::travel::PresenceState>(entity)
                                    .map(|p| &p.0),
                                Some(travel::Presence::SlipTransit(_))
                            ),
                            "wait for slip arrival before changing the queue"
                        );
                        ensure!(
                            world
                                .get::<super::travel::Travel>(entity)
                                .is_some_and(|state| state.0.revision == expected_revision),
                            "stale travel revision"
                        );
                        ensure!(preferences.valid(), "invalid planning preference");
                        super::travel::cancel_pending(world, entity);
                        if engage {
                            self.command(
                                world,
                                entity,
                                Command::Manual {
                                    throttle: 0.,
                                    steering: [0.; 3],
                                },
                            )?;
                            self.command(world, entity, Command::HoldAttitude)?;
                        }
                        let mut state = world.get_mut::<super::travel::Travel>(entity).unwrap();
                        let enabled = engage || state.0.autopilot_enabled;
                        state.0 = travel::TravelState {
                            autopilot_enabled: enabled,
                            preferences,
                            revision: state.0.revision + 1,
                            orders: orders.into_iter().map(Into::into).collect(),
                            status: if enabled {
                                travel::Status::Planning
                            } else {
                                travel::Status::Paused
                            },
                            ..Default::default()
                        };
                    }
                    ShipCommand::SetAutopilot(enabled) => {
                        ensure!(
                            world
                                .get::<super::travel::PresenceState>(entity)
                                .is_some_and(|p| p.0 == travel::Presence::Space),
                            "ship is not in space"
                        );
                        super::travel::cancel_pending(world, entity);
                        self.command(
                            world,
                            entity,
                            Command::Manual {
                                throttle: 0.,
                                steering: [0.; 3],
                            },
                        )?;
                        self.command(world, entity, Command::HoldAttitude)?;
                        let mut state = world.get_mut::<super::travel::Travel>(entity).unwrap();
                        state.0.revision += 1;
                        state.0.autopilot_enabled = enabled;
                        state.0.status = if enabled {
                            travel::Status::Planning
                        } else {
                            travel::Status::Paused
                        };
                    }
                    ShipCommand::SetThrottle(throttle) => {
                        ensure_manual_control(world, entity)?;
                        self.command(world, entity, Command::SetThrottle(throttle))?;
                    }
                    ShipCommand::Dock { station, bay } => {
                        let station = identity::lookup(world, station)?;
                        super::travel::dock(world, entity, station, bay)?;
                    }
                    ShipCommand::TransferCargo {
                        target,
                        resource,
                        quantity,
                    } => {
                        let destination = control(world, self.account, target, None)?;
                        super::hardware::utilities::transfer_cargo(
                            world,
                            entity,
                            destination,
                            &resource,
                            quantity,
                        )?;
                    }
                    ShipCommand::SetDockServices { cargo, power } => {
                        ensure!(
                            matches!(
                                world
                                    .get::<super::travel::PresenceState>(entity)
                                    .map(|p| &p.0),
                                Some(travel::Presence::Docked { .. })
                            ),
                            "ship must be docked to request services"
                        );
                        world.entity_mut(entity).insert(
                            super::hardware::utilities::DockServiceRequest { cargo, power },
                        );
                    }
                    ShipCommand::Undock => super::travel::undock(world, entity)?,
                    ShipCommand::UnmarkTarget => {
                        self.command(world, entity, Command::UnmarkTarget)?
                    }
                    ShipCommand::StartFiring => {
                        self.command(world, entity, Command::StartFiring)?
                    }
                    ShipCommand::StopFiring => self.command(world, entity, Command::StopFiring)?,
                    ShipCommand::Aim { group, track } => {
                        let handle = self.target(world, entity, group, track)?;
                        self.command(world, entity, Command::AimContact(handle))?;
                    }
                    ShipCommand::MarkTarget {
                        group,
                        track,
                        maximum_flight_time_s,
                    } => {
                        let handle = self.target(world, entity, group, track)?;
                        self.command(
                            world,
                            entity,
                            Command::MarkTarget {
                                contact: handle,
                                maximum_flight_time_s,
                            },
                        )?;
                    }
                    ShipCommand::Flight(command) => {
                        ensure_manual_control(world, entity)?;
                        let command = match command {
                            FlightCommand::HoldAttitude => Command::HoldAttitude,
                            FlightCommand::StopGuidance => Command::StopGuidance,
                            FlightCommand::AimDirection(direction) => {
                                Command::AimDirection(direction)
                            }
                            FlightCommand::SelectTarget(target) => Command::SelectTarget(
                                self.target(world, entity, target.group, target.track)?,
                            ),
                            FlightCommand::EngageNavigation {
                                throttle_limit,
                                stand_off_m,
                            } => Command::EngageNavigation {
                                throttle_limit,
                                stand_off_m,
                            },
                        };
                        self.command(world, entity, command)?;
                    }
                    ShipCommand::ScreenInput {
                        slot,
                        revision,
                        kind,
                        code,
                        modifiers,
                        xy,
                        text,
                    } => {
                        ensure!(
                            self.screens.contains_key(&(ship, slot)),
                            "display is not subscribed"
                        );
                        super::displays::input(
                            world, entity, slot, revision, kind, code, modifiers, xy, &text,
                        )?;
                    }
                }
            }
        }
        Ok(None)
    }
}

impl Session {
    fn frame(&mut self, world: &mut World) -> Result<Frame> {
        self.views.retain(|_, view| {
            view.focused_ship
                .is_none_or(|ship| control(world, self.account, ship, None).is_ok())
        });
        self.screens
            .retain(|(ship, _), _| control(world, self.account, *ship, None).is_ok());
        self.instruments
            .retain(|ship| control(world, self.account, *ship, None).is_ok());
        let tick = world.resource::<SimulationCounters>().ticks;
        let mut tracks: BTreeMap<GroupId, BTreeMap<TrackId, Track>> = self
            .groups
            .iter()
            .map(|group| (*group, BTreeMap::new()))
            .collect();
        let mut views = Vec::new();
        let mut work = 2_000_000_u64;
        for view in self.views.values() {
            let mut query = view.query.clone();
            query.work = query.work.min(work);
            let focus_pose = view
                .focused_ship
                .and_then(|id| identity::lookup(world, id).ok())
                .and_then(|entity| ship_pose(world, entity));
            if let (Some(pose), Some((position, _))) = (&focus_pose, &mut query.sphere) {
                *position = pose.position;
            }
            let group = identity::lookup(world, view.group)?;
            let snapshot = world
                .get::<Group>(group)
                .ok_or_else(|| anyhow::anyhow!("group unavailable"))?
                .snapshot
                .clone();
            let page = toy_sim_intel::query::Queries::default().start(snapshot, query, tick)?;
            work = work.saturating_sub(page.gas_used);
            views.push(ViewState {
                focused_ship: view.focused_ship,
                origin: focus_pose
                    .map(|pose| pose.position)
                    .or(view.query.sphere.map(|(position, _)| position))
                    .unwrap_or(GalacticPosition::ZERO),
                id: view.id,
                revision: view.revision,
                group: view.group,
                tracks: page.tracks.iter().map(|track| track.id).collect(),
                completion: page.completion,
            });
            tracks
                .get_mut(&view.group)
                .unwrap()
                .extend(page.tracks.into_iter().map(|track| (track.id, track)));
        }
        let visible: BTreeSet<_> = tracks
            .values()
            .flat_map(|tracks| tracks.values().filter_map(|track| track.entity))
            .collect();
        let history = &world.resource::<Events>().0;
        let events = history
            .iter()
            .filter(|event| event.sequence > self.sent_event)
            .filter(|event| {
                event.subject.is_some_and(|id| {
                    visible.contains(&id) || control(world, self.account, id, None).is_ok()
                })
            })
            .cloned()
            .collect();
        let published_event = history
            .back()
            .map_or(self.sent_event, |event| event.sequence);
        self.sequence += 1;
        let owner = identity::lookup(world, self.account)?;
        let mut owned: Vec<_> = world
            .get::<identity::OwnedShips>(owner)
            .into_iter()
            .flat_map(|ships| ships.iter())
            .filter_map(|entity| {
                let authority = world.get::<Control>(entity)?;
                (authority.account == self.account)
                    .then(|| world.get::<Identity>(entity).map(|id| (id.0, entity)))
                    .flatten()
            })
            .collect();
        let focused: BTreeSet<_> = self
            .views
            .values()
            .filter_map(|view| view.focused_ship)
            .chain(self.screens.keys().map(|(ship, _)| *ship))
            .chain(self.instruments.iter().copied())
            .collect();
        owned.sort_by_key(|(id, _)| (!focused.contains(id), *id));
        owned.truncate(64);
        let ships = owned
            .iter()
            .filter_map(|(_, entity)| telemetry(world, *entity))
            .collect();
        let mut presentation = PresentationFrame::default();
        presentation.navigation = super::infrastructure::catalogue(world);
        presentation.ships = owned
            .iter()
            .filter(|(id, _)| focused.contains(id))
            .filter_map(|(id, entity)| {
                super::presentation::ship(world, *entity, self.instruments.contains(id))
            })
            .collect();
        for (&group, group_tracks) in &tracks {
            for track in group_tracks.values() {
                if track.observed_tick != tick {
                    continue;
                }
                if let Some(entity) = track.entity.and_then(|id| identity::lookup(world, id).ok()) {
                    if let Some(visual) = super::presentation::visual(
                        world,
                        entity,
                        ContactRef {
                            group,
                            track: track.id,
                        },
                    ) {
                        presentation.visuals.push(visual);
                    }
                }
            }
        }
        presentation.combat =
            super::combat::for_session(world, self.account, &tracks, self.sent_event);
        let owner = identity::lookup(world, self.account)?;
        let debug = world
            .get::<Account>(owner)
            .is_some_and(|account| account.debug);
        if let Some(registry) = world.get_resource::<super::registry::UniverseRegistry>() {
            let inspected = debug
                .then(|| {
                    world
                        .get_resource::<super::orrery::activity::UniverseDebug>()
                        .and_then(|debug| debug.inspect.as_deref())
                })
                .flatten();
            presentation.celestial_systems = registry.system_refs(&mut views, inspected);

            let active_systems = if debug {
                world
                    .get_resource::<super::orrery::activity::ActiveSystems>()
                    .into_iter()
                    .flat_map(|active| {
                        active.entities.keys().filter_map(|index| {
                            let system = registry.universe.systems.get(*index)?;
                            Some(ActiveSystem {
                                system: super::registry::system_identity(&system.solver.name),
                                reason: if active.ship_systems.contains(index) {
                                    "ships"
                                } else {
                                    "debug inspection"
                                }
                                .into(),
                            })
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            presentation.universe = Some(UniverseStatus {
                catalogue: registry.catalogue,
                active_systems,
                inspected_body: debug
                    .then(|| {
                        world
                            .get_resource::<super::orrery::activity::UniverseDebug>()
                            .and_then(|debug| debug.inspect.as_deref())
                            .map(super::registry::identity)
                    })
                    .flatten(),
            });
        }
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
                for (dormant, software) in world.query_filtered::<(Has<super::travel::Dormant>, Option<&ShipSoftware>), With<super::vessel::Vessel>>().iter(world) {
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
                            detailed_queries: stats.detailed_queries,
                            impacts: stats.impacts,
                            contact_reviews: stats.contact_reviews,
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
        Ok(Frame {
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
            tracks: tracks
                .into_iter()
                .map(|(group, tracks)| (group, tracks.into_values().collect()))
                .collect(),
            ships,
            screens,
            events,
            results: self.results.drain(..).collect(),
        })
    }
}

pub fn ship_pose(world: &World, entity: Entity) -> Option<Pose> {
    let pose = world.get::<super::precision::PreciseTransform>(entity)?;
    if let Some(super::travel::PresenceState(travel::Presence::Docked { host, bay })) =
        world.get::<super::travel::PresenceState>(entity)
    {
        let station = identity::lookup(world, *host).ok()?;
        let host_pose = ship_pose(world, station)?;
        return Some(super::travel::bay_pose(
            &host_pose,
            world
                .get::<super::travel::DockingBays>(station)?
                .0
                .get(*bay as usize)?,
        ));
    }
    if let Some(transit) = world.get::<super::travel::Transit>(entity) {
        let now = world.resource::<SimulationCounters>().ticks;
        let duration = transit.next_attempt.saturating_sub(transit.departed).max(1) as f64;
        let progress = (now.saturating_sub(transit.departed) as f64 / duration).clamp(0., 1.);
        let delta = transit.destination.relative_to(transit.origin);
        return Some(Pose {
            position: transit.origin.offset_by(delta * progress),
            rotation: pose.rotation.to_array(),
            velocity: (delta / (duration * 0.1)).to_array(),
            angular_velocity: [0.; 3],
        });
    }
    Some(super::intelligence::pose(
        pose,
        world.get::<super::physics::Velocity>(entity),
        world.get::<super::physics::AngularVelocity>(entity),
    ))
}

fn telemetry(world: &World, entity: Entity) -> Option<ShipTelemetry> {
    let inventory = &world.get::<super::hardware::ShipInventory>(entity)?.0;
    let thermal = &world.get::<super::hardware::ShipThermal>(entity)?.0;
    let design = &world.get::<super::vessel::ShipDesign>(entity)?.0;
    let authority = world.get::<Control>(entity)?;
    let group = world.get::<Membership>(entity)?.0;
    Some(ShipTelemetry {
        appearance: world
            .get::<super::identity::Appearance>(entity)
            .map(|appearance| appearance.0),
        radius_m: design.radius,
        dock_services: world
            .get::<super::hardware::utilities::DockServiceRequest>(entity)
            .map_or_else(DockServiceSettings::default, |request| {
                DockServiceSettings {
                    cargo: request.cargo,
                    power: request.power,
                }
            }),
        spatial_instance: world.get::<super::identity::SpatialInstance>(entity)?.0,
        info_group: world.get::<Group>(group)?.key?,
        iff: world.get::<Transponder>(entity)?.0.clone(),
        ship: world.get::<Identity>(entity)?.0,
        authority_revision: authority.revision,
        presence: world
            .get::<super::travel::PresenceState>(entity)
            .map(|presence| presence.0.clone())
            .unwrap_or(travel::Presence::Space),
        pose: world
            .get::<super::travel::PresenceState>(entity)
            .is_none_or(|presence| {
                matches!(
                    presence.0,
                    travel::Presence::Space
                        | travel::Presence::Docked { .. }
                        | travel::Presence::SlipTransit(_)
                )
            })
            .then(|| ship_pose(world, entity))
            .flatten(),
        battery_j: inventory.energy_j,
        hull_heat_j: thermal.hull_energy_j,
        shield_temperature_k: thermal.shield_temperature(design.as_ref().into()),
        coolant_reserve_kg: thermal.shield_reserve_kg(),
        travel: world
            .get::<super::travel::Travel>(entity)
            .map(|travel| travel.0.clone())
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
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
                Transponder(IffIdentity {
                    owner: account,
                    faction: None,
                    labels: BTreeSet::new(),
                    enabled: true,
                    range_m: 1e8,
                }),
            ))
            .id();
        identity::register(&mut world, ship, ship_id);
        let session = connect(&mut world, account).unwrap();
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
    fn publications_deliver_events_and_results_once_without_client_acknowledgements() {
        let account = Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let first = connect(world, account).unwrap();
        let second = connect(world, account).unwrap();
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
            batch(world, 1, command, Action::Debug(DebugCommand::Step)),
        )
        .unwrap();
        world
            .resource_mut::<Events>()
            .0
            .extend((1..=9000).map(|sequence| toy_sim_model::Event {
                sequence,
                tick: 0,
                subject: Some(ship),
                kind: "test".into(),
                position: None,
            }));
        let one = frame(world, first).unwrap();
        assert_eq!(one.events.len(), 9000);
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
        let input_frame = batch(world, 2, Id::new(), Action::Debug(DebugCommand::Step));
        input(world, first, input_frame).unwrap();
        assert_eq!(frame(world, first).unwrap().results.len(), 1);
    }

    #[test]
    fn group_secret_grants_views_without_granting_ship_control() {
        let (mut world, session, _, ship_id, ship) = fixture();
        let other_owner = Id::new();
        let other = identity::add_account(&mut world, other_owner, false);
        let group_entity = world.get::<Account>(other).unwrap().group;
        let group = world.get::<Group>(group_entity).unwrap();
        let key = group.key.unwrap();
        let group_id = group.id;
        world.get_mut::<Control>(ship).unwrap().account = other_owner;
        let view = ViewSubscription {
            id: 1,
            revision: 1,
            group: group_id,
            focused_ship: None,
            query: TrackQuery {
                limit: 16,
                work: 10_000,
                ..Default::default()
            },
        };
        let command = batch(&world, 1, Id::new(), Action::Subscribe(view.clone()));
        input(&mut world, session, command).unwrap();
        assert!(world.get::<Session>(session).unwrap().views.is_empty());

        let command = batch(&world, 2, Id::new(), Action::JoinGroup(key));
        input(&mut world, session, command).unwrap();
        let command = batch(&world, 3, Id::new(), Action::Subscribe(view));
        input(&mut world, session, command).unwrap();
        assert_eq!(world.get::<Session>(session).unwrap().views.len(), 1);

        let command = batch(
            &world,
            4,
            Id::new(),
            Action::Ship {
                ship: ship_id,
                authority_revision: 1,
                command: ShipCommand::SetTransponderEnabled(false),
            },
        );
        input(&mut world, session, command).unwrap();
        assert!(world.get::<Transponder>(ship).unwrap().0.enabled);
    }

    #[test]
    fn capture_revokes_commands_without_rewriting_iff() {
        let (mut world, session, account, ship_id, ship) = fixture();
        world.get_mut::<Control>(ship).unwrap().account = Id::new();
        world.get_mut::<Control>(ship).unwrap().revision += 1;
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
            Action::Debug(DebugCommand::SetRate(0.)),
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
            Action::Debug(DebugCommand::SetRate(0.)),
        );
        input(&mut world, session, command).unwrap();
        assert_eq!(world.resource::<Clock>().rate, 0.);
    }

    #[test]
    fn repeated_commands_are_idempotent_and_frames_reject_replay() {
        let (mut world, session, account, _, _) = fixture();
        let owner = identity::lookup(&world, account).unwrap();
        world.get_mut::<Account>(owner).unwrap().debug = true;
        world.resource_mut::<Clock>().rate = 0.;
        let id = Id::new();
        let first = batch(&world, 1, id, Action::Debug(DebugCommand::Step));
        input(&mut world, session, first.clone()).unwrap();
        assert!(input(&mut world, session, first).is_err());
        let second = batch(&world, 2, id, Action::Debug(DebugCommand::Step));
        input(&mut world, session, second).unwrap();
        assert_eq!(world.resource::<Clock>().steps, 1);
        assert_eq!(world.get::<Session>(session).unwrap().results.len(), 1);
    }
}

fn ensure_manual_control(world: &World, entity: Entity) -> anyhow::Result<()> {
    ensure!(
        world
            .get::<ShipSoftware>(entity)
            .is_some_and(|s| !s.controller.is_booting() && s.controller.fault.is_none())
            && world
                .get::<super::hardware::Avionics>(entity)
                .is_some_and(|a| a.0.operational && a.0.powered)
            && world
                .get::<super::hardware::Hull>(entity)
                .is_some_and(|h| h.0 > 0.),
        "flight computer unavailable"
    );
    ensure!(
        !world
            .get::<super::travel::Travel>(entity)
            .unwrap()
            .0
            .autopilot_enabled,
        "manual controls locked by autopilot"
    );
    ensure!(
        world
            .get::<super::travel::PresenceState>(entity)
            .is_some_and(|p| p.0 == travel::Presence::Space),
        "ship is not in space"
    );
    Ok(())
}
