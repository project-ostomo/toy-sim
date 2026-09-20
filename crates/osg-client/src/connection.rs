use anyhow::{Result, ensure};
use osg_model::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct AssetClient {
    requests: tokio::sync::mpsc::Sender<AssetRequest>,
    uploads: tokio::sync::mpsc::Sender<BlueprintUploadRequest>,
}

struct AssetRequest {
    hash: [u8; 32],
    response: tokio::sync::oneshot::Sender<Result<Vec<u8>>>,
}

struct BlueprintUploadRequest {
    bytes: Arc<[u8]>,
    response: tokio::sync::oneshot::Sender<Result<[u8; 32]>>,
}

impl AssetClient {
    pub async fn upload_blueprint(&self, bytes: Arc<[u8]>) -> Result<[u8; 32]> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= osg_ships::MAX_FILE,
            "blueprint must contain between 1 byte and 16 MiB"
        );
        let (response, received) = tokio::sync::oneshot::channel();
        self.uploads
            .send(BlueprintUploadRequest { bytes, response })
            .await
            .map_err(|_| anyhow::anyhow!("blueprint connection closed"))?;
        received
            .await
            .map_err(|_| anyhow::anyhow!("blueprint upload cancelled"))?
    }

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
    pub descriptor: tokio::sync::watch::Receiver<Option<(Id, UniverseDescriptor)>>,
    pub assets: AssetClient,
    pub input: tokio::sync::mpsc::Sender<InputFrame>,
    pub state: tokio::sync::mpsc::UnboundedReceiver<std::sync::Arc<Frame>>,
    pub status: tokio::sync::watch::Receiver<Option<String>>,
}

#[cfg(test)]
impl Endpoint {
    pub(crate) fn with_test_input(input: tokio::sync::mpsc::Sender<InputFrame>) -> Self {
        let (requests, _) = tokio::sync::mpsc::channel(1);
        let (uploads, _) = tokio::sync::mpsc::channel(1);
        let (_, state) = tokio::sync::mpsc::unbounded_channel();
        let (_, status) = tokio::sync::watch::channel(None);
        let (_, descriptor) = tokio::sync::watch::channel(None);
        Self {
            descriptor,
            assets: AssetClient { requests, uploads },
            input,
            state,
            status,
        }
    }
}

pub async fn connect(
    address: &str,
    key: ed25519_dalek::VerifyingKey,
    account: AccountId,
    secret: &ed25519_dalek::SigningKey,
) -> Result<Endpoint> {
    let mux = Arc::new(osg_net::connect(address, key, account, secret).await?);
    let local = tokio::task::spawn_blocking(crate::universe::shared_universe).await??;
    let stream = mux.open(b"main").await?;
    let (mut read, mut write) = tokio::io::split(stream);
    let (send, mut input) = tokio::sync::mpsc::channel(16);
    let (state, receive) = tokio::sync::mpsc::unbounded_channel();
    let (requests, requested) = tokio::sync::mpsc::channel(8);
    let (uploads, uploaded) = tokio::sync::mpsc::channel(8);
    let (status, connection_status) = tokio::sync::watch::channel(None);
    let (descriptor, session_descriptor) = tokio::sync::watch::channel(None);
    tokio::spawn(async move {
        let loading = load_assets(mux.clone(), requested);
        let uploading = upload_blueprints(mux.clone(), uploaded);
        let reader = async {
            let osg_protocol::Message::Session { world, universe } =
                osg_net::read_message(&mut read).await?
            else {
                anyhow::bail!("expected universe session descriptor");
            };
            ensure!(
                local.fingerprint == universe.fingerprint,
                "universe data does not match server"
            );
            ensure!(universe.epoch_mjd_utc.is_finite(), "invalid universe epoch");
            descriptor.send(Some((world, universe)))?;
            let mut session_world = world;
            let mut last_arrival = None;
            let mut summary_at = std::time::Instant::now();
            let mut received = 0_u64;
            let mut max_gap_ms = 0_f64;
            loop {
                let frame = match osg_net::read_message(&mut read).await? {
                    osg_protocol::Message::Session { world, universe } => {
                        ensure!(
                            local.fingerprint == universe.fingerprint,
                            "universe data does not match server"
                        );
                        ensure!(universe.epoch_mjd_utc.is_finite(), "invalid universe epoch");
                        descriptor.send(Some((world, universe)))?;
                        session_world = world;
                        last_arrival = None;
                        continue;
                    }
                    osg_protocol::Message::State(frame) => frame,
                    _ => anyhow::bail!("expected session descriptor or state"),
                };
                ensure!(
                    frame.world == session_world,
                    "state arrived before its universe descriptor"
                );
                let now = std::time::Instant::now();
                if let Some(previous) = last_arrival {
                    let gap_ms = now.duration_since(previous).as_secs_f64() * 1000.;
                    max_gap_ms = max_gap_ms.max(gap_ms);
                    if gap_ms >= 500. {
                        tracing::warn!(target: "osg_client::diagnostics", gap_ms,
                            tick = frame.tick, sequence = frame.sequence,
                            sim_time_ns = frame.sim_time_ns, "snapshot arrival gap (network task)");
                    }
                }
                last_arrival = Some(now);
                received += 1;
                if now.duration_since(summary_at).as_secs() >= 5 {
                    tracing::debug!(target: "osg_client::diagnostics", received, max_gap_ms,
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
                osg_net::write_message(&mut write, &osg_protocol::Message::Input(input)).await?;
            }
            Ok::<(), anyhow::Error>(())
        };
        let result = tokio::select! {
            result = reader => result.map_err(|error| error.context("receive game state")),
            result = writer => result.map_err(|error| error.context("send client input")),
            result = loading => result.map_err(|error| error.context("load asset")),
            result = uploading => result.map_err(|error| error.context("upload blueprint")),
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
        descriptor: session_descriptor,
        assets: AssetClient { requests, uploads },
        input: send,
        state: receive,
        status: connection_status,
    })
}

async fn load_assets(
    mux: Arc<osg_net::picomux::PicoMux>,
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

async fn fetch_asset(mux: &osg_net::picomux::PicoMux, hash: [u8; 32]) -> Result<Vec<u8>> {
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

async fn upload_blueprints(
    mux: Arc<osg_net::picomux::PicoMux>,
    mut requests: tokio::sync::mpsc::Receiver<BlueprintUploadRequest>,
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
                            let result = tokio::time::timeout(
                                std::time::Duration::from_secs(60),
                                upload_blueprint(&mux, &request.bytes),
                            ).await.map_err(anyhow::Error::from).and_then(|result| result);
                            let _ = request.response.send(result);
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

async fn upload_blueprint(mux: &osg_net::picomux::PicoMux, bytes: &[u8]) -> Result<[u8; 32]> {
    let hash = *blake3::hash(bytes).as_bytes();
    let mut stream = mux.open(b"blueprint-upload").await?;
    stream.write_all(&hash).await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;

    let mut ack = Vec::new();
    stream
        .take((industry::MAX_BLUEPRINT_UPLOAD_ACK_BYTES + 1) as u64)
        .read_to_end(&mut ack)
        .await?;
    match osg_protocol::decode_blueprint_upload_ack(&ack)? {
        industry::BlueprintUploadAck::Ready { hash: received } => {
            ensure!(received == hash, "blueprint acknowledgement hash mismatch");
            Ok(hash)
        }
        industry::BlueprintUploadAck::Rejected { reason } => anyhow::bail!(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn uploads_wait_for_eof_ack_and_progress_independently_of_stalled_transfers() {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            let client = Arc::new(osg_net::picomux::PicoMux::new(ar, aw));
            let server = osg_net::picomux::PicoMux::new(br, bw);
            let (requests, requested) = tokio::sync::mpsc::channel(8);
            let (uploads, uploaded) = tokio::sync::mpsc::channel(8);
            let assets = AssetClient { requests, uploads };
            let loading = tokio::spawn(load_assets(client.clone(), requested));
            let uploading = tokio::spawn(upload_blueprints(client.clone(), uploaded));

            let waiting_client = assets.clone();
            let mut waiting = tokio::spawn(async move {
                waiting_client
                    .upload_blueprint(Arc::from(&b"waiting"[..]))
                    .await
            });
            let mut held = server.accept().await.unwrap();
            assert_eq!(held.metadata(), b"blueprint-upload");
            let mut held_bytes = Vec::new();
            held.read_to_end(&mut held_bytes).await.unwrap();
            let held_hash = *blake3::hash(b"waiting").as_bytes();
            assert_eq!(&held_bytes[..32], &held_hash);
            assert_eq!(&held_bytes[32..], b"waiting");

            let ack =
                osg_protocol::encode_blueprint_upload_ack(&industry::BlueprintUploadAck::Ready {
                    hash: held_hash,
                })
                .unwrap();
            held.write_all(&ack).await.unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(10), &mut waiting)
                    .await
                    .is_err()
            );

            let ready_client = assets.clone();
            let ready_bytes: Arc<[u8]> = vec![19; 70 * 1024].into();
            let expected_hash = *blake3::hash(&ready_bytes).as_bytes();
            let ready =
                tokio::spawn(async move { ready_client.upload_blueprint(ready_bytes).await });
            let asset_client = assets.clone();
            let asset_hash = *blake3::hash(b"visible asset").as_bytes();
            let asset = tokio::spawn(async move { asset_client.fetch(asset_hash).await });

            for _ in 0..2 {
                let mut stream = server.accept().await.unwrap();
                let uploading = stream.metadata() == b"blueprint-upload";
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.unwrap();
                if uploading {
                    assert_eq!(&request[..32], &expected_hash);
                    assert_eq!(&request[32..], vec![19; 70 * 1024]);
                    let ack = osg_protocol::encode_blueprint_upload_ack(
                        &industry::BlueprintUploadAck::Ready {
                            hash: expected_hash,
                        },
                    )
                    .unwrap();
                    stream.write_all(&ack).await.unwrap();
                } else {
                    assert_eq!(stream.metadata(), b"assets");
                    assert_eq!(request, asset_hash);
                    stream.write_all(b"visible asset").await.unwrap();
                }
                stream.shutdown().await.unwrap();
            }
            assert_eq!(ready.await.unwrap().unwrap(), expected_hash);
            assert_eq!(asset.await.unwrap().unwrap(), b"visible asset");
            assert!(!waiting.is_finished());
            held.shutdown().await.unwrap();
            assert_eq!(waiting.await.unwrap().unwrap(), held_hash);
            drop(assets);
            uploading.await.unwrap().unwrap();
            loading.await.unwrap().unwrap();
            assert!(client.is_alive());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn invalid_upload_acknowledgements_fail_only_their_transfer() {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            let client = Arc::new(osg_net::picomux::PicoMux::new(ar, aw));
            let server = osg_net::picomux::PicoMux::new(br, bw);
            let hash = *blake3::hash(b"blueprint").as_bytes();
            let valid =
                osg_protocol::encode_blueprint_upload_ack(&industry::BlueprintUploadAck::Ready {
                    hash,
                })
                .unwrap();
            let wrong_hash =
                osg_protocol::encode_blueprint_upload_ack(&industry::BlueprintUploadAck::Ready {
                    hash: [99; 32],
                })
                .unwrap();
            let rejected = osg_protocol::encode_blueprint_upload_ack(
                &industry::BlueprintUploadAck::Rejected {
                    reason: "quota exceeded".into(),
                },
            )
            .unwrap();
            let mut trailing = valid.clone();
            trailing.push(0);
            let responses = [
                wrong_hash,
                rejected,
                Vec::new(),
                valid[..2].to_vec(),
                trailing,
                vec![0; industry::MAX_BLUEPRINT_UPLOAD_ACK_BYTES + 1],
                valid,
            ];
            for (index, response) in responses.into_iter().enumerate() {
                let caller = client.clone();
                let transfer =
                    tokio::spawn(async move { upload_blueprint(&caller, b"blueprint").await });
                let mut stream = server.accept().await.unwrap();
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.unwrap();
                assert_eq!(&request[..32], &hash);
                assert_eq!(&request[32..], b"blueprint");
                stream.write_all(&response).await.unwrap();
                stream.shutdown().await.unwrap();
                assert_eq!(transfer.await.unwrap().is_ok(), index == 6);
                assert!(client.is_alive());
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn assets_start_concurrently_and_report_failures_independently() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            let client = Arc::new(osg_net::picomux::PicoMux::new(ar, aw));
            let server = osg_net::picomux::PicoMux::new(br, bw);
            let (requests, requested) = tokio::sync::mpsc::channel(8);
            let loading = tokio::spawn(load_assets(client.clone(), requested));
            let (uploads, _) = tokio::sync::mpsc::channel(1);
            let assets = AssetClient { requests, uploads };
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
        use crate::assets::{self, NavigationDefinition};
        use bevy::prelude::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let definition = osg_model::InhabitedDirectory {
            systems: vec![Id([200; 16])],
            ..Default::default()
        };
        let bytes = osg_protocol::navigation::encode_directory(&definition).unwrap();
        let hash = *blake3::hash(&bytes).as_bytes();
        let other = osg_model::InhabitedDirectory {
            systems: vec![Id([201; 16])],
            ..Default::default()
        };
        let other_bytes = osg_protocol::navigation::encode_directory(&other).unwrap();
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
        let (uploads, _) = tokio::sync::mpsc::channel(1);
        assets::register_source(&mut app, AssetClient { requests, uploads });
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        assets::install(&mut app);
        let server = app.world().resource::<AssetServer>().clone();
        let first: Handle<NavigationDefinition> = server.load(assets::path(hash));
        let second: Handle<NavigationDefinition> = server.load(assets::path(hash));
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
                .resource::<Assets<NavigationDefinition>>()
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
                .resource::<Assets<NavigationDefinition>>()
                .contains(id)
        );
        drop(second);
        settle(&mut app, |world| {
            !world
                .resource::<Assets<NavigationDefinition>>()
                .contains(id)
        })
        .await;

        let failed: Handle<NavigationDefinition> = server.load(assets::path(other_hash));
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
                .resource::<Assets<NavigationDefinition>>()
                .get(&failed)
                .is_some()
        })
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(
            app.world()
                .resource::<Assets<NavigationDefinition>>()
                .get(&failed)
                .unwrap()
                .0
                .as_ref(),
            &other
        );
        drop(failed);
        drop(server);
        drop(app);
        worker.abort();
    }
}
