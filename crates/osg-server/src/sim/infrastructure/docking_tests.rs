use super::*;
use std::time::{Duration, Instant};

fn finish_route_planning(world: &mut World, ship: Entity) -> osg_model::travel::TravelState {
    use osg_model::travel::Status;

    let started = Instant::now();
    let deadline = started + Duration::from_secs(30);
    loop {
        travel::plan_orders(world);
        super::super::route_service::advance(world);
        travel::plan_orders(world);

        let state = world.get::<travel::Travel>(ship).unwrap();
        match &state.0.status {
            Status::Planning => {
                assert!(state.0.planning.is_some(), "planning must report progress");
            }
            Status::Active => {
                eprintln!("server route completed in {:?}", started.elapsed());
                return state.0.clone();
            }
            status => panic!("route planning failed: {status:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "route worker did not finish within 30 seconds: {:?}",
            state.0.planning
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn docking_removes_physics_and_private_pose_follows_the_station() {
    let account = Id::new();
    let mut app = super::super::provision(&[account], None, None).unwrap();
    app.update();
    let world = app.world_mut();
    let player = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let station = world
        .query_filtered::<Entity, With<travel::DockingBays>>()
        .single(world)
        .unwrap();
    let berth = travel::reserve_bay(world, player, station, 0).unwrap();
    world.entity_mut(player).insert((
        precision::PreciseTransform {
            translation_um: berth.position,
            rotation: bevy::math::DQuat::from_array(berth.rotation),
        },
        physics::Velocity(DVec3::from_array(berth.velocity)),
        physics::AngularVelocity(DVec3::ZERO),
    ));
    travel::dock(world, player, station, 0).unwrap();
    assert!(world.get::<physics::RigidBody>(player).is_none());
    world
        .get_mut::<precision::PreciseTransform>(station)
        .unwrap()
        .translation_um = world
        .get::<precision::PreciseTransform>(station)
        .unwrap()
        .translation_um
        .offset_by(DVec3::X * 1000.);
    let private = super::super::session::ship_pose(world, player).unwrap();
    assert!((private.position.relative_to(berth.position) - DVec3::X * 1000.).length() < 0.001);
    crate::sim::spatial::rebuild(world);
    let destination = private.position.offset_by(DVec3::Z * 1000.);
    world.get_mut::<travel::Travel>(player).unwrap().0 = osg_model::travel::TravelState {
        autopilot_enabled: true,
        revision: 1,
        goals: vec![osg_model::travel::Order::TravelTo(
            osg_model::travel::Destination::Galactic(destination),
        )],
        orders: vec![osg_model::travel::Order::TravelTo(
            osg_model::travel::Destination::Galactic(destination),
        )]
        .into_iter()
        .map(Into::into)
        .collect(),
        status: osg_model::travel::Status::Planning,
        ..default()
    };
    travel::advance(world);
    assert!(world.get::<physics::RigidBody>(player).is_none());
    assert!(world.get::<travel::DockedIn>(player).is_some());

    let planned = finish_route_planning(world, player);
    assert!(matches!(
        planned.orders[0].action,
        osg_model::travel::Order::Undock
    ));
    assert!(world.get::<physics::RigidBody>(player).is_none());
    travel::advance(world);

    assert!(world.get::<physics::RigidBody>(player).is_some());
    assert!(
        world
            .get::<physics::collision::CollisionBody>(player)
            .is_some()
    );
    assert_eq!(world.get::<travel::Travel>(player).unwrap().0.order, 1);
    assert!(world.get::<travel::DockedIn>(player).is_none());
    assert!(matches!(
        world.get::<travel::PresenceState>(player).unwrap().0,
        osg_model::travel::Presence::Space
    ));
}

#[test]
fn stock_computer_docks_from_default_spawn_without_entering_station() {
    let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
    app.update();
    let world = app.world_mut();
    let player = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let station = world
        .query_filtered::<Entity, With<travel::DockingBays>>()
        .single(world)
        .unwrap();
    let unrelated: Vec<_> = world
        .query_filtered::<Entity, With<vessel::Vessel>>()
        .iter(world)
        .filter(|entity| *entity != player && *entity != station)
        .collect();
    for ship in unrelated {
        world.despawn(ship);
    }
    let station_id = world.get::<identity::Identity>(station).unwrap().0;
    world
        .entity_mut(player)
        .insert((travel::Travel(osg_model::travel::TravelState {
            autopilot_enabled: true,
            revision: 1,
            goals: vec![osg_model::travel::Order::Dock(station_id)],
            orders: vec![osg_model::travel::Order::Dock(station_id)]
                .into_iter()
                .map(Into::into)
                .collect(),
            status: osg_model::travel::Status::Planning,
            ..default()
        }),));
    let minimum_distance = world.get::<vessel::ShipDesign>(station).unwrap().0.radius
        + world.get::<vessel::ShipDesign>(player).unwrap().0.radius;
    for _ in 0..3000 {
        app.update();
        if matches!(
            app.world().get::<travel::PresenceState>(player).unwrap().0,
            osg_model::travel::Presence::Docked { .. }
        ) {
            return;
        }
        let world = app.world();
        let player_pose = super::super::session::ship_pose(world, player).unwrap();
        let station_pose = super::super::session::ship_pose(world, station).unwrap();
        assert!(
            player_pose
                .position
                .relative_to(station_pose.position)
                .length()
                >= minimum_distance,
            "autopilot entered station collision envelope: player {player_pose:?}, station {station_pose:?}, travel {:?}, navigation {:?}",
            world.get::<travel::Travel>(player).unwrap().0,
            world
                .get::<vessel::ShipSoftware>(player)
                .unwrap()
                .controller
                .state
                .navigation,
        );
    }
    let world = app.world();
    let software = world.get::<vessel::ShipSoftware>(player).unwrap();
    panic!(
        "did not dock: {:?}, fault {:?}, navigation {:?}, hull {}, delta {:?}",
        world.get::<travel::Travel>(player).unwrap().0,
        software.controller.fault,
        software.controller.state.navigation,
        world.get::<hardware::Hull>(player).unwrap().0,
        super::super::session::ship_pose(world, player)
            .unwrap()
            .position
            .relative_to(
                super::super::session::ship_pose(world, station)
                    .unwrap()
                    .position
            )
    );
}

#[test]
fn docked_inventory_accepts_multiple_ships_and_transfers_only_cargo() {
    let account = Id::new();
    let mut app = super::super::provision(&[account], None, None).unwrap();
    app.update();
    let world = app.world_mut();
    let player = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let station = world
        .query_filtered::<Entity, With<travel::DockingBays>>()
        .single(world)
        .unwrap();
    let design = world.get::<vessel::ShipDesign>(player).unwrap().0.clone();
    let other = vessel::spawn_ship(
        world,
        design,
        precision::PreciseTransform::default(),
        DVec3::ZERO,
        "Docked tender".into(),
    )
    .unwrap();
    identity::attach_ship(world, other, account).unwrap();
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let hold = world
        .get::<vessel::ShipDesign>(player)
        .unwrap()
        .0
        .capacity_m3;
    world
        .get_mut::<hardware::ShipInventory>(player)
        .unwrap()
        .0
        .insert_item(
            &osg_model::industry::CargoItem::Resource("rocket_propellant".into()),
            20,
            hold,
            &catalogue,
        )
        .unwrap();
    crate::sim::industry::synchronize_mass(world, &[player]);
    for ship in [player, other] {
        let berth = travel::reserve_bay(world, ship, station, 0).unwrap();
        world.entity_mut(ship).insert((
            precision::PreciseTransform {
                translation_um: berth.position,
                rotation: bevy::math::DQuat::from_array(berth.rotation),
            },
            physics::Velocity(DVec3::from_array(berth.velocity)),
            physics::AngularVelocity(DVec3::ZERO),
        ));
        travel::dock(world, ship, station, 0).unwrap();
        assert!(world.get::<physics::RigidBody>(ship).is_none());
    }
    assert_eq!(world.get::<travel::StoredShips>(station).unwrap().len(), 2);
    let tanks = world
        .get::<hardware::ShipInventory>(player)
        .unwrap()
        .0
        .quantities
        .clone();
    let stored_mass = world.get::<travel::StoredMass>(station).unwrap().0;
    crate::sim::industry::transfer(
        world,
        account,
        player,
        other,
        osg_model::industry::CargoItem::Resource("rocket_propellant".into()),
        20,
    )
    .unwrap();
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(player)
            .unwrap()
            .0
            .quantities,
        tanks
    );
    assert_eq!(
        world.get::<travel::StoredMass>(station).unwrap().0,
        stored_mass
    );
    assert!(
        crate::sim::industry::transfer(
            world,
            account,
            player,
            other,
            osg_model::industry::CargoItem::Resource("water".into()),
            1
        )
        .is_err()
    );
}
