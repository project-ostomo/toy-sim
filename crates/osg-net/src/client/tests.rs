use super::*;
use tokio::io::AsyncWriteExt;

type ApplicationResult = Result<(), String>;

#[osg_net_macros::rpc]
trait Probe {
    async fn probe_delay(&self, value: u64) -> u64;
    async fn probe_unit(&self) -> ();
    async fn probe_result(&self, fail: bool) -> ApplicationResult;
}

struct ProbeHandler;

impl Probe for ProbeHandler {
    async fn probe_delay(&self, value: u64) -> u64 {
        tokio::time::sleep(std::time::Duration::from_millis(value)).await;
        value
    }

    async fn probe_unit(&self) {}

    async fn probe_result(&self, fail: bool) -> ApplicationResult {
        if fail {
            Err("application rejected request".into())
        } else {
            Ok(())
        }
    }
}

async fn pair() -> (OsgNetClient, Arc<PicoMux>, crate::picomux::Stream) {
    let (a, b) = tokio::io::duplex(65536);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    let server = Arc::new(PicoMux::new(br, bw));
    let client = OsgNetClient::from_mux(PicoMux::new(ar, aw)).await.unwrap();
    let main = server.accept().await.unwrap();
    assert_eq!(main.metadata(), b"main");
    (client, server, main)
}

fn serve(server: Arc<PicoMux>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                stream = server.accept() => {
                    let Ok(stream) = stream else { break };
                    tasks.spawn(async move {
                        let _ = ProbeDispatcher(ProbeHandler).dispatch(stream).await;
                    });
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
    })
}

fn descriptor(world: Id) -> osg_protocol::Message {
    osg_protocol::Message::Session {
        world,
        universe: UniverseDescriptor {
            fingerprint: [0; 32],
            epoch_mjd_utc: 0.,
            sim_time_origin_ns: 0,
        },
    }
}

#[tokio::test]
async fn uploads_wait_for_acknowledgement_and_downloads_verify_content() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, server, _main) = pair().await;
        let bytes: Arc<[u8]> = Arc::from(&b"blueprint payload"[..]);
        let hash = *blake3::hash(&bytes).as_bytes();
        let uploader = client.clone();
        let upload = tokio::spawn(async move { uploader.upload_blueprint(bytes).await });
        let mut stream = server.accept().await.unwrap();
        assert_eq!(stream.metadata(), b"blueprint-upload");
        let mut received = Vec::new();
        stream.read_to_end(&mut received).await.unwrap();
        assert_eq!(&received[..32], &hash);
        assert_eq!(&received[32..], b"blueprint payload");
        assert!(!upload.is_finished());

        let caller = client.clone();
        let call = tokio::spawn(async move { caller.probe_unit().await });
        ProbeDispatcher(ProbeHandler)
            .dispatch(server.accept().await.unwrap())
            .await
            .unwrap();
        call.await.unwrap().unwrap();
        assert!(!upload.is_finished());

        let ack = osg_protocol::encode_blueprint_upload_ack(
            &osg_model::industry::BlueprintUploadAck::Ready { hash },
        )
        .unwrap();
        stream.write_all(&ack).await.unwrap();
        stream.shutdown().await.unwrap();
        assert_eq!(upload.await.unwrap().unwrap(), hash);

        let downloader = client.clone();
        let download = tokio::spawn(async move { downloader.fetch_asset(hash).await });
        let mut stream = server.accept().await.unwrap();
        assert_eq!(stream.metadata(), b"assets");
        let mut requested = Vec::new();
        stream.read_to_end(&mut requested).await.unwrap();
        assert_eq!(requested, hash);
        stream.write_all(b"corrupt content").await.unwrap();
        stream.shutdown().await.unwrap();
        assert!(matches!(
            download.await.unwrap(),
            Err(RpcError::Protocol(_))
        ));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn generated_calls_preserve_values_errors_and_complete_independently() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, server, _main) = pair().await;
        let task = serve(server);
        let delayed = client.clone();
        let slow = tokio::spawn(async move { delayed.probe_delay(300).await });
        assert_eq!(client.probe_delay(1).await.unwrap(), 1);
        assert!(!slow.is_finished());
        client.probe_unit().await.unwrap();
        assert_eq!(client.probe_result(false).await.unwrap(), Ok(()));
        assert_eq!(
            client.probe_result(true).await.unwrap(),
            Err("application rejected request".into())
        );
        assert_eq!(slow.await.unwrap().unwrap(), 300);
        task.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn malformed_and_unknown_calls_do_not_break_sibling_streams() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, server, _main) = pair().await;
        let task = serve(server);
        assert!(client.rpc_call("missing", Vec::new()).await.is_err());
        assert!(client.rpc_call("probe_unit", vec![1]).await.is_err());

        let mut bad = client.0.mux.open(b"rpc:probe_unit").await.unwrap();
        bad.write_u32((rpc::MAX_REQUEST + 1) as u32).await.unwrap();
        bad.shutdown().await.unwrap();
        assert!(rpc::read(&mut bad, rpc::MAX_RESPONSE).await.is_err());

        let mut trailing = client.0.mux.open(b"rpc:probe_unit").await.unwrap();
        trailing.write_all(&[0, 0, 0, 0, 42]).await.unwrap();
        trailing.shutdown().await.unwrap();
        assert!(rpc::read(&mut trailing, rpc::MAX_RESPONSE).await.is_err());
        client.probe_unit().await.unwrap();
        task.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn subscriptions_bootstrap_report_lag_and_keep_other_listeners_running() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, _server, mut main) = pair().await;
        let mut fast = client.subscribe_events();
        crate::write_message(&mut main, &descriptor(Id([1; 16]))).await.unwrap();
        assert!(matches!(fast.recv().await.unwrap(), NetEvent::Session { world, .. } if world == Id([1; 16])));
        let mut slow = client.subscribe_events();
        assert!(matches!(slow.recv().await.unwrap(), NetEvent::Session { world, .. } if world == Id([1; 16])));
        for value in 2..=80 {
            crate::write_message(&mut main, &descriptor(Id([value; 16]))).await.unwrap();
            assert!(matches!(fast.recv().await.unwrap(), NetEvent::Session { world, .. } if world == Id([value; 16])));
        }
        assert!(matches!(slow.recv().await, Err(broadcast::error::RecvError::Lagged(_))));
        drop(slow);
        crate::write_message(&mut main, &descriptor(Id([81; 16]))).await.unwrap();
        assert!(fast.recv().await.is_ok());
        client.close();
        assert!(matches!(fast.recv().await, Err(broadcast::error::RecvError::Closed)));
    }).await.unwrap();
}

#[tokio::test]
async fn callers_outside_tokio_can_await_and_close_cancels_pending_calls() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, server, _main) = pair().await;
        let server = serve(server);
        let external = client.clone();
        tokio::task::spawn_blocking(move || {
            futures_lite::future::block_on(external.probe_unit()).unwrap();
        })
        .await
        .unwrap();
        let pending = client.clone();
        let pending = tokio::spawn(async move { pending.probe_delay(30_000).await });
        tokio::task::yield_now().await;
        client.close();
        assert!(matches!(pending.await.unwrap(), Err(RpcError::Closed)));
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn clones_share_input_sequence_and_old_world_batches_are_rejected() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, _server, mut main) = pair().await;
        let mut events = client.subscribe_events();
        let world = Id([1; 16]);
        crate::write_message(&mut main, &descriptor(world))
            .await
            .unwrap();
        events.recv().await.unwrap();
        let clone = client.clone();
        for sender in [&client, &clone] {
            sender
                .send_inputs(world, &[(Id::new(), Action::Unsubscribe(1))])
                .await
                .unwrap();
        }
        let osg_protocol::Message::Input(first) = crate::read_message(&mut main).await.unwrap()
        else {
            panic!()
        };
        let osg_protocol::Message::Input(second) = crate::read_message(&mut main).await.unwrap()
        else {
            panic!()
        };
        assert!(first.sequence < second.sequence);
        crate::write_message(&mut main, &descriptor(Id([2; 16])))
            .await
            .unwrap();
        events.recv().await.unwrap();
        let actions = vec![(Id::new(), Action::Unsubscribe(1))];
        let error = client.try_send_inputs(world, actions.clone()).unwrap_err();
        assert_eq!(error.actions, actions);
    })
    .await
    .unwrap();
}
