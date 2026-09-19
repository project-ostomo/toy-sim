use anyhow::{Result, ensure};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use toy_sim_model::*;

#[derive(Clone)]
pub struct AssetClient {
    requests: tokio::sync::mpsc::Sender<AssetRequest>,
}

struct AssetRequest {
    hash: [u8; 32],
    response: tokio::sync::oneshot::Sender<Result<Vec<u8>>>,
}

impl AssetClient {
    pub async fn fetch(&self, hash: [u8; 32]) -> Result<Vec<u8>> {
        let (response, received) = tokio::sync::oneshot::channel();
        self.requests
            .send(AssetRequest { hash, response })
            .await
            .map_err(|_| anyhow::anyhow!("asset connection closed"))?;
        received
            .await
            .map_err(|_| anyhow::anyhow!("asset transfer cancelled"))?
    }
}

pub struct Endpoint {
    pub assets: AssetClient,
    pub input: tokio::sync::mpsc::Sender<InputFrame>,
    pub state: tokio::sync::mpsc::UnboundedReceiver<std::sync::Arc<Frame>>,
    pub status: tokio::sync::watch::Receiver<Option<String>>,
}

pub async fn connect(
    address: &str,
    key: ed25519_dalek::VerifyingKey,
    account: AccountId,
    secret: &ed25519_dalek::SigningKey,
) -> Result<Endpoint> {
    let mux = Arc::new(toy_sim_net::connect(address, key, account, secret).await?);
    let stream = mux.open(b"main").await?;
    let (mut read, mut write) = tokio::io::split(stream);
    let (send, mut input) = tokio::sync::mpsc::channel(16);
    let (state, receive) = tokio::sync::mpsc::unbounded_channel();
    let (requests, requested) = tokio::sync::mpsc::channel(8);
    let (status, connection_status) = tokio::sync::watch::channel(None);
    tokio::spawn(async move {
        let loading = load_assets(mux.clone(), requested);
        let reader = async {
            let mut last_arrival = None;
            let mut summary_at = std::time::Instant::now();
            let mut received = 0_u64;
            let mut max_gap_ms = 0_f64;
            loop {
                let toy_sim_protocol::Message::State(frame) =
                    toy_sim_net::read_message(&mut read).await?
                else {
                    anyhow::bail!("expected state")
                };
                let now = std::time::Instant::now();
                if let Some(previous) = last_arrival {
                    let gap_ms = now.duration_since(previous).as_secs_f64() * 1000.;
                    max_gap_ms = max_gap_ms.max(gap_ms);
                    if gap_ms >= 500. {
                        tracing::warn!(target: "toy_sim_client::diagnostics", gap_ms,
                            tick = frame.tick, sequence = frame.sequence,
                            sim_time_ns = frame.sim_time_ns, "snapshot arrival gap (network task)");
                    }
                }
                last_arrival = Some(now);
                received += 1;
                if now.duration_since(summary_at).as_secs() >= 5 {
                    tracing::debug!(target: "toy_sim_client::diagnostics", received, max_gap_ms,
                        tick = frame.tick, sequence = frame.sequence,
                        sim_time_ns = frame.sim_time_ns, "snapshot reception (network task)");
                    summary_at = now;
                    received = 0;
                    max_gap_ms = 0.;
                }
                state.send(std::sync::Arc::new(frame))?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let writer = async {
            while let Some(input) = input.recv().await {
                toy_sim_net::write_message(&mut write, &toy_sim_protocol::Message::Input(input))
                    .await?;
            }
            Ok::<(), anyhow::Error>(())
        };
        let result = tokio::select! {
            result = reader => result.map_err(|error| error.context("receive game state")),
            result = writer => result.map_err(|error| error.context("send client input")),
            result = loading => result.map_err(|error| error.context("load asset")),
            result = mux.wait_until_dead() => result.map_err(|error| error.context("connection transport")),
        };
        let reason = match result {
            Ok(()) => "Connection closed".to_owned(),
            Err(error) => format!("Connection closed: {error:#}"),
        };
        eprintln!("{reason}");
        let _ = status.send(Some(reason));
    });
    Ok(Endpoint {
        assets: AssetClient { requests },
        input: send,
        state: receive,
        status: connection_status,
    })
}

async fn load_assets(
    mux: Arc<toy_sim_net::picomux::PicoMux>,
    mut requests: tokio::sync::mpsc::Receiver<AssetRequest>,
) -> Result<()> {
    let mut transfers = tokio::task::JoinSet::new();
    let mut accepting = true;
    loop {
        tokio::select! {
            request = requests.recv(), if accepting => {
                match request {
                    Some(request) => {
                        let mux = mux.clone();
                        transfers.spawn(async move {
                            let bytes = fetch_asset(&mux, request.hash).await;
                            let _ = request.response.send(bytes);
                        });
                    }
                    None => accepting = false,
                }
            }
            Some(result) = transfers.join_next(), if !transfers.is_empty() => {
                result?;
            }
            else => return Ok(()),
        }
    }
}

async fn fetch_asset(mux: &toy_sim_net::picomux::PicoMux, hash: [u8; 32]) -> Result<Vec<u8>> {
    let mut stream = mux.open(b"assets").await?;
    stream.write_all(&hash).await?;
    stream.shutdown().await?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    ensure!(
        *blake3::hash(&bytes).as_bytes() == hash,
        "asset unavailable, truncated, or hash mismatch"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[tokio::test]
    async fn assets_start_concurrently_and_report_failures_independently() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            let client = Arc::new(toy_sim_net::picomux::PicoMux::new(ar, aw));
            let server = toy_sim_net::picomux::PicoMux::new(br, bw);
            let (requests, requested) = tokio::sync::mpsc::channel(8);
            let loading = tokio::spawn(load_assets(client.clone(), requested));
            let assets = AssetClient { requests };
            let payloads: BTreeMap<_, _> = (0..128_u32)
                .map(|n| {
                    let bytes = n.to_le_bytes().repeat(n as usize * 64);
                    (*blake3::hash(&bytes).as_bytes(), bytes)
                })
                .collect();
            let bad = [255; 32];
            let mut hashes: Vec<_> = payloads.keys().copied().collect();
            hashes.push(bad);
            let total = hashes.len();
            let mut fetching = tokio::task::JoinSet::new();
            for hash in hashes {
                let assets = assets.clone();
                fetching.spawn(async move { (hash, assets.fetch(hash).await) });
            }
            drop(assets);
            let expected = payloads.clone();
            let responder = tokio::spawn(async move {
                let mut pending = Vec::new();
                for _ in 0..total {
                    let mut stream = server.accept().await.unwrap();
                    assert_eq!(stream.metadata(), b"assets");
                    let mut hash = [0; 32];
                    stream.read_exact(&mut hash).await.unwrap();
                    let mut extra = Vec::new();
                    stream.read_to_end(&mut extra).await.unwrap();
                    assert!(extra.is_empty());
                    pending.push((hash, stream));
                }
                let mut writers = tokio::task::JoinSet::new();
                for (hash, mut stream) in pending {
                    let bytes = payloads
                        .get(&hash)
                        .cloned()
                        .unwrap_or_else(|| b"corrupt".to_vec());
                    writers.spawn(async move {
                        stream.write_all(&bytes).await.unwrap();
                        stream.shutdown().await.unwrap();
                    });
                }
                while let Some(result) = writers.join_next().await {
                    result.unwrap();
                }
                server
            });
            let mut completed = BTreeMap::new();
            for _ in 0..total {
                let (hash, bytes) = fetching.join_next().await.unwrap().unwrap();
                assert!(completed.insert(hash, bytes).is_none());
            }
            assert!(completed.remove(&bad).unwrap().is_err());
            for (hash, bytes) in expected {
                assert_eq!(completed.remove(&hash).unwrap().unwrap(), bytes);
            }
            loading.await.unwrap().unwrap();
            let server = responder.await.unwrap();
            assert!(client.is_alive());
            assert!(server.is_alive());
        })
        .await
        .unwrap();
    }
    #[cfg(feature = "ui")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bevy_assets_share_downloads_retry_explicitly_and_release_after_last_owner() {
        use crate::assets::{self, SystemDefinition};
        use bevy::prelude::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use toy_sim_universe::replication::{BodyIdentity, SystemAsset};

        let config = toy_sim_universe::example_config();
        let definition = SystemAsset {
            version: 1,
            system_id: [200; 16],
            body_ids: config
                .bodies
                .iter()
                .enumerate()
                .map(|(index, body)| BodyIdentity {
                    name: body.name.to_string(),
                    id: [(index + 1) as u8; 16],
                })
                .collect(),
            config,
        };
        let bytes = definition.encode().unwrap();
        let hash = *blake3::hash(&bytes).as_bytes();
        let other = SystemAsset {
            system_id: [201; 16],
            ..definition
        };
        let other_bytes = other.encode().unwrap();
        let other_hash = *blake3::hash(&other_bytes).as_bytes();
        let (requests, mut requested) = tokio::sync::mpsc::channel::<AssetRequest>(8);
        let count = Arc::new(AtomicUsize::new(0));
        let requests_seen = count.clone();
        let worker = tokio::spawn(async move {
            let mut fail = true;
            while let Some(request) = requested.recv().await {
                requests_seen.fetch_add(1, Ordering::SeqCst);
                let result = if request.hash == hash {
                    Ok(bytes.clone())
                } else if fail {
                    fail = false;
                    Err(anyhow::anyhow!("temporary test failure"))
                } else {
                    assert_eq!(request.hash, other_hash);
                    Ok(other_bytes.clone())
                };
                let _ = request.response.send(result);
            }
        });
        let mut app = App::new();
        assets::register_source(&mut app, AssetClient { requests });
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        assets::install(&mut app);
        let server = app.world().resource::<AssetServer>().clone();
        let first: Handle<SystemDefinition> = server.load(assets::path(hash));
        let second: Handle<SystemDefinition> = server.load(assets::path(hash));
        assert_eq!(first.id(), second.id());

        async fn settle(app: &mut App, ready: impl Fn(&World) -> bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                app.update();
                if ready(app.world()) {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "asset pipeline did not settle"
                );
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        }
        settle(&mut app, |world| {
            world
                .resource::<Assets<SystemDefinition>>()
                .get(&first)
                .is_some()
        })
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let id = first.id();
        drop(first);
        for _ in 0..5 {
            app.update();
        }
        assert!(
            app.world()
                .resource::<Assets<SystemDefinition>>()
                .contains(id)
        );
        drop(second);
        settle(&mut app, |world| {
            !world.resource::<Assets<SystemDefinition>>().contains(id)
        })
        .await;

        let failed: Handle<SystemDefinition> = server.load(assets::path(other_hash));
        settle(&mut app, |_| {
            matches!(
                server.load_state(failed.id()),
                bevy::asset::LoadState::Failed(_)
            )
        })
        .await;
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
        server.reload(assets::path(other_hash));
        settle(&mut app, |world| {
            world
                .resource::<Assets<SystemDefinition>>()
                .get(&failed)
                .is_some()
        })
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(
            app.world()
                .resource::<Assets<SystemDefinition>>()
                .get(&failed)
                .unwrap()
                .system,
            Id(other.system_id)
        );
        drop(failed);
        drop(server);
        drop(app);
        worker.abort();
    }
}
