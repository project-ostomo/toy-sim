pub mod launch;
pub mod provision;
mod sim;
use anyhow::{Result, ensure};
use bevy::prelude::*;
use ed25519_dalek::{SigningKey, VerifyingKey};
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
    pub state: watch::Sender<Option<Arc<Frame>>>,
}

struct Endpoint {
    pub input: mpsc::Sender<InputFrame>,
    pub state: watch::Receiver<Option<Arc<Frame>>>,
}

fn channels(account: AccountId) -> (Connection, Endpoint) {
    let (input, receive) = mpsc::channel(16);
    let (state, frames) = watch::channel(None);
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
            app = scenario(&config.accounts, config.debug_account, config.ship)?;
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
            app.world_mut()
                .resource_mut::<sim::simulation::TickMetrics>()
                .last_duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        sim::displays::update(app.world_mut());
        let sessions = app
            .world_mut()
            .query_filtered::<Entity, With<Connection>>()
            .iter(app.world())
            .collect::<Vec<_>>();
        for entity in sessions {
            match sim::session::frame(app.world_mut(), entity) {
                Ok(frame) => {
                    app.world()
                        .get::<Connection>(entity)
                        .unwrap()
                        .state
                        .send_replace(Some(Arc::new(frame)));
                }
                Err(error) => {
                    eprintln!("Session publication failed: {error:#}");
                    sim::session::disconnect(app.world_mut(), entity);
                }
            }
        }
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
    assets: Arc<BTreeMap<[u8; 32], Vec<u8>>>,
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

async fn main_stream(stream: toy_sim_net::picomux::Stream, mut endpoint: Endpoint) -> Result<()> {
    let (mut read, mut write) = tokio::io::split(stream);
    let incoming = async {
        loop {
            let Message::Input(frame) = toy_sim_net::read_message(&mut read).await? else {
                anyhow::bail!("expected input frame")
            };
            endpoint
                .input
                .try_send(frame)
                .map_err(|_| anyhow::anyhow!("input rate exceeded"))?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let outgoing = async {
        loop {
            endpoint.state.changed().await?;
            let frame = endpoint.state.borrow_and_update().clone();
            if let Some(frame) = frame {
                tokio::time::timeout(
                    Duration::from_secs(10),
                    toy_sim_net::write_message(&mut write, &Message::State((*frame).clone())),
                )
                .await??;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! { result = incoming => result, result = outgoing => result }
}

async fn asset_streams(
    mux: &toy_sim_net::picomux::PicoMux,
    assets: Arc<BTreeMap<[u8; 32], Vec<u8>>>,
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
                        stream.write_all(bytes).await?;
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

pub fn assets(app: &App) -> Arc<BTreeMap<[u8; 32], Vec<u8>>> {
    Arc::new(
        app.world()
            .resource::<sim::identity::AppearanceAssets>()
            .0
            .iter()
            .map(|(hash, bytes)| (*hash, bytes.clone()))
            .collect(),
    )
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
            let assets = Arc::new(payloads);
            let server_assets = assets.clone();
            let serving = tokio::spawn(async move { asset_streams(&server, server_assets).await });
            let mut stalled = client.open(b"assets").await.unwrap();
            stalled.write_all(&[0]).await.unwrap();
            let mut transfers = JoinSet::new();
            let hashes: Vec<_> = assets.keys().copied().chain([[255; 32]]).collect();
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
                    assert_eq!(&bytes, &assets[&hash]);
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
