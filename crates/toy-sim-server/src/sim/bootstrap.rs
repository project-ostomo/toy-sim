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
    provision_inner(accounts, debug_account, ship, false)
}

#[cfg(test)]
pub(crate) fn provision_combat_fixture(
    accounts: &[AccountId],
    debug_account: Option<AccountId>,
    ship: Option<PathBuf>,
) -> Result<App> {
    provision_inner(accounts, debug_account, ship, true)
}

fn provision_inner(
    accounts: &[AccountId],
    debug_account: Option<AccountId>,
    ship: Option<PathBuf>,
    spawn_hostile: bool,
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
    super::ownership::affiliate(
        world,
        hostile_account,
        Some(super::ownership::organization_id("Terminus Privateers")),
    )?;
    let owner = accounts.first().copied().unwrap_or_else(Id::new);
    identity::attach_ship(world, player, owner)?;
    let design = world.get::<vessel::ShipDesign>(player).unwrap().0.clone();
    let pose = *world.get::<precision::PreciseTransform>(player).unwrap();
    let velocity = world.get::<physics::Velocity>(player).unwrap().0;
    if spawn_hostile {
        let mut hostile_pose = pose;
        hostile_pose.translation_um = pose.translation_um.offset_by(DVec3::Y * 1_000.0);
        vessel::spawn_ship(
            world,
            design.clone(),
            hostile_pose,
            velocity,
            "Hostile patrol".into(),
        )?;
    }
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
    }
    let unowned = world
        .query_filtered::<Entity, (With<vessel::Vessel>, Without<identity::Identity>)>()
        .iter(world)
        .collect::<Vec<_>>();
    for &ship in &unowned {
        identity::attach_ship(world, ship, hostile_account)?;
        world.entity_mut(ship).insert(super::ownership::AssetOwner(
            toy_sim_model::ownership::Principal::Organization(super::ownership::organization_id(
                "Terminus Privateers",
            )),
        ));
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
    super::infrastructure::spawn(world, player)?;
    travel::geometry::refresh(world);
    super::infrastructure::enforce_exclusion(world);
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
        software.command(toy_sim_ship_wasm::Command::MarkTarget {
            contact,
            maximum_flight_time_s: 2.0,
        });
        software.command(toy_sim_ship_wasm::Command::StartFiring);
    }
    super::infrastructure::publish_navigation(world);
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
            DebugCommand::SetRate(_) | DebugCommand::Reset | DebugCommand::Inspect(_) => {}
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
        let mut app = provision_combat_fixture(&[account], Some(account), None).unwrap();
        let world = app.world_mut();
        let ships = world
            .query_filtered::<Entity, (
                With<vessel::Vessel>,
                Without<super::super::infrastructure::Landmark>,
            )>()
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
        let mut own_projectiles = std::collections::HashSet::new();
        let mut previous_shots = 0;
        let mut multiple_shots_in_tick = false;
        for _ in 0..200 {
            app.update();
            if let Some(report) = app
                .world()
                .get_resource::<super::super::physics::collision::CollisionReport>()
            {
                own_projectiles.extend(
                    report
                        .report
                        .shots
                        .iter()
                        .filter(|shot| shot.owner == hostile)
                        .map(|shot| shot.projectile),
                );
                assert!(
                    !report.report.impact_events.iter().any(|impact| impact
                        .entities
                        .contains(&hostile)
                        && impact
                            .entities
                            .iter()
                            .any(|id| own_projectiles.contains(id))),
                    "patrol gun hits its own voxel hull"
                );
            }
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
