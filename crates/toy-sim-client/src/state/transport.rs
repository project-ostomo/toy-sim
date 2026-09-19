use super::*;

pub(super) fn receive(
    mut transport: ResMut<Transport>,
    mut playback: ResMut<BufferedPlayback>,
    mut outgoing: ResMut<Outgoing>,
    mut info: ResMut<SessionInfo>,
) {
    loop {
        match transport.endpoint.state.try_recv() {
            Ok(frame) => {
                if playback.0.world != Some(frame.world) {
                    outgoing.clear();
                }
                if let Err(error) = playback.0.receive(frame) {
                    info.status = error.to_string();
                }
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                info.status = transport
                    .endpoint
                    .status
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| "Disconnected".into());
                break;
            }
        }
    }
}

pub(super) fn send(
    mut transport: ResMut<Transport>,
    playback: Res<BufferedPlayback>,
    mut outgoing: ResMut<Outgoing>,
    mut info: ResMut<SessionInfo>,
) {
    let Some(world) = playback.0.world else {
        return;
    };
    transport.input_sequence += 1;
    let frame = InputFrame {
        world,
        sequence: transport.input_sequence,
        actions: outgoing.take(),
    };
    match transport.endpoint.input.try_send(frame) {
        Ok(()) => {
            if info.status == "Input queue busy" {
                info.status.clear();
            }
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(frame)) => {
            outgoing.restore(frame.actions);
            info.status = "Input queue busy".into();
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            outgoing.clear();
            info.status = transport
                .endpoint
                .status
                .borrow()
                .clone()
                .unwrap_or_else(|| "Disconnected".into());
        }
    }
}
