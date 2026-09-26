use super::{identity, physics, precision, sensors, session, travel, vessel};
use anyhow::Result;
use bevy::{math::DVec3, prelude::*};
use osg_model::{
    AccountId, DebugCommand, Id,
    economy::{Currency, MONEY_SCALE},
    ownership::Principal,
};
use std::path::PathBuf;

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
    identity::attach_ship(world, player, owner, osg_model::Id::new())?;
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
        identity::attach_ship(world, ship, account, osg_model::Id::new())?;
        vessel::seed_exotic_fuel(world, ship, vessel::STARTING_EXOTIC_RANGE_LY)?;
    }
    let unowned = world
        .query_filtered::<Entity, (With<vessel::Vessel>, Without<identity::Identity>)>()
        .iter(world)
        .collect::<Vec<_>>();
    for &ship in &unowned {
        identity::attach_ship(world, ship, hostile_account, osg_model::Id::new())?;
        world.entity_mut(ship).insert(super::ownership::AssetOwner(
            osg_model::ownership::Principal::Organization(super::ownership::organization_id(
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
        let now = osg_model::calendar::now_unix_ms();
        let mut economy = world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy);
        for currency in [Currency::Uec, Currency::Lat] {
            economy.issue(
                Principal::Player(account),
                currency,
                1_000_000_000 * MONEY_SCALE,
                now,
            )?;
        }
    }
    super::infrastructure::spawn(world, player)?;
    let mut publish = Schedule::default();
    publish.add_systems(
        (
            super::hardware::generators,
            super::hardware::avionics,
            super::hardware::utilities::run,
            super::orrery::activity::activate,
            super::spatial::rebuild,
            super::location::refresh,
            identity::identify_celestials,
            sensors::publish,
        )
            .chain(),
    );
    publish.run(world);
    for hostile in unowned {
        let contact = super::services::handle_for_entity(world, hostile, player)?;
        let mut mailbox = world.get_mut::<vessel::ShipMailbox>(hostile).unwrap();
        mailbox.command(osg_ship_wasm::Command::AimContact(contact));
        mailbox.command(osg_ship_wasm::Command::MarkTarget {
            contact,
            maximum_flight_time_s: 2.0,
        });
        mailbox.command(osg_ship_wasm::Command::StartFiring);
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
                identity::renew_spatial_instance(world, entity);
                if let Some(mut mailbox) = world.get_mut::<vessel::ShipMailbox>(entity) {
                    mailbox.command(osg_ship_wasm::Command::StopGuidance);
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
                if let Some(mut damage) = world.get_mut::<vessel::PendingDamage>(entity) {
                    damage.hull_energy_j += joules;
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
                    && let Some(mut damage) = world.get_mut::<vessel::PendingDamage>(entity)
                {
                    damage.shield_energy_j += joules;
                }
            }
            DebugCommand::InspectBody { body } => {
                let reference = body.filter(|reference| {
                    world
                        .resource::<super::registry::UniverseRegistry>()
                        .universe
                        .body(super::registry::universe_reference(*reference))
                        .is_some()
                });
                world
                    .resource_mut::<super::orrery::activity::UniverseDebug>()
                    .inspect = reference;
            }
            DebugCommand::RelocateToBody { ship, body } => {
                let Ok(entity) = identity::lookup(world, ship) else {
                    continue;
                };
                if world.get::<travel::Dormant>(entity).is_some() {
                    continue;
                }
                let registry = world.resource::<super::registry::UniverseRegistry>();
                let Some(definition) = registry
                    .universe
                    .body(super::registry::universe_reference(body))
                else {
                    continue;
                };
                if matches!(
                    definition.class_params,
                    super::orrery::BodyClass::Star { .. }
                ) {
                    continue;
                }
                let time = physics::sim_time(world.resource::<Time<Fixed>>());
                let (pose, velocity) = super::orrery::activity::arrival(
                    world.resource::<super::orrery::Universe>(),
                    body,
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
                identity::renew_spatial_instance(world, entity);
                if let Some(mut mailbox) = world.get_mut::<vessel::ShipMailbox>(entity) {
                    mailbox.command(osg_ship_wasm::Command::StopGuidance);
                }
            }
            DebugCommand::SetRate(_) | DebugCommand::Reset | DebugCommand::Inspect(_) => {}
        }
    }
    Ok(())
}
