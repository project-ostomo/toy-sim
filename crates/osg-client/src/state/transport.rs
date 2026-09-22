use super::*;

pub(super) fn receive(
    real_time: Res<Time<Real>>,
    mut calendar: ResMut<CalendarClock>,
    mut transport: ResMut<Transport>,
    mut playback: ResMut<BufferedPlayback>,
    mut outgoing: ResMut<Outgoing>,
    mut info: ResMut<SessionInfo>,
) {
    loop {
        match transport.endpoint.state.try_recv() {
            Ok(frame) => {
                info.universe_descriptor = transport
                    .endpoint
                    .descriptor
                    .borrow()
                    .as_ref()
                    .filter(|(world, _)| *world == frame.world)
                    .map(|(_, descriptor)| descriptor.clone());
                if playback.0.world != Some(frame.world) {
                    outgoing.clear();
                }
                let calendar_unix_ms = frame.calendar_unix_ms;
                playback.0.receive(frame);
                calendar.observe(calendar_unix_ms, real_time.elapsed());
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
    let actions = outgoing.take();
    if actions.is_empty() {
        return;
    }
    transport.input_sequence += 1;
    let frame = InputFrame {
        world,
        sequence: transport.input_sequence,
        actions,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn idle_flushes_send_nothing_and_busy_input_preserves_actions() {
        let (input, mut received) = tokio::sync::mpsc::channel(1);
        let world_id = Id::new();
        let mut playback = Playback::new(true);
        playback.world = Some(world_id);
        let mut world = World::new();
        world.insert_resource(Transport {
            endpoint: Endpoint::with_test_input(input),
            input_sequence: 0,
        });
        world.insert_resource(BufferedPlayback(playback));
        world.init_resource::<Outgoing>();
        world.init_resource::<SessionInfo>();
        for _ in 0..100 {
            world.run_system_once(send).unwrap();
        }
        assert!(received.try_recv().is_err());
        assert_eq!(world.resource::<Transport>().input_sequence, 0);

        let ship = Id::new();
        let action = |command| Action::Ship {
            ship,
            authority_revision: 1,
            command,
        };
        let first = world
            .resource_mut::<Outgoing>()
            .push(action(ShipCommand::StartFiring));
        world.run_system_once(send).unwrap();
        let second = world
            .resource_mut::<Outgoing>()
            .push(action(ShipCommand::StopFiring));
        world.run_system_once(send).unwrap();
        assert_eq!(world.resource::<Outgoing>().pending()[0].0, second);
        assert_eq!(world.resource::<SessionInfo>().status, "Input queue busy");
        let first_frame = received.try_recv().unwrap();
        assert_eq!(first_frame.world, world_id);
        assert_eq!(first_frame.actions[0].0, first);
        world.run_system_once(send).unwrap();
        let second_frame = received.try_recv().unwrap();
        assert_eq!(second_frame.actions[0].0, second);
        assert!(second_frame.sequence > first_frame.sequence);
        assert!(world.resource::<Outgoing>().pending().is_empty());
        assert!(world.resource::<SessionInfo>().status.is_empty());
        world.run_system_once(send).unwrap();
        assert!(received.try_recv().is_err());
    }
}
