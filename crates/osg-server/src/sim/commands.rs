use anyhow::{Result, ensure};
use bevy::prelude::*;
use osg_model::*;
use osg_ship_wasm::{Command, ScanSource};
use std::sync::Arc;

use super::identity::{self, Control, Identity, Membership, Transponder};
use super::intelligence::Group;
use super::vessel::ShipSoftware;

#[cfg(test)]
mod tests;

pub fn authorize(
    world: &World,
    account: Id,
    ship: Id,
    revision: Option<u64>,
    permission: ownership::Permission,
) -> Result<Entity> {
    let entity = identity::lookup(world, ship)?;
    let authority = world
        .get::<Control>(entity)
        .ok_or_else(|| anyhow::anyhow!("ship unavailable"))?;
    super::ownership::authorize(world, account, entity, permission)?;
    ensure!(
        revision.is_none_or(|revision| revision == authority.revision),
        "control authority changed"
    );
    Ok(entity)
}

pub fn observe(world: &World, account: Id, ship: Id) -> Result<Entity> {
    authorize(world, account, ship, None, ownership::Permission::View)
        .or_else(|_| authorize(world, account, ship, None, ownership::Permission::Control))
}

#[derive(serde::Serialize)]
pub struct ShipState {
    pub name: String,
    pub group: GroupId,
    pub telemetry: ShipTelemetry,
    pub presentation: ShipPresentation,
}

pub fn telemetry(world: &World, account: AccountId, ship: EntityId) -> Result<ShipState> {
    let entity = observe(world, account, ship)?;
    Ok(ShipState {
        group: world
            .get::<Membership>(entity)
            .and_then(|membership| world.get::<Group>(membership.0))
            .ok_or_else(|| anyhow::anyhow!("ship group unavailable"))?
            .id,
        name: world
            .get::<super::vessel::Vessel>(entity)
            .map_or_else(|| "Ship".into(), |vessel| vessel.vessel_name.to_string()),
        telemetry: ship_telemetry(world, entity, account)
            .ok_or_else(|| anyhow::anyhow!("ship telemetry unavailable"))?,
        presentation: super::presentation::ship(world, entity, true)
            .ok_or_else(|| anyhow::anyhow!("ship presentation unavailable"))?,
    })
}

pub fn source(
    world: &mut World,
    account: AccountId,
    ship: EntityId,
) -> Result<Arc<dyn ScanSource>> {
    let entity = authorize(world, account, ship, None, ownership::Permission::Control)?;
    super::services::current_source(world, entity)
        .ok_or_else(|| anyhow::anyhow!("ship observations unavailable"))
}

pub(crate) fn permission(command: &ShipCommand) -> ownership::Permission {
    match command {
        ShipCommand::SetTransponderEnabled(_)
        | ShipCommand::SetGroup(_)
        | ShipCommand::SetIff(_)
        | ShipCommand::SetDockServices { .. } => ownership::Permission::Configure,
        ShipCommand::UseRoute { .. }
        | ShipCommand::Flight(_)
        | ShipCommand::MarkTarget { .. }
        | ShipCommand::StopFiring
        | ShipCommand::UnmarkTarget
        | ShipCommand::StartFiring
        | ShipCommand::Aim { .. }
        | ShipCommand::SetTravel { .. }
        | ShipCommand::SetAutopilot(_)
        | ShipCommand::SetThrottle(_)
        | ShipCommand::Undock
        | ShipCommand::Dock { .. }
        | ShipCommand::ScreenInput { .. } => ownership::Permission::Control,
    }
}

fn target_handle(world: &mut World, ship: Entity, group: Id, track: Id) -> Result<u64> {
    let member = world
        .get::<Membership>(ship)
        .ok_or_else(|| anyhow::anyhow!("ship group unavailable"))?
        .0;
    ensure!(
        world
            .get::<Group>(member)
            .is_some_and(|item| item.id == group),
        "target group access denied"
    );
    ensure!(
        world
            .get::<Group>(member)
            .unwrap()
            .snapshot
            .tracks
            .contains_key(&track),
        "target track unavailable"
    );
    super::services::contact_handle(world, ship, group, track)
}

fn enqueue(world: &mut World, ship: Entity, command: Command) -> Result<()> {
    queue_capacity(world, ship, 1)?;
    let mut software = world
        .get_mut::<ShipSoftware>(ship)
        .expect("queue capacity checked");
    software.command(command);
    Ok(())
}

fn queue_capacity(world: &World, ship: Entity, count: usize) -> Result<()> {
    let software = world
        .get::<ShipSoftware>(ship)
        .ok_or_else(|| anyhow::anyhow!("ship computer unavailable"))?;
    ensure!(
        software.inbox.len().saturating_add(count) <= 255,
        "ship command queue full"
    );
    Ok(())
}

pub fn execute(
    world: &mut World,
    account: AccountId,
    ship: EntityId,
    authority_revision: u64,
    command: ShipCommand,
) -> Result<()> {
    osg_protocol::validate_ship_command(&command)?;

    let entity = authorize(
        world,
        account,
        ship,
        Some(authority_revision),
        permission(&command),
    )?;
    let wake = matches!(
        command,
        ShipCommand::UseRoute { .. }
            | ShipCommand::SetTravel { .. }
            | ShipCommand::SetAutopilot(_)
            | ShipCommand::Dock { .. }
            | ShipCommand::Undock
    );
    let cancel_haul = wake
        || matches!(
            command,
            ShipCommand::SetThrottle(_) | ShipCommand::Flight(_)
        );
    match command {
        ShipCommand::SetTransponderEnabled(enabled) => {
            world.get_mut::<Transponder>(entity).unwrap().0.enabled = enabled
        }
        ShipCommand::SetGroup(key) => {
            let group = super::intelligence::join(world, key);
            world.entity_mut(entity).insert(Membership(group));
        }
        ShipCommand::SetIff(iff) => {
            ensure!(
                iff.owner == account,
                "IFF owner must identify current controller"
            );
            ensure!(
                world
                    .resource::<super::ownership::Directory>()
                    .0
                    .can_advertise(account, iff.faction),
                "IFF faction access denied"
            );
            ensure!(iff.range_m <= 1e8, "transponder range exceeds hardware");
            world.entity_mut(entity).insert(Transponder(iff));
        }
        ShipCommand::UseRoute {
            id,
            expected_revision,
            engage,
        } => {
            use_route(world, entity, id, expected_revision, engage)?;
        }
        ShipCommand::SetTravel {
            preferences,
            engage,
            expected_revision,
            orders,
        } => {
            ensure!(
                !matches!(
                    world
                        .get::<super::travel::PresenceState>(entity)
                        .map(|p| &p.0),
                    Some(travel::Presence::SlipTransit(_))
                ),
                "wait for slip arrival before changing the queue"
            );
            ensure!(
                world
                    .get::<super::travel::Travel>(entity)
                    .is_some_and(|state| state.0.revision == expected_revision),
                "stale travel revision"
            );
            ensure!(preferences.valid(), "invalid planning preference");
            if engage {
                queue_capacity(world, entity, 2)?;
            }
            super::travel::cancel_pending(world, entity);
            if engage {
                enqueue(
                    world,
                    entity,
                    Command::Manual {
                        throttle: 0.,
                        steering: [0.; 3],
                    },
                )?;
                enqueue(world, entity, Command::HoldAttitude)?;
            }
            let mut state = world.get_mut::<super::travel::Travel>(entity).unwrap();
            let enabled = engage || state.0.autopilot_enabled;
            let status = if orders.is_empty() {
                travel::Status::Completed
            } else {
                travel::Status::Planning
            };
            state.0 = travel::TravelState {
                autopilot_enabled: enabled,
                preferences,
                revision: state.0.revision + 1,
                risk_budget: travel::RiskBudget::new(preferences.max_loss_ppm),
                goals: orders.clone(),
                orders: orders.into_iter().map(Into::into).collect(),
                status,
                ..Default::default()
            };
        }
        ShipCommand::SetAutopilot(enabled) => {
            ensure!(
                world
                    .get::<super::travel::PresenceState>(entity)
                    .is_some_and(|p| p.0 == travel::Presence::Space),
                "ship is not in space"
            );
            queue_capacity(world, entity, 2)?;
            super::travel::cancel_pending(world, entity);
            enqueue(
                world,
                entity,
                Command::Manual {
                    throttle: 0.,
                    steering: [0.; 3],
                },
            )?;
            enqueue(world, entity, Command::HoldAttitude)?;
            let mut state = world.get_mut::<super::travel::Travel>(entity).unwrap();
            state.0.revision += 1;
            state.0.autopilot_enabled = enabled;
            state.0.planning = None;
            let needs_planning = matches!(
                state.0.status,
                travel::Status::Planning | travel::Status::Blocked(_)
            ) || state
                .0
                .orders
                .iter()
                .skip(state.0.order)
                .any(|stage| matches!(stage.action, travel::Order::TravelTo(_)));
            state.0.status = if state.0.order >= state.0.orders.len() {
                travel::Status::Completed
            } else if needs_planning {
                travel::Status::Planning
            } else if enabled {
                travel::Status::Active
            } else {
                travel::Status::Paused
            };
        }
        ShipCommand::SetThrottle(throttle) => {
            ensure_manual_control(world, entity)?;
            enqueue(world, entity, Command::SetThrottle(throttle))?;
        }
        ShipCommand::Dock { station, bay } => {
            let station = identity::lookup(world, station)?;
            super::travel::dock(world, entity, station, bay)?;
        }
        ShipCommand::SetDockServices { cargo, power } => {
            ensure!(
                matches!(
                    world
                        .get::<super::travel::PresenceState>(entity)
                        .map(|p| &p.0),
                    Some(travel::Presence::Docked { .. })
                ),
                "ship must be docked to request services"
            );
            world
                .entity_mut(entity)
                .insert(super::hardware::utilities::DockServiceRequest { cargo, power });
        }
        ShipCommand::Undock => super::travel::undock(world, entity)?,
        ShipCommand::UnmarkTarget => enqueue(world, entity, Command::UnmarkTarget)?,
        ShipCommand::StartFiring => enqueue(world, entity, Command::StartFiring)?,
        ShipCommand::StopFiring => enqueue(world, entity, Command::StopFiring)?,
        ShipCommand::Aim { group, track } => {
            let handle = target_handle(world, entity, group, track)?;
            enqueue(world, entity, Command::AimContact(handle))?;
        }
        ShipCommand::MarkTarget {
            group,
            track,
            maximum_flight_time_s,
        } => {
            let handle = target_handle(world, entity, group, track)?;
            enqueue(
                world,
                entity,
                Command::MarkTarget {
                    contact: handle,
                    maximum_flight_time_s,
                },
            )?;
        }
        ShipCommand::Flight(command) => {
            ensure_manual_control(world, entity)?;
            let command = match command {
                FlightCommand::HoldAttitude => Command::HoldAttitude,
                FlightCommand::StopGuidance => Command::StopGuidance,
                FlightCommand::AimDirection(direction) => Command::AimDirection(direction),
                FlightCommand::SelectTarget(target) => {
                    Command::SelectTarget(target_handle(world, entity, target.group, target.track)?)
                }
                FlightCommand::EngageNavigation {
                    throttle_limit,
                    stand_off_m,
                } => Command::EngageNavigation {
                    throttle_limit,
                    stand_off_m,
                },
            };
            enqueue(world, entity, command)?;
        }
        ShipCommand::ScreenInput {
            slot,
            revision,
            kind,
            code,
            modifiers,
            xy,
            text,
        } => {
            super::displays::input(
                world, entity, slot, revision, kind, code, modifiers, xy, &text,
            )?;
        }
    }

    if cancel_haul {
        super::npc::logistics::cancel(world, entity);
    }
    if wake && let Some(mut software) = world.get_mut::<ShipSoftware>(entity) {
        software.schedule.wake();
    }
    Ok(())
}

pub(crate) fn use_route(
    world: &mut World,
    ship: Entity,
    request: u64,
    revision: u64,
    engage: bool,
) -> Result<()> {
    let route = super::route_service::ready(world, ship, request, revision)?;
    let enabled = engage
        || world
            .get::<super::travel::Travel>(ship)
            .is_some_and(|travel| travel.0.autopilot_enabled);
    if enabled {
        queue_capacity(world, ship, 2)?;
    }
    super::travel::apply_plan(
        world,
        ship,
        revision,
        route.plan,
        route.preferences,
        engage,
        route.goals,
        true,
    )?;
    if enabled {
        enqueue(
            world,
            ship,
            Command::Manual {
                throttle: 0.,
                steering: [0.; 3],
            },
        )?;
        enqueue(world, ship, Command::HoldAttitude)?;
    }
    super::npc::logistics::cancel(world, ship);
    Ok(())
}

pub(crate) fn ship_telemetry(
    world: &World,
    entity: Entity,
    account: AccountId,
) -> Option<ShipTelemetry> {
    let inventory = &world.get::<super::hardware::ShipInventory>(entity)?.0;
    let thermal = &world.get::<super::hardware::ShipThermal>(entity)?.0;
    let design = &world.get::<super::vessel::ShipDesign>(entity)?.0;
    let authority = world.get::<Control>(entity)?;
    let group = world.get::<Membership>(entity)?.0;
    Some(ShipTelemetry {
        can_control: super::ownership::can_access(
            world,
            account,
            entity,
            ownership::Permission::Control,
        ),
        appearance: world
            .get::<super::identity::Appearance>(entity)
            .map(|appearance| appearance.0),
        radius_m: design.radius,
        dock_services: world
            .get::<super::hardware::utilities::DockServiceRequest>(entity)
            .map_or_else(DockServiceSettings::default, |request| {
                DockServiceSettings {
                    cargo: request.cargo,
                    power: request.power,
                }
            }),
        spatial_instance: world.get::<super::identity::SpatialInstance>(entity)?.0,
        info_group: world.get::<Group>(group)?.key?,
        iff: world.get::<Transponder>(entity)?.0.clone(),
        ship: world.get::<Identity>(entity)?.0,
        authority_revision: authority.revision,
        presence: world
            .get::<super::travel::PresenceState>(entity)
            .map(|presence| presence.0.clone())
            .unwrap_or(travel::Presence::Space),
        pose: world
            .get::<super::travel::PresenceState>(entity)
            .is_none_or(|presence| {
                matches!(
                    presence.0,
                    travel::Presence::Space
                        | travel::Presence::Docked { .. }
                        | travel::Presence::SlipTransit(_)
                )
            })
            .then(|| super::session::ship_pose(world, entity))
            .flatten(),
        battery_j: inventory.energy_j,
        hull_heat_j: thermal.hull_energy_j,
        shield_temperature_k: thermal.shield_temperature(design.as_ref().into()),
        coolant_reserve_kg: thermal.shield_reserve_kg(),
        travel: world
            .get::<super::travel::Travel>(entity)
            .map(|travel| travel.0.clone())
            .unwrap_or_default(),
    })
}

fn ensure_manual_control(world: &World, entity: Entity) -> anyhow::Result<()> {
    ensure!(
        world
            .get::<ShipSoftware>(entity)
            .is_some_and(|s| !s.controller.is_booting() && s.controller.fault.is_none())
            && world
                .get::<super::hardware::Avionics>(entity)
                .is_some_and(|a| a.0.operational && a.0.powered)
            && world
                .get::<super::hardware::Hull>(entity)
                .is_some_and(|h| h.0 > 0.),
        "flight computer unavailable"
    );
    ensure!(
        !world
            .get::<super::travel::Travel>(entity)
            .unwrap()
            .0
            .autopilot_enabled,
        "manual controls locked by autopilot"
    );
    ensure!(
        world
            .get::<super::travel::PresenceState>(entity)
            .is_some_and(|p| p.0 == travel::Presence::Space),
        "ship is not in space"
    );
    Ok(())
}
