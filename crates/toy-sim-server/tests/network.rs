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
    let (observed, result) = tokio::time::timeout(Duration::from_secs(15), async {
        let mut result = None;
        loop {
            let frame = client.state.recv().await.unwrap();
            if let Some(received) = frame.results.iter().find(|result| result.id == action) {
                assert!(result.is_none(), "command result delivered more than once");
                result = Some(received.clone());
            }
            if result.is_some()
                && frame.screens.iter().any(|screen| {
                    screen
                        .frame
                        .as_ref()
                        .is_some_and(|frame| frame.draws.len() >= 5)
                })
            {
                break (frame, result.unwrap());
            }
        }
    })
    .await
    .unwrap();
    assert!(result.error.is_none());
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
                        command: ShipCommand::StartFiring,
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
            assert!(frame.results.iter().all(|result| result.id != action));
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

async fn submit_action(
    client: &mut toy_sim_client::Endpoint,
    world: Id,
    sequence: u64,
    action: Action,
) -> std::sync::Arc<Frame> {
    let id = Id::new();
    client
        .input
        .send(InputFrame {
            world,
            sequence,
            actions: vec![(id, action)],
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = client.state.recv().await.unwrap();
            if frame.results.iter().any(|result| result.id == id) {
                return frame;
            }
        }
    })
    .await
    .expect("action response timed out")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn society_changes_cross_the_protocol_and_preserve_permission_boundaries() {
    use ownership::{AccessGrant, AccessPolicy, Permission, Principal, SocietyCommand, Standing};
    use std::collections::BTreeSet;

    let alice = Id([11; 16]);
    let bob = Id([12; 16]);
    let alice_key = SigningKey::from_bytes(&[21; 32]);
    let bob_key = SigningKey::from_bytes(&[22; 32]);
    let server_key = SigningKey::from_bytes(&[23; 32]);
    let mut server =
        ServerProcess::start(&server_key, &[(alice, &alice_key), (bob, &bob_key)], None);
    let address = server.ready().await;
    let mut a = toy_sim_client::connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = toy_sim_client::connect(&address, server_key.verifying_key(), bob, &bob_key)
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), a.state.recv())
        .await
        .unwrap()
        .unwrap();
    let other = tokio::time::timeout(Duration::from_secs(10), b.state.recv())
        .await
        .unwrap()
        .unwrap();
    let world = first.world;
    let ship = first.ships[0].ship;
    assert_eq!(first.society.account, alice);
    assert_eq!(other.society.account, bob);
    assert!(
        !other
            .society
            .assets
            .iter()
            .any(|asset| asset.entity == ship)
    );

    let created = submit_action(
        &mut a,
        world,
        1,
        Action::Society(SocietyCommand::CreateOrganization {
            name: "Network Test Cooperative".into(),
        }),
    )
    .await;
    assert!(
        created.results.iter().all(|result| result.error.is_none()),
        "{:?}",
        created.results
    );
    let organization = created.society.directory.players[&alice]
        .organization
        .unwrap();
    assert!(
        created.society.directory.organizations[&organization]
            .officers
            .contains(&alice)
    );

    let marked = submit_action(
        &mut a,
        world,
        2,
        Action::Society(SocietyCommand::SetStanding {
            target: Principal::Player(bob),
            standing: Some(Standing::Hostile),
        }),
    )
    .await;
    assert_eq!(
        marked
            .society
            .directory
            .standing(Principal::Player(alice), Principal::Player(bob)),
        Standing::Hostile
    );
    let other_view = submit_action(
        &mut b,
        world,
        1,
        Action::Society(SocietyCommand::SetStanding {
            target: Principal::Player(alice),
            standing: Some(Standing::Neutral),
        }),
    )
    .await;
    assert!(
        !other_view
            .society
            .directory
            .standings
            .contains_key(&(Principal::Player(alice), Principal::Player(bob)))
    );

    let transferred = submit_action(
        &mut a,
        world,
        3,
        Action::Society(SocietyCommand::TransferAsset {
            asset: ship,
            owner: Principal::Organization(organization),
        }),
    )
    .await;
    assert!(
        transferred
            .results
            .iter()
            .all(|result| result.error.is_none()),
        "{:?}",
        transferred.results
    );
    assert_eq!(
        transferred
            .society
            .assets
            .iter()
            .find(|asset| asset.entity == ship)
            .unwrap()
            .owner,
        Principal::Organization(organization)
    );
    assert_eq!(
        transferred
            .ships
            .iter()
            .find(|item| item.ship == ship)
            .unwrap()
            .iff,
        first.ships[0].iff
    );

    let granted = submit_action(
        &mut a,
        world,
        4,
        Action::Society(SocietyCommand::SetAssetAccess {
            asset: ship,
            policy: AccessPolicy {
                public: BTreeSet::new(),
                grants: vec![AccessGrant {
                    principal: Principal::Player(bob),
                    permissions: BTreeSet::from([Permission::Control]),
                }],
            },
        }),
    )
    .await;
    assert!(granted.results.iter().all(|result| result.error.is_none()));
    let revision = granted
        .ships
        .iter()
        .find(|item| item.ship == ship)
        .unwrap()
        .authority_revision;
    let subscribed = submit_action(&mut b, world, 2, Action::InstrumentSubscribe { ship }).await;
    assert!(
        subscribed
            .results
            .iter()
            .all(|result| result.error.is_none())
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = b.state.recv().await.unwrap();
            if frame.presentation.ships.iter().any(|item| {
                item.ship == ship && matches!(item.computer, ComputerStatus::Running { .. })
            }) {
                break;
            }
        }
    })
    .await
    .expect("delegated ship computer did not finish booting");
    let controlled = submit_action(
        &mut b,
        world,
        3,
        Action::Ship {
            ship,
            authority_revision: revision,
            command: ShipCommand::SetThrottle(0.),
        },
    )
    .await;
    assert!(
        controlled
            .results
            .iter()
            .all(|result| result.error.is_none()),
        "{:?}",
        controlled.results
    );
    assert!(controlled.ships.iter().any(|item| item.ship == ship));

    let denied = submit_action(
        &mut b,
        world,
        4,
        Action::Ship {
            ship,
            authority_revision: revision,
            command: ShipCommand::SetTransponderEnabled(false),
        },
    )
    .await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));
    let denied = submit_action(
        &mut b,
        world,
        5,
        Action::Society(SocietyCommand::SetAssetAccess {
            asset: ship,
            policy: AccessPolicy::default(),
        }),
    )
    .await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));

    let revoked = submit_action(
        &mut a,
        world,
        5,
        Action::Society(SocietyCommand::SetAssetAccess {
            asset: ship,
            policy: AccessPolicy::default(),
        }),
    )
    .await;
    assert!(revoked.results.iter().all(|result| result.error.is_none()));
    let denied = submit_action(
        &mut b,
        world,
        6,
        Action::Ship {
            ship,
            authority_revision: revision,
            command: ShipCommand::SetThrottle(0.),
        },
    )
    .await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));
    assert!(!denied.ships.iter().any(|item| item.ship == ship));

    drop(a);
    drop(b);
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
