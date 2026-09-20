use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use toy_sim_model::{
    Id, ShipCommand,
    industry::{CargoItem, FacilityView, IndustryCommand, IndustrySubscription},
    ownership::Permission,
    travel::{Order, PlanningPreferences, Presence, Status},
};

use super::super::{commands, identity, industry, ownership, simulation::SimulationCounters};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HaulStage {
    Loading,
    Outbound,
    Unloading,
    Inbound,
    Paused,
}

#[derive(Component, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HaulDuty {
    pub account: Id,
    pub source: Id,
    pub destination: Id,
    pub item: CargoItem,
    pub quantity: u64,
    pub stage: HaulStage,
    pub next_check_tick: u64,
    pub problem: Option<String>,
}

pub fn cancel(world: &mut World, entity: Entity) {
    if let Some(mut duty) = world.get_mut::<HaulDuty>(entity) {
        duty.stage = HaulStage::Paused;
        duty.problem = Some("Standing order superseded by a navigation command".into());
    }
}

impl HaulDuty {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.source != self.destination,
            "haul endpoints must differ"
        );
        ensure!(self.quantity > 0, "empty haul assignment");
        ensure!(
            self.problem.as_ref().is_none_or(|text| text.len() <= 256),
            "haul status is too long"
        );
        Ok(())
    }
}

pub fn advance(world: &mut World) {
    let tick = world.resource::<SimulationCounters>().ticks;
    let due = world
        .query::<(Entity, &HaulDuty)>()
        .iter(world)
        .filter(|(_, duty)| duty.stage != HaulStage::Paused && tick >= duty.next_check_tick)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();

    for entity in due {
        let Some(mut duty) = world.entity_mut(entity).take::<HaulDuty>() else {
            continue;
        };
        duty.next_check_tick = tick.saturating_add(10);
        if let Err(error) = step(world, entity, &mut duty) {
            let message = error.to_string();
            let mut end = message.len().min(256);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            duty.problem = Some(message[..end].to_owned());
            duty.stage = HaulStage::Paused;
            warn!(ship = ?entity, %error, "Freight standing order paused");
        }
        world.entity_mut(entity).insert(duty);
    }
}

fn step(world: &mut World, entity: Entity, duty: &mut HaulDuty) -> Result<()> {
    duty.validate()?;
    let ship = world
        .get::<identity::Identity>(entity)
        .context("freighter unavailable")?
        .0;
    commands::authorize(world, duty.account, ship, None, Permission::Control)?;
    for inventory in [ship, duty.source, duty.destination] {
        let inventory = identity::lookup(world, inventory)?;
        ownership::authorize(world, duty.account, inventory, Permission::TransferCargo)?;
    }
    let state = commands::telemetry(world, duty.account, ship)?;
    let telemetry = &state.telemetry;
    ensure!(
        !matches!(telemetry.travel.status, Status::Blocked(_)),
        "flight computer blocked the freight route"
    );

    match duty.stage {
        HaulStage::Outbound | HaulStage::Inbound => {
            let destination = if duty.stage == HaulStage::Outbound {
                duty.destination
            } else {
                duty.source
            };
            if matches!(telemetry.presence, Presence::Docked { host, .. } if host == destination) {
                duty.stage = if duty.stage == HaulStage::Outbound {
                    HaulStage::Unloading
                } else {
                    HaulStage::Loading
                };
            }
        }
        HaulStage::Loading | HaulStage::Unloading => {
            let host = if duty.stage == HaulStage::Loading {
                duty.source
            } else {
                duty.destination
            };
            ensure!(
                matches!(telemetry.presence, Presence::Docked { host: current, .. } if current == host),
                "freighter left its assigned loading berth"
            );
            commands::execute(
                world,
                duty.account,
                ship,
                telemetry.authority_revision,
                ShipCommand::SetDockServices {
                    cargo: duty.stage == HaulStage::Loading,
                    power: true,
                },
            )?;
            let inventory = industry::snapshot(
                world,
                duty.account,
                &IndustrySubscription {
                    inventories: vec![ship, host],
                    ..Default::default()
                },
            );
            let ship_inventory = inventory
                .facilities
                .iter()
                .find(|item| item.entity == ship)
                .context("freighter inventory access denied")?;
            let host_inventory = inventory
                .facilities
                .iter()
                .find(|item| item.entity == host)
                .context("terminal inventory access denied")?;
            let aboard = available(ship_inventory, &duty.item);
            if duty.stage == HaulStage::Loading && aboard < duty.quantity {
                if let Some(stack) = host_inventory
                    .items
                    .iter()
                    .find(|stack| stack.item == duty.item)
                {
                    let free =
                        (ship_inventory.cargo_capacity_m3 - ship_inventory.cargo_used_m3).max(0.0);
                    let fitting = if stack.unit_volume_m3 > 0.0 {
                        (free / stack.unit_volume_m3).floor() as u64
                    } else {
                        u64::MAX
                    };
                    let quantity = (duty.quantity - aboard)
                        .min(available(host_inventory, &duty.item))
                        .min(fitting);
                    if quantity > 0 {
                        industry::execute(
                            world,
                            duty.account,
                            IndustryCommand::Transfer {
                                source: host,
                                target: ship,
                                item: duty.item.clone(),
                                quantity,
                            },
                            None,
                        )?;
                    }
                }
                return Ok(());
            }
            if duty.stage == HaulStage::Unloading && aboard > 0 {
                industry::execute(
                    world,
                    duty.account,
                    IndustryCommand::Transfer {
                        source: ship,
                        target: host,
                        item: duty.item.clone(),
                        quantity: aboard,
                    },
                    None,
                )?;
                return Ok(());
            }

            for resource in &state.presentation.inventory {
                let capacity = (resource.capacity_kg / resource.unit_mass_kg).floor() as u64;
                let quantity = capacity.saturating_sub(resource.quantity).min(available(
                    host_inventory,
                    &CargoItem::Resource(resource.resource.clone()),
                ));
                if quantity > 0 {
                    industry::execute(
                        world,
                        duty.account,
                        IndustryCommand::Refill {
                            source: host,
                            ship,
                            resource: resource.resource.clone(),
                            quantity,
                        },
                        None,
                    )?;
                }
            }
            let ready = commands::telemetry(world, duty.account, ship)?;
            if ready.telemetry.battery_j < ready.presentation.battery_capacity_j / 2 {
                return Ok(());
            }
            ensure!(
                ready
                    .presentation
                    .inventory
                    .iter()
                    .filter(|resource| ready
                        .presentation
                        .propulsion
                        .propellants
                        .contains(&resource.resource))
                    .all(|resource| resource.amount_kg >= resource.capacity_kg * 0.1),
                "freighter needs propellant before its next departure"
            );
            let destination = if duty.stage == HaulStage::Loading {
                duty.destination
            } else {
                duty.source
            };
            commands::execute(
                world,
                duty.account,
                ship,
                ready.telemetry.authority_revision,
                ShipCommand::SetTravel {
                    preferences: PlanningPreferences {
                        fuel_fraction: 0.42,
                        ..Default::default()
                    },
                    engage: true,
                    expected_revision: ready.telemetry.travel.revision,
                    orders: vec![Order::Undock, Order::Dock(destination)],
                },
            )?;
            duty.stage = if duty.stage == HaulStage::Loading {
                HaulStage::Outbound
            } else {
                HaulStage::Inbound
            };
            duty.problem = None;
            info!(%ship, %destination, "Freighter departed on a physical cargo run");
        }
        HaulStage::Paused => {}
    }
    Ok(())
}

fn available(inventory: &FacilityView, item: &CargoItem) -> u64 {
    inventory
        .items
        .iter()
        .find(|stack| &stack.item == item)
        .map_or(0, |stack| stack.quantity.saturating_sub(stack.reserved))
}
