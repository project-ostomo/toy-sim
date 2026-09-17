use super::{identity, intelligence, physics, precision, session, travel, vessel};
use anyhow::Result;
use bevy::{math::DVec3, prelude::*};
use std::{collections::BTreeSet, path::PathBuf};
use toy_sim_model::{AccountId, DebugCommand, Id};

#[derive(Resource, Clone)]
pub struct ScenarioConfig {
    pub accounts: Vec<AccountId>,
    pub debug_account: Option<AccountId>,
    pub ship: Option<PathBuf>,
}

pub fn provision(
    accounts: &[AccountId],
    debug_account: Option<AccountId>,
    ship: Option<PathBuf>,
) -> Result<App> {
    let mut app = super::application(ship.clone());
    app.insert_resource(ScenarioConfig {
        accounts: accounts.to_vec(),
        debug_account,
        ship,
    });
    app.update();
    app.update();
    let world = app.world_mut();
    let player = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)?;
    let neutral = Id::new();
    let owner = accounts.first().copied().unwrap_or(neutral);
    identity::attach_ship(world, player, owner)?;
    world
        .entity_mut(player)
        .insert(travel::SlipDrive::default());
    let design = world.get::<vessel::ShipDesign>(player).unwrap().0.clone();
    let pose = *world.get::<precision::PreciseTransform>(player).unwrap();
    let velocity = world.get::<physics::Velocity>(player).unwrap().0;
    for (index, &account) in accounts.iter().enumerate().skip(1) {
        let mut pose = pose;
        pose.translation_um = pose
            .translation_um
            .offset_by(DVec3::Y * (index as f64 * 1000.0));
        let ship = vessel::spawn_ship(
            world,
            design.clone(),
            pose,
            velocity,
            format!("Explorer {}", index + 1),
        )?;
        identity::attach_ship(world, ship, account)?;
        world.entity_mut(ship).insert(travel::SlipDrive::default());
    }
    let unowned = world
        .query_filtered::<Entity, (With<vessel::Vessel>, Without<identity::Identity>)>()
        .iter(world)
        .collect::<Vec<_>>();
    for ship in unowned {
        identity::attach_ship(world, ship, neutral)?;
    }
    if let Some(account) = debug_account {
        identity::add_account(world, account, true);
    }
    setup_demo(world, player, neutral)?;
    travel::geometry::refresh(world);
    let mut publish = Schedule::default();
    publish.add_systems(
        (
            identity::identify_celestials,
            intelligence::acquire,
            intelligence::coast,
            intelligence::fuse,
            intelligence::publish,
        )
            .chain(),
    );
    publish.run(world);
    Ok(app)
}

fn setup_demo(world: &mut World, reference: Entity, neutral: AccountId) -> Result<()> {
    let design = world
        .get::<vessel::ShipDesign>(reference)
        .unwrap()
        .0
        .clone();
    let origin = *world.get::<precision::PreciseTransform>(reference).unwrap();
    let velocity = world.get::<physics::Velocity>(reference).unwrap().0;
    let spawn = |world: &mut World, offset: DVec3, label: String| -> Result<Entity> {
        let mut pose = origin;
        pose.translation_um = pose.translation_um.offset_by(offset);
        let entity = vessel::spawn_ship(world, design.clone(), pose, velocity, label.clone())?;
        identity::attach_ship(world, entity, neutral)?;
        world.entity_mut(entity).insert(identity::BeaconEmitter);
        world
            .get_mut::<identity::Transponder>(entity)
            .unwrap()
            .0
            .labels
            .insert(label);
        Ok(entity)
    };
    let station = spawn(world, DVec3::Z * 10_000.0, "Demo station".into())?;
    let bays = (0..8)
        .map(|bay| travel::Bay {
            centre_m: [100.0 + bay as f64 * 100.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            radius_m: 40.0,
            mass_capacity_kg: 1e9,
            public: true,
            allowed: BTreeSet::new(),
            reservation: None,
            occupant: None,
        })
        .collect();
    world.entity_mut(station).insert(travel::DockingBays(bays));
    let first = spawn(world, DVec3::X * 20_000_000.0, "Demo gate 1".into())?;
    let second = spawn(world, DVec3::X * 1e12, "Demo gate 2".into())?;
    for (entity, paired) in [(first, second), (second, first)] {
        let paired = world.get::<identity::Identity>(paired).unwrap().0;
        world.entity_mut(entity).remove::<(
            physics::collision::CollisionBody,
            super::spatial::SpatialBody,
        )>();
        world
            .entity_mut(entity)
            .insert((identity::FixedBeacon, physics::Velocity::default()));
        world.entity_mut(entity).insert(travel::Gate {
            paired,
            radius_m: 100.0,
            exclusion_m: 1e7,
            enabled: true,
            public: true,
            allowed: BTreeSet::new(),
        });
    }
    Ok(())
}

pub fn apply_debug_requests(world: &mut World) -> Result<()> {
    let requests = std::mem::take(&mut world.resource_mut::<session::Clock>().requests);
    for request in requests {
        match request {
            DebugCommand::Relocate { ship, pose } => {
                let Ok(entity) = identity::lookup(world, ship) else {
                    continue;
                };
                if world.get::<travel::Dormant>(entity).is_some() {
                    continue;
                }
                world.entity_mut(entity).insert((
                    precision::PreciseTransform {
                        translation_um: pose.position,
                        rotation: bevy::math::DQuat::from_array(pose.rotation),
                    },
                    physics::Velocity(DVec3::from_array(pose.velocity)),
                    physics::AngularVelocity(DVec3::from_array(pose.angular_velocity)),
                    physics::AccumulatedForce::default(),
                    physics::AccumulatedTorque::default(),
                ));
                world.entity_mut(entity).remove::<physics::WithinSoi>();
                travel::geometry::update(world, entity);
                identity::renew_spatial_instance(world, entity);
                if let Some(mut software) = world.get_mut::<vessel::ShipSoftware>(entity) {
                    software.command(toy_sim_ship_wasm::Command::StopGuidance);
                }
            }
            DebugCommand::Recover { ship } => {
                let Ok(entity) = identity::lookup(world, ship) else {
                    continue;
                };
                if travel::debug_recover(world, entity).is_ok() {
                    identity::renew_spatial_instance(world, entity);
                }
            }
            DebugCommand::InjectHeat { ship, joules } => {
                let Ok(entity) = identity::lookup(world, ship) else {
                    continue;
                };
                if let Some(mut software) = world.get_mut::<vessel::ShipSoftware>(entity) {
                    software.hull_energy_j += joules;
                }
            }
            DebugCommand::ConfigureSensor {
                ship,
                range_m,
                occlusion,
            } => {
                if let Ok(entity) = identity::lookup(world, ship) {
                    world
                        .entity_mut(entity)
                        .insert(super::hardware::SensorOverride { range_m, occlusion });
                }
            }
            DebugCommand::InjectShieldHeat { ship, joules } => {
                if let Ok(entity) = identity::lookup(world, ship)
                    && let Some(mut software) = world.get_mut::<vessel::ShipSoftware>(entity)
                {
                    software.shield_energy_j += joules;
                }
            }
            DebugCommand::InspectBody { body } => {
                let name = body.and_then(|id| {
                    world
                        .resource::<super::registry::UniverseRegistry>()
                        .names
                        .get(&id)
                        .cloned()
                });
                world
                    .resource_mut::<super::orrery::activity::UniverseDebug>()
                    .inspect = name;
            }
            DebugCommand::RelocateToBody { ship, body } => {
                let Ok(entity) = identity::lookup(world, ship) else {
                    continue;
                };
                if world.get::<travel::Dormant>(entity).is_some() {
                    continue;
                }
                let registry = world.resource::<super::registry::UniverseRegistry>();
                let Some(name) = registry.names.get(&body).cloned() else {
                    continue;
                };
                if matches!(
                    registry.universe.get_body(&name).unwrap().class_params,
                    super::orrery::BodyClass::Star { .. }
                ) {
                    continue;
                }
                let time = physics::sim_time(world.resource::<Time<Fixed>>());
                let (pose, velocity) = super::orrery::activity::arrival(
                    world.resource::<super::orrery::Universe>(),
                    &name,
                    time,
                );
                world
                    .entity_mut(entity)
                    .insert((
                        pose,
                        physics::Velocity(velocity),
                        physics::AngularVelocity::default(),
                        physics::AccumulatedForce::default(),
                        physics::AccumulatedTorque::default(),
                    ))
                    .remove::<physics::WithinSoi>();
                travel::cancel_pending(world, entity);
                travel::geometry::update(world, entity);
                identity::renew_spatial_instance(world, entity);
                if let Some(mut software) = world.get_mut::<vessel::ShipSoftware>(entity) {
                    software.command(toy_sim_ship_wasm::Command::StopGuidance);
                }
            }
            DebugCommand::SetRate(_)
            | DebugCommand::Step
            | DebugCommand::Reset
            | DebugCommand::Inspect(_) => {}
        }
    }
    Ok(())
}
