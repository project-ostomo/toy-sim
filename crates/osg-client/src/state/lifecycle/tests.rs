use super::*;
use crate::NetEvent;
use bevy::time::TimeUpdateStrategy;

#[derive(Resource, Default)]
struct Runs(u32);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_resource(Time::<Fixed>::from_duration(TICK_DURATION))
        .insert_resource(TimeUpdateStrategy::ManualDuration(TICK_DURATION))
        .init_resource::<Bootstrap>()
        .init_resource::<SessionEvents>()
        .init_resource::<Replication>()
        .init_resource::<RenderTime>()
        .init_resource::<SessionInfo>()
        .init_resource::<SlipEffects>()
        .init_resource::<NavigationState>()
        .init_resource::<PlaybackState>()
        .init_resource::<CommandState>()
        .init_resource::<ChatState>()
        .init_resource::<ScreenFrames>()
        .init_resource::<Outgoing>()
        .init_resource::<Runs>()
        .insert_resource(BufferedPlayback(Playback::new(false)))
        .add_observer(reset_resource::<Outgoing>)
        .add_observer(reset_resource::<ScreenFrames>);
    install(&mut app);
    app.add_systems(
        Update,
        (|session: Res<GameSession>,
          society: Res<SocietyUiState>,
          calendar: Res<CalendarClock>,
          mut runs: ResMut<Runs>| {
            assert!(
                society
                    .society
                    .directory
                    .players
                    .contains_key(&session.account)
            );
            assert_eq!(calendar.now(Duration::ZERO), 1234);
            runs.0 += 1;
        })
        .in_set(ClientSystems::Gameplay),
    );
    app.update();
    app
}

fn player() -> PlayerAffiliation {
    PlayerAffiliation {
        account: Id([7; 16]),
        name: "Real player".into(),
        organization: None,
    }
}

fn frame(world: Id) -> Frame {
    Frame {
        world,
        sequence: 0,
        tick: 0,
        sim_time_ns: 0,
        calendar_unix_ms: 1234,
        rate: 1.,
        chat: None,
        optical: Vec::new(),
        presentation: Default::default(),
        views: Vec::new(),
        contacts: Default::default(),
        ships: Vec::new(),
        screens: Vec::new(),
        events: Vec::new(),
        results: Vec::new(),
    }
}

fn receive(app: &mut App, event: Result<NetEvent, String>) {
    app.world_mut()
        .resource_mut::<SessionEvents>()
        .0
        .push_back(event);
}

fn descriptor(world: Id) -> NetEvent {
    NetEvent::Session {
        world,
        universe: GameSession::test(world, 0).universe_descriptor,
    }
}

fn complete_dependencies(app: &mut App) {
    let mut bootstrap = app.world_mut().resource_mut::<Bootstrap>();
    bootstrap.universe = Some(crate::universe::shared_universe().unwrap());
    bootstrap.player = Some(player());
}

fn start(app: &mut App, world: Id) {
    receive(app, Ok(descriptor(world)));
    receive(app, Ok(NetEvent::Frame(Arc::new(frame(world)))));
    app.update();
    complete_dependencies(app);
    ready(app);
}

fn ready(app: &mut App) {
    for _ in 0..4 {
        if *app.world().resource::<State<ClientPhase>>().get() == ClientPhase::Running {
            return;
        }
        app.update();
    }
    assert_eq!(
        *app.world().resource::<State<ClientPhase>>().get(),
        ClientPhase::Running
    );
}

#[test]
fn delayed_dependencies_and_first_frame_gate_gameplay_with_zero_ships() {
    let mut app = app();
    let world = Id([2; 16]);
    receive(&mut app, Ok(NetEvent::Frame(Arc::new(frame(world)))));
    app.update();
    let key = app.world().resource::<Bootstrap>().key.unwrap();
    let (send, receive_identity) = oneshot::channel();
    app.world_mut().resource_mut::<Bootstrap>().identity = Some((key, receive_identity));
    app.update();
    assert_eq!(app.world().resource::<Runs>().0, 0);
    assert!(!app.world().contains_resource::<GameSession>());

    send.send(Ok(player())).unwrap();
    app.update();
    assert!(!app.world().contains_resource::<GameSession>());
    app.world_mut().resource_mut::<Bootstrap>().universe =
        Some(crate::universe::shared_universe().unwrap());
    app.update();
    assert!(!app.world().contains_resource::<GameSession>());
    receive(&mut app, Ok(descriptor(world)));
    // Installation alone does not authorize gameplay before the fixed replication schedule.
    app.update();
    assert!(app.world().contains_resource::<GameSession>());
    assert_eq!(
        *app.world().resource::<State<ClientPhase>>().get(),
        ClientPhase::Loading
    );
    assert_eq!(app.world().resource::<Runs>().0, 0);
    app.update();
    assert_eq!(
        *app.world().resource::<State<ClientPhase>>().get(),
        ClientPhase::Loading
    );
    assert_eq!(app.world().resource::<Runs>().0, 0);
    ready(&mut app);
    assert!(app.world().resource::<Runs>().0 > 0);
    assert!(
        app.world_mut()
            .query::<&OwnedShip>()
            .iter(app.world())
            .next()
            .is_none()
    );
    assert!(
        !app.world()
            .resource::<NavigationState>()
            .navigation
            .systems
            .is_empty()
    );
}

#[test]
fn descriptor_identity_and_universe_wait_for_a_frame() {
    let mut app = app();
    let world = Id([2; 16]);
    receive(&mut app, Ok(descriptor(world)));
    app.update();
    complete_dependencies(&mut app);
    app.update();
    assert_eq!(app.world().resource::<Runs>().0, 0);
    assert!(!app.world().contains_resource::<CalendarClock>());
    receive(&mut app, Ok(NetEvent::Frame(Arc::new(frame(world)))));
    app.update();
    ready(&mut app);
}

#[test]
fn entering_running_observes_the_materialized_first_frame() {
    let mut app = app();
    app.add_systems(
        OnEnter(ClientPhase::Running),
        |session: Res<GameSession>, replication: Res<Replication>, contacts: Query<&Contact>| {
            assert_eq!(replication.applied, Some(session.key));
            assert_eq!(contacts.single().unwrap().0.id, 17);
        },
    );
    let world = Id([2; 16]);
    let mut first = frame(world);
    first.contacts.insert(
        Id([3; 16]),
        vec![SensorObservation {
            id: 17,
            entity: None,
            spatial_instance: Id([4; 16]),
            pose: Pose::default(),
            iff: None,
            radius_m: 1.0,
        }],
    );
    receive(&mut app, Ok(descriptor(world)));
    receive(&mut app, Ok(NetEvent::Frame(Arc::new(first))));
    app.update();
    complete_dependencies(&mut app);
    ready(&mut app);
    assert!(app.world().resource::<Runs>().0 > 0);
}

#[test]
fn world_replacement_disables_gameplay_and_discards_late_identity() {
    for running in [false, true] {
        let mut app = app();
        let old = Id([2; 16]);
        if running {
            start(&mut app, old);
        } else {
            receive(&mut app, Ok(descriptor(old)));
            app.update();
        }
        let old_key = app.world().resource::<Bootstrap>().key.unwrap();
        let stale = app.world_mut().spawn(WorldMember).id();
        app.world_mut()
            .resource_mut::<Outgoing>()
            .push(Action::ChatUnsubscribe);
        let before = app.world().resource::<Runs>().0;
        let next = Id([3; 16]);
        receive(&mut app, Ok(descriptor(next)));
        receive(&mut app, Ok(NetEvent::Frame(Arc::new(frame(old)))));
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, before);
        assert!(!app.world().contains_resource::<GameSession>());
        assert!(app.world().get_entity(stale).is_err());
        assert!(app.world().resource::<Outgoing>().pending().is_empty());
        assert!(app.world().resource::<Bootstrap>().sample.is_none());
        let new_key = app.world().resource::<Bootstrap>().key.unwrap();
        assert!(new_key.generation > old_key.generation);

        let (send, pending) = oneshot::channel();
        send.send(Ok(player())).unwrap();
        app.world_mut().resource_mut::<Bootstrap>().identity = Some((old_key, pending));
        app.update();
        assert!(app.world().resource::<Bootstrap>().player.is_none());
        complete_dependencies(&mut app);
        receive(&mut app, Ok(NetEvent::Frame(Arc::new(frame(next)))));
        app.update();
        assert_eq!(app.world().resource::<GameSession>().key, new_key);
        ready(&mut app);
    }
}

#[test]
fn disconnect_clears_actions_and_session_before_gameplay() {
    let mut app = app();
    start(&mut app, Id([2; 16]));
    let before = app.world().resource::<Runs>().0;
    app.world_mut()
        .resource_mut::<Outgoing>()
        .push(Action::ChatUnsubscribe);
    receive(&mut app, Err("Connection lost".into()));
    app.update();
    assert_eq!(
        *app.world().resource::<State<ClientPhase>>().get(),
        ClientPhase::Failed
    );
    let failure = app.world().resource::<SessionFailure>();
    assert_eq!(failure.message, "Connection lost");
    assert!(!failure.retryable);
    assert!(!app.world().contains_resource::<GameSession>());
    assert!(!app.world().contains_resource::<CalendarClock>());
    assert!(app.world().resource::<Outgoing>().pending().is_empty());
    assert_eq!(app.world().resource::<Runs>().0, before);
}

#[test]
fn required_identity_failure_stays_failed_until_retry() {
    let mut app = app();
    let world = Id([2; 16]);
    receive(&mut app, Ok(descriptor(world)));
    receive(&mut app, Ok(NetEvent::Frame(Arc::new(frame(world)))));
    app.update();
    let key = app.world().resource::<Bootstrap>().key.unwrap();
    let (send, pending) = oneshot::channel();
    send.send(Err("Identity denied".into())).unwrap();
    app.world_mut().resource_mut::<Bootstrap>().identity = Some((key, pending));
    app.update();
    app.update();
    assert_eq!(
        *app.world().resource::<State<ClientPhase>>().get(),
        ClientPhase::Failed
    );
    assert_eq!(
        app.world().resource::<SessionFailure>().message,
        "Identity denied"
    );
    assert!(app.world().resource::<SessionFailure>().retryable);
    complete_dependencies(&mut app);
    app.update();
    assert!(!app.world().contains_resource::<GameSession>());
    app.world_mut()
        .resource_mut::<NextState<ClientPhase>>()
        .set(ClientPhase::Loading);
    ready(&mut app);
    assert!(!app.world().contains_resource::<SessionFailure>());
}

#[test]
fn session_events_override_pending_activation() {
    for disconnect in [false, true] {
        let mut app = app();
        let world = Id([2; 16]);
        receive(&mut app, Ok(descriptor(world)));
        receive(&mut app, Ok(NetEvent::Frame(Arc::new(frame(world)))));
        app.update();
        complete_dependencies(&mut app);
        app.update();
        app.update();
        assert!(matches!(
            app.world().resource::<NextState<ClientPhase>>(),
            NextState::Pending(ClientPhase::Running)
        ));
        assert_eq!(app.world().resource::<Runs>().0, 0);
        app.world_mut()
            .resource_mut::<Outgoing>()
            .push(Action::ChatUnsubscribe);
        receive(
            &mut app,
            if disconnect {
                Err("Lost before activation".into())
            } else {
                Ok(descriptor(Id([3; 16])))
            },
        );
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 0);
        assert!(!app.world().contains_resource::<GameSession>());
        assert!(app.world().resource::<Outgoing>().pending().is_empty());
        assert_eq!(
            *app.world().resource::<State<ClientPhase>>().get(),
            if disconnect {
                ClientPhase::Failed
            } else {
                ClientPhase::Loading
            }
        );
    }
}
