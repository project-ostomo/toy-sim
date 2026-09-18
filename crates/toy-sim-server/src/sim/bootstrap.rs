use super::{identity, intelligence, physics, precision, session, travel, vessel};
use anyhow::Result;
use bevy::{math::DVec3, prelude::*};
use std::path::PathBuf;
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
    let hostile_account = Id::new();
    let owner = accounts.first().copied().unwrap_or_else(Id::new);
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
    for &ship in &unowned {
        identity::attach_ship(world, ship, hostile_account)?;
        world
            .get_mut::<identity::Transponder>(ship)
            .unwrap()
            .0
            .labels
            .insert("Hostile patrol".into());
    }
    if let Some(account) = debug_account {
        identity::add_account(world, account, true);
    }
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
    for hostile in unowned {
        let contact = super::services::handle_for_entity(world, hostile, player)?;
        let mut software = world.get_mut::<vessel::ShipSoftware>(hostile).unwrap();
        software.command(toy_sim_ship_wasm::Command::AimContact(contact));
        software.command(toy_sim_ship_wasm::Command::EngageWeapons {
            contact,
            maximum_flight_time_s: 2.0,
        });
    }
    Ok(app)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_patrol_encounter_starts_close_and_hostile_fires_repeatedly_without_player_input() {
        let account = Id::new();
        let mut app = provision(&[account], Some(account), None).unwrap();
        let world = app.world_mut();
        let ships = world
            .query_filtered::<Entity, With<vessel::Vessel>>()
            .iter(world)
            .collect::<Vec<_>>();
        assert_eq!(ships.len(), 2);
        let player = *ships
            .iter()
            .find(|&&ship| world.get::<vessel::ControlledVessel>(ship).is_some())
            .unwrap();
        let hostile = *ships.iter().find(|&&ship| ship != player).unwrap();
        let player_pose = world.get::<precision::PreciseTransform>(player).unwrap();
        let hostile_pose = world.get::<precision::PreciseTransform>(hostile).unwrap();
        let separation = hostile_pose
            .translation_um
            .relative_to(player_pose.translation_um)
            .length();
        assert!((separation - 1_000.).abs() < 1.);
        assert_ne!(
            world.get::<identity::Control>(hostile).unwrap().account,
            account
        );
        for &ship in &ships {
            assert!(
                world
                    .get::<vessel::ShipDesign>(ship)
                    .unwrap()
                    .0
                    .blueprint
                    .parts
                    .iter()
                    .any(|part| part.prototype == "micropulse_engine_4m")
            );
        }
        let mut previous_shots = 0;
        let mut multiple_shots_in_tick = false;
        for _ in 0..200 {
            app.update();
            let state = super::super::hardware::snapshot(app.world(), hostile).unwrap();
            let shots = state
                .weapons
                .iter()
                .map(|weapon| weapon.shots_fired)
                .sum::<u64>();
            multiple_shots_in_tick |= shots.saturating_sub(previous_shots) >= 2;
            previous_shots = shots;
            if shots >= 20 && multiple_shots_in_tick {
                return;
            }
        }
        let state = super::super::hardware::snapshot(app.world(), hostile).unwrap();
        let software = app.world().get::<vessel::ShipSoftware>(hostile).unwrap();
        panic!(
            "hostile patrol did not fire repeatedly: weapons={:?}, energy={}, resources={:?}, controller={:?}, inbox={:?}",
            state.weapons,
            state.inventory.energy_j,
            state.inventory.quantities,
            software.controller.state.weapons,
            software.inbox
        );
    }
}
