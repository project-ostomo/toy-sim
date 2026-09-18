use super::*;
use crate::sim::{self, identity, session, vessel};
use std::sync::Arc;
use toy_sim_model::{Action, InputFrame, ShipCommand};

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
    let mut blueprint = toy_sim_ships::starter(toy_sim_ships::EXAMPLE_CONTROLLER.to_vec());
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
fn travel_order_runs_in_stock_wasm_and_brakes_at_destination() {
    let (mut app, ship, account) = fixture();
    let world = app.world_mut();
    let destination = world
        .get::<PreciseTransform>(ship)
        .unwrap()
        .translation_um
        .offset_by(DVec3::NEG_Z * 100.);
    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let connection = session::connect(world, account).unwrap();
    let epoch = world.resource::<identity::WorldEpoch>().0;
    session::input(
        world,
        connection,
        InputFrame {
            world: epoch,
            sequence: 1,
            acknowledged_event: 0,
            acknowledged_frame: 0,
            actions: vec![(
                Id::new(),
                Action::Ship {
                    ship: ship_id,
                    authority_revision: 1,
                    command: ShipCommand::SetTravel {
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
        BeaconEmitter,
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
        revision: 1,
        orders: vec![Order::Dock(station_id)],
        legs: vec![Leg::Dock(station_id)],
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
