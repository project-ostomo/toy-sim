use super::*;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};

#[derive(Resource, Default, Clone, Copy)]
pub struct ClientDiagnostics {
    pub buffered_ms: f64,
    pub queued_frames: usize,
    pub catching_up: bool,
    pub buffering: bool,
    pub underruns: u64,
    pub fps: f64,
}

#[derive(Default)]
pub struct LogState {
    summary_at: f64,
    stall_at: f64,
    underrun_at: f64,
    underruns: u64,
    catching_up: bool,
    max_frame_ms: f64,
}

pub fn update(
    time: Res<Time<Real>>,
    store: Option<Res<DiagnosticsStore>>,
    playback: Res<BufferedPlayback>,
    session: Res<SessionInfo>,
    transport: Res<Transport>,
    mut metrics: ResMut<ClientDiagnostics>,
    mut log: Local<LogState>,
) {
    let playback = &playback.0;
    metrics.buffered_ms = playback.buffered_ns() as f64 * 1e-6;
    metrics.queued_frames = playback.queued_frames();
    metrics.catching_up = playback.catching_up();
    metrics.buffering = playback.buffering();
    metrics.underruns = playback.underruns;
    metrics.fps = store
        .as_ref()
        .and_then(|store| store.get(&FrameTimeDiagnosticsPlugin::FPS))
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or(0.);

    let now = time.elapsed_secs_f64();
    let frame_ms = time.delta_secs_f64() * 1000.;
    log.max_frame_ms = log.max_frame_ms.max(frame_ms);
    if frame_ms >= 250. && now - log.stall_at >= 1. {
        warn!(target: "osg_client::diagnostics", frame_ms, tick = session.tick,
            sequence = session.sequence, buffered_ms = metrics.buffered_ms,
            queued_frames = metrics.queued_frames,
            inbox_frames = transport.events.len(), "client display frame stalled");
        log.stall_at = now;
    }
    if metrics.underruns > log.underruns && now - log.underrun_at >= 1. {
        warn!(target: "osg_client::diagnostics", tick = session.tick,
            sequence = session.sequence, underruns = metrics.underruns,
            target_frames = playback.target_frames, "client jitter buffer underrun");
        log.underrun_at = now;
        log.underruns = metrics.underruns;
    }
    if metrics.catching_up != log.catching_up {
        info!(target: "osg_client::diagnostics", catching_up = metrics.catching_up,
            queued_frames = metrics.queued_frames, buffered_ms = metrics.buffered_ms,
            tick = session.tick, "client catch-up changed");
    }
    log.catching_up = metrics.catching_up;
    if metrics.underruns < log.underruns {
        log.underruns = metrics.underruns;
    }
    if now - log.summary_at >= 5. {
        debug!(target: "osg_client::diagnostics", fps = metrics.fps,
            max_frame_ms = log.max_frame_ms, buffered_ms = metrics.buffered_ms,
            queued_frames = metrics.queued_frames, inbox_frames = transport.events.len(),
            underruns = metrics.underruns, catching_up = metrics.catching_up,
            buffering = metrics.buffering,
            tick = session.tick, sequence = session.sequence, "client performance");
        log.summary_at = now;
        log.max_frame_ms = 0.;
    }
}
