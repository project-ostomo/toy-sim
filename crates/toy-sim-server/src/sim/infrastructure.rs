use super::{hardware, identity, physics, precision, registry, spatial, travel, vessel};
use anyhow::Result;
use bevy::{math::DVec3, prelude::*};
use std::sync::Arc;
use toy_sim_model::*;

#[derive(Component)]
pub struct Landmark {
    pub system: Id,
    pub name: String,
}

#[derive(Component)]
pub struct GateOrbit {
    body: String,
    offset: DVec3,
    axis: DVec3,
    rate: f64,
    epoch_seconds: f64,
}

pub fn move_gates(
    universe: Res<super::orrery::Universe>,
    time: Res<Time<Fixed>>,
    mut gates: Query<(
        &GateOrbit,
        &mut precision::PreciseTransform,
        &mut physics::Velocity,
    )>,
) {
    let epoch = physics::sim_time(&time);
    for (orbit, mut pose, mut velocity) in &mut gates {
        let angle = orbit.rate * (time.elapsed_secs_f64() - orbit.epoch_seconds);
        let offset = bevy::math::DQuat::from_axis_angle(orbit.axis, angle) * orbit.offset;
        if let (Some(position), Some(motion)) = (
            universe.solve_position(&orbit.body, epoch),
            universe.solve_velocity(&orbit.body, epoch),
        ) {
            pose.translation_um = position.offset_by(offset);
            velocity.0 = motion + orbit.axis.cross(offset) * orbit.rate;
        }
    }
}

pub fn spawn(world: &mut World, player: Entity) -> Result<()> {
    let owner = Id::new();
    let player_pose = *world.get::<precision::PreciseTransform>(player).unwrap();
    let velocity = world.get::<physics::Velocity>(player).unwrap().0;
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
    let design = toy_sim_ships::ShipBlueprint::from_bytes(include_bytes!(
        "../../../../assets/ships/neris-anchorage.ship"
    ))?
    .compile(&world.resource::<vessel::ShipCatalogue>().0)?;
    let station_pose = precision::PreciseTransform {
        translation_um: player_pose
            .translation_um
            .offset_by(player_pose.rotation * DVec3::new(0., 0., -5000.)),
        rotation: player_pose.rotation,
    };
    let station = vessel::spawn_ship(
        world,
        Arc::new(design),
        station_pose,
        velocity,
        "Neris Anchorage".into(),
    )?;
    identity::attach_ship(world, station, owner)?;
    world.entity_mut(station).insert((
        identity::BeaconEmitter,
        Landmark {
            system: registry::system_identity("Helion system"),
            name: "Neris Anchorage".into(),
        },
    ));
    world
        .get_mut::<identity::Transponder>(station)
        .unwrap()
        .0
        .labels
        .insert("Neris Anchorage".into());
    let design = &world.get::<vessel::ShipDesign>(station).unwrap().0;
    let bays = design
        .parts
        .iter()
        .filter_map(|part| {
            let toy_sim_ships::Equipment::Utility {
                utility:
                    toy_sim_ships::utilities::UtilityDef::Docking {
                        radius_m,
                        mass_capacity_kg,
                    },
            } = part.definition.equipment
            else {
                return None;
            };
            Some(travel::Bay {
                centre_m: (part.centre - design.centre).to_array(),
                rotation: bevy::math::DQuat::from_mat3(&part.rotation).to_array(),
                radius_m,
                mass_capacity_kg,
                public: true,
                allowed: Default::default(),
                reservation: None,
            })
        })
        .collect();
    world.entity_mut(station).insert(travel::DockingBays(bays));
    let resource = world
        .resource::<vessel::ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|r| r.id == "repair_material");
    if let Some(resource) = resource {
        world
            .get_mut::<hardware::ShipInventory>(station)
            .unwrap()
            .0
            .cargo[resource] = 20000;
        world
            .get_mut::<hardware::ShipInventory>(player)
            .unwrap()
            .0
            .cargo[resource] = 100;
    }

    let links = [(0, 1), (1, 2), (2, 3), (3, 0), (0, 2)];
    for (link, (a, b)) in links.into_iter().enumerate() {
        if a >= universe.systems.len() || b >= universe.systems.len() {
            continue;
        }
        let ids = [Id::new(), Id::new()];
        for (side, index) in [a, b].into_iter().enumerate() {
            let Some(system) = universe.systems.get(index) else {
                continue;
            };
            let remote = &universe.systems[if side == 0 { b } else { a }];
            let position = if index == 0 {
                player_pose.translation_um.offset_by(DVec3::new(
                    20000. + link as f64 * 5000.,
                    0.,
                    -10000.,
                ))
            } else {
                system
                    .solver
                    .anchor
                    .offset_by(DVec3::new(1.5e11, link as f64 * 20000., 0.))
            };
            let epoch = physics::sim_time(world.resource::<Time<Fixed>>());
            let reference = if index == 0 {
                super::scenario::INITIAL_SCENARIO.body
            } else {
                system.star_name.as_str()
            };
            let body = universe.get_body(reference).unwrap();
            let centre = universe.solve_position(reference, epoch).unwrap();
            let body_velocity = universe.solve_velocity(reference, epoch).unwrap();
            let offset = position.relative_to(centre);
            let axis = if index == 0 {
                offset.cross(velocity - body_velocity).normalize()
            } else {
                DVec3::Z
            };
            let rate =
                (physics::GRAVITATIONAL_CONSTANT * body.mass / offset.length().powi(3)).sqrt();
            let orbit = GateOrbit {
                body: reference.into(),
                offset,
                axis,
                rate,
                epoch_seconds: world.resource::<Time<Fixed>>().elapsed_secs_f64(),
            };
            let name = format!("{} gate", remote.solver.name);
            let entity = world
                .spawn((
                    precision::PreciseTransform {
                        translation_um: position,
                        ..default()
                    },
                    identity::Control {
                        account: owner,
                        revision: 1,
                    },
                    identity::Transponder(IffIdentity {
                        owner,
                        faction: None,
                        labels: [name.clone()].into(),
                        enabled: true,
                        range_m: 1e12,
                    }),
                    identity::BeaconEmitter,
                    identity::FixedBeacon,
                    physics::Velocity(body_velocity + axis.cross(offset) * rate),
                    orbit,
                    spatial::SpatialBody {
                        radius_m: 256.,
                        occludes: false,
                    },
                    travel::Gate {
                        paired: ids[1 - side],
                        radius_m: 220.,
                        exclusion_m: 1e7,
                        enabled: true,
                        public: true,
                        allowed: Default::default(),
                    },
                    Landmark {
                        system: registry::system_identity(&system.solver.name),
                        name,
                    },
                ))
                .id();
            identity::register(world, entity, ids[side]);
        }
    }
    Ok(())
}

pub fn catalogue(world: &mut World) -> NavigationCatalogue {
    let systems: Vec<_> = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .systems
        .iter()
        .map(|system| NavigationSystem {
            id: registry::system_identity(&system.solver.name),
            name: system.solver.name.to_string(),
            position: system.solver.anchor,
        })
        .collect();
    let mut beacons: Vec<_> = world
        .query_filtered::<(
            &identity::Identity,
            Option<&Landmark>,
            &identity::Transponder,
            &precision::PreciseTransform,
            Option<&physics::Velocity>,
            Option<&physics::AngularVelocity>,
            &spatial::SpatialBody,
            Option<&travel::Gate>,
            Option<&travel::DockingBays>,
        ), (With<identity::BeaconEmitter>, Without<travel::Dormant>)>()
        .iter(world)
        .map(
            |(id, landmark, iff, pose, velocity, angular, spatial, gate, bays)| NavigationBeacon {
                id: id.0,
                system: landmark.map_or_else(
                    || {
                        systems
                            .iter()
                            .min_by(|a, b| {
                                a.position
                                    .relative_to(pose.translation_um)
                                    .length_squared()
                                    .total_cmp(
                                        &b.position
                                            .relative_to(pose.translation_um)
                                            .length_squared(),
                                    )
                            })
                            .unwrap()
                            .id
                    },
                    |landmark| landmark.system,
                ),
                name: landmark.map_or_else(
                    || {
                        iff.0
                            .labels
                            .iter()
                            .next()
                            .cloned()
                            .unwrap_or_else(|| "Station beacon".into())
                    },
                    |landmark| landmark.name.clone(),
                ),
                pose: super::intelligence::pose(pose, velocity, angular),
                radius_m: gate.map_or(spatial.radius_m, |g| g.radius_m),
                gate_exit: gate.filter(|g| g.enabled && g.public).map(|g| g.paired),
                docking: bays.is_some(),
            },
        )
        .collect();
    beacons.sort_by_key(|beacon| beacon.id);
    NavigationCatalogue { systems, beacons }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_network_is_connected_and_fixed_gates_have_no_physics_bodies() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let catalogue = catalogue(world);
        assert_eq!(catalogue.systems.len(), 4);
        assert_eq!(
            catalogue
                .beacons
                .iter()
                .filter(|b| b.gate_exit.is_some())
                .count(),
            10
        );
        for system in &catalogue.systems {
            assert!(
                toy_sim_model::navigation::gate_route(
                    &catalogue,
                    catalogue.systems[0].id,
                    system.id
                )
                .is_some()
            );
        }
        let gates: Vec<_> = world
            .query_filtered::<Entity, With<travel::Gate>>()
            .iter(world)
            .collect();
        for gate in gates {
            assert!(world.get::<physics::RigidBody>(gate).is_none());
            assert!(world.get::<physics::AccumulatedForce>(gate).is_none());
            assert!(world.get::<GateOrbit>(gate).is_some());
        }
        let station = catalogue.beacons.iter().find(|b| b.docking).unwrap();
        let station = identity::lookup(world, station.id).unwrap();
        assert!(world.get::<travel::DockingBays>(station).unwrap().0[0].public);
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
        travel::geometry::refresh(world);
        let destination = private.position.offset_by(DVec3::Z * 1000.);
        world.get_mut::<travel::Travel>(player).unwrap().0 = toy_sim_model::travel::TravelState {
            revision: 1,
            orders: vec![toy_sim_model::travel::Order::TravelTo(
                toy_sim_model::travel::Destination::Galactic(destination),
            )],
            status: toy_sim_model::travel::Status::Planning,
            ..default()
        };
        travel::advance(world);
        assert!(world.get::<physics::RigidBody>(player).is_some());
        assert_eq!(world.get::<travel::Travel>(player).unwrap().0.order, 0);
        assert!(world.get::<travel::DockedIn>(player).is_none());
    }
    #[test]
    fn stock_computer_executes_a_queued_gate_jump() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let player = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let entry = world
            .query_filtered::<Entity, With<travel::Gate>>()
            .iter(world)
            .find(|entity| {
                world.get::<Landmark>(*entity).unwrap().system
                    == registry::system_identity("Helion system")
            })
            .unwrap();
        let exit =
            identity::lookup(world, world.get::<travel::Gate>(entry).unwrap().paired).unwrap();
        let entry_id = world.get::<identity::Identity>(entry).unwrap().0;
        let pose = *world.get::<precision::PreciseTransform>(entry).unwrap();
        let velocity = world.get::<physics::Velocity>(entry).unwrap().0;
        world.entity_mut(player).insert((
            pose,
            physics::Velocity(velocity),
            physics::AngularVelocity(DVec3::ZERO),
            travel::Travel(toy_sim_model::travel::TravelState {
                revision: 1,
                orders: vec![toy_sim_model::travel::Order::Jump(entry_id)],
                status: toy_sim_model::travel::Status::Planning,
                ..default()
            }),
        ));
        for _ in 0..1500 {
            app.update();
            if app.world().get::<travel::Travel>(player).unwrap().0.status
                == toy_sim_model::travel::Status::Completed
            {
                let world = app.world();
                let position = world
                    .get::<precision::PreciseTransform>(player)
                    .unwrap()
                    .translation_um;
                let mouth = world
                    .get::<precision::PreciseTransform>(exit)
                    .unwrap()
                    .translation_um;
                assert!(position.relative_to(mouth).length() < 220.);
                return;
            }
        }
        panic!(
            "gate jump did not complete: {:?}",
            app.world().get::<travel::Travel>(player).unwrap().0
        );
    }

    #[test]
    fn stock_computer_guides_through_the_hangar_mouth_and_docks() {
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
        let hostile: Vec<_> = world
            .query_filtered::<Entity, (
                With<vessel::Vessel>,
                Without<Landmark>,
                Without<vessel::ControlledVessel>,
            )>()
            .iter(world)
            .collect();
        for ship in hostile {
            world.despawn(ship);
        }
        let station_id = world.get::<identity::Identity>(station).unwrap().0;
        let berth = travel::reserve_bay(world, player, station, 0).unwrap();
        let rotation = bevy::math::DQuat::from_array(berth.rotation);
        let clearance = world.get::<vessel::ShipDesign>(station).unwrap().0.radius
            + world.get::<vessel::ShipDesign>(player).unwrap().0.radius
            + 35.;
        let position = berth
            .position
            .offset_by(rotation * DVec3::NEG_Z * clearance);
        world.entity_mut(player).insert((
            precision::PreciseTransform {
                translation_um: position,
                rotation,
            },
            physics::Velocity(DVec3::from_array(berth.velocity)),
            physics::AngularVelocity(DVec3::ZERO),
            travel::Travel(toy_sim_model::travel::TravelState {
                revision: 1,
                orders: vec![toy_sim_model::travel::Order::Dock(station_id)],
                status: toy_sim_model::travel::Status::Planning,
                ..default()
            }),
        ));
        for _ in 0..3000 {
            app.update();
            if matches!(
                app.world().get::<travel::PresenceState>(player).unwrap().0,
                toy_sim_model::travel::Presence::Docked { .. }
            ) {
                return;
            }
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
        hardware::utilities::transfer_cargo(world, player, other, "repair_material", 20).unwrap();
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
        assert!(hardware::utilities::transfer_cargo(world, player, other, "water", 1).is_err());
    }
}
