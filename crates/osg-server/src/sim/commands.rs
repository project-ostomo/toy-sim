use anyhow::{Result, ensure};
use bevy::prelude::*;
use osg_model::*;
use osg_ship_wasm::Command;
#[cfg(test)]
use osg_ship_wasm::ScanSource;

use super::identity::{self, Control, Identity, Transponder};
use super::vessel::{ShipMailbox, ShipSoftware};

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
    pub telemetry: ShipTelemetry,
    pub presentation: ShipPresentation,
}

pub fn telemetry(world: &World, account: AccountId, ship: EntityId) -> Result<ShipState> {
    let entity = observe(world, account, ship)?;
    Ok(ShipState {
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
    world: &World,
    account: AccountId,
    ship: EntityId,
) -> Result<super::services::ShipScan<'_>> {
    let entity = authorize(world, account, ship, None, ownership::Permission::Control)?;
    super::services::current_source(world, entity)
        .ok_or_else(|| anyhow::anyhow!("ship observations unavailable"))
}

pub(crate) fn permission(command: &ShipCommand) -> ownership::Permission {
    match command {
        ShipCommand::SetTransponderEnabled(_)
        | ShipCommand::SetIff(_)
        | ShipCommand::SetDockServices { .. } => ownership::Permission::Configure,
        ShipCommand::Flight(_)
        | ShipCommand::MarkTarget { .. }
        | ShipCommand::StopFiring
        | ShipCommand::UnmarkTarget
        | ShipCommand::StartFiring
        | ShipCommand::Aim { .. }
        | ShipCommand::SetItinerary { .. }
        | ShipCommand::SetGuidance(_)
        | ShipCommand::SetAutopilot(_)
        | ShipCommand::SetThrottle(_)
        | ShipCommand::Undock
        | ShipCommand::Dock { .. }
        | ShipCommand::ScreenInput { .. } => ownership::Permission::Control,
    }
}

fn target_handle(world: &World, ship: Entity, target: ContactRef) -> Result<u64> {
    ensure!(
        world
            .get::<Identity>(ship)
            .is_some_and(|id| id.0 == target.observer),
        "contact observer unavailable"
    );
    ensure!(
        super::services::contact_ref(world, ship, target.contact).is_some(),
        "target contact unavailable"
    );
    Ok(target.contact)
}

fn enqueue(world: &mut World, ship: Entity, command: Command) -> Result<()> {
    queue_capacity(world, ship, 1)?;
    let mut mailbox = world
        .get_mut::<ShipMailbox>(ship)
        .expect("queue capacity checked");
    mailbox.command(command);
    Ok(())
}

fn queue_capacity(world: &World, ship: Entity, count: usize) -> Result<()> {
    ensure!(
        world.get::<ShipSoftware>(ship).is_some(),
        "ship computer unavailable"
    );
    let mailbox = world
        .get::<ShipMailbox>(ship)
        .ok_or_else(|| anyhow::anyhow!("ship computer unavailable"))?;
    ensure!(
        mailbox.inbox.len().saturating_add(count) <= 255,
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
        ShipCommand::SetItinerary { .. }
            | ShipCommand::SetGuidance(_)
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
            world.get_mut::<Transponder>(entity).unwrap().0.enabled = enabled;
            super::sensors::refresh_iff(world, entity);
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
            world.entity_mut(entity).insert(Transponder(iff));
            super::sensors::refresh_iff(world, entity);
        }
        ShipCommand::SetItinerary {
            preferences,
            engage,
            expected_revision,
            itinerary,
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
                    .is_some_and(|state| state.0.directive_revision == expected_revision),
                "stale directive revision"
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
            let fuel = exotic_fuel_kg(world, entity);
            let mut state = world.get_mut::<super::travel::Travel>(entity).unwrap();
            let enabled = engage && !itinerary.is_empty();
            state.0 = travel::AutopilotState {
                enabled,
                preferences,
                directive_revision: state.0.directive_revision.wrapping_add(1),
                risk_budget: travel::RiskBudget::new(preferences.max_loss_ppm),
                fuel_budget: Some(travel::FuelBudget {
                    resources: vec![travel::FuelRequirement {
                        resource: travel::slip::EXOTIC_RESOURCE.into(),
                        required_kg: 0.,
                        available_kg: fuel,
                    }],
                    complete: false,
                }),
                itinerary: itinerary
                    .into_iter()
                    .map(|directive| travel::ItineraryEntry {
                        label: directive.label(),
                        directive,
                    })
                    .collect(),
                status: travel::FirmwareStatus {
                    phase: if enabled {
                        travel::FirmwarePhase::Planning
                    } else {
                        travel::FirmwarePhase::Idle
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            super::travel::validate_active_directive(world, entity);
        }
        ShipCommand::SetAutopilot(enabled) => {
            ensure!(
                world
                    .get::<super::travel::PresenceState>(entity)
                    .is_some_and(|p| matches!(
                        p.0,
                        travel::Presence::Space | travel::Presence::Docked { .. }
                    )),
                "ship cannot change autopilot in its current state"
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
            state.0.directive_revision = state.0.directive_revision.wrapping_add(1);
            state.0.enabled = enabled && !state.0.itinerary.is_empty();
            state.0.failure = None;
            state.0.status = travel::FirmwareStatus {
                spent_loss_ppm: state.0.status.spent_loss_ppm,
                spent_exotic_fuel_kg: state.0.status.spent_exotic_fuel_kg,
                phase: if state.0.enabled {
                    travel::FirmwarePhase::Planning
                } else {
                    travel::FirmwarePhase::Idle
                },
                ..Default::default()
            };
            super::travel::validate_active_directive(world, entity);
        }
        ShipCommand::SetGuidance(guidance) => {
            ensure_manual_control(world, entity)?;
            if let Some(travel::Guidance {
                target: travel::Target::Contact(target),
                ..
            }) = &guidance
            {
                target_handle(world, entity, *target)?;
            }
            enqueue(world, entity, Command::SetGuidance(guidance))?;
            disengage_autopilot(world, entity);
        }
        ShipCommand::SetThrottle(throttle) => {
            ensure_manual_control(world, entity)?;
            enqueue(world, entity, Command::SetThrottle(throttle))?;
            disengage_autopilot(world, entity);
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
        ShipCommand::Aim { target } => {
            let handle = target_handle(world, entity, target)?;
            enqueue(world, entity, Command::AimContact(handle))?;
        }
        ShipCommand::MarkTarget {
            target,
            maximum_flight_time_s,
        } => {
            let handle = target_handle(world, entity, target)?;
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
                    Command::SelectTarget(target_handle(world, entity, target)?)
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
            disengage_autopilot(world, entity);
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

    if cancel_haul {}
    if wake && let Some(mut software) = world.get_mut::<ShipSoftware>(entity) {
        software.schedule.wake();
    }
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
        location: world
            .get::<super::location::SpatialLocation>(entity)
            .map(|location| location.0.clone())
            .unwrap_or_default(),
    })
}

fn exotic_fuel_kg(world: &World, entity: Entity) -> f64 {
    let Some(catalogue) = world.get_resource::<super::vessel::ShipCatalogue>() else {
        return 0.;
    };
    let Some(index) = catalogue
        .0
        .resources
        .iter()
        .position(|resource| resource.id == travel::slip::EXOTIC_RESOURCE)
    else {
        return 0.;
    };
    world
        .get::<super::hardware::ShipInventory>(entity)
        .and_then(|inventory| inventory.0.quantities.get(index))
        .copied()
        .unwrap_or(0) as f64
        / 1000.
}

pub(crate) fn disengage_autopilot(world: &mut World, entity: Entity) {
    if !world
        .get::<super::travel::Travel>(entity)
        .is_some_and(|state| state.0.enabled)
    {
        return;
    }
    super::travel::cancel_pending(world, entity);
    let mut state = world.get_mut::<super::travel::Travel>(entity).unwrap();
    state.0.enabled = false;
    state.0.directive_revision = state.0.directive_revision.wrapping_add(1);
    state.0.status = travel::FirmwareStatus {
        spent_loss_ppm: state.0.status.spent_loss_ppm,
        spent_exotic_fuel_kg: state.0.status.spent_exotic_fuel_kg,
        ..Default::default()
    };
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
        world
            .get::<super::travel::PresenceState>(entity)
            .is_some_and(|p| p.0 == travel::Presence::Space),
        "ship is not in space"
    );
    Ok(())
}
