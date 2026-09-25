use super::*;
use crate::NetEvent;
use tokio::sync::broadcast::error::TryRecvError;

#[derive(Resource, Default)]
pub struct SessionEvents(pub std::collections::VecDeque<Result<NetEvent, String>>);

pub fn receive_network_events(mut transport: ResMut<Transport>, mut events: ResMut<SessionEvents>) {
    if transport.failed {
        return;
    }
    loop {
        match transport.events.try_recv() {
            Ok(event) => events.0.push_back(Ok(event)),
            Err(TryRecvError::Empty) => break,
            Err(error) => {
                transport.failed = true;
                let message = match error {
                    TryRecvError::Lagged(count) => {
                        format!("Missed {count} main events; reconnect to continue")
                    }
                    _ => transport
                        .client
                        .status()
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| "Disconnected".into()),
                };
                events.0.push_back(Err(message));
                transport.client.close();
                break;
            }
        }
    }
}

pub fn receive_session_events(
    mut commands: Commands,
    mut next: ResMut<NextState<ClientPhase>>,
    session: Option<Res<GameSession>>,
    mut bootstrap: ResMut<Bootstrap>,
    real_time: Res<Time<Real>>,
    mut calendar: Option<ResMut<CalendarClock>>,
    mut events: ResMut<SessionEvents>,
    mut playback: ResMut<BufferedPlayback>,
    mut outgoing: ResMut<Outgoing>,
) {
    while let Some(event) = events.0.pop_front() {
        match event {
            Ok(NetEvent::Session { world, universe }) => {
                if bootstrap.key.is_none_or(|key| key.world != world)
                    || bootstrap
                        .descriptor
                        .as_ref()
                        .is_some_and(|descriptor| descriptor != &universe)
                {
                    outgoing.clear();
                    playback.0.stop();
                    bootstrap.begin(world);
                    next.set(ClientPhase::Loading);
                }
                bootstrap.descriptor = Some(universe);
            }
            Ok(NetEvent::Frame(frame)) => {
                if bootstrap.key.is_none() {
                    bootstrap.begin(frame.world);
                    next.set(ClientPhase::Loading);
                }
                if bootstrap.key.is_some_and(|key| key.world != frame.world) {
                    continue;
                }
                bootstrap.sample = Some((frame.calendar_unix_ms, real_time.elapsed()));
                if session
                    .as_ref()
                    .is_some_and(|session| Some(session.key) == bootstrap.key)
                {
                    if let Some(calendar) = &mut calendar {
                        calendar.observe(frame.calendar_unix_ms, real_time.elapsed());
                    }
                }
                playback.0.receive(frame);
            }
            Err(message) => {
                playback.0.stop();
                outgoing.clear();
                commands.insert_resource(SessionFailure {
                    message,
                    retryable: false,
                });
                next.set(ClientPhase::Failed);
                bootstrap.disconnect();
                events.0.clear();
                break;
            }
        }
    }
}

pub fn send(
    transport: Res<Transport>,
    session: Res<GameSession>,
    mut outgoing: ResMut<Outgoing>,
    mut info: ResMut<SessionInfo>,
) {
    let world = session.key.world;
    let actions = outgoing.take();
    if actions.is_empty() {
        return;
    }
    match transport.client.try_send_inputs(world, actions) {
        Ok(()) => {
            if info.status == "input queue busy" {
                info.status.clear();
            }
        }
        Err(error) => {
            if error.reason == "input queue busy" {
                outgoing.restore(error.actions);
            }
            info.status = error.reason.into();
        }
    }
}
