use super::*;
use crate::sim::{self, hardware, identity, session, vessel};
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
fn route_expansion_preserves_the_queue_and_rejects_stale_progress() {
    let (mut app, ship, _) = fixture();
    let world = app.world_mut();
    let destination = world.get::<PreciseTransform>(ship).unwrap().translation_um;
    let goal = Order::TravelTo(Destination::Galactic(destination));
    let expanded: Vec<QueuedOrder> = vec![
        Order::Sublight(Destination::Galactic(destination)),
        Order::Slip {
            destination: toy_sim_model::travel::Destination::Galactic(destination),
        },
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let state = TravelState {
        autopilot_enabled: true,
        revision: 7,
        orders: vec![Order::WaitUntil(0), goal, Order::WaitUntil(9999)]
            .into_iter()
            .map(Into::into)
            .collect(),
        order: 1,
        status: Status::Planning,
        ..Default::default()
    };
    world.entity_mut(ship).insert(Travel(state));
    let progress = PlanningProgress {
        stage: PlanningStage::SearchingRoutes,
        completed: 2,
        total: Some(10),
    };
    dispatch(
        world,
        ship,
        toy_sim_model::ProgramAction::PlanningProgress {
            revision: 7,
            progress,
        },
    )
    .unwrap();
    assert_eq!(
        world.get::<Travel>(ship).unwrap().0.planning,
        Some(progress)
    );
    submit_route(world, ship, 7, expanded.clone()).unwrap();
    assert!(world.get::<Travel>(ship).unwrap().0.planning.is_none());
    assert!(
        dispatch(
            world,
            ship,
            toy_sim_model::ProgramAction::PlanningProgress {
                revision: 7,
                progress,
            }
        )
        .is_err()
    );
    let queued = world.get::<Travel>(ship).unwrap().0.clone();
    assert_eq!(
        queued.orders,
        vec![
            Order::WaitUntil(0).into(),
            expanded[0].clone(),
            expanded[1].clone(),
            Order::WaitUntil(9999).into()
        ]
    );
    assert_eq!(queued.order, 1);
    assert_eq!(queued.revision, 8);
    assert!(submit_route(world, ship, 7, expanded).is_err());
    assert!(
        dispatch(
            world,
            ship,
            toy_sim_model::ProgramAction::CompleteOrder {
                revision: 7,
                order: 1
            }
        )
        .is_err()
    );
    assert_eq!(world.get::<Travel>(ship).unwrap().0, queued);
    assert!(
        dispatch(
            world,
            ship,
            toy_sim_model::ProgramAction::Estimate {
                revision: 7,
                order: 1,
                remaining_ticks: Some(100),
                fuel_budget: Default::default(),
            }
        )
        .is_err()
    );
    dispatch(
        world,
        ship,
        toy_sim_model::ProgramAction::Estimate {
            revision: 8,
            order: 1,
            remaining_ticks: Some(100),
            fuel_budget: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(
        world.get::<Travel>(ship).unwrap().0.estimated_arrival_tick,
        Some(tick(world) + 100)
    );

    dispatch(
        world,
        ship,
        toy_sim_model::ProgramAction::CompleteOrder {
            revision: 8,
            order: 1,
        },
    )
    .unwrap();
    assert_eq!(world.get::<Travel>(ship).unwrap().0.order, 2);
    assert!(
        dispatch(
            world,
            ship,
            toy_sim_model::ProgramAction::CompleteOrder {
                revision: 8,
                order: 1
            }
        )
        .is_err()
    );

    let before = world.get::<Travel>(ship).unwrap().0.clone();
    assert!(submit_route(world, ship, 8, vec![Order::Undock.into(); 256]).is_err());
    assert_eq!(world.get::<Travel>(ship).unwrap().0, before);
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
fn directional_alignment_runs_through_the_public_queue_without_translation() {
    let (mut app, ship, account) = fixture();
    let world = app.world_mut();
    let ship_id = world.get::<Identity>(ship).unwrap().0;
    let connection = session::connect(world, account).unwrap();
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
                        orders: vec![Order::Guidance(toy_sim_model::travel::Guidance {
                            mode: toy_sim_model::travel::GuidanceMode::Align,
                            target: toy_sim_model::travel::Target::Direction(DVec3::X.to_array()),
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
    let connection = session::connect(world, account).unwrap();
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
            ShipCommand::Flight(toy_sim_model::FlightCommand::AimDirection([0., 0., -1.]))
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
            ShipCommand::Flight(toy_sim_model::FlightCommand::AimDirection([1., 0., 0.]))
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

#[test]
fn fitted_slipdrive_is_selected_and_flies_a_faster_route() {
    let (mut app, old_ship, account) = fixture();
    let world = app.world_mut();
    let origin = world
        .get::<PreciseTransform>(old_ship)
        .unwrap()
        .translation_um;
    world.despawn(old_ship);
    let design = toy_sim_ships::expedition_patrol()
        .compile(&world.resource::<vessel::ShipCatalogue>().0)
        .unwrap();
    let ship = vessel::spawn_ship(
        world,
        Arc::new(design),
        PreciseTransform {
            translation_um: origin,
            rotation: DQuat::IDENTITY,
        },
        DVec3::NEG_Z * 100.,
        "Slip routing fixture".into(),
    )
    .unwrap();
    identity::attach_ship(world, ship, account).unwrap();
    let destination = origin.offset_by(DVec3::NEG_Z * LIGHT_YEAR_M);
    world.entity_mut(ship).insert(Travel(TravelState {
        autopilot_enabled: true,
        revision: 1,
        orders: vec![Order::TravelTo(Destination::Galactic(destination))]
            .into_iter()
            .map(Into::into)
            .collect(),
        status: Status::Planning,
        ..Default::default()
    }));
    let mut planned_slip = false;
    let mut transited = false;
    for _ in 0..12000 {
        app.update();
        let world = app.world();
        let travel = &world.get::<Travel>(ship).unwrap().0;
        planned_slip |= travel
            .orders
            .iter()
            .any(|leg| matches!(&leg.action, Order::Slip { .. }));
        transited |= world.get::<Transit>(ship).is_some();
        if travel.status == Status::Completed {
            assert!(planned_slip && transited);
            assert!(
                ship_pose(world, ship)
                    .unwrap()
                    .position
                    .relative_to(destination)
                    .length()
                    <= 2.
            );
            return;
        }
    }
    let world = app.world();
    panic!(
        "slip route failed: {:?}, drive {:?}, fault {:?}",
        world.get::<Travel>(ship).unwrap().0,
        world.get::<SlipDrive>(ship),
        world
            .get::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller
            .fault
    );
}
