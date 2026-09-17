use ed25519_dalek::SigningKey;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use toy_sim_model::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_main_stream_carries_authorized_snapshots_and_results() {
    let account = Id::new();
    let other_account = Id::new();
    let other_key = SigningKey::from_bytes(&[4; 32]);
    let server_key = SigningKey::from_bytes(&[1; 32]);
    let account_key = SigningKey::from_bytes(&[2; 32]);
    let mut server = ServerProcess::start(
        &server_key,
        &[(account, &account_key), (other_account, &other_key)],
        None,
    );
    let address = server.ready().await;
    assert!(
        toy_sim_client::connect(
            &address,
            SigningKey::from_bytes(&[9; 32]).verifying_key(),
            account,
            &account_key,
        )
        .await
        .is_err()
    );
    assert!(
        toy_sim_client::connect(&address, server_key.verifying_key(), account, &other_key,)
            .await
            .is_err()
    );
    let mut client =
        toy_sim_client::connect(&address, server_key.verifying_key(), account, &account_key)
            .await
            .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), client.state.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.ships.len(), 1);
    let group = *first.tracks.keys().next().unwrap();
    let action = Id::new();
    client
        .input
        .send(InputFrame {
            world: first.world,
            sequence: 1,
            acknowledged_event: first.event_watermark,
            acknowledged_frame: first.sequence,
            actions: vec![
                (
                    Id::new(),
                    Action::InstrumentSubscribe {
                        ship: first.ships[0].ship,
                    },
                ),
                (
                    Id::new(),
                    Action::ScreenSubscribe {
                        ship: first.ships[0].ship,
                        slot: 0,
                        hz: 10,
                    },
                ),
                (
                    action,
                    Action::Subscribe(ViewSubscription {
                        id: 1,
                        revision: 1,
                        group,
                        focused_ship: Some(first.ships[0].ship),
                        query: TrackQuery {
                            limit: 64,
                            work: 100_000,
                            ..Default::default()
                        },
                    }),
                ),
            ],
        })
        .await
        .unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = client.state.recv().await.unwrap();
            if frame.results.iter().any(|result| result.id == action)
                && frame.screens.iter().any(|screen| {
                    screen
                        .frame
                        .as_ref()
                        .is_some_and(|frame| frame.draws.len() >= 5)
                })
            {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        observed
            .results
            .iter()
            .find(|result| result.id == action)
            .unwrap()
            .error
            .is_none()
    );
    assert_eq!(observed.views.len(), 1);
    assert_eq!(observed.screens.len(), 1);
    assert!(observed.screens[0].frame.as_ref().unwrap().draws.len() >= 5);
    assert!(!observed.tracks[&group].is_empty());
    let appearance = observed.tracks[&group]
        .iter()
        .find_map(|track| track.appearance)
        .unwrap();
    let asset = tokio::time::timeout(Duration::from_secs(5), client.assets.fetch(appearance))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*blake3::hash(&asset).as_bytes(), appearance);
    assert!(!asset.is_empty());
    let mut other = toy_sim_client::connect(
        &address,
        server_key.verifying_key(),
        other_account,
        &other_key,
    )
    .await
    .unwrap();
    let other_first = tokio::time::timeout(Duration::from_secs(5), other.state.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other_first.ships.len(), 1);
    assert_ne!(other_first.ships[0].ship, first.ships[0].ship);
    let join = Id::new();
    let view = Id::new();
    let forbidden = Id::new();
    other
        .input
        .send(InputFrame {
            world: first.world,
            sequence: 1,
            acknowledged_event: other_first.event_watermark,
            acknowledged_frame: other_first.sequence,
            actions: vec![
                (join, Action::JoinGroup(first.ships[0].info_group)),
                (
                    view,
                    Action::Subscribe(ViewSubscription {
                        id: 1,
                        revision: 1,
                        group,
                        focused_ship: None,
                        query: TrackQuery {
                            limit: 64,
                            work: 100_000,
                            ..Default::default()
                        },
                    }),
                ),
                (
                    forbidden,
                    Action::Ship {
                        ship: first.ships[0].ship,
                        authority_revision: first.ships[0].authority_revision,
                        command: ShipCommand::Manual {
                            throttle: 1.,
                            steering: [0.; 3],
                        },
                    },
                ),
            ],
        })
        .await
        .unwrap();
    let shared = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = other.state.recv().await.unwrap();
            if frame.results.iter().any(|result| result.id == forbidden) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        shared
            .results
            .iter()
            .find(|result| result.id == join)
            .unwrap()
            .error
            .is_none()
    );
    assert!(
        shared
            .results
            .iter()
            .find(|result| result.id == view)
            .unwrap()
            .error
            .is_none()
    );
    assert!(
        shared
            .results
            .iter()
            .find(|result| result.id == forbidden)
            .unwrap()
            .error
            .is_some()
    );
    assert!(
        shared.tracks[&group]
            .iter()
            .any(|track| track.entity == Some(first.ships[0].ship))
    );
    assert_eq!(shared.ships.len(), 1);
    drop(other);
    let start_tick = observed.tick;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let frame = client.state.recv().await.unwrap_or_else(|| {
                panic!(
                    "sustained subscription closed: {:?}",
                    client.status.borrow()
                )
            });
            client
                .input
                .send(InputFrame {
                    world: frame.world,
                    sequence: frame.sequence + 2,
                    acknowledged_event: frame.event_watermark,
                    acknowledged_frame: frame.sequence,
                    actions: Vec::new(),
                })
                .await
                .unwrap();
            if frame.tick >= start_tick + 100 {
                break;
            }
        }
    })
    .await
    .expect("sustained subscription stalled");
    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_discards_old_world_inputs_and_keeps_the_connection_usable() {
    let account = Id::new();
    let server_key = SigningKey::from_bytes(&[5; 32]);
    let account_key = SigningKey::from_bytes(&[6; 32]);
    let mut server = ServerProcess::start(&server_key, &[(account, &account_key)], Some(account));
    let address = server.ready().await;
    let mut client =
        toy_sim_client::connect(&address, server_key.verifying_key(), account, &account_key)
            .await
            .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let first = client.state.recv().await.unwrap();
        client
            .input
            .send(InputFrame {
                world: first.world,
                sequence: 1,
                acknowledged_event: first.event_watermark,
                acknowledged_frame: first.sequence,
                actions: vec![(Id::new(), Action::Debug(DebugCommand::Reset))],
            })
            .await
            .unwrap();
        let reset = loop {
            let frame = client.state.recv().await.unwrap();
            if frame.world != first.world {
                break frame;
            }
        };
        assert_ne!(reset.ships[0].ship, first.ships[0].ship);
        assert!(reset.views.is_empty());

        let stale_action = Id::new();
        client
            .input
            .send(InputFrame {
                world: first.world,
                sequence: 2,
                acknowledged_event: first.event_watermark,
                acknowledged_frame: first.sequence,
                actions: vec![(stale_action, Action::Debug(DebugCommand::SetRate(0.)))],
            })
            .await
            .unwrap();
        let subscribe = Id::new();
        client
            .input
            .send(InputFrame {
                world: reset.world,
                sequence: 3,
                acknowledged_event: reset.event_watermark,
                acknowledged_frame: reset.sequence,
                actions: vec![(
                    subscribe,
                    Action::Subscribe(ViewSubscription {
                        id: 1,
                        revision: 1,
                        group: *reset
                            .tracks
                            .keys()
                            .find(|group| **group != PUBLIC_GROUP)
                            .unwrap(),
                        focused_ship: Some(reset.ships[0].ship),
                        query: TrackQuery {
                            limit: 64,
                            work: 100_000,
                            ..Default::default()
                        },
                    }),
                )],
            })
            .await
            .unwrap();
        loop {
            let frame = client
                .state
                .recv()
                .await
                .expect("reset closed the main stream");
            assert_eq!(frame.world, reset.world);
            assert_eq!(frame.rate, 1.);
            assert!(!frame.results.iter().any(|result| result.id == stale_action));
            if let Some(result) = frame.results.iter().find(|result| result.id == subscribe) {
                assert!(result.error.is_none(), "{result:?}");
                assert_eq!(frame.views.len(), 1);
                break;
            }
        }
    })
    .await
    .expect("reset did not resume the session");
    drop(client);
    server.shutdown().await;
}

struct ServerProcess {
    child: Child,
    directory: PathBuf,
}

impl ServerProcess {
    fn start(
        server_key: &SigningKey,
        accounts: &[(Id, &SigningKey)],
        debug_account: Option<Id>,
    ) -> Self {
        let directory = std::env::temp_dir().join(format!("toy-sim-network-test-{}", Id::new()));
        std::fs::create_dir(&directory).unwrap();
        let mut config = format!(
            "listen = \"127.0.0.1:0\"\nserver_secret = \"{}\"\n",
            hex(&server_key.to_bytes())
        );
        if let Some(account) = debug_account {
            config.push_str(&format!("debug_account = \"{account}\"\n"));
        }
        for (id, key) in accounts {
            config.push_str(&format!(
                "\n[[accounts]]\nid = \"{id}\"\npublic_key = \"{}\"\n",
                hex(&key.verifying_key().to_bytes())
            ));
        }
        let path = directory.join("server.toml");
        std::fs::write(&path, config).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_toy-sim-server"))
            .arg(path)
            .arg("--ready-file")
            .arg(directory.join("ready"))
            .arg("--shutdown-on-stdin-close")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        Self { child, directory }
    }

    async fn ready(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "server exited before readiness"
            );
            if let Ok(address) = std::fs::read_to_string(self.directory.join("ready")) {
                if address.parse::<std::net::SocketAddr>().is_ok() {
                    return address;
                }
            }
            assert!(Instant::now() < deadline, "server readiness timed out");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn shutdown(&mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "server shutdown failed: {status}");
                assert!(!self.directory.join("ready").exists());
                return;
            }
            assert!(
                Instant::now() < deadline,
                "server did not shut down after stdin EOF"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
