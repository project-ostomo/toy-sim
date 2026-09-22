use ed25519_dalek::SigningKey;
use osg_model::*;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn initial_patrol(frame: &Frame, account: Id) -> &ShipTelemetry {
    use ownership::{Permission, Principal};

    let mut controlled = frame.ships.iter().filter(|ship| ship.can_control);
    let patrol = controlled
        .next()
        .expect("account has a controllable patrol");
    assert!(
        controlled.next().is_none(),
        "initial account controls one patrol"
    );
    let directory = &frame.society.directory;
    for ship in &frame.ships {
        let asset = frame
            .society
            .assets
            .iter()
            .find(|asset| asset.entity == ship.ship)
            .expect("published ship has an authorized affiliation");
        if ship.ship == patrol.ship {
            assert_eq!(asset.owner, Principal::Player(account));
        } else {
            assert!(!ship.can_control);
            assert!(!directory.administers(account, asset.owner));
            assert!(asset.access.permits(directory, account, Permission::View));
            assert!(
                !asset
                    .access
                    .permits(directory, account, Permission::Control)
            );
        }
    }
    patrol
}

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
        osg_client::connect(
            &address,
            SigningKey::from_bytes(&[9; 32]).verifying_key(),
            account,
            &account_key,
        )
        .await
        .is_err()
    );
    assert!(
        osg_client::connect(&address, server_key.verifying_key(), account, &other_key,)
            .await
            .is_err()
    );
    let mut client =
        osg_client::connect(&address, server_key.verifying_key(), account, &account_key)
            .await
            .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), client.state.recv())
        .await
        .unwrap()
        .unwrap();
    let own = initial_patrol(&first, account);
    let (session_world, descriptor) = client.descriptor.borrow().clone().unwrap();
    assert_eq!(session_world, first.world);
    assert!(descriptor.epoch_mjd_utc.is_finite());
    let directory_hash = first.presentation.navigation.directory.unwrap();
    let directory_bytes =
        tokio::time::timeout(Duration::from_secs(10), client.assets.fetch(directory_hash))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(*blake3::hash(&directory_bytes).as_bytes(), directory_hash);
    let directory = osg_protocol::navigation::decode_directory(&directory_bytes).unwrap();
    assert!(!directory.systems.is_empty());
    assert!(directory.systems.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        first
            .presentation
            .navigation
            .beacons
            .iter()
            .flat_map(|beacon| &beacon.systems)
            .all(|system| directory.systems.binary_search(system).is_ok())
    );
    let action = Id::new();
    client
        .input
        .send(InputFrame {
            world: first.world,
            sequence: 1,
            actions: vec![
                (Id::new(), Action::InstrumentSubscribe { ship: own.ship }),
                (
                    Id::new(),
                    Action::ScreenSubscribe {
                        ship: own.ship,
                        slot: 0,
                        hz: 10,
                    },
                ),
                (
                    action,
                    Action::Subscribe(ViewSubscription {
                        id: 1,
                        revision: 1,
                        focused_ship: Some(own.ship),
                    }),
                ),
            ],
        })
        .await
        .unwrap();
    let mut last_display_state = None;
    let (observed, result) = tokio::time::timeout(Duration::from_secs(15), async {
        let mut result = None;
        loop {
            let frame = client.state.recv().await.unwrap();
            last_display_state = Some((
                frame.tick,
                frame
                    .presentation
                    .ships
                    .iter()
                    .find(|item| item.ship == own.ship)
                    .map(|item| item.computer.clone()),
                frame.screens.clone(),
            ));
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
    .unwrap_or_else(|_| panic!("display publication stalled: {last_display_state:?}"));
    assert!(result.error.is_none());
    assert_eq!(observed.views.len(), 1);
    assert_eq!(observed.screens.len(), 1);
    assert!(observed.screens[0].frame.as_ref().unwrap().draws.len() >= 5);
    assert!(!observed.contacts[&own.ship].is_empty());
    let own_optical = observed
        .optical
        .iter()
        .find(|observation| observation.known_entity == Some(own.ship))
        .expect("focused ship must arrive through the optical snapshot");
    assert_eq!(own_optical.view, observed.views[0].id);
    let appearance = own_optical.appearance.unwrap();
    let asset = tokio::time::timeout(Duration::from_secs(5), client.assets.fetch(appearance))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*blake3::hash(&asset).as_bytes(), appearance);
    assert!(!asset.is_empty());
    let mut other = osg_client::connect(
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
    let other_own = initial_patrol(&other_first, other_account);
    assert_ne!(other_own.ship, own.ship);
    let view = Id::new();
    let forbidden = Id::new();
    other
        .input
        .send(InputFrame {
            world: first.world,
            sequence: 1,
            actions: vec![
                (
                    view,
                    Action::Subscribe(ViewSubscription {
                        id: 1,
                        revision: 1,
                        focused_ship: Some(own.ship),
                    }),
                ),
                (
                    forbidden,
                    Action::Ship {
                        ship: own.ship,
                        authority_revision: own.authority_revision,
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
            .find(|result| result.id == view)
            .unwrap()
            .error
            .is_some()
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
    assert_eq!(initial_patrol(&shared, other_account).ship, other_own.ship);
    assert!(
        shared.optical.is_empty(),
        "an unauthorized focus has no optical observations"
    );
    assert!(shared.contacts.is_empty());
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
        osg_client::connect(&address, server_key.verifying_key(), account, &account_key)
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
        assert_eq!(client.descriptor.borrow().as_ref().unwrap().0, reset.world);

        let stale_action = Id::new();
        client
            .input
            .send(InputFrame {
                world: first.world,
                sequence: 2,
                actions: vec![(stale_action, Action::Debug(DebugCommand::SetRate(2.)))],
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
                        focused_ship: Some(reset.ships[0].ship),
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
    client: &mut osg_client::Endpoint,
    world: Id,
    sequence: u64,
    action: Action,
) -> std::sync::Arc<Frame> {
    let id = Id::new();
    let description = format!("{action:?}").chars().take(200).collect::<String>();
    client
        .input
        .send(InputFrame {
            world,
            sequence,
            actions: vec![(id, action)],
        })
        .await
        .unwrap();
    let mut last_frame = None;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = client.state.recv().await.unwrap();
            last_frame = Some((frame.world, frame.tick, frame.sequence, frame.results.len()));
            if frame.results.iter().any(|result| result.id == id) {
                return frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "action response timed out: sequence={sequence}, action={description}, last_frame={last_frame:?}, connection={:?}",
            client.status.borrow()
        )
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn industry_subscriptions_recheck_private_inventory_access_and_cargo_authority() {
    use industry::{CargoItem, IndustryCommand, IndustrySubscription};
    use ownership::{AccessGrant, AccessPolicy, Permission, Principal, SocietyCommand};

    let alice = Id([41; 16]);
    let bob = Id([42; 16]);
    let alice_key = SigningKey::from_bytes(&[43; 32]);
    let bob_key = SigningKey::from_bytes(&[44; 32]);
    let server_key = SigningKey::from_bytes(&[45; 32]);
    let mut server =
        ServerProcess::start(&server_key, &[(alice, &alice_key), (bob, &bob_key)], None);
    let address = server.ready().await;
    let mut a = osg_client::connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = osg_client::connect(&address, server_key.verifying_key(), bob, &bob_key)
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
    assert!(first.industry.is_none());
    assert!(other.industry.is_none());
    let world = first.world;
    let alice_ship = first.ships[0].ship;
    let bob_ship = other.ships[0].ship;

    let interest = |revision, catalogue| {
        Action::IndustrySubscribe(IndustrySubscription {
            revision,
            directory: true,
            inventories: vec![alice_ship, bob_ship],
            catalogue,
            ..Default::default()
        })
    };
    let subscribed = submit_action(&mut a, world, 1, interest(1, true)).await;
    assert!(
        subscribed
            .results
            .iter()
            .all(|result| result.error.is_none())
    );
    let snapshot = subscribed.industry.as_ref().unwrap();
    assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
    assert!(snapshot.catalogue.is_some());
    assert_eq!(snapshot.facilities.len(), 1);
    assert_eq!(snapshot.facilities[0].entity, alice_ship);
    assert!(
        !snapshot
            .directory
            .iter()
            .any(|entry| entry.entity == bob_ship)
    );

    let subscribed = submit_action(&mut b, world, 1, interest(1, false)).await;
    let snapshot = subscribed.industry.as_ref().unwrap();
    assert!(snapshot.catalogue.is_none());
    assert_eq!(snapshot.facilities.len(), 1);
    assert_eq!(snapshot.facilities[0].entity, bob_ship);

    let granted = submit_action(
        &mut a,
        world,
        2,
        Action::Society(SocietyCommand::SetAssetAccess {
            asset: alice_ship,
            policy: AccessPolicy {
                grants: vec![AccessGrant {
                    principal: Principal::Player(bob),
                    permissions: [Permission::View].into(),
                }],
                ..Default::default()
            },
        }),
    )
    .await;
    assert!(granted.results.iter().all(|result| result.error.is_none()));
    assert!(
        granted
            .industry
            .as_ref()
            .is_none_or(|snapshot| snapshot.catalogue.is_none())
    );
    let subscribed = submit_action(&mut b, world, 2, interest(2, false)).await;
    let snapshot = subscribed.industry.as_ref().unwrap();
    assert_eq!(snapshot.facilities.len(), 2);
    let shared = snapshot
        .facilities
        .iter()
        .find(|view| view.entity == alice_ship)
        .unwrap();
    assert!(!shared.can_transfer);
    assert!(!shared.can_manage);

    let denied = submit_action(
        &mut b,
        world,
        3,
        Action::Industry(IndustryCommand::Transfer {
            source: alice_ship,
            target: bob_ship,
            item: CargoItem::Resource("water".into()),
            quantity: 1,
        }),
    )
    .await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));

    let revoked = submit_action(
        &mut a,
        world,
        3,
        Action::Society(SocietyCommand::SetAssetAccess {
            asset: alice_ship,
            policy: AccessPolicy::default(),
        }),
    )
    .await;
    assert!(revoked.results.iter().all(|result| result.error.is_none()));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = b.state.recv().await.unwrap();
            if frame.industry.as_ref().is_some_and(|snapshot| {
                snapshot.subscription_revision == 2
                    && snapshot
                        .facilities
                        .iter()
                        .all(|view| view.entity != alice_ship)
            }) {
                break;
            }
        }
    })
    .await
    .expect("revoked inventory remained published");

    let unsubscribed = submit_action(&mut b, world, 4, Action::IndustryUnsubscribe).await;
    assert!(unsubscribed.industry.is_none());
    let stale = submit_action(&mut b, world, 5, interest(1, false)).await;
    assert!(stale.results.iter().any(|result| result.error.is_some()));
    assert!(stale.industry.is_none());

    drop(a);
    drop(b);
    server.shutdown().await;
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
    let mut a = osg_client::connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = osg_client::connect(&address, server_key.verifying_key(), bob, &bob_key)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_restart_restores_running_world_and_advances_real_calendar() {
    use ownership::{AccessPolicy, Permission, Principal, SocietyCommand, Standing};
    use std::collections::BTreeSet;

    let account = Id::new();
    let server_key = SigningKey::from_bytes(&[31; 32]);
    let account_key = SigningKey::from_bytes(&[32; 32]);
    let mut server = ServerProcess::start(&server_key, &[(account, &account_key)], Some(account));
    let address = server.ready().await;
    let mut client =
        osg_client::connect(&address, server_key.verifying_key(), account, &account_key)
            .await
            .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), client.state.recv())
        .await
        .unwrap()
        .unwrap();
    let own = initial_patrol(&first, account);
    let ship = own.ship;
    let world = first.world;
    let created = submit_action(
        &mut client,
        world,
        1,
        Action::Society(SocietyCommand::CreateOrganization {
            name: "Persistent Cooperative".into(),
        }),
    )
    .await;
    let organization = created.society.directory.players[&account]
        .organization
        .unwrap();
    let transfer = submit_action(
        &mut client,
        world,
        2,
        Action::Society(SocietyCommand::TransferAsset {
            asset: ship,
            owner: Principal::Organization(organization),
        }),
    )
    .await;
    assert!(transfer.results.iter().all(|result| result.error.is_none()));
    let policy = AccessPolicy {
        public: BTreeSet::from([Permission::View]),
        grants: Vec::new(),
    };
    let granted = submit_action(
        &mut client,
        world,
        3,
        Action::Society(SocietyCommand::SetAssetAccess {
            asset: ship,
            policy: policy.clone(),
        }),
    )
    .await;
    assert!(granted.results.iter().all(|result| result.error.is_none()));
    let target = *created
        .society
        .directory
        .organizations
        .keys()
        .find(|&&id| id != organization)
        .unwrap();
    let standing = submit_action(
        &mut client,
        world,
        4,
        Action::Society(SocietyCommand::SetStanding {
            target: Principal::Organization(target),
            standing: Some(Standing::Hostile),
        }),
    )
    .await;
    assert!(standing.results.iter().all(|result| result.error.is_none()));
    let mut iff = own.iff.clone();
    iff.faction = Some(organization);
    iff.labels.insert("restart-tested".into());
    let renamed = submit_action(
        &mut client,
        world,
        5,
        Action::Ship {
            ship,
            authority_revision: granted
                .ships
                .iter()
                .find(|item| item.ship == ship)
                .unwrap()
                .authority_revision,
            command: ShipCommand::SetIff(iff.clone()),
        },
    )
    .await;
    assert!(
        renamed.results.iter().all(|result| result.error.is_none()),
        "{:?}",
        renamed.results
    );
    let mut pose = own.pose.clone().unwrap();
    pose.position.x += 123_000_000_000;
    pose.velocity = [0.; 3];
    pose.angular_velocity = [0.; 3];
    let relocated = submit_action(
        &mut client,
        world,
        6,
        Action::Debug(DebugCommand::Relocate {
            ship,
            pose: pose.clone(),
        }),
    )
    .await;
    assert!(
        relocated
            .results
            .iter()
            .all(|result| result.error.is_none())
    );
    let saved = submit_action(&mut client, world, 7, Action::InstrumentSubscribe { ship }).await;
    let saved_ship = saved.ships.iter().find(|item| item.ship == ship).unwrap();
    let inventory = saved
        .presentation
        .ships
        .iter()
        .find(|item| item.ship == ship)
        .unwrap()
        .inventory
        .clone();
    drop(client);
    server.shutdown().await;
    assert!(
        server
            .directory
            .join("world.sqlite")
            .metadata()
            .unwrap()
            .len()
            > 0
    );

    let config_path = server.directory.join("server.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        format!("ship = \"removed-original-design.ship\"\n{config}"),
    )
    .unwrap();
    server.restart();
    let address = server.ready().await;
    let mut client =
        osg_client::connect(&address, server_key.verifying_key(), account, &account_key)
            .await
            .unwrap();
    let restored = tokio::time::timeout(Duration::from_secs(10), client.state.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.world, saved.world);
    assert!(restored.tick >= saved.tick);
    assert!(restored.sim_time_ns >= saved.sim_time_ns);
    assert_eq!(restored.rate, 1.);
    assert!(restored.calendar_unix_ms > saved.calendar_unix_ms);
    assert_eq!(restored.society.directory, saved.society.directory);
    let asset = restored
        .society
        .assets
        .iter()
        .find(|item| item.entity == ship)
        .unwrap();
    assert_eq!(asset.owner, Principal::Organization(organization));
    assert_eq!(asset.access, policy);
    let telemetry = restored
        .ships
        .iter()
        .find(|item| item.ship == ship)
        .unwrap();
    let restored_pose = telemetry.pose.as_ref().unwrap();
    let displacement_m = [
        restored_pose.position.x - pose.position.x,
        restored_pose.position.y - pose.position.y,
        restored_pose.position.z - pose.position.z,
    ]
    .map(|distance| distance as f64 / 1_000_000.);
    assert!(
        displacement_m.iter().map(|axis| axis * axis).sum::<f64>() < 100.0_f64.powi(2),
        "restored pose must continue near the saved relocation: {displacement_m:?}"
    );
    assert_eq!(telemetry.iff, iff);
    assert_eq!(telemetry.authority_revision, saved_ship.authority_revision);
    let instruments =
        submit_action(&mut client, world, 1, Action::InstrumentSubscribe { ship }).await;
    assert!(
        instruments
            .results
            .iter()
            .all(|result| result.error.is_none())
    );
    let restored_inventory = &instruments
        .presentation
        .ships
        .iter()
        .find(|item| item.ship == ship)
        .unwrap()
        .inventory;
    assert_eq!(restored_inventory.len(), inventory.len());
    for (restored, saved) in restored_inventory.iter().zip(&inventory) {
        assert_eq!(restored.resource, saved.resource);
        assert_eq!(restored.capacity_kg, saved.capacity_kg);
        assert_eq!(restored.unit_mass_kg, saved.unit_mass_kg);
        assert_eq!(restored.unit_volume_m3, saved.unit_volume_m3);
        assert!(restored.amount_kg <= restored.capacity_kg);
    }
    let mut last_restart_state = None;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = client.state.recv().await.unwrap();
            last_restart_state = Some((
                frame.tick,
                frame
                    .presentation
                    .ships
                    .iter()
                    .find(|item| item.ship == ship)
                    .map(|item| item.computer.clone()),
            ));
            if frame.tick > saved.tick + 20
                && frame.presentation.ships.iter().any(|item| {
                    item.ship == ship && matches!(item.computer, ComputerStatus::Running { .. })
                })
            {
                break;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "restored world stalled: saved_tick={}, latest={last_restart_state:?}",
            saved.tick
        )
    });
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
    let mut a = osg_client::connect(&address, server_key.verifying_key(), alice, &alice_key)
        .await
        .unwrap();
    let mut b = osg_client::connect(&address, server_key.verifying_key(), bob, &bob_key)
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
        let frame = submit_action(client, world, 1, focus(ship, 1)).await;
        assert!(frame.results.iter().all(|result| result.error.is_none()));
        let frame = submit_action(
            client,
            world,
            2,
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
        3,
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
        4,
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
            let frame = b.state.recv().await.unwrap();
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

    let denied = submit_action(&mut b, world, 3, focus(own.ship, 2)).await;
    assert!(denied.results.iter().any(|result| result.error.is_some()));
    submit_action(&mut b, world, 4, Action::ChatUnsubscribe).await;
    submit_action(
        &mut a,
        world,
        5,
        Action::ChatSend {
            subscription_revision: 1,
            text: "While the window is closed".into(),
        },
    )
    .await;
    let reopened = submit_action(
        &mut b,
        world,
        5,
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
        6,
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
