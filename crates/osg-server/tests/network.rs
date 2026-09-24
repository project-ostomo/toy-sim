use ed25519_dalek::SigningKey;
use osg_client::{HeadlessClient, OsgNetClient};
use osg_model::*;
use osg_model::{ownership::*, rpc::Operation};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

async fn connect(
    address: &str,
    server: ed25519_dalek::VerifyingKey,
    account: Id,
    key: &SigningKey,
) -> anyhow::Result<HeadlessClient> {
    Ok(HeadlessClient::new(
        OsgNetClient::connect(address, server, account, key).await?,
    ))
}

async fn first(client: &mut HeadlessClient) -> Arc<Frame> {
    tokio::time::timeout(Duration::from_secs(20), client.receive())
        .await
        .unwrap()
        .unwrap()
}

fn initial_patrol(frame: &Frame, _account: Id) -> &ShipTelemetry {
    let mut ships = frame.ships.iter().filter(|ship| ship.can_control);
    let ship = ships.next().expect("account has a patrol");
    assert!(ships.next().is_none());
    ship
}

fn operation(world: Id) -> Operation {
    Operation {
        world,
        id: Id::new(),
    }
}

async fn submit_action(client: &mut HeadlessClient, world: Id, action: Action) -> Arc<Frame> {
    let id = Id::new();
    client
        .client
        .send_inputs(world, &[(id, action)])
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = client.receive().await.unwrap();
            if frame.results.iter().any(|result| result.id == id) {
                return frame;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_main_stream_and_rpc_share_authority_and_asset_transfers() {
    let alice = Id::new();
    let bob = Id::new();
    let alice_key = SigningKey::from_bytes(&[2; 32]);
    let bob_key = SigningKey::from_bytes(&[3; 32]);
    let server_key = SigningKey::from_bytes(&[1; 32]);
    let mut server =
        ServerProcess::start(&server_key, &[(alice, &alice_key), (bob, &bob_key)], None);
    let address = server.ready().await;
    assert!(
        connect(
            &address,
            SigningKey::from_bytes(&[9; 32]).verifying_key(),
            alice,
            &alice_key
        )
        .await
        .is_err()
    );
    assert!(
        connect(&address, server_key.verifying_key(), alice, &bob_key)
            .await
            .is_err()
    );
    let mut a = connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = connect(&address, server_key.verifying_key(), bob, &bob_key)
        .await
        .unwrap();
    let first_a = first(&mut a).await;
    let first_b = first(&mut b).await;
    let world = first_a.world;
    let ship = initial_patrol(&first_a, alice);
    let other = initial_patrol(&first_b, bob);
    assert_eq!(a.client.session().unwrap().0, world);
    let standing = a
        .client
        .resolve_standing(world, Principal::Player(alice))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(standing.standing, osg_model::ownership::Standing::Friendly);
    let declaration = osg_model::diplomacy::Declaration {
        source: Principal::Player(alice),
        target: Principal::Player(bob),
        category: osg_model::diplomacy::DeclarationCategory::Standing,
        revision: 0,
        standing: osg_model::ownership::Standing::Friendly,
        enabled: true,
        note: "Trusted trading partner".into(),
    };
    a.client
        .publish_declaration(operation(world), declaration.clone())
        .await
        .unwrap()
        .unwrap();
    let history = a
        .client
        .declaration_history(
            world,
            declaration.source,
            declaration.category,
            declaration.target,
            None,
            32,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(history.items.len(), 1);
    assert_eq!(history.items[0].revision, 1);
    assert!(
        a.client
            .diplomacy(world, declaration.source)
            .await
            .unwrap()
            .unwrap()
            .declaration_history
            .is_empty()
    );
    let standing = a
        .client
        .resolve_standing(world, declaration.target)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        standing.source,
        osg_model::ownership::StandingSource::Declaration { revision: 1, .. }
    ));
    let hash = first_a.presentation.navigation.directory.unwrap();
    let bytes = a.client.fetch_asset(hash).await.unwrap();
    let directory = osg_protocol::navigation::decode_directory(&bytes).unwrap();
    assert!(!directory.systems.is_empty());
    assert_eq!(*blake3::hash(&bytes).as_bytes(), hash);
    assert_eq!(
        a.client
            .my_affiliation(world)
            .await
            .unwrap()
            .unwrap()
            .account,
        alice
    );
    assert!(
        a.client
            .wallet_balance(world, Principal::Player(bob))
            .await
            .unwrap()
            .is_err()
    );
    let visible = a
        .client
        .list_assets(world, String::new(), None, None, 128)
        .await
        .unwrap()
        .unwrap();
    assert!(visible.items.iter().any(|entry| entry.id == ship.ship));
    assert!(!visible.items.iter().any(|entry| entry.id == other.ship));
    let observed = submit_action(
        &mut a,
        world,
        Action::Subscribe(ViewSubscription {
            id: 1,
            revision: 1,
            focused_ship: Some(ship.ship),
        }),
    )
    .await;
    assert!(observed.results.iter().all(|result| result.error.is_none()));
    let denied = submit_action(
        &mut a,
        world,
        Action::Ship {
            ship: other.ship,
            authority_revision: other.authority_revision,
            command: ShipCommand::SetThrottle(0.),
        },
    )
    .await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));
    assert!(
        a.client
            .route_status(world, other.ship, other.authority_revision, 42)
            .await
            .unwrap()
            .is_err()
    );
    let route = a
        .client
        .route_request(
            operation(world),
            ship.ship,
            ship.authority_revision,
            routing::Request {
                id: 42,
                directives: vec![],
                preferences: Default::default(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(!matches!(route, routing::Status::Unknown));
    assert!(!matches!(
        a.client
            .route_status(world, ship.ship, ship.authority_revision, 42)
            .await
            .unwrap()
            .unwrap(),
        routing::Status::Unknown
    ));
    a.client
        .route_cancel(operation(world), ship.ship, ship.authority_revision, 42)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        a.client
            .route_status(world, ship.ship, ship.authority_revision, 42)
            .await
            .unwrap()
            .unwrap(),
        routing::Status::Unknown
    ));
    drop(a);
    drop(b);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_inventory_queries_recheck_permissions_and_mutations_preserve_cargo_authority() {
    let alice = Id::new();
    let bob = Id::new();
    let alice_key = SigningKey::from_bytes(&[12; 32]);
    let bob_key = SigningKey::from_bytes(&[13; 32]);
    let server_key = SigningKey::from_bytes(&[11; 32]);
    let mut server =
        ServerProcess::start(&server_key, &[(alice, &alice_key), (bob, &bob_key)], None);
    let address = server.ready().await;
    let mut a = connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = connect(&address, server_key.verifying_key(), bob, &bob_key)
        .await
        .unwrap();
    let first_a = first(&mut a).await;
    let first_b = first(&mut b).await;
    let world = first_a.world;
    let own = initial_patrol(&first_a, alice).ship;
    let other = initial_patrol(&first_b, bob).ship;
    assert!(a.client.facility(world, own).await.unwrap().is_ok());
    assert!(b.client.facility(world, own).await.unwrap().is_err());
    assert!(a.client.industry_catalogue(world).await.unwrap().is_ok());
    assert!(
        b.client
            .set_asset_access(operation(world), own, AccessPolicy::default())
            .await
            .unwrap()
            .is_err()
    );
    a.client
        .set_asset_access(
            operation(world),
            own,
            AccessPolicy {
                public: Default::default(),
                grants: vec![AccessGrant {
                    principal: Principal::Player(bob),
                    permissions: [Permission::View].into(),
                }],
            },
        )
        .await
        .unwrap()
        .unwrap();
    let shared = b.client.facility(world, own).await.unwrap().unwrap();
    assert!(!shared.can_manage && !shared.can_transfer);
    assert!(
        b.client
            .transfer_cargo(
                operation(world),
                own,
                other,
                industry::CargoItem::Resource("water".into()),
                1
            )
            .await
            .unwrap()
            .is_err()
    );
    a.client
        .set_asset_access(operation(world), own, AccessPolicy::default())
        .await
        .unwrap()
        .unwrap();
    assert!(b.client.facility(world, own).await.unwrap().is_err());
    assert!(b.client.asset_access(world, own).await.unwrap().is_err());
    drop(a);
    drop(b);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_operation_results_and_ownership_survive_process_restart() {
    let account = Id::new();
    let key = SigningKey::from_bytes(&[22; 32]);
    let server_key = SigningKey::from_bytes(&[21; 32]);
    let mut server = ServerProcess::start(&server_key, &[(account, &key)], None);
    let address = server.ready().await;
    let mut client = connect(&address, server_key.verifying_key(), account, &key)
        .await
        .unwrap();
    let initial = first(&mut client).await;
    let world = initial.world;
    let ship = initial_patrol(&initial, account).ship;
    let create = operation(world);
    client
        .client
        .create_organization(create, "RPC persistence test".into())
        .await
        .unwrap()
        .unwrap();
    client
        .client
        .create_organization(create, "RPC persistence test".into())
        .await
        .unwrap()
        .unwrap();
    assert!(
        client
            .client
            .create_organization(create, "Different arguments".into())
            .await
            .unwrap()
            .is_err()
    );
    let entries = client
        .client
        .list_identities(world, "RPC persistence test".into(), None, 128)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entries.items.len(), 1);
    let owner = entries.items[0].principal();
    assert!(matches!(owner, Principal::Organization(_)));
    let transfer = operation(world);
    client
        .client
        .transfer_asset(transfer, ship, owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        client
            .client
            .asset_access(world, ship)
            .await
            .unwrap()
            .unwrap()
            .asset
            .owner,
        owner
    );
    drop(client);
    server.shutdown().await;
    server.restart();
    let address = server.ready().await;
    let mut client = connect(&address, server_key.verifying_key(), account, &key)
        .await
        .unwrap();
    let restored = first(&mut client).await;
    assert_eq!(restored.world, world);
    assert!(restored.tick >= initial.tick);
    assert!(restored.calendar_unix_ms >= initial.calendar_unix_ms);
    client
        .client
        .create_organization(create, "RPC persistence test".into())
        .await
        .unwrap()
        .unwrap();
    client
        .client
        .transfer_asset(transfer, ship, owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        client
            .client
            .list_identities(world, "RPC persistence test".into(), None, 128)
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        client
            .client
            .asset_access(world, ship)
            .await
            .unwrap()
            .unwrap()
            .asset
            .owner,
        owner
    );
    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn world_reset_rejects_old_rpc_and_input_contexts() {
    let account = Id::new();
    let key = SigningKey::from_bytes(&[32; 32]);
    let server_key = SigningKey::from_bytes(&[31; 32]);
    let mut server = ServerProcess::start(&server_key, &[(account, &key)], Some(account));
    let address = server.ready().await;
    let mut client = connect(&address, server_key.verifying_key(), account, &key)
        .await
        .unwrap();
    let old = first(&mut client).await;
    client
        .client
        .send_inputs(
            old.world,
            &[(Id::new(), Action::Debug(DebugCommand::Reset))],
        )
        .await
        .unwrap();
    let current = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = client.receive().await.unwrap();
            if frame.world != old.world {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        client
            .client
            .my_affiliation(old.world)
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        client
            .client
            .try_send_inputs(old.world, vec![(Id::new(), Action::Unsubscribe(1))])
            .is_err()
    );
    assert_eq!(
        client
            .client
            .my_affiliation(current.world)
            .await
            .unwrap()
            .unwrap()
            .account,
        account
    );
    drop(client);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_chat_delivers_to_focused_controlled_ships_without_identity_or_history_leaks() {
    let alice = Id([61; 16]);
    let bob = Id([62; 16]);
    let alice_key = SigningKey::from_bytes(&[63; 32]);
    let bob_key = SigningKey::from_bytes(&[64; 32]);
    let server_key = SigningKey::from_bytes(&[65; 32]);
    let mut server =
        ServerProcess::start(&server_key, &[(alice, &alice_key), (bob, &bob_key)], None);
    let address = server.ready().await;
    let mut a = connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = connect(&address, server_key.verifying_key(), bob, &bob_key)
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), a.receive())
        .await
        .unwrap()
        .unwrap();
    let other = tokio::time::timeout(Duration::from_secs(10), b.receive())
        .await
        .unwrap()
        .unwrap();
    let world = first.world;
    let own = &first.ships[0];
    let remote = &other.ships[0];
    let focus = |ship, revision| {
        Action::Subscribe(ViewSubscription {
            id: 1,
            revision,
            focused_ship: Some(ship),
        })
    };
    for (client, ship) in [(&mut a, own.ship), (&mut b, remote.ship)] {
        let frame = submit_action(client, world, focus(ship, 1)).await;
        assert!(frame.results.iter().all(|result| result.error.is_none()));
        let frame = submit_action(
            client,
            world,
            Action::ChatSubscribe(chat::ChatSubscription {
                revision: 1,
                view: 1,
            }),
        )
        .await;
        assert!(frame.results.iter().all(|result| result.error.is_none()));
        assert!(frame.chat.as_ref().unwrap().page.messages.is_empty());
    }
    let disabled = submit_action(
        &mut a,
        world,
        Action::Ship {
            ship: own.ship,
            authority_revision: own.authority_revision,
            command: ShipCommand::SetTransponderEnabled(false),
        },
    )
    .await;
    assert!(disabled.results.iter().all(|result| result.error.is_none()));
    let sent = submit_action(
        &mut a,
        world,
        Action::ChatSend {
            subscription_revision: 1,
            text: "Hello from a dark transmitter".into(),
        },
    )
    .await;
    assert!(sent.results.iter().all(|result| result.error.is_none()));
    let echoed = &sent.chat.as_ref().unwrap().page.messages[0];
    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = b.receive().await.unwrap();
            if let Some(message) = frame
                .chat
                .as_ref()
                .and_then(|chat| chat.page.messages.first())
            {
                break message.clone();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(received.id, echoed.id);
    assert_eq!(received.text, "Hello from a dark transmitter");
    assert_eq!(received.sender_name, "Unidentified transmission");
    assert_eq!(received.advertised_owner, None);
    assert_eq!(received.advertised_organization, None);

    let denied = submit_action(&mut b, world, focus(own.ship, 2)).await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));
    submit_action(&mut b, world, Action::ChatUnsubscribe).await;
    submit_action(
        &mut a,
        world,
        Action::ChatSend {
            subscription_revision: 1,
            text: "While the window is closed".into(),
        },
    )
    .await;
    let reopened = submit_action(
        &mut b,
        world,
        Action::ChatSubscribe(chat::ChatSubscription {
            revision: 2,
            view: 1,
        }),
    )
    .await;
    assert!(reopened.results.iter().all(|result| result.error.is_none()));
    assert!(reopened.chat.as_ref().unwrap().page.messages.is_empty());
    let stale = submit_action(
        &mut b,
        world,
        Action::ChatSend {
            subscription_revision: 1,
            text: "Stale channel".into(),
        },
    )
    .await;
    assert!(stale.results.iter().any(|result| result.error.is_some()));
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
        let directory = std::env::temp_dir().join(format!("osg-network-test-{}", Id::new()));
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
        let child = Self::spawn(&directory);
        Self { child, directory }
    }

    fn spawn(directory: &std::path::Path) -> Child {
        Command::new(env!("CARGO_BIN_EXE_osg-server"))
            .arg(directory.join("server.toml"))
            .arg("--ready-file")
            .arg(directory.join("ready"))
            .arg("--shutdown-on-stdin-close")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn restart(&mut self) {
        assert!(self.child.try_wait().unwrap().is_some());
        self.child = Self::spawn(&self.directory);
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
