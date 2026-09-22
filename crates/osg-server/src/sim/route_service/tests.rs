use super::*;
use crate::sim::{
    gas::{GasLedger, STARTING_GAS},
    precision::PreciseTransform,
    vessel,
};
use bevy::math::DVec3;
use osg_model::travel::{Destination, Order, PlanningPreferences};

#[test]
fn starting_orbit_allows_slip_departure_without_a_clearance_burn() {
    let account = Id::new();
    let mut app = crate::sim::provision(&[account], Some(account), None).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let ship = app
        .world_mut()
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(app.world())
        .unwrap();
    let universe = app
        .world()
        .resource::<crate::sim::orrery::Universe>()
        .0
        .clone();
    let system = universe
        .systems
        .iter()
        .find(|system| system.name == "HIP 117953")
        .unwrap();
    let request = Request {
        id: 1,
        orders: vec![Order::TravelToSystem(Id(system.id))],
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
    let plan = routing::plan(&input, &environment).unwrap();
    assert!(matches!(plan.orders[0].action, Order::Slip { .. }));
    assert!(matches!(
        plan.orders.last().unwrap().action,
        Order::Slip { .. }
    ));
    assert!(
        plan.fuel_budget
            .resources
            .iter()
            .all(|resource| resource.required_kg
                <= resource.available_kg * input.preferences.fuel_fraction + 1e-9)
    );
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
