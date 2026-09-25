use super::*;

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
    travel::advance(world);
    assert!(world.get::<physics::RigidBody>(player).is_none());
    assert!(world.get::<travel::DockedIn>(player).is_some());

    travel::dispatch(world, player, osg_model::ProgramAction::Undock).unwrap();

    assert!(world.get::<physics::RigidBody>(player).is_some());
    assert!(
        world
            .get::<physics::collision::CollisionBody>(player)
            .is_some()
    );
    assert!(world.get::<travel::DockedIn>(player).is_none());
    assert!(matches!(
        world.get::<travel::PresenceState>(player).unwrap().0,
        osg_model::travel::Presence::Space
    ));
}

#[test]
fn stock_computer_docks_at_local_station_several_megameters_away() {
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
    let unrelated: Vec<_> = world
        .query_filtered::<Entity, With<vessel::Vessel>>()
        .iter(world)
        .filter(|entity| *entity != player && *entity != station)
        .collect();
    for ship in unrelated {
        world.despawn(ship);
    }
    let player_pose = *world.get::<precision::PreciseTransform>(player).unwrap();
    world
        .get_mut::<precision::PreciseTransform>(station)
        .unwrap()
        .translation_um = player_pose
        .translation_um
        .offset_by(player_pose.rotation * DVec3::NEG_Z * 7.85e6);
    let ship_id = world.get::<identity::Identity>(player).unwrap().0;
    let station_id = world.get::<identity::Identity>(station).unwrap().0;
    let revision = world.get::<identity::Control>(player).unwrap().revision;
    super::super::commands::execute(
        world,
        account,
        ship_id,
        revision,
        osg_model::ShipCommand::SetItinerary {
            preferences: Default::default(),
            engage: true,
            expected_revision: world
                .get::<travel::Travel>(player)
                .unwrap()
                .0
                .directive_revision,
            itinerary: vec![osg_model::travel::Directive::DockAt(station_id)],
        },
    )
    .unwrap();

    let mut applied_thrust = false;
    for tick in 0..20_000 {
        app.update();
        let world = app.world();
        let state = &world.get::<travel::Travel>(player).unwrap().0;
        assert!(state.failure.is_none(), "docking failed: {state:?}");
        if tick > 300 {
            let computer = &world
                .get::<vessel::ShipSoftware>(player)
                .unwrap()
                .controller;
            let now = world
                .resource::<super::super::simulation::SimulationCounters>()
                .ticks as f64
                * osg_model::TICK_SECONDS;
            assert!(
                computer
                    .state
                    .navigation
                    .as_ref()
                    .is_some_and(|navigation| navigation.valid_until_s >= now),
                "planning starved control publication at {tick}: {:?}",
                computer.state.navigation
            );
        }
        applied_thrust |= world
            .get::<hardware::propulsion::ActuatorOutput>(player)
            .is_some_and(|output| output.force.length() > 1.);
        if tick % 2_000 == 0 {
            let ship_pose = super::super::session::ship_pose(world, player).unwrap();
            let station_pose = super::super::session::ship_pose(world, station).unwrap();
            eprintln!(
                "rendezvous {tick}: distance {:.0} m, speed {:.1} m/s, {:?}",
                ship_pose
                    .position
                    .relative_to(station_pose.position)
                    .length(),
                (DVec3::from_array(ship_pose.velocity) - DVec3::from_array(station_pose.velocity))
                    .length(),
                world
                    .get::<vessel::ShipSoftware>(player)
                    .unwrap()
                    .controller
                    .state
                    .navigation
            );
        }
        if tick == 300 {
            assert!(
                applied_thrust,
                "local rendezvous never applied thrust: {state:?}"
            );
            assert!(
                !matches!(
                    state.status.phase,
                    osg_model::travel::FirmwarePhase::Planning
                ),
                "local rendezvous is still searching capture bodies: {state:?}"
            );
        }
        if matches!(world.get::<travel::PresenceState>(player).unwrap().0,
            osg_model::travel::Presence::Docked { host, .. } if host == station_id)
        {
            return;
        }
    }
    let world = app.world();
    let distance = super::super::session::ship_pose(world, player)
        .unwrap()
        .position
        .relative_to(
            super::super::session::ship_pose(world, station)
                .unwrap()
                .position,
        )
        .length();
    panic!(
        "local rendezvous did not dock within 2,000 seconds: distance {distance}, {:?}",
        world.get::<travel::Travel>(player).unwrap().0
    );
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
        .insert((travel::Travel(osg_model::travel::AutopilotState {
            enabled: true,
            directive_revision: 1,
            itinerary: vec![osg_model::travel::ItineraryEntry {
                directive: osg_model::travel::Directive::DockAt(station_id),
                label: "Dock at local station".into(),
            }],
            ..default()
        }),));
    let player_shape = crate::sim::physics::collision::Geometry::ship(
        &world.get::<vessel::ShipDesign>(player).unwrap().0,
    )
    .surface;
    let station_shape = crate::sim::physics::collision::Geometry::ship(
        &world.get::<vessel::ShipDesign>(station).unwrap().0,
    )
    .surface;
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
        let intersects = rapier3d_f64::parry::query::intersection_test(
            &crate::sim::physics::collision::pose(
                player_pose.position.relative_to(station_pose.position),
                bevy::math::DQuat::from_array(player_pose.rotation),
            ),
            player_shape.as_ref(),
            &crate::sim::physics::collision::pose(
                DVec3::ZERO,
                bevy::math::DQuat::from_array(station_pose.rotation),
            ),
            station_shape.as_ref(),
        )
        .unwrap();
        assert!(
            !intersects,
            "autopilot entered station hull: player {player_pose:?}, station {station_pose:?}, travel {:?}, navigation {:?}",
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
    identity::attach_ship(world, other, account, osg_model::Id::new()).unwrap();
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
    crate::sim::hardware::synchronize_mass(world, &[player]);
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
    let source = world.get::<identity::Identity>(player).unwrap().0;
    let target = world.get::<identity::Identity>(other).unwrap().0;
    crate::sim::industry::enqueue_command(
        world,
        account,
        osg_model::industry::IndustryCommand::Transfer {
            source,
            target,
            item: osg_model::industry::CargoItem::Resource("rocket_propellant".into()),
            quantity: 20,
        },
        None,
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
        crate::sim::industry::enqueue_command(
            world,
            account,
            osg_model::industry::IndustryCommand::Transfer {
                source,
                target,
                item: osg_model::industry::CargoItem::Resource("water".into()),
                quantity: 1,
            },
            None,
        )
        .is_err()
    );
}
