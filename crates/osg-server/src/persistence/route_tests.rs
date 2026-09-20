use super::*;
use osg_model::{
    ProgramQuery, ProgramReply,
    travel::{
        Destination, FuelBudget, FuelRequirement, Order, PlanningPreferences, QueuedOrder, Status,
    },
};

#[test]
fn paused_server_route_survives_computer_restart_with_its_full_fuel_budget() {
    let account = Id::new();
    let mut app = crate::scenario(&[account], Some(account), None).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let ship_id = id(world, ship).unwrap();
    let (gate, exit) = world
        .query::<(&identity::Identity, &travel::Gate)>()
        .iter(world)
        .next()
        .map(|(identity, gate)| (identity.0, gate.paired))
        .unwrap();
    let station = world
        .query::<(&identity::Identity, &travel::DockingBays)>()
        .iter(world)
        .next()
        .map(|(identity, _)| identity.0)
        .unwrap();
    let destination = pose(world, ship)
        .unwrap()
        .position
        .offset_by(DVec3::X * 1_000_000.);
    let original = TravelState {
        autopilot_enabled: false,
        preferences: PlanningPreferences {
            fuel_fraction: 0.42,
            ..Default::default()
        },
        revision: 27,
        order: 1,
        orders: vec![
            QueuedOrder::estimated(Order::WaitUntil(1), 1.).with_propellant(0.),
            QueuedOrder::estimated(Order::Jump(gate), 240.).with_propellant(123.),
            QueuedOrder::estimated(
                Order::Slip {
                    destination: Destination::Beacon(exit),
                },
                60.,
            )
            .with_propellant(0.),
            QueuedOrder::estimated(Order::Dock(station), 120.).with_propellant(45.),
        ],
        status: Status::Paused,
        fuel_budget: Some(FuelBudget {
            complete: true,
            resources: vec![FuelRequirement {
                resource: "water".into(),
                required_kg: 168.,
                available_kg: 1000.,
            }],
        }),
        estimated_arrival_tick: Some(12345),
        ..Default::default()
    };
    world
        .entity_mut(ship)
        .insert(travel::Travel(original.clone()));

    let preview = osg_model::routing::Request {
        id: 94,
        orders: vec![Order::TravelTo(Destination::Galactic(destination))],
        preferences: Default::default(),
    };
    assert!(matches!(
        crate::sim::route_service::submit(world, ship, preview).unwrap(),
        osg_model::routing::Status::Pending { .. }
    ));
    crate::sim::route_service::advance(world);

    let checkpoint = capture(world).unwrap();
    restore(world, &checkpoint).unwrap();
    let ship = identity::lookup(world, ship_id).unwrap();
    assert!(matches!(
        crate::sim::route_service::poll(world, ship, 94).unwrap(),
        osg_model::routing::Status::Unknown
    ));
    let restored = &world.get::<travel::Travel>(ship).unwrap().0;
    let mut expected = original;
    expected.estimated_arrival_tick = None;
    assert_eq!(restored, &expected);
    assert!(
        world
            .get::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller
            .is_booting()
    );

    let source = crate::sim::commands::source(world, account, ship_id).unwrap();
    let ProgramReply::Travel { state, .. } = source
        .query(
            ProgramQuery::Travel,
            false,
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap()
    else {
        panic!("expected the current command");
    };
    assert_eq!(state.order, Some(expected.orders[1].clone()));
    assert_eq!(state.index, 1);
    assert_eq!(state.revision, 27);
    assert!(!state.autopilot_enabled);
    assert_eq!(state.status, Status::Paused);
}

#[test]
fn interrupted_server_planning_restarts_from_saved_requested_orders_while_paused() {
    let account = Id::new();
    let mut app = crate::scenario(&[account], Some(account), None).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let ship_id = id(world, ship).unwrap();
    let destination = pose(world, ship)
        .unwrap()
        .position
        .offset_by(DVec3::X * 1_000.);
    let request = vec![
        Order::TravelTo(Destination::Galactic(destination)).into(),
        Order::WaitUntil(9999).into(),
    ];
    world.entity_mut(ship).insert(travel::Travel(TravelState {
        autopilot_enabled: false,
        revision: 31,
        orders: request.clone(),
        status: Status::Planning,
        ..Default::default()
    }));
    let checkpoint = capture(world).unwrap();
    restore(world, &checkpoint).unwrap();
    let ship = identity::lookup(world, ship_id).unwrap();
    assert_eq!(world.get::<travel::Travel>(ship).unwrap().0.orders, request);
    world.entity_mut(ship).remove::<vessel::ShipSoftware>();

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let world = app.world_mut();
        crate::sim::route_service::advance(world);
        travel::plan_orders(world);
        let state = &world.get::<travel::Travel>(ship).unwrap().0;
        assert!(!state.autopilot_enabled);
        assert!(!matches!(state.status, Status::Blocked(_)), "{state:?}");
        if state.status == Status::Paused
            && state
                .orders
                .iter()
                .all(|stage| !matches!(stage.action, Order::TravelTo(_)))
        {
            let (last, movement) = state.orders.split_last().unwrap();
            assert_eq!(last.action, Order::WaitUntil(9999));
            assert!(!movement.is_empty());
            assert!(movement.iter().all(|stage| match &stage.action {
                Order::Sublight(Destination::Galactic(target))
                | Order::Slip {
                    destination: Destination::Galactic(target),
                } => *target == destination,
                _ => false,
            }));
            assert_eq!(
                movement.last().unwrap().action,
                Order::Sublight(Destination::Galactic(destination))
            );
            assert_eq!(state.order, 0);
            assert!(state.revision > 31);
            assert!(capture(world).is_ok());
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{state:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}
