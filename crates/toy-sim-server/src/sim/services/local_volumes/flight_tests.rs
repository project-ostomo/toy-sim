use crate::sim::{
    self, hardware, identity, infrastructure, physics, precision, registry, services, travel,
    vessel,
};
use bevy::{math::DVec3, prelude::*};
use toy_sim_model::{
    Id,
    travel::{GATE_ENTRY_SPEED_M_S, Order, Status, TravelState},
};

#[test]
fn stock_computer_returns_through_a_solar_gate_with_complete_local_geometry() {
    let mut app = sim::provision(&[Id::new()], None, None).unwrap();
    let world = app.world_mut();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let solar_system = registry::system_identity("Sol");
    let helion_system = registry::system_identity("Helion system");
    let entry = world
        .query::<(Entity, &travel::Gate, &infrastructure::Landmark)>()
        .iter(world)
        .find(|(_, gate, landmark)| {
            landmark.system == solar_system
                && identity::lookup(world, gate.paired)
                    .ok()
                    .and_then(|exit| world.get::<infrastructure::Landmark>(exit))
                    .is_some_and(|exit| exit.system == helion_system)
        })
        .map(|(entity, _, _)| entity)
        .unwrap();
    let entry_id = world.get::<identity::Identity>(entry).unwrap().0;
    let exit = identity::lookup(world, world.get::<travel::Gate>(entry).unwrap().paired).unwrap();
    let gate_pose = *world.get::<precision::PreciseTransform>(entry).unwrap();
    let gate_velocity = world.get::<physics::Velocity>(entry).unwrap().0;
    assert!(gate_velocity.length() > 10_000.);

    world
        .get_mut::<precision::PreciseTransform>(ship)
        .unwrap()
        .translation_um = gate_pose
        .translation_um
        .offset_by(DVec3::new(41., 0., -1221.));
    world.get_mut::<physics::Velocity>(ship).unwrap().0 =
        gate_velocity + DVec3::new(0.016, 0., -0.5);
    world.entity_mut(ship).remove::<physics::WithinSoi>();
    world.entity_mut(ship).insert(travel::Travel(TravelState {
        autopilot_enabled: true,
        revision: 1,
        orders: vec![Order::Jump(entry_id).into()],
        status: Status::Active,
        ..Default::default()
    }));
    travel::geometry::update(world, ship);

    app.update();
    let world = app.world_mut();
    let source = services::current_fused_source(world, ship).unwrap();
    let beacon = source.orbital.beacons.get(&entry_id).unwrap();
    let pose = source.orbital_pose(&beacon.beacon.pose, beacon.orbit.as_deref());
    let reply = source
        .orrery(pose.position.offset_by(DVec3::NEG_Z * 333.))
        .unwrap();
    assert!(reply.iter().any(|obstacle| {
        obstacle.reference
            == toy_sim_model::travel::Target::Destination(
                toy_sim_model::travel::Destination::Beacon(entry_id),
            )
    }));

    for elapsed in 0..3000 {
        app.update();
        let world = app.world();
        let status = &world.get::<travel::Travel>(ship).unwrap().0.status;
        assert!(
            !matches!(status, Status::Blocked(_)),
            "gate command blocked: {status:?}"
        );
        if *status == Status::Completed {
            let position = world
                .get::<precision::PreciseTransform>(ship)
                .unwrap()
                .translation_um;
            let exit_position = world
                .get::<precision::PreciseTransform>(exit)
                .unwrap()
                .translation_um;
            let separation = position.relative_to(exit_position).length();
            let velocity = world.get::<physics::Velocity>(ship).unwrap().0
                - world.get::<physics::Velocity>(exit).unwrap().0;
            assert!(separation > world.get::<travel::Gate>(exit).unwrap().radius_m);
            assert!(separation < 1000.);
            assert!(velocity.length() <= GATE_ENTRY_SPEED_M_S);
            assert!(world.get::<hardware::Hull>(ship).unwrap().0 > 0.);
            eprintln!(
                "solar return completed after {elapsed} ticks; exit separation={separation:.3} m, relative speed={:.3} m/s",
                velocity.length()
            );
            return;
        }
    }
    panic!(
        "solar return gate stalled: {:?}",
        app.world().get::<travel::Travel>(ship).unwrap().0
    );
}
