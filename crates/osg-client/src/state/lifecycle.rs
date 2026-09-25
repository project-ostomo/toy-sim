use super::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use osg_model::ownership::PlayerAffiliation;
use osg_universe::universe::Universe;
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;

#[cfg(test)]
mod tests;

#[derive(States, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub enum ClientPhase {
    #[default]
    Loading,
    Running,
    Failed,
}

#[derive(Resource, Debug)]
pub struct SessionFailure {
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionKey {
    pub world: Id,
    pub generation: u64,
}

#[derive(Resource)]
pub struct GameSession {
    pub key: SessionKey,
    pub account: AccountId,
    pub universe_descriptor: UniverseDescriptor,
    pub universe: Arc<Universe>,
}

impl GameSession {
    fn new(
        key: SessionKey,
        account: AccountId,
        universe_descriptor: UniverseDescriptor,
        universe: Arc<Universe>,
    ) -> Self {
        Self {
            key,
            account,
            universe_descriptor,
            universe,
        }
    }

    #[cfg(test)]
    pub fn test(world: Id, generation: u64) -> Self {
        let universe = crate::universe::shared_universe().unwrap();
        Self::new(
            SessionKey { world, generation },
            Id([1; 16]),
            UniverseDescriptor {
                fingerprint: universe.fingerprint(),
                epoch_mjd_utc: 60_000.0,
                sim_time_origin_ns: 0,
            },
            universe,
        )
    }
}

#[derive(Resource, Default)]
pub struct Bootstrap {
    pub key: Option<SessionKey>,
    pub descriptor: Option<UniverseDescriptor>,
    pub sample: Option<(i64, Duration)>,
    generation: u64,
    player: Option<PlayerAffiliation>,
    identity: Option<(
        SessionKey,
        oneshot::Receiver<Result<PlayerAffiliation, String>>,
    )>,
    universe: Option<Arc<Universe>>,
    universe_task: Option<Task<Result<Arc<Universe>, String>>>,
}

impl Bootstrap {
    #[cfg(test)]
    pub fn prepared(&mut self, world: Id) {
        self.begin(world);
        let session = GameSession::test(world, self.generation);
        self.descriptor = Some(session.universe_descriptor);
        self.universe = Some(session.universe);
        self.player = Some(PlayerAffiliation {
            account: session.account,
            name: "Authenticated player".into(),
            organization: None,
        });
        self.sample = Some((0, Duration::ZERO));
    }

    pub fn begin(&mut self, world: Id) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            key: Some(SessionKey { world, generation }),
            generation,
            ..Default::default()
        };
    }

    pub fn disconnect(&mut self) {
        *self = Self {
            generation: self.generation,
            ..Default::default()
        };
    }
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientSystems {
    Gameplay,
}

pub fn install(app: &mut App) {
    app.init_state::<ClientPhase>()
        .configure_sets(
            Update,
            ClientSystems::Gameplay.run_if(in_state(ClientPhase::Running)),
        )
        .configure_sets(
            Update,
            (
                PresentationSet::Interpolate,
                PresentationSet::Views,
                PresentationSet::Render,
            )
                .in_set(ClientSystems::Gameplay),
        )
        .configure_sets(
            PostUpdate,
            ClientSystems::Gameplay.run_if(in_state(ClientPhase::Running)),
        )
        .configure_sets(
            Last,
            ClientSystems::Gameplay.run_if(in_state(ClientPhase::Running)),
        )
        .configure_sets(
            FixedPostUpdate,
            ClientSystems::Gameplay.run_if(in_state(ClientPhase::Running)),
        )
        .configure_sets(
            osg_ui::bevy_egui::EguiPrimaryContextPass,
            ClientSystems::Gameplay.run_if(in_state(ClientPhase::Running)),
        )
        .add_systems(PreUpdate, receive_session_events)
        .add_systems(
            OnEnter(ClientPhase::Loading),
            (reset_session, clear_failure),
        )
        .add_systems(OnEnter(ClientPhase::Failed), reset_session)
        .add_systems(
            Update,
            advance_bootstrap.run_if(in_state(ClientPhase::Loading)),
        )
        .add_systems(FixedUpdate, apply.run_if(resource_exists::<GameSession>));
}

fn clear_failure(mut commands: Commands) {
    commands.remove_resource::<SessionFailure>();
}

pub fn reset_session(
    mut commands: Commands,
    mut replication: ResMut<Replication>,
    mut clock: ResMut<RenderTime>,
    mut info: ResMut<SessionInfo>,
    members: Query<Entity, With<WorldMember>>,
) {
    for entity in &members {
        commands.entity(entity).despawn();
    }
    *replication = Replication::default();
    *clock = RenderTime::default();
    *info = SessionInfo::default();
    commands.remove_resource::<GameSession>();
    commands.remove_resource::<CalendarClock>();
    commands.trigger(SessionReset);
}

pub fn start_bootstrap(mut bootstrap: ResMut<Bootstrap>, client: Res<requests::NetworkClient>) {
    let Some(key) = bootstrap.key else {
        return;
    };

    if bootstrap.universe.is_none() && bootstrap.universe_task.is_none() {
        bootstrap.universe_task = Some(AsyncComputeTaskPool::get().spawn(async {
            crate::universe::shared_universe().map_err(|error| error.to_string())
        }));
    }
    if bootstrap.player.is_none() && bootstrap.identity.is_none() {
        let net = client.0.clone();
        let (send, receive) = oneshot::channel();
        bootstrap.identity = Some((key, receive));
        bevy::tasks::IoTaskPool::get()
            .spawn(async move {
                let _ = send.send(requests::call(net.my_affiliation(key.world)).await);
            })
            .detach();
    }
}

pub fn advance_bootstrap(
    mut commands: Commands,
    mut bootstrap: ResMut<Bootstrap>,
    mut next: ResMut<NextState<ClientPhase>>,
    session: Option<Res<GameSession>>,
    replication: Res<Replication>,
) {
    if let Some(session) = session {
        if bootstrap.key == Some(session.key) && replication.applied == Some(session.key) {
            next.set(ClientPhase::Running);
        }
        return;
    }
    let Some(key) = bootstrap.key else {
        return;
    };

    if let Some(task) = &mut bootstrap.universe_task {
        if let Some(result) = block_on(poll_once(task)) {
            bootstrap.universe_task = None;
            match result {
                Ok(universe) => bootstrap.universe = Some(universe),
                Err(error) => {
                    fail_bootstrap(&mut commands, &mut next, error);
                    return;
                }
            }
        }
    }
    if let Some((job_key, receive)) = &mut bootstrap.identity {
        let job_key = *job_key;
        let result = match receive.try_recv() {
            Ok(result) => Some(result),
            Err(oneshot::error::TryRecvError::Closed) => {
                Some(Err("Identity request cancelled".into()))
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
        };
        if let Some(result) = result {
            bootstrap.identity = None;
            if job_key == key {
                match result {
                    Ok(player) => bootstrap.player = Some(player),
                    Err(error) => {
                        fail_bootstrap(&mut commands, &mut next, error);
                        return;
                    }
                }
            }
        }
    }
    let (Some(key), Some(descriptor), Some(player), Some(universe), Some((unix_ms, received_at))) = (
        bootstrap.key,
        bootstrap.descriptor.as_ref(),
        bootstrap.player.as_ref(),
        bootstrap.universe.as_ref(),
        bootstrap.sample,
    ) else {
        return;
    };
    if descriptor.fingerprint != universe.fingerprint() {
        fail_bootstrap(
            &mut commands,
            &mut next,
            "Universe catalogue mismatch".into(),
        );
        return;
    }
    let mut society = SocietyState::default();
    society.context = Some(key);
    society.society.account = player.account;
    society
        .society
        .directory
        .players
        .insert(player.account, player.clone());
    commands.insert_resource(society);
    commands.insert_resource(NavigationState {
        navigation: Arc::new(NavigationCatalogue {
            topology_revision: 1,
            systems: universe
                .systems()
                .iter()
                .map(|system| NavigationSystem {
                    id: Id(system.id),
                    name: system.name.to_string(),
                    position: system.position,
                    sovereignty: None,
                })
                .collect(),
            beacons: Vec::new(),
        }),
        ..Default::default()
    });
    commands.insert_resource(CalendarClock::new(unix_ms, received_at));
    commands.insert_resource(GameSession::new(
        key,
        player.account,
        descriptor.clone(),
        universe.clone(),
    ));
    commands.trigger(SessionInstalled {
        account: player.account,
    });
}

#[derive(Event)]
pub struct SessionInstalled {
    pub account: AccountId,
}

fn fail_bootstrap(commands: &mut Commands, next: &mut NextState<ClientPhase>, message: String) {
    commands.insert_resource(SessionFailure {
        message,
        retryable: true,
    });
    next.set(ClientPhase::Failed);
}

pub fn draw_session_status(
    mut contexts: osg_ui::bevy_egui::EguiContexts,
    phase: Res<State<ClientPhase>>,
    mut next: ResMut<NextState<ClientPhase>>,
    failure: Option<Res<SessionFailure>>,
) -> Result {
    if *phase.get() == ClientPhase::Running {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    if render_session_status(ctx, phase.get(), failure.as_deref()) {
        next.set(ClientPhase::Loading);
    }
    Ok(())
}

pub fn render_session_status(
    ctx: &osg_ui::egui::Context,
    phase: &ClientPhase,
    failure: Option<&SessionFailure>,
) -> bool {
    let mut clicked = false;
    osg_ui::egui::CentralPanel::default().show(
        &mut osg_ui::egui::Ui::new(
            ctx.clone(),
            osg_ui::egui::Id::new("session_status"),
            osg_ui::egui::UiBuilder::new().max_rect(ctx.content_rect()),
        ),
        |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.35);
                match phase {
                    ClientPhase::Loading => {
                        ui.heading("Loading session");
                        ui.spinner();
                    }
                    ClientPhase::Failed => {
                        ui.heading("Session unavailable");
                        let failure = failure.expect("Failed state has a failure reason");
                        ui.label(&failure.message);
                        if failure.retryable {
                            clicked = ui.button("Retry").clicked();
                        } else {
                            ui.label("Connect again to continue.");
                        }
                    }
                    ClientPhase::Running => {}
                }
            });
        },
    );
    clicked
}
