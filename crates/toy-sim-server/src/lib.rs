pub mod launch;
pub mod persistence;
pub mod provision;
mod sim;
use anyhow::{Result, ensure};
use bevy::prelude::*;
use ed25519_dalek::{SigningKey, VerifyingKey};
pub use sim::identity::AppearanceAssets;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, watch},
    task::JoinSet,
};
use toy_sim_model::*;
use toy_sim_protocol::Message;

#[derive(Component)]
pub struct Connection {
    pub account: AccountId,
    pub input: mpsc::Receiver<InputFrame>,
    pub state: SnapshotSender,
}

struct Endpoint {
    pub input: mpsc::Sender<InputFrame>,
    pub state: SnapshotReceiver,
}

pub struct SnapshotSender {
    frames: mpsc::UnboundedSender<QueuedSnapshot>,
    budget: Arc<tokio::sync::Semaphore>,
    failed: watch::Sender<bool>,
}

struct SnapshotReceiver {
    frames: mpsc::UnboundedReceiver<QueuedSnapshot>,
    failed: watch::Receiver<bool>,
}

struct QueuedSnapshot {
    bytes: Vec<u8>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

fn snapshot_queue(byte_limit: usize) -> (SnapshotSender, SnapshotReceiver) {
    let (frames, receiver) = mpsc::unbounded_channel();
    let (failed, failure) = watch::channel(false);
    (
        SnapshotSender {
            frames,
            budget: Arc::new(tokio::sync::Semaphore::new(byte_limit)),
            failed,
        },
        SnapshotReceiver {
            frames: receiver,
            failed: failure,
        },
    )
}

impl SnapshotSender {
    fn is_closed(&self) -> bool {
        self.frames.is_closed() || *self.failed.borrow()
    }

    fn send(&self, frame: Frame) -> Result<()> {
        ensure!(!self.is_closed(), "connection closed");
        let bytes = encode_snapshot(frame);
        let permit = self
            .budget
            .clone()
            .try_acquire_many_owned(bytes.len().try_into()?);
        let Ok(permit) = permit else {
            self.failed.send_replace(true);
            anyhow::bail!("client too slow: outbound snapshot buffer full");
        };
        self.frames
            .send(QueuedSnapshot {
                bytes,
                _permit: permit,
            })
            .map_err(|_| anyhow::anyhow!("connection closed"))
    }
}

fn channels(account: AccountId) -> (Connection, Endpoint) {
    let (input, receive) = mpsc::channel(16);
    let (state, frames) = snapshot_queue(64 * 1024 * 1024);
    (
        Connection {
            account,
            input: receive,
            state,
        },
        Endpoint {
            input,
            state: frames,
        },
    )
}

pub fn run(
    mut app: App,
    incoming: mpsc::Receiver<Connection>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let result = run_loop(&mut app, incoming, stop);
    let saved = persistence::shutdown(app.world_mut());
    if let Err(error) = &saved {
        error!(%error, "Final world checkpoint failed");
    }
    result.and(saved)
}

fn run_loop(
    app: &mut App,
    mut incoming: mpsc::Receiver<Connection>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut next = Instant::now();
    let mut tick_credit = 0.0_f64;
    while !stop.load(Ordering::Acquire) {
        while let Ok(connection) = incoming.try_recv() {
            let count = app
                .world_mut()
                .query::<&Connection>()
                .iter(app.world())
                .count();
            if count >= 1024 {
                continue;
            }
            if let Ok(entity) = sim::session::connect(app.world_mut(), connection.account) {
                app.world_mut().entity_mut(entity).insert(connection);
            }
        }

        let sessions = app
            .world_mut()
            .query_filtered::<Entity, With<Connection>>()
            .iter(app.world())
            .collect::<Vec<_>>();
        for entity in sessions {
            let mut connection = app
                .world_mut()
                .entity_mut(entity)
                .take::<Connection>()
                .unwrap();
            let mut connected = !connection.state.is_closed();
            for _ in 0..4 {
                match connection.input.try_recv() {
                    Ok(frame) => {
                        if sim::session::input(app.world_mut(), entity, frame).is_err() {
                            connected = false;
                            break;
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        connected = false;
                        break;
                    }
                }
            }
            if connected {
                app.world_mut().entity_mut(entity).insert(connection);
            } else {
                sim::session::disconnect(app.world_mut(), entity);
            }
        }

        if app
            .world()
            .resource::<sim::session::Clock>()
            .reset_requested
        {
            let config = app.world().resource::<sim::ScenarioConfig>().clone();
            let sessions = app
                .world_mut()
                .query_filtered::<Entity, With<Connection>>()
                .iter(app.world())
                .collect::<Vec<_>>();
            let connections = sessions
                .into_iter()
                .filter_map(|entity| app.world_mut().entity_mut(entity).take::<Connection>())
                .collect::<Vec<_>>();
            let checkpoints = app
                .world_mut()
                .remove_resource::<persistence::Checkpoints>();
            let assets = assets(app);
            *app = scenario(&config.accounts, config.debug_account, config.ship)?;
            assets.extend(app.world().resource::<AppearanceAssets>().snapshot());
            app.insert_resource(assets);
            if let Some(checkpoints) = checkpoints {
                app.insert_resource(checkpoints);
                persistence::request(app.world());
            }
            tick_credit = 0.0;
            for connection in connections {
                let entity = sim::session::connect(app.world_mut(), connection.account)?;
                app.world_mut().entity_mut(entity).insert(connection);
            }
        }
        sim::apply_debug_requests(app.world_mut())?;
        let ticks = {
            let mut clock = app.world_mut().resource_mut::<sim::session::Clock>();
            if clock.rate == 0.0 {
                tick_credit = 0.0;
                let ticks = clock.steps.min(1);
                clock.steps -= ticks;
                ticks
            } else {
                clock.steps = 0;
                tick_credit += clock.rate;
                let ticks = tick_credit.floor() as u32;
                tick_credit -= ticks as f64;
                ticks
            }
        };
        for _ in 0..ticks {
            let started = Instant::now();
            app.update();
            let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
            app.world_mut()
                .resource_mut::<sim::simulation::TickMetrics>()
                .last_duration_ms = duration_ms;
            sim::diagnostics::tick(app.world_mut(), duration_ms);
        }
        let publication_started = Instant::now();
        sim::infrastructure::publish_navigation(app.world_mut());
        sim::displays::update(app.world_mut());
        let display_ms = publication_started.elapsed().as_secs_f64() * 1000.0;
        let sessions = app
            .world_mut()
            .query_filtered::<Entity, With<Connection>>()
            .iter(app.world())
            .collect::<Vec<_>>();
        let session_count = sessions.len();
        for entity in sessions {
            match sim::session::frame(app.world_mut(), entity) {
                Ok(frame) => {
                    let result = app
                        .world()
                        .get::<Connection>(entity)
                        .unwrap()
                        .state
                        .send(frame);
                    if let Err(error) = result {
                        eprintln!("Session publication failed: {error:#}");
                        sim::session::disconnect(app.world_mut(), entity);
                    }
                }
                Err(error) => {
                    eprintln!("Session publication failed: {error:#}");
                    sim::session::disconnect(app.world_mut(), entity);
                }
            }
        }
        sim::session::prune_events(app.world_mut());
        sim::diagnostics::publication(
            app.world()
                .resource::<sim::simulation::SimulationCounters>()
                .ticks,
            publication_started.elapsed().as_secs_f64() * 1000.0,
            display_ms,
            session_count,
        );
        persistence::poll(app.world_mut())?;
        if incoming.is_closed()
            && app
                .world_mut()
                .query::<&Connection>()
                .iter(app.world())
                .next()
                .is_none()
        {
            break;
        }
        next += Duration::from_millis(100);
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else {
            next = Instant::now();
        }
    }
    Ok(())
}

pub async fn listen(
    listener: TcpListener,
    key: SigningKey,
    accounts: BTreeMap<AccountId, VerifyingKey>,
    connections: mpsc::Sender<Connection>,
    assets: AppearanceAssets,
) -> Result<()> {
    let accounts = Arc::new(accounts);
    let key = Arc::new(key);
    let permits = Arc::new(tokio::sync::Semaphore::new(1024));
    loop {
        let (stream, peer) = listener.accept().await?;
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            continue;
        };
        let accounts = accounts.clone();
        let key = key.clone();
        let connections = connections.clone();
        let assets = assets.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = async {
                let (account, mux) = toy_sim_net::accept(stream, &key, &accounts).await?;
                let main = tokio::time::timeout(Duration::from_secs(10), mux.accept()).await??;
                ensure!(main.metadata() == b"main", "first stream must be main");
                let (connection, endpoint) = channels(account);
                connections.send(connection).await?;
                tokio::select! {
                    result = main_stream(main, endpoint) => result,
                    result = asset_streams(&mux, assets) => result,
                    result = mux.wait_until_dead() => result,
                }
            }
            .await as Result<()>;
            if let Err(error) = result {
                eprintln!("Connection {peer} closed: {error:#}");
            }
        });
    }
}

fn encode_snapshot(frame: Frame) -> Vec<u8> {
    toy_sim_protocol::encode(&Message::State(frame))
        .expect("internal server bug: invalid outgoing snapshot")
}

async fn send_snapshot<W: tokio::io::AsyncWrite + Unpin>(
    write: &mut W,
    bytes: &[u8],
) -> Result<()> {
    write.write_all(bytes).await?;
    write.flush().await?;
    Ok(())
}

async fn main_stream(stream: toy_sim_net::picomux::Stream, mut endpoint: Endpoint) -> Result<()> {
    let (mut read, mut write) = tokio::io::split(stream);
    let incoming = async {
        loop {
            let Message::Input(frame) = toy_sim_net::read_message(&mut read).await? else {
                anyhow::bail!("expected input frame")
            };
            endpoint
                .input
                .send(frame)
                .await
                .map_err(|_| anyhow::anyhow!("session input closed"))?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let outgoing = async {
        while let Some(frame) = endpoint.state.frames.recv().await {
            send_snapshot(&mut write, &frame.bytes).await?;
        }
        anyhow::bail!("session closed")
    };
    let failed = async {
        if !*endpoint.state.failed.borrow() {
            let _ = endpoint.state.failed.changed().await;
        }
        anyhow::bail!("session closed or outbound snapshot buffer full")
    };
    tokio::select! { result = incoming => result, result = outgoing => result, result = failed => result }
}

async fn asset_streams(
    mux: &toy_sim_net::picomux::PicoMux,
    assets: AppearanceAssets,
) -> Result<()> {
    let mut transfers = JoinSet::new();
    loop {
        tokio::select! {
            stream = mux.accept() => {
                let mut stream = stream?;
                ensure!(stream.metadata() == b"assets", "unknown stream label");
                let assets = assets.clone();
                transfers.spawn(async move {
                    let mut hash = [0; 32];
                    stream.read_exact(&mut hash).await?;
                    if let Some(bytes) = assets.get(&hash) {
                        stream.write_all(&bytes).await?;
                    }
                    stream.shutdown().await?;
                    Ok::<(), anyhow::Error>(())
                });
            }
            Some(result) = transfers.join_next(), if !transfers.is_empty() => {
                if let Err(error) = result.map_err(anyhow::Error::from).and_then(|result| result) {
                    eprintln!("Asset transfer failed: {error:#}");
                }
            }
        }
    }
}

pub fn scenario(
    accounts: &[AccountId],
    debug_account: Option<AccountId>,
    ship: Option<std::path::PathBuf>,
) -> Result<App> {
    sim::provision(accounts, debug_account, ship)
}

pub fn assets(app: &App) -> AppearanceAssets {
    app.world().resource::<AppearanceAssets>().clone()
}

pub fn key_bytes(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64 && value.is_ascii(),
        "key must be 64 hexadecimal characters"
    );
    let mut bytes = [0; 32];
    for (byte, chunk) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    fn empty_snapshot() -> Frame {
        Frame {
            optical: Vec::new(),
            calendar_unix_ms: 0,
            society: Default::default(),
            world: Id::new(),
            sequence: 1,
            tick: 1,
            sim_time_ns: 100_000_000,
            rate: 1.0,
            views: Vec::new(),
            tracks: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
            presentation: PresentationFrame::default(),
        }
    }

    #[tokio::test]
    #[should_panic(expected = "internal server bug: invalid outgoing snapshot")]
    async fn invalid_outgoing_snapshot_panics() {
        let mut frame = empty_snapshot();
        frame.rate = f64::NAN;
        encode_snapshot(frame);
    }

    #[tokio::test]
    async fn disconnected_client_remains_an_io_error() {
        let (mut server, client) = tokio::io::duplex(4096);
        drop(client);
        assert!(
            send_snapshot(&mut server, &encode_snapshot(empty_snapshot()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn snapshot_queue_preserves_order_and_accounts_for_inflight_bytes() {
        let first = empty_snapshot();
        let mut second = first.clone();
        second.sequence = 2;
        let frame_bytes = encode_snapshot(first.clone()).len();
        let (sender, mut receiver) = snapshot_queue(frame_bytes * 2);
        sender.send(first.clone()).unwrap();
        sender.send(second.clone()).unwrap();
        assert_eq!(sender.budget.available_permits(), 0);

        let queued = receiver.frames.recv().await.unwrap();
        assert_eq!(
            toy_sim_protocol::decode(&queued.bytes).unwrap(),
            Message::State(first)
        );
        assert_eq!(sender.budget.available_permits(), 0);
        drop(queued);
        assert_eq!(sender.budget.available_permits(), frame_bytes);

        let queued = receiver.frames.recv().await.unwrap();
        assert_eq!(
            toy_sim_protocol::decode(&queued.bytes).unwrap(),
            Message::State(second)
        );
        drop(queued);
        assert_eq!(sender.budget.available_permits(), frame_bytes * 2);
    }

    #[tokio::test]
    async fn snapshot_queue_overflow_disconnects_instead_of_replacing_frames() {
        let frame = empty_snapshot();
        let (sender, mut receiver) = snapshot_queue(encode_snapshot(frame.clone()).len());
        sender.send(frame.clone()).unwrap();
        assert!(
            sender
                .send(frame.clone())
                .unwrap_err()
                .to_string()
                .contains("client too slow")
        );
        receiver.failed.changed().await.unwrap();
        assert!(*receiver.failed.borrow());
        assert!(sender.is_closed());
        let queued = receiver.frames.recv().await.unwrap();
        assert_eq!(
            toy_sim_protocol::decode(&queued.bytes).unwrap(),
            Message::State(frame)
        );
        assert!(receiver.frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn saturated_input_queue_preserves_order_and_keeps_snapshots_flowing() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            let client = toy_sim_net::picomux::PicoMux::new(ar, aw);
            let server = toy_sim_net::picomux::PicoMux::new(br, bw);
            let client_stream = client.open(b"main").await.unwrap();
            let server_stream = server.accept().await.unwrap();
            let (mut connection, endpoint) = channels(Id::new());
            let serving = tokio::spawn(main_stream(server_stream, endpoint));
            let (mut read, mut write) = tokio::io::split(client_stream);
            let snapshot = empty_snapshot();
            let world = snapshot.world;
            let writer = tokio::spawn(async move {
                for sequence in 1..=64 {
                    toy_sim_net::write_message(
                        &mut write,
                        &Message::Input(InputFrame {
                            world,
                            sequence,
                            actions: Vec::new(),
                        }),
                    )
                    .await
                    .unwrap();
                }
                write
            });
            while connection.input.len() < 16 {
                assert!(
                    !serving.is_finished(),
                    "full queue disconnected the session"
                );
                tokio::task::yield_now().await;
            }
            assert_eq!(connection.input.len(), 16);
            connection.state.send(snapshot.clone()).unwrap();
            assert_eq!(
                toy_sim_net::read_message(&mut read).await.unwrap(),
                Message::State(snapshot)
            );
            assert!(!serving.is_finished());

            for sequence in 1..=64 {
                let frame = connection.input.recv().await.unwrap();
                assert_eq!(frame.world, world);
                assert_eq!(frame.sequence, sequence);
            }
            let write = writer.await.unwrap();
            assert!(connection.input.try_recv().is_err());
            assert!(!serving.is_finished());
            serving.abort();
            drop(write);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn assets_stream_to_eof_concurrently_despite_a_stalled_request() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            let client = Arc::new(toy_sim_net::picomux::PicoMux::new(ar, aw));
            let server = toy_sim_net::picomux::PicoMux::new(br, bw);
            let mut payloads = BTreeMap::new();
            for n in 0..128_u32 {
                let bytes = if n == 127 {
                    vec![127; 17 * 1024 * 1024 + 3]
                } else {
                    n.to_le_bytes().repeat(n as usize * 32)
                };
                payloads.insert(*blake3::hash(&bytes).as_bytes(), bytes);
            }
            let assets = AppearanceAssets::default();
            let server_assets = assets.clone();
            let serving = tokio::spawn(async move { asset_streams(&server, server_assets).await });
            let mut stalled = client.open(b"assets").await.unwrap();
            stalled.write_all(&[0]).await.unwrap();
            assets.extend(payloads.clone());
            let mut transfers = JoinSet::new();
            let hashes: Vec<_> = payloads.keys().copied().chain([[255; 32]]).collect();
            for hash in hashes {
                let client = client.clone();
                transfers.spawn(async move {
                    let mut stream = client.open(b"assets").await.unwrap();
                    stream.write_all(&hash).await.unwrap();
                    stream.shutdown().await.unwrap();
                    let mut bytes = Vec::new();
                    stream.read_to_end(&mut bytes).await.unwrap();
                    (hash, bytes)
                });
            }
            let mut completed = 0;
            while let Some(result) = transfers.join_next().await {
                let (hash, bytes) = result.unwrap();
                if hash == [255; 32] {
                    assert!(bytes.is_empty());
                } else {
                    assert_eq!(&bytes, &payloads[&hash]);
                }
                completed += 1;
            }
            assert_eq!(completed, 129);
            drop(stalled);
            let mut final_stream = client.open(b"assets").await.unwrap();
            let hash = *blake3::hash(b"").as_bytes();
            final_stream.write_all(&hash).await.unwrap();
            final_stream.shutdown().await.unwrap();
            let mut bytes = Vec::new();
            final_stream.read_to_end(&mut bytes).await.unwrap();
            assert!(bytes.is_empty());
            assert!(client.is_alive());
            serving.abort();
        })
        .await
        .unwrap();
    }
}
