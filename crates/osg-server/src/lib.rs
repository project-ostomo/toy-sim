#[cfg(all(
    feature = "dynamic_linking",
    debug_assertions,
    not(target_family = "wasm")
))]
use bevy_dylib as _;

mod blueprint_uploads;
#[cfg(feature = "collision-prototype")]
pub mod collision_prototype;
pub mod launch;
pub mod persistence;
pub mod provision;
mod rpc;
mod sim;
use anyhow::{Result, ensure};
use bevy::prelude::*;
use blueprint_uploads::{BlueprintUploadBudget, BlueprintUploads};
use ed25519_dalek::{SigningKey, VerifyingKey};
use osg_model::*;
use osg_protocol::Message;
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

#[derive(Component)]
pub struct Connection {
    pub account: AccountId,
    pub uploads: BlueprintUploads,
    pub input: mpsc::Receiver<InputFrame>,
    pub state: SnapshotSender,
    rpc: mpsc::Receiver<rpc::Request>,
}

struct Endpoint {
    pub input: mpsc::Sender<InputFrame>,
    pub state: SnapshotReceiver,
    rpc: rpc::Handler,
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
    world: Id,
    universe: UniverseDescriptor,
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

    fn send(&self, frame: Frame, universe: UniverseDescriptor) -> Result<()> {
        ensure!(!self.is_closed(), "connection closed");
        let world = frame.world;
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
                world,
                universe,
                bytes,
                _permit: permit,
            })
            .map_err(|_| anyhow::anyhow!("connection closed"))
    }
}

fn channels(account: AccountId, uploads: BlueprintUploads) -> (Connection, Endpoint) {
    let (input, receive) = mpsc::channel(16);
    let (state, frames) = snapshot_queue(64 * 1024 * 1024);
    let (rpc, requests) = rpc::channel();
    (
        Connection {
            account,
            uploads,
            input: receive,
            state,
            rpc: requests,
        },
        Endpoint {
            input,
            state: frames,
            rpc,
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
            if let Ok(entity) = sim::session::connect(
                app.world_mut(),
                connection.account,
                connection.uploads.clone(),
            ) {
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
                for _ in 0..4 {
                    let Ok(request) = connection.rpc.try_recv() else {
                        break;
                    };
                    request(app.world_mut(), connection.account, &connection.uploads);
                }
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
                let entity = sim::session::connect(
                    app.world_mut(),
                    connection.account,
                    connection.uploads.clone(),
                )?;
                app.world_mut().entity_mut(entity).insert(connection);
            }
        }
        sim::apply_debug_requests(app.world_mut())?;
        let ticks = {
            tick_credit += app.world().resource::<sim::session::Clock>().rate;
            let ticks = tick_credit.floor() as u32;
            tick_credit -= ticks as f64;
            ticks
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
        let universe = UniverseDescriptor {
            fingerprint: app
                .world()
                .resource::<sim::registry::UniverseRegistry>()
                .universe
                .fingerprint(),
            epoch_mjd_utc: 0.,
            sim_time_origin_ns: 0,
        };
        sim::session::prepare_publication(app.world_mut());
        for entity in sessions {
            match sim::session::frame(app.world_mut(), entity) {
                Ok(frame) => {
                    let result = app
                        .world()
                        .get::<Connection>(entity)
                        .unwrap()
                        .state
                        .send(frame, universe.clone());
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
        next += osg_model::TICK_DURATION;
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
    let upload_budget = BlueprintUploadBudget::default();
    loop {
        let (stream, peer) = listener.accept().await?;
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            continue;
        };
        let accounts = accounts.clone();
        let key = key.clone();
        let connections = connections.clone();
        let assets = assets.clone();
        let upload_budget = upload_budget.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = async {
                let (account, mux) = osg_net::accept(stream, &key, &accounts).await?;
                let main = tokio::time::timeout(Duration::from_secs(10), mux.accept()).await??;
                ensure!(main.metadata() == b"main", "first stream must be main");
                let uploads = upload_budget.session(account);
                let (connection, endpoint) = channels(account, uploads.clone());
                let rpc = endpoint.rpc.clone();
                connections.send(connection).await?;
                tokio::select! {
                    result = main_stream(main, endpoint) => result,
                    result = asset_streams(&mux, assets, uploads, rpc) => result,
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
    osg_protocol::encode(&Message::State(frame))
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

async fn main_stream(stream: osg_net::picomux::Stream, mut endpoint: Endpoint) -> Result<()> {
    let (mut read, mut write) = tokio::io::split(stream);
    let incoming = async {
        loop {
            let Message::Input(frame) = osg_net::read_message(&mut read).await? else {
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
        let mut published_world = None;
        while let Some(frame) = endpoint.state.frames.recv().await {
            if published_world != Some(frame.world) {
                osg_net::write_message(
                    &mut write,
                    &Message::Session {
                        world: frame.world,
                        universe: frame.universe.clone(),
                    },
                )
                .await?;
                published_world = Some(frame.world);
            }
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
    mux: &osg_net::picomux::PicoMux,
    assets: AppearanceAssets,
    uploads: BlueprintUploads,
    rpc: rpc::Handler,
) -> Result<()> {
    let mut transfers = JoinSet::new();
    let permits = Arc::new(tokio::sync::Semaphore::new(osg_net::rpc::MAX_CALLS));
    loop {
        tokio::select! {
            stream = mux.accept() => {
                let mut stream = stream?;
                match stream.metadata() {
                    b"assets" => {
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
                    b"blueprint-upload" => {
                        let uploads = uploads.clone();
                        transfers.spawn(async move {
                            blueprint_uploads::serve(&mut stream, &uploads).await
                        });
                    }
                    label if label.starts_with(b"rpc:") => {
                        let Ok(permit) = permits.clone().try_acquire_owned() else {
                            eprintln!("RPC rejected: concurrent call limit reached");
                            continue;
                        };
                        let handler = osg_net::GameRpcDispatcher(rpc.clone());
                        transfers.spawn(async move {
                            let _permit = permit;
                            tokio::time::timeout(osg_net::rpc::DEADLINE, handler.dispatch(stream)).await??;
                            Ok(())
                        });
                    }
                    _ => eprintln!("Unknown auxiliary stream label; closing stream"),
                }
            }
            Some(result) = transfers.join_next(), if !transfers.is_empty() => {
                if let Err(error) = result.map_err(anyhow::Error::from).and_then(|result| result) {
                    eprintln!("Auxiliary stream failed: {error:#}");
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

    fn universe() -> UniverseDescriptor {
        UniverseDescriptor {
            fingerprint: [7; 32],
            epoch_mjd_utc: 0.,
            sim_time_origin_ns: 0,
        }
    }

    fn empty_snapshot() -> Frame {
        Frame {
            chat: None,
            optical: Vec::new(),
            calendar_unix_ms: 0,
            world: Id::new(),
            sequence: 1,
            tick: 1,
            sim_time_ns: osg_model::TICK_NS,
            rate: 1.0,
            views: Vec::new(),
            contacts: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
            presentation: PresentationFrame::default(),
        }
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
        sender.send(first.clone(), universe()).unwrap();
        sender.send(second.clone(), universe()).unwrap();
        assert_eq!(sender.budget.available_permits(), 0);

        let queued = receiver.frames.recv().await.unwrap();
        assert_eq!(queued.world, first.world);
        assert_eq!(queued.universe, universe());
        assert_eq!(
            osg_protocol::decode(&queued.bytes).unwrap(),
            Message::State(first)
        );
        assert_eq!(sender.budget.available_permits(), 0);
        drop(queued);
        assert_eq!(sender.budget.available_permits(), frame_bytes);

        let queued = receiver.frames.recv().await.unwrap();
        assert_eq!(
            osg_protocol::decode(&queued.bytes).unwrap(),
            Message::State(second)
        );
        drop(queued);
        assert_eq!(sender.budget.available_permits(), frame_bytes * 2);
    }

    #[tokio::test]
    async fn snapshot_queue_overflow_disconnects_instead_of_replacing_frames() {
        let frame = empty_snapshot();
        let (sender, mut receiver) = snapshot_queue(encode_snapshot(frame.clone()).len());
        sender.send(frame.clone(), universe()).unwrap();
        assert!(
            sender
                .send(frame.clone(), universe())
                .unwrap_err()
                .to_string()
                .contains("client too slow")
        );
        receiver.failed.changed().await.unwrap();
        assert!(*receiver.failed.borrow());
        assert!(sender.is_closed());
        let queued = receiver.frames.recv().await.unwrap();
        assert_eq!(
            osg_protocol::decode(&queued.bytes).unwrap(),
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
            let client = osg_net::picomux::PicoMux::new(ar, aw);
            let server = osg_net::picomux::PicoMux::new(br, bw);
            let client_stream = client.open(b"main").await.unwrap();
            let server_stream = server.accept().await.unwrap();
            let (mut connection, endpoint) = channels(Id::new(), BlueprintUploads::default());
            let serving = tokio::spawn(main_stream(server_stream, endpoint));
            let (mut read, mut write) = tokio::io::split(client_stream);
            let snapshot = empty_snapshot();
            let world = snapshot.world;
            let writer = tokio::spawn(async move {
                for sequence in 1..=64 {
                    osg_net::write_message(
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
            connection.state.send(snapshot.clone(), universe()).unwrap();
            assert_eq!(
                osg_net::read_message(&mut read).await.unwrap(),
                Message::Session {
                    world,
                    universe: universe()
                }
            );
            assert_eq!(
                osg_net::read_message(&mut read).await.unwrap(),
                Message::State(snapshot.clone())
            );
            assert!(!serving.is_finished());

            let mut next = snapshot;
            next.sequence += 1;
            connection.state.send(next.clone(), universe()).unwrap();
            assert_eq!(
                osg_net::read_message(&mut read).await.unwrap(),
                Message::State(next.clone())
            );

            next.world = Id::new();
            connection.state.send(next.clone(), universe()).unwrap();
            assert_eq!(
                osg_net::read_message(&mut read).await.unwrap(),
                Message::Session {
                    world: next.world,
                    universe: universe(),
                }
            );
            assert_eq!(
                osg_net::read_message(&mut read).await.unwrap(),
                Message::State(next)
            );

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
            let client = Arc::new(osg_net::picomux::PicoMux::new(ar, aw));
            let server = osg_net::picomux::PicoMux::new(br, bw);
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
            let uploads = BlueprintUploads::default();
            let server_uploads = uploads.clone();
            let serving = tokio::spawn(async move {
                asset_streams(&server, server_assets, server_uploads, rpc::channel().0).await
            });
            let mut stalled = client.open(b"assets").await.unwrap();
            stalled.write_all(&[0]).await.unwrap();
            let mut stalled_upload = client.open(b"blueprint-upload").await.unwrap();
            stalled_upload.write_all(&[0; 33]).await.unwrap();
            let upload_client = client.clone();
            let upload = tokio::spawn(async move {
                let bytes = vec![49; 70 * 1024];
                let hash = *blake3::hash(&bytes).as_bytes();
                let mut stream = upload_client.open(b"blueprint-upload").await.unwrap();
                stream.write_all(&hash).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
                stream.shutdown().await.unwrap();
                let mut ack = Vec::new();
                stream.read_to_end(&mut ack).await.unwrap();
                assert!(matches!(
                    osg_protocol::decode_blueprint_upload_ack(&ack).unwrap(),
                    industry::BlueprintUploadAck::Ready { hash: received } if received == hash
                ));
                (hash, bytes)
            });
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
            let (hash, bytes) = upload.await.unwrap();
            assert_eq!(
                uploads.get(hash).unwrap().as_ref().as_ref(),
                bytes.as_slice()
            );
            assert!(assets.get(&hash).is_none());
            drop(stalled);
            drop(stalled_upload);
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
