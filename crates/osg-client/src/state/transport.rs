use super::*;
use crate::NetEvent;
use tokio::sync::broadcast::error::TryRecvError;

pub(super) fn receive(
    real_time: Res<Time<Real>>,
    mut calendar: ResMut<CalendarClock>,
    mut transport: ResMut<Transport>,
    mut playback: ResMut<BufferedPlayback>,
    mut outgoing: ResMut<Outgoing>,
    mut info: ResMut<SessionInfo>,
    mut navigation: ResMut<NavigationState>,
) {
    if transport.failed {
        return;
    }
    loop {
        match transport.events.try_recv() {
            Ok(NetEvent::Session { world, universe }) => {
                if playback.0.world != Some(world) {
                    outgoing.clear();
                }
                navigation.universe_descriptor = Some(universe);
            }
            Ok(NetEvent::Frame(frame)) => {
                if playback.0.world != Some(frame.world) {
                    outgoing.clear();
                }
                calendar.observe(frame.calendar_unix_ms, real_time.elapsed());
                playback.0.receive(frame);
            }
            Err(TryRecvError::Empty) => break,
            Err(error) => {
                transport.failed = true;
                playback.0.stop();
                outgoing.clear();
                info.status = match error {
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
                transport.client.close();
                break;
            }
        }
    }
}

pub(super) fn send(
    transport: Res<Transport>,
    playback: Res<BufferedPlayback>,
    mut outgoing: ResMut<Outgoing>,
    mut info: ResMut<SessionInfo>,
) {
    if transport.failed {
        return;
    }
    let Some(world) = playback.0.world else {
        return;
    };
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
