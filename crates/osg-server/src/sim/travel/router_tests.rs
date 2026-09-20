use super::*;
use crate::sim::{self, hardware, identity, session, vessel};
use osg_model::{Action, InputFrame, ShipCommand};
use std::sync::Arc;

fn fixture() -> (App, Entity, Id) {
    let mut app = sim::application(None);
    app.update();
    app.update();
    let world = app.world_mut();
    let ships = world
        .query_filtered::<Entity, With<vessel::Vessel>>()
        .iter(world)
        .collect::<Vec<_>>();
    for ship in ships {
        world.despawn(ship);
    }
    let account = Id::new();
    let mut blueprint = osg_ships::starter(osg_ships::EXAMPLE_CONTROLLER.to_vec());
    // Provide a fuel reserve for the full 600-second guidance deadline.
    for tank in &mut blueprint.parts[0].tanks {
        match tank.resource.as_str() {
            "propellant" => tank.volume_m3 = 0.69,
            "fuel" => tank.volume_m3 = 0.08,
            _ => {}
        }
    }
    let design = blueprint
        .compile(&world.resource::<vessel::ShipCatalogue>().0)
        .unwrap();
    let ship = vessel::spawn_ship(
        world,
        Arc::new(design),
        PreciseTransform {
            translation_um: GalacticPosition {
                x: 1_i128 << 100,
                y: 0,
                z: 0,
            },
            rotation: DQuat::IDENTITY,
        },
        DVec3::ZERO,
        "Router fixture".into(),
    )
    .unwrap();
    identity::attach_ship(world, ship, account).unwrap();
    (app, ship, account)
}

#[test]
fn computer_reads_only_the_current_order_and_cannot_complete_a_stale_queue() {
    let (mut app, ship, account) = fixture();
    let world = app.world_mut();
    let destination = world.get::<PreciseTransform>(ship).unwrap().translation_um;
    let current = QueuedOrder::estimated(Order::Sublight(Destination::Galactic(destination)), 100.)
        .with_propellant(20.);
    let future = QueuedOrder::estimated(Order::WaitUntil(9999), 200.);
    world.entity_mut(ship).insert(Travel(TravelState {
        autopilot_enabled: true,
        revision: 7,
        orders: vec![Order::WaitUntil(0).into(), current.clone(), future.clone()],
        order: 1,
        status: Status::Active,
        ..Default::default()
    }));

    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let source = crate::sim::commands::source(world, account, ship_id).unwrap();
    let osg_model::ProgramReply::Travel { state, .. } = source
        .query(
            osg_model::ProgramQuery::Travel,
            false,
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap()
    else {
        panic!("expected the active command");
    };
    assert_eq!(state.index, 1);
    assert_eq!(state.revision, 7);
    assert_eq!(state.order, Some(current));
    assert_eq!(state.status, Status::Active);

    dispatch(
        world,
        ship,
        osg_model::ProgramAction::Estimate {
            revision: 7,
            order: 1,
            remaining_ticks: Some(100),
            remaining_propellant_kg: Some(12.),
        },
    )
    .unwrap();
    let queue = &world.get::<Travel>(ship).unwrap().0;
    assert_eq!(queue.estimated_arrival_tick, Some(tick(world) + 100));
    assert_eq!(queue.orders[1].estimated_propellant_kg, Some(20.));
    assert_eq!(queue.orders[2], future);

    world.get_mut::<Travel>(ship).unwrap().0.revision = 8;
    let replacement = world.get::<Travel>(ship).unwrap().0.clone();
    for action in [
        osg_model::ProgramAction::CompleteOrder {
            revision: 7,
            order: 1,
        },
        osg_model::ProgramAction::Estimate {
            revision: 7,
            order: 1,
            remaining_ticks: Some(1),
            remaining_propellant_kg: Some(0.),
        },
        osg_model::ProgramAction::Block {
            revision: 7,
            order: 1,
            reason: "obsolete flight callback".into(),
        },
        osg_model::ProgramAction::Slip {
            revision: 7,
            order: 1,
            destination,
            speed_ly_s: 0.01,
            navigation_beacon: None,
        },
    ] {
        assert!(dispatch(world, ship, action).is_err());
        assert_eq!(world.get::<Travel>(ship).unwrap().0, replacement);
    }

    dispatch(
        world,
        ship,
        osg_model::ProgramAction::CompleteOrder {
            revision: 8,
            order: 1,
        },
    )
    .unwrap();
    assert_eq!(world.get::<Travel>(ship).unwrap().0.order, 2);
    assert_eq!(world.get::<Travel>(ship).unwrap().0.revision, 9);
    assert!(
        dispatch(
            world,
            ship,
            osg_model::ProgramAction::CompleteOrder {
                revision: 8,
                order: 1,
            },
        )
        .is_err()
    );
}

#[test]
fn completed_server_plan_atomically_replaces_the_queue_and_stale_results_are_rejected() {
    let (mut app, ship, _) = fixture();
    let world = app.world_mut();
    let destination = world.get::<PreciseTransform>(ship).unwrap().translation_um;
    world.entity_mut(ship).insert(Travel(TravelState {
        revision: 7,
        orders: vec![Order::WaitUntil(1).into(), Order::WaitUntil(9999).into()],
        order: 1,
        status: Status::Paused,
        ..Default::default()
    }));
    let orders = vec![
        QueuedOrder::estimated(Order::Sublight(Destination::Galactic(destination)), 10.)
            .with_propellant(1.),
        Order::WaitUntil(500).into(),
    ];
    let plan = osg_model::routing::Plan {
        planned_tick: tick(world),
        travel_revision: 7,
        topology_revision: 0,
        orders: orders.clone(),
        fuel_budget: FuelBudget {
            complete: true,
            ..Default::default()
        },
        estimated_loss_ppm: 0.0,
        beacon_assumptions: Vec::new(),
        exotic_fuel_kg: 0.0,
    };
    let goals = vec![Order::TravelTo(Destination::Galactic(destination))];
    apply_plan(
        world,
        ship,
        7,
        plan.clone(),
        Default::default(),
        false,
        goals.clone(),
        true,
    )
    .unwrap();
    let applied = world.get::<Travel>(ship).unwrap().0.clone();
    assert_eq!(applied.orders, orders);
    assert_eq!(applied.order, 0);
    assert_eq!(applied.revision, 8);
    assert_eq!(applied.status, Status::Paused);
    assert_eq!(applied.fuel_budget, Some(plan.fuel_budget.clone()));
    assert_eq!(applied.goals, goals);

    assert!(
        apply_plan(
            world,
            ship,
            7,
            plan,
            Default::default(),
            true,
            goals.clone(),
            true
        )
        .is_err()
    );
    assert_eq!(world.get::<Travel>(ship).unwrap().0, applied);

    let incomplete = osg_model::routing::Plan {
        planned_tick: tick(world),
        travel_revision: 8,
        topology_revision: 0,
        orders: vec![Order::TravelTo(Destination::Galactic(destination)).into()],
        fuel_budget: Default::default(),
        estimated_loss_ppm: 0.0,
        beacon_assumptions: Vec::new(),
        exotic_fuel_kg: 0.0,
    };
    assert!(
        apply_plan(
            world,
            ship,
            8,
            incomplete,
            Default::default(),
            true,
            goals,
            true
        )
        .is_err()
    );
    assert_eq!(world.get::<Travel>(ship).unwrap().0, applied);

    world
        .get_mut::<Travel>(ship)
        .unwrap()
        .0
        .risk_budget
        .spend(25.0);
    let remaining = world.get::<Travel>(ship).unwrap().0.risk_budget;
    let replacement = osg_model::routing::Plan {
        planned_tick: tick(world),
        travel_revision: 8,
        topology_revision: 0,
        orders: vec![Order::Sublight(Destination::Galactic(destination)).into()],
        fuel_budget: FuelBudget {
            complete: true,
            ..Default::default()
        },
        estimated_loss_ppm: 0.0,
        beacon_assumptions: Vec::new(),
        exotic_fuel_kg: 0.0,
    };
    apply_plan(
        world,
        ship,
        8,
        replacement,
        PlanningPreferences {
            max_loss_ppm: 1000.0,
            ..Default::default()
        },
        false,
        applied.goals,
        false,
    )
    .unwrap();
    let replanned = &world.get::<Travel>(ship).unwrap().0;
    assert_eq!(replanned.risk_budget, remaining);
    assert_eq!(replanned.preferences.max_loss_ppm, 100.0);
}

#[test]
fn travel_order_runs_in_stock_wasm_and_brakes_at_destination() {
    let (mut app, ship, account) = fixture();
    let world = app.world_mut();
    let destination = world
        .get::<PreciseTransform>(ship)
        .unwrap()
        .translation_um
        .offset_by(DVec3::NEG_Z * 100.);
    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let connection = session::connect(
        world,
        account,
        crate::blueprint_uploads::BlueprintUploads::default(),
    )
    .unwrap();
    let epoch = world.resource::<identity::WorldEpoch>().0;
    session::input(
        world,
        connection,
        InputFrame {
            world: epoch,
            sequence: 1,
            actions: vec![(
                Id::new(),
                Action::Ship {
                    ship: ship_id,
                    authority_revision: 1,
                    command: ShipCommand::SetTravel {
                        preferences: Default::default(),
                        engage: true,
                        expected_revision: 0,
                        orders: vec![Order::TravelTo(Destination::Galactic(destination))],
                    },
                },
            )],
        },
    )
    .unwrap();
    let response = session::frame(world, connection).unwrap();
    assert!(response.results.iter().all(|result| result.error.is_none()));

    for _ in 0..6000 {
        app.update();
        if app.world().get::<Travel>(ship).unwrap().0.status == Status::Completed {
            break;
        }
    }

    let world = app.world();
    let travel = &world.get::<Travel>(ship).unwrap().0;
    let pose = ship_pose(world, ship).unwrap();
    let software = world.get::<vessel::ShipSoftware>(ship).unwrap();
    assert_eq!(
        travel.status,
        Status::Completed,
        "pose {pose:?}, fault {:?}, travel {travel:?}, inventory {:?}",
        software.controller.fault,
        world
            .get::<crate::sim::hardware::ShipInventory>(ship)
            .unwrap()
            .0,
    );
    assert!(software.controller.fault.is_none());
    assert!(!software.controller.is_booting());
    assert!(pose.position.relative_to(destination).length() <= 2.);
    assert!(DVec3::from_array(pose.velocity).length() <= 0.5);
}

#[test]
fn server_plans_the_whole_paused_queue_before_any_flight_computer_runs() {
    let (mut app, ship, account) = fixture();
    let origin = app
        .world()
        .get::<PreciseTransform>(ship)
        .unwrap()
        .translation_um;
    let first = origin.offset_by(DVec3::NEG_Z * 100.);
    let second = origin.offset_by(DVec3::NEG_Z * 200.);
    app.world_mut()
        .entity_mut(ship)
        .remove::<vessel::ShipSoftware>();
    submit_orders(
        &mut app,
        ship,
        account,
        vec![
            Order::TravelTo(Destination::Galactic(first)),
            Order::WaitUntil(9999),
            Order::TravelTo(Destination::Galactic(second)),
        ],
        false,
        PlanningPreferences {
            max_loss_ppm: 1000.0,
            ..Default::default()
        },
    );

    app.update();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let world = app.world_mut();
        crate::sim::route_service::advance(world);
        plan_orders(world);
        let queue = &world.get::<Travel>(ship).unwrap().0;
        assert!(!queue.autopilot_enabled);
        assert!(!matches!(queue.status, Status::Blocked(_)), "{queue:?}");
        assert!(
            world
                .resource::<crate::sim::gas::GasLedger>()
                .snapshot()
                .is_ok(),
            "background route planning must leave gas settled for checkpoints"
        );
        if queue.status == Status::Paused
            && queue
                .orders
                .iter()
                .all(|stage| !matches!(stage.action, Order::TravelTo(_)))
        {
            assert_eq!(
                queue
                    .orders
                    .iter()
                    .map(|stage| stage.action.clone())
                    .collect::<Vec<_>>(),
                vec![
                    Order::Sublight(Destination::Galactic(first)),
                    Order::WaitUntil(9999),
                    Order::Sublight(Destination::Galactic(second)),
                ]
            );
            assert_eq!(queue.order, 0);
            assert_eq!(queue.preferences.max_loss_ppm, 1000.0);
            assert!((queue.risk_budget.remaining_ppm() - 1000.0).abs() < 1e-9);
            assert!(queue.fuel_budget.is_some());
            assert_eq!(
                world.get::<PreciseTransform>(ship).unwrap().translation_um,
                origin
            );
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{queue:?}");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn directional_alignment_runs_through_the_public_queue_without_translation() {
    let (mut app, ship, account) = fixture();
    let world = app.world_mut();
    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let connection = session::connect(
        world,
        account,
        crate::blueprint_uploads::BlueprintUploads::default(),
    )
    .unwrap();
    let epoch = world.resource::<identity::WorldEpoch>().0;
    session::input(
        world,
        connection,
        InputFrame {
            world: epoch,
            sequence: 1,
            actions: vec![(
                Id::new(),
                Action::Ship {
                    ship: ship_id,
                    authority_revision: 1,
                    command: ShipCommand::SetTravel {
                        preferences: Default::default(),
                        engage: true,
                        expected_revision: 0,
                        orders: vec![Order::Guidance(osg_model::travel::Guidance {
                            mode: osg_model::travel::GuidanceMode::Align,
                            target: osg_model::travel::Target::Direction(DVec3::X.to_array()),
                            range_m: 0.,
                        })],
                    },
                },
            )],
        },
    )
    .unwrap();
    assert!(
        session::frame(world, connection)
            .unwrap()
            .results
            .iter()
            .all(|r| r.error.is_none())
    );

    for _ in 0..600 {
        app.update();
        if app.world().get::<Travel>(ship).unwrap().0.status == Status::Completed {
            break;
        }
    }

    let world = app.world();
    let travel = &world.get::<Travel>(ship).unwrap().0;
    let pose = ship_pose(world, ship).unwrap();
    assert_eq!(travel.status, Status::Completed, "{travel:?}");
    assert!((DQuat::from_array(pose.rotation) * DVec3::NEG_Z).angle_between(DVec3::X) < 0.02);
    assert!(DVec3::from_array(pose.velocity).length() < 1e-4);
    assert!(
        world
            .get::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller
            .fault
            .is_none()
    );
}

#[test]
fn rejected_dock_does_not_complete_the_order() {
    let (mut app, ship, account) = fixture();
    let world = app.world_mut();
    let design = world.get::<ShipDesign>(ship).unwrap().0.clone();
    let mut station_pose = *world.get::<PreciseTransform>(ship).unwrap();
    station_pose.translation_um = station_pose.translation_um.offset_by(DVec3::NEG_X * 50.);
    let station = vessel::spawn_ship(
        world,
        design,
        station_pose,
        DVec3::ZERO,
        "Reserved docking host".into(),
    )
    .unwrap();
    identity::attach_ship(world, station, account).unwrap();
    let station_id = world.get::<Identity>(station).unwrap().0;
    world.entity_mut(station).remove::<vessel::ShipSoftware>();
    world.entity_mut(station).insert((
        identity::DirectoryEmitter,
        DockingBays(vec![Bay {
            centre_m: [50., 0., 0.],
            rotation: [0., 0., 0., 1.],
            radius_m: 100.,
            mass_capacity_kg: 1e12,
            public: true,
            allowed: BTreeSet::new(),
            reservation: Some((Id::new(), 1000)),
        }]),
    ));
    world.get_mut::<Travel>(ship).unwrap().0 = TravelState {
        autopilot_enabled: true,
        revision: 1,
        orders: vec![Order::Dock(station_id)]
            .into_iter()
            .map(Into::into)
            .collect(),
        status: Status::Active,
        ..Default::default()
    };

    let mut rejected = false;
    for _ in 0..80 {
        app.update();
        let travel = &app.world().get::<Travel>(ship).unwrap().0;
        rejected |= matches!(travel.status, Status::Blocked(_));
        assert_ne!(travel.status, Status::Completed);
        assert_eq!(travel.order, 0);
        assert_eq!(
            app.world().get::<PresenceState>(ship).unwrap().0,
            Presence::Space
        );
    }
    assert!(
        rejected,
        "firmware must report the unavailable reserved bay"
    );
    assert!(
        app.world()
            .get::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller
            .fault
            .is_none()
    );
    assert!(
        app.world()
            .get::<super::StoredShips>(station)
            .is_none_or(|ships| ships.is_empty())
    );
}

#[test]
fn autopilot_locks_manual_controls_and_off_cuts_thrust() {
    let (mut app, ship, account) = fixture();
    for _ in 0..150 {
        app.update();
    }
    let world = app.world_mut();
    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let connection = session::connect(
        world,
        account,
        crate::blueprint_uploads::BlueprintUploads::default(),
    )
    .unwrap();
    let mut sequence = 0;
    let mut send = |app: &mut App, command| {
        sequence += 1;
        let world = app.world_mut();
        session::input(
            world,
            connection,
            InputFrame {
                world: world.resource::<identity::WorldEpoch>().0,
                sequence,
                actions: vec![(
                    Id::new(),
                    Action::Ship {
                        ship: ship_id,
                        authority_revision: 1,
                        command,
                    },
                )],
            },
        )
        .unwrap();
        let frame = session::frame(world, connection).unwrap();
        frame.results[0].error.clone()
    };
    assert!(send(&mut app, ShipCommand::SetThrottle(0.6)).is_none());
    for _ in 0..5 {
        app.update();
    }
    let force = app
        .world()
        .get::<hardware::propulsion::ActuatorOutput>(ship)
        .unwrap()
        .force
        .length();
    assert!(force > 1.);
    assert!(
        send(
            &mut app,
            ShipCommand::Flight(osg_model::FlightCommand::AimDirection([0., 0., -1.]))
        )
        .is_none()
    );
    for _ in 0..5 {
        app.update();
    }
    assert!(
        app.world()
            .get::<hardware::propulsion::ActuatorOutput>(ship)
            .unwrap()
            .force
            .length()
            > 1.
    );
    assert!(send(&mut app, ShipCommand::SetAutopilot(true)).is_none());
    assert!(
        send(&mut app, ShipCommand::SetThrottle(1.))
            .unwrap()
            .contains("locked")
    );
    assert!(
        send(
            &mut app,
            ShipCommand::Flight(osg_model::FlightCommand::AimDirection([1., 0., 0.]))
        )
        .unwrap()
        .contains("locked")
    );
    assert!(send(&mut app, ShipCommand::SetAutopilot(false)).is_none());
    for _ in 0..10 {
        app.update();
    }
    assert!(
        app.world()
            .get::<hardware::propulsion::ActuatorOutput>(ship)
            .unwrap()
            .force
            .length()
            < 1e-6
    );
    assert!(!app.world().get::<Travel>(ship).unwrap().0.autopilot_enabled);
}

fn submit_orders(
    app: &mut App,
    ship: Entity,
    account: Id,
    orders: Vec<Order>,
    engage: bool,
    preferences: PlanningPreferences,
) {
    let world = app.world_mut();
    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let connection = session::connect(
        world,
        account,
        crate::blueprint_uploads::BlueprintUploads::default(),
    )
    .unwrap();
    let epoch = world.resource::<identity::WorldEpoch>().0;
    let expected_revision = world.get::<Travel>(ship).unwrap().0.revision;
    session::input(
        world,
        connection,
        InputFrame {
            world: epoch,
            sequence: 1,
            actions: vec![(
                Id::new(),
                Action::Ship {
                    ship: ship_id,
                    authority_revision: 1,
                    command: ShipCommand::SetTravel {
                        preferences,
                        engage,
                        expected_revision,
                        orders,
                    },
                },
            )],
        },
    )
    .unwrap();
    let frame = session::frame(world, connection).unwrap();
    assert!(frame.results.iter().all(|result| result.error.is_none()));
}
