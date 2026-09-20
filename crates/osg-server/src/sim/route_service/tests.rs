use super::*;
use crate::sim::{
    gas::{GasLedger, STARTING_GAS},
    precision::PreciseTransform,
    vessel,
};
use bevy::math::DVec3;
use osg_model::travel::{Destination, Order, PlanningPreferences};

#[test]
#[ignore = "full seeded world route performance measurement"]
fn seeded_routes_performance() {
    let account = Id::new();
    let mut app = crate::sim::provision(&[account], Some(account), None).unwrap();
    crate::sim::npc::seed::populate(app.world_mut()).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let ship = app
        .world_mut()
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .iter(app.world())
        .next()
        .unwrap();
    crate::sim::infrastructure::publish_navigation(app.world_mut());
    let origin = app
        .world()
        .get::<PreciseTransform>(ship)
        .unwrap()
        .translation_um;
    let universe = app
        .world()
        .resource::<crate::sim::orrery::Universe>()
        .0
        .clone();
    let mut destinations: Vec<_> = app
        .world()
        .resource::<crate::sim::infrastructure::NavigationPublication>()
        .directory
        .systems
        .iter()
        .filter_map(|id| {
            let system = &universe.systems[universe.system_index(id.0)?];
            let distance =
                system.position.relative_to(origin).length() / osg_model::travel::slip::LY_M;
            (distance > 0.1).then_some((distance, system.position))
        })
        .collect();
    destinations.sort_by(|a, b| a.0.total_cmp(&b.0));
    for distance in [1., 10., 100.] {
        let (actual, position) = destinations
            .iter()
            .min_by(|a, b| (a.0 - distance).abs().total_cmp(&(b.0 - distance).abs()))
            .unwrap();
        let request = Request {
            id: 1,
            orders: vec![Order::TravelTo(Destination::Galactic(*position))],
            preferences: PlanningPreferences::default(),
        };
        let admitted = caller(app.world(), ship).unwrap();
        let (input, mut environment) = prepare(
            app.world_mut(),
            admitted,
            &request,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        environment.prepare().unwrap();
        let started = std::time::Instant::now();
        let (result, work) = routing::plan_metered(&input, &environment);
        eprintln!(
            "seeded route distance_ly={actual:.3} elapsed_ms={:.2} work={work} result={:?}",
            started.elapsed().as_secs_f64() * 1000.,
            result
                .as_ref()
                .map(|plan| (plan.orders.len(), plan.estimated_loss_ppm))
        );
        assert!(result.is_ok(), "{result:?}");
    }
}

fn identity() -> Caller {
    Caller {
        world: Id::new(),
        ship: Id::new(),
        owner: Principal::Player(Id::new()),
        authority_revision: 1,
        travel_revision: 1,
        topology_revision: 7,
        origin: Origin::Explicit,
    }
}

fn request(id: u64) -> Request {
    Request {
        id,
        orders: vec![Order::WaitUntil(500)],
        preferences: PlanningPreferences::default(),
    }
}

#[test]
fn enqueue_idempotence_namespace_and_revision_checks_precede_mutation() {
    let service = RouteService::default();
    let caller = identity();
    assert!(matches!(service.poll(caller, 1), Status::Unknown));
    assert!(service.0.lock().unwrap().queue.is_empty());

    service
        .submit(
            caller,
            request(1),
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap();
    service
        .submit(
            caller,
            request(1),
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap();
    assert_eq!(service.0.lock().unwrap().queue.len(), 1);
    let mut conflict = request(1);
    conflict.orders = vec![Order::WaitUntil(700)];
    assert!(
        service
            .submit(
                caller,
                conflict,
                osg_model::wasm_world::ReplyCapacity::UNLIMITED
            )
            .is_err()
    );

    let mut automatic = caller;
    automatic.origin = Origin::Automatic;
    service
        .submit(
            automatic,
            request(1),
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap();
    assert_eq!(service.0.lock().unwrap().queue.len(), 2);
    let mut stale = caller;
    stale.travel_revision += 1;
    assert!(matches!(service.poll(stale, 1), Status::Failed { .. }));
    stale.owner = Principal::Organization(Id::new());
    assert!(matches!(service.poll(stale, 1), Status::Unknown));
    stale = caller;
    stale.authority_revision += 1;
    assert!(matches!(service.poll(stale, 1), Status::Unknown));

    let mut changed_catalogue = caller;
    changed_catalogue.topology_revision += 1;
    assert!(matches!(
        service.poll(changed_catalogue, 1),
        Status::Pending { .. }
    ));
}

#[test]
fn prepaid_background_work_is_snapshot_safe_and_refunds_exactly_once() {
    let ledger = GasLedger::default();
    let owner = Principal::Organization(Id::new());
    ledger.ensure_account(owner, 1000);
    let payment = ledger.prepay(owner, 600).unwrap();
    let checkpoint = ledger.snapshot().unwrap();
    let saved = checkpoint.accounts[&owner];
    assert_eq!(
        (saved.available, saved.spent, saved.reserved),
        (400, 600, 0)
    );

    ledger.reserve(owner, 50).unwrap().settle(50).unwrap();
    payment.settle(123).unwrap();
    let account = ledger.account(owner).unwrap();
    assert_eq!(
        (account.available, account.spent, account.reserved),
        (827, 173, 0)
    );
    assert_eq!(account.available + account.spent, 1000);
    assert_eq!(
        checkpoint.accounts[&owner].spent, 600,
        "a checkpoint retains its own prepaid cost"
    );

    let payment = ledger.prepay(owner, 200).unwrap();
    assert!(payment.settle(201).is_err());
    assert_eq!(ledger.account(owner).unwrap().spent, 373);
    drop(ledger.prepay(owner, 100).unwrap());
    assert_eq!(
        ledger.account(owner).unwrap().spent,
        473,
        "interrupted work retains its bounded charge"
    );
    assert!(ledger.snapshot().is_ok());
}

fn completed(world: &mut World, ship: Entity, id: u64) -> Plan {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        advance(world);
        assert!(world.resource::<GasLedger>().snapshot().is_ok());
        match poll(world, ship, id).unwrap() {
            Status::Ready { plan } => return plan,
            Status::Failed { reason } => panic!("route failed: {reason}"),
            _ => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "route worker did not complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn worker_uses_public_inputs_without_charging_route_computation() {
    let player = Id::new();
    let mut app = crate::sim::provision(&[player], None, None).unwrap();
    app.update();
    let world = app.world_mut();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let current = caller(world, ship).unwrap();
    let initial = world
        .resource::<GasLedger>()
        .account(current.owner)
        .unwrap();
    let origin = world.get::<PreciseTransform>(ship).unwrap().translation_um;
    submit(
        world,
        ship,
        Request {
            id: 19,
            orders: vec![Order::Sublight(Destination::Galactic(
                origin.offset_by(DVec3::Z * 10_000.0),
            ))],
            preferences: PlanningPreferences::default(),
        },
    )
    .unwrap();
    let plan = completed(world, ship, 19);
    assert_eq!(plan.travel_revision, current.travel_revision);
    assert_eq!(plan.topology_revision, current.topology_revision);
    assert!(!plan.orders.is_empty());
    assert!(ready(world, ship, 19, current.travel_revision).is_ok());
    let paid = world
        .resource::<GasLedger>()
        .account(current.owner)
        .unwrap();
    assert_eq!(paid.spent, initial.spent);
    assert_eq!(paid.available + paid.spent, STARTING_GAS);

    reset(world);
    assert!(matches!(poll(world, ship, 19).unwrap(), Status::Unknown));
}
