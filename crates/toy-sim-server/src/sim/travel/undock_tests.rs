use super::*;
use crate::sim::{hardware::ShipThermal, ownership::AssetOwner, spatial::SpatialBody};
use std::sync::Arc;
use toy_sim_model::ownership::Principal;

#[test]
fn conservative_radius_contains_voxel_and_shield_geometry_for_dormant_ships() {
    let catalogue = toy_sim_ships::Catalogue::builtin();
    let mut world = World::new();
    for blueprint in [
        toy_sim_ships::armed_starter(),
        toy_sim_ships::missiles::blueprint(),
    ] {
        let design = Arc::new(blueprint.compile(&catalogue).unwrap());
        let actual = super::super::physics::collision::Geometry::ship(&design).shield_radius;
        let ship = world.spawn((ShipDesign(design), Dormant)).id();
        assert!(collision_radius(&world, ship).unwrap() >= actual);
    }
}

#[test]
fn departure_composes_bay_rotation_and_inherits_motion_at_large_coordinates() {
    let host = Pose {
        position: GalacticPosition {
            x: 1_i128 << 100,
            y: -(1_i128 << 95),
            z: 0,
        },
        rotation: DQuat::from_rotation_y(0.7).to_array(),
        velocity: [100_000., -20_000., 500.],
        angular_velocity: [0., 0.02, 0.],
    };
    let bay = DQuat::from_rotation_x(0.4);
    let departure = departure_pose(&host, bay.to_array(), 1100., 15.);
    let rotation = DQuat::from_array(host.rotation) * bay;
    let expected_offset = rotation * DVec3::NEG_Z * 1125.;
    let actual_offset = departure.position.relative_to(host.position);
    assert!((actual_offset - expected_offset).length() < 2e-6);
    assert!(DQuat::from_array(departure.rotation).angle_between(rotation) < 1e-8);
    assert!(
        (DVec3::from_array(departure.velocity)
            - DVec3::from_array(host.velocity)
            - DVec3::from_array(host.angular_velocity).cross(expected_offset))
        .length()
            < 1e-8
    );
}

#[test]
fn actual_undock_uses_the_shared_pose_and_clears_station_shield_envelope() {
    let mut world = World::new();
    world.init_resource::<identity::IdentityIndex>();
    world.init_resource::<SimulationCounters>();
    let account = Id::new();
    let catalogue = toy_sim_ships::Catalogue::builtin();
    let station_design = Arc::new(
        toy_sim_ships::missiles::missile_defense_station()
            .compile(&catalogue)
            .unwrap(),
    );
    let child_design = Arc::new(toy_sim_ships::armed_starter().compile(&catalogue).unwrap());
    let station_radius = station_design.radius;
    let child_radius = child_design.radius;
    let mut thermal = toy_sim_ships::ShipState::new(&station_design, &catalogue).thermal;
    thermal.shield_enabled = true;
    thermal.shield_powered = true;
    thermal.shield_state = toy_sim_ship_api::abi::SHIELD_ACTIVE;
    let host = world
        .spawn((
            ShipDesign(station_design),
            ShipThermal(thermal),
            PreciseTransform {
                translation_um: GalacticPosition::ZERO,
                rotation: DQuat::from_rotation_y(0.6),
            },
            Velocity(DVec3::X * 700.),
            AngularVelocity(DVec3::Y * 0.01),
            MassProps {
                mass: 10_000.,
                ..Default::default()
            },
            StoredMass(1000.),
            PresenceState::default(),
            SpatialBody {
                radius_m: station_radius,
                occludes: true,
            },
            DockingBays(vec![Bay {
                centre_m: [0., 0., 0.],
                rotation: DQuat::from_rotation_x(0.3).to_array(),
                radius_m: 100.,
                mass_capacity_kg: 1e9,
                public: true,
                allowed: Default::default(),
                reservation: None,
            }]),
        ))
        .id();
    let host_id = Id::new();
    identity::register(&mut world, host, host_id);
    let child = world
        .spawn((
            ShipDesign(child_design),
            AssetOwner(Principal::Player(account)),
            PreciseTransform::default(),
            Velocity(DVec3::ZERO),
            AngularVelocity(DVec3::ZERO),
            MassProps {
                mass: 1000.,
                ..Default::default()
            },
            SpatialBody {
                radius_m: child_radius,
                occludes: true,
            },
            DockedIn(host),
        ))
        .id();
    identity::register(&mut world, child, Id::new());
    set_dormant(
        &mut world,
        child,
        Presence::Docked {
            host: host_id,
            bay: 0,
        },
    );
    assert!(world.get::<SpatialBody>(child).is_none());

    let expected = undock_pose(&world, child, host, 0).unwrap();
    let safe_separation =
        collision_radius(&world, host).unwrap() + collision_radius(&world, child).unwrap();
    undock(&mut world, child).unwrap();
    let actual = ship_pose(&world, child).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.position.relative_to(GalacticPosition::ZERO).length() > safe_separation + 9.999);
    assert!(world.get::<Dormant>(child).is_none());
    assert!(world.get::<SpatialBody>(child).is_some());
}
