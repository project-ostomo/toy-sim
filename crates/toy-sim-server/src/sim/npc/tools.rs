use super::state::NpcOrganization;
use crate::sim::{commands, gas::GasLedger, industry, ownership};
use anyhow::{Context, Result, ensure};
use bevy::prelude::World;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toy_sim_model::{
    ContactRef, Id, ProgramQuery, ProgramReply, ShipCommand, Tag, TrackQuery,
    industry::{CargoItem, IndustryCommand, IndustrySubscription},
    ownership::Principal,
    travel::{Destination, Guidance, GuidanceMode, Order, PlanningPreferences, Target},
};

pub const MAX_BATCH: usize = 4;
const RESULT_BYTES: usize = 8 * 1024;
const QUERY_GAS: u64 = 20_000;

fn default_limit() -> usize {
    8
}

fn default_radius() -> f64 {
    1_000_000.0
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum Query {
    Ships {
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Ship {
        ship: String,
    },
    Contacts {
        ship: String,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default = "default_radius")]
        radius_m: f64,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        faction: Option<String>,
    },
    Beacons {
        ship: String,
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Navigation {
        ship: String,
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Facilities {
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Inventory {
        inventory: String,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Recipes {
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Blueprints {
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "order", rename_all = "snake_case", deny_unknown_fields)]
pub enum NavigationOrder {
    Travel {
        beacon: String,
    },
    Dock {
        beacon: String,
    },
    Undock,
    KeepRange {
        group: String,
        track: String,
        range_m: f64,
    },
    Wait {
        until_tick: u64,
    },
}

impl NavigationOrder {
    fn decode(&self) -> Result<Order> {
        Ok(match self {
            Self::Travel { beacon } => Order::TravelTo(Destination::Beacon(id(beacon)?)),
            Self::Dock { beacon } => Order::Dock(id(beacon)?),
            Self::Undock => Order::Undock,
            Self::Wait { until_tick } => Order::WaitUntil(*until_tick),
            Self::KeepRange {
                group,
                track,
                range_m,
            } => Order::Guidance(Guidance {
                mode: GuidanceMode::KeepRange,
                target: Target::Contact(ContactRef {
                    group: id(group)?,
                    track: id(track)?,
                }),
                range_m: *range_m,
            }),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Queue {
        ship: String,
        authority_revision: u64,
        travel_revision: u64,
        #[serde(default = "default_fuel_priority")]
        fuel_priority: f64,
        orders: Vec<NavigationOrder>,
    },
    DockServices {
        ship: String,
        authority_revision: u64,
        cargo: bool,
        power: bool,
    },
    Weapons {
        ship: String,
        authority_revision: u64,
        #[serde(default)]
        group: Option<String>,
        #[serde(default)]
        track: Option<String>,
        fire: bool,
    },
    Transfer {
        source: String,
        target: String,
        kind: String,
        item: String,
        quantity: u64,
    },
    Refill {
        source: String,
        ship: String,
        resource: String,
        quantity: u64,
    },
    UnloadProduct {
        source: String,
        target: String,
        resource: String,
        quantity: u64,
    },
    Recipe {
        facility: String,
        recipe: String,
        batches: u32,
    },
    Build {
        facility: String,
        blueprint: String,
    },
    CancelJob {
        facility: String,
        job: String,
    },
}

fn default_fuel_priority() -> f64 {
    1.0
}

pub fn authorize(world: &World, organization: &NpcOrganization) -> Result<()> {
    ensure!(
        world.resource::<ownership::Directory>().0.administers(
            organization.officer,
            Principal::Organization(organization.organization),
        ),
        "director officer authority revoked"
    );
    Ok(())
}

fn id(value: &str) -> Result<Id> {
    value.parse().context("expected a UUID")
}

fn page_size(limit: usize) -> Result<usize> {
    ensure!((1..=16).contains(&limit), "page limit must be 1–16");
    Ok(limit)
}

fn charge(world: &World, organization: &NpcOrganization, gas: u64) -> Result<()> {
    world
        .resource::<GasLedger>()
        .reserve(Principal::Organization(organization.organization), gas)?
        .settle(gas)
}

fn item(value: &CargoItem) -> Value {
    match value {
        CargoItem::Resource(id) => json!({"kind": "resource", "item": id}),
        CargoItem::Part(id) => json!({"kind": "part", "item": id}),
    }
}

fn ship(world: &World, organization: &NpcOrganization, ship: Id) -> Result<Value> {
    let state = commands::telemetry(world, organization.officer, ship)?;
    let private = &state.telemetry;
    let status = &state.presentation;
    Ok(json!({
        "ship": ship.to_string(),
        "name": state.name,
        "group": state.group.to_string(),
        "authority_revision": private.authority_revision,
        "travel_revision": private.travel.revision,
        "presence": private.presence,
        "pose": private.pose,
        "travel_status": private.travel.status,
        "autopilot_enabled": private.travel.autopilot_enabled,
        "current_order": private.travel.order,
        "orders": private.travel.orders.iter().skip(private.travel.order).take(4).collect::<Vec<_>>(),
        "orders_remaining": private.travel.orders.len().saturating_sub(private.travel.order),
        "fuel_budget": private.travel.fuel_budget,
        "battery_j": private.battery_j,
        "battery_capacity_j": status.battery_capacity_j,
        "computer": status.computer,
        "health": status.health,
        "mass_kg": status.mass_kg,
        "shield_temperature_k": private.shield_temperature_k,
        "consumables": status.inventory.iter().map(|resource| json!({
            "resource": resource.resource,
            "quantity": resource.quantity,
            "amount_kg": resource.amount_kg,
            "capacity_kg": resource.capacity_kg,
        })).take(24).collect::<Vec<_>>(),
        "consumables_omitted": status.inventory.len().saturating_sub(24),
        "cargo_used_m3": status.cargo_used_m3,
        "cargo_capacity_m3": status.cargo_capacity_m3,
    }))
}

fn ships(world: &World, organization: &NpcOrganization, offset: usize, limit: usize) -> Value {
    let ships = organization.assets.iter().skip(offset).take(limit).filter_map(|asset| {
        let state = commands::telemetry(world, organization.officer, asset.id).ok()?;
        Some(json!({
            "ship": asset.id.to_string(),
            "role": asset.role,
            "name": state.name,
            "authority_revision": state.telemetry.authority_revision,
            "travel_revision": state.telemetry.travel.revision,
            "presence": state.telemetry.presence,
            "travel_status": state.telemetry.travel.status,
            "autopilot_enabled": state.telemetry.travel.autopilot_enabled,
            "orders_remaining": state.telemetry.travel.orders.len().saturating_sub(state.telemetry.travel.order),
            "cargo_used_m3": state.presentation.cargo_used_m3,
            "cargo_capacity_m3": state.presentation.cargo_capacity_m3,
        }))
    }).collect::<Vec<_>>();
    let next = offset.saturating_add(limit);
    json!({
        "ships": ships,
        "next_offset": (next < organization.assets.len()).then_some(next),
    })
}

fn world_query(
    world: &mut World,
    organization: &NpcOrganization,
    ship: Id,
    query: ProgramQuery,
) -> Result<ProgramReply> {
    let source = commands::source(world, organization.officer, ship)?;
    charge(world, organization, source.query_work(&query)?)?;
    source.query(query, false, 64 * 1024)
}

pub fn query(world: &mut World, organization: &NpcOrganization, query: &Query) -> Result<Value> {
    authorize(world, organization)?;
    charge(world, organization, QUERY_GAS)?;

    let value = match query {
        Query::Ships { offset, limit } => ships(world, organization, *offset, page_size(*limit)?),
        Query::Ship { ship: target } => ship(world, organization, id(target)?)?,
        Query::Contacts {
            ship,
            offset,
            limit,
            radius_m,
            kind,
            faction,
        } => {
            let limit = page_size(*limit)?;
            ensure!(
                *offset <= 256,
                "contact offset exceeds 256; narrow the radius or tags"
            );
            ensure!(
                radius_m.is_finite() && *radius_m > 0.0 && *radius_m <= 1e22,
                "invalid contact radius"
            );
            let ship = id(ship)?;
            let state = commands::telemetry(world, organization.officer, ship)?;
            let source = commands::source(world, organization.officer, ship)?;
            let pose = if let Some(pose) = state.telemetry.pose.clone() {
                pose
            } else {
                let travel = ProgramQuery::Travel;
                charge(world, organization, source.query_work(&travel)?)?;
                let ProgramReply::Travel { pose, .. } = source.query(travel, false, 64 * 1024)?
                else {
                    anyhow::bail!("travel observation unavailable");
                };
                pose
            };
            let mut tags = std::collections::BTreeSet::new();
            if let Some(kind) = kind {
                let tag = Tag::Kind(kind.clone());
                ensure!(tag.valid(), "invalid contact kind");
                tags.insert(tag);
            }
            if let Some(faction) = faction {
                tags.insert(Tag::IffFaction(id(faction)?));
            }
            let mut request = ProgramQuery::Tracks(TrackQuery {
                sphere: Some((pose.position, *radius_m)),
                all: tags,
                limit: 16,
                work: 20_000,
                max_age_ticks: Some(600),
                ..Default::default()
            });
            let mut tracks = Vec::new();
            let mut visited = 0;
            let mut revision = 0;
            let mut complete = false;

            for _ in 0..18 {
                charge(world, organization, source.query_work(&request)?)?;
                let ProgramReply::Tracks(page) = source.query(request, false, 16 * 1024)? else {
                    anyhow::bail!("unexpected contact reply");
                };
                revision = page.revision;
                for track in page.tracks {
                    if visited >= *offset && tracks.len() < limit {
                        tracks.push(json!({
                            "track": track.id.to_string(),
                            "known_entity": track.entity.map(|id| id.to_string()),
                            "distance_m": track.pose.position.relative_to(pose.position).length(),
                            "pose": track.pose,
                            "position_sigma_m": track.position_sigma_m,
                            "velocity_sigma_m_s": track.velocity_sigma_m_s,
                            "observed_tick": track.observed_tick,
                            "tags": track.tags,
                            "provenance": track.provenance,
                        }));
                    }
                    visited += 1;
                }
                complete = page.continuation.is_none();
                if tracks.len() == limit || complete {
                    break;
                }
                request = ProgramQuery::Continue {
                    cursor: page.continuation.unwrap(),
                    work: 20_000,
                };
            }
            let next = offset.saturating_add(tracks.len());
            json!({
                "group": state.group.to_string(),
                "revision": revision,
                "tracks": tracks,
                "next_offset": (next < visited || !complete).then_some(next),
                "incomplete": !complete,
                "narrow_filter": !complete && tracks.len() < limit,
            })
        }
        Query::Beacons { ship, after, limit } => {
            let query = ProgramQuery::Beacons {
                after: after.as_deref().map(id).transpose()?,
                limit: page_size(*limit)? as u16,
            };
            let ProgramReply::Beacons(beacons) =
                world_query(world, organization, id(ship)?, query)?
            else {
                anyhow::bail!("unexpected beacon reply");
            };
            json!({
                "beacons": beacons.iter().map(|beacon| json!({
                    "beacon": beacon.entity.to_string(),
                    "labels": beacon.iff.labels,
                    "owner": beacon.iff.owner.to_string(),
                    "organization": beacon.iff.faction.map(|id| id.to_string()),
                    "pose": beacon.pose,
                    "radius_m": beacon.radius_m,
                    "has_docking": !beacon.bays.is_empty(),
                    "gate_exit": beacon.gate_exit.map(|id| id.to_string()),
                    "exclusion_m": beacon.exclusion_m,
                })).collect::<Vec<_>>(),
                "after": beacons.last().map(|beacon| beacon.entity.to_string()),
            })
        }
        Query::Navigation { ship, after, limit } => {
            let ship = id(ship)?;
            let ProgramReply::Travel { pose, .. } =
                world_query(world, organization, ship, ProgramQuery::Travel)?
            else {
                anyhow::bail!("travel observation unavailable");
            };
            let query = ProgramQuery::Navigation {
                after: after.as_deref().map(id).transpose()?,
                limit: page_size(*limit)? as u16,
                reference: pose.position,
            };
            let ProgramReply::Navigation { revision, gates } =
                world_query(world, organization, ship, query)?
            else {
                anyhow::bail!("navigation unavailable");
            };
            json!({
                "revision": revision,
                "gates": gates.iter().map(|gate| json!({
                    "beacon": gate.entity.to_string(),
                    "system": gate.system.to_string(),
                    "exit": gate.exit.to_string(),
                    "pose": gate.pose,
                    "staging": gate.staging,
                    "slip_ready": gate.slip_ready,
                })).collect::<Vec<_>>(),
                "after": gates.last().map(|gate| gate.entity.to_string()),
            })
        }
        Query::Facilities { after, limit } => {
            let limit = page_size(*limit)?;
            let snapshot = industry::snapshot(
                world,
                organization.officer,
                &IndustrySubscription {
                    directory: true,
                    directory_after: after.as_deref().map(id).transpose()?,
                    ..Default::default()
                },
            );
            let entries = snapshot
                .directory
                .iter()
                .take(limit)
                .map(|facility| {
                    json!({
                        "inventory": facility.entity.to_string(),
                        "name": facility.name,
                        "location": facility.location.map(|id| id.to_string()),
                        "capabilities": facility.capabilities,
                        "can_manage": facility.can_manage,
                        "can_transfer": facility.can_transfer,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "facilities": entries,
                "after": snapshot.directory.iter().take(limit).last().map(|facility| facility.entity.to_string()),
            })
        }
        Query::Inventory {
            inventory,
            offset,
            limit,
        } => {
            let limit = page_size(*limit)?;
            let snapshot = industry::snapshot(
                world,
                organization.officer,
                &IndustrySubscription {
                    inventories: vec![id(inventory)?],
                    ..Default::default()
                },
            );
            let inventory = snapshot
                .facilities
                .first()
                .context("inventory unavailable or unauthorized")?;
            let next = offset.saturating_add(limit);
            json!({
                "inventory": inventory.entity.to_string(),
                "name": inventory.name,
                "location": inventory.location.map(|id| id.to_string()),
                "can_manage": inventory.can_manage,
                "can_transfer": inventory.can_transfer,
                "capacity_m3": inventory.cargo_capacity_m3,
                "used_m3": inventory.cargo_used_m3,
                "items": inventory.items.iter().skip(*offset).take(limit).map(|stack| json!({
                    "item": item(&stack.item),
                    "quantity": stack.quantity,
                    "reserved": stack.reserved,
                })).collect::<Vec<_>>(),
                "products": inventory.products.iter().skip(*offset).take(limit).map(|stack| json!({
                    "item": item(&stack.item),
                    "quantity": stack.quantity,
                })).collect::<Vec<_>>(),
                "next_offset": (next < inventory.items.len().max(inventory.products.len())).then_some(next),
                "jobs": inventory.jobs.iter().take(8).map(|job| json!({
                    "job": job.id.to_string(),
                    "name": job.name,
                    "status": job.status,
                    "progress_ticks": job.progress_ticks,
                    "duration_ticks": job.duration_ticks,
                })).collect::<Vec<_>>(),
                "jobs_omitted": inventory.jobs.len().saturating_sub(8),
                "capabilities": inventory.capabilities.iter().take(16).collect::<Vec<_>>(),
            })
        }
        Query::Recipes { after, limit } => {
            let limit = page_size(*limit)?;
            let snapshot = industry::snapshot(
                world,
                organization.officer,
                &IndustrySubscription {
                    catalogue: true,
                    ..Default::default()
                },
            );
            let catalogue = snapshot
                .catalogue
                .context("industry catalogue unavailable")?;
            let mut recipes = catalogue
                .recipes
                .iter()
                .filter(|recipe| after.as_ref().is_none_or(|after| recipe.id > *after))
                .collect::<Vec<_>>();
            recipes.sort_unstable_by(|a, b| a.id.cmp(&b.id));
            recipes.truncate(limit);
            json!({"recipes": recipes, "after": recipes.last().map(|recipe| &recipe.id)})
        }
        Query::Blueprints { after, limit } => {
            let limit = page_size(*limit)?;
            let snapshot = industry::snapshot(
                world,
                organization.officer,
                &IndustrySubscription {
                    catalogue: true,
                    ..Default::default()
                },
            );
            let catalogue = snapshot
                .catalogue
                .context("industry catalogue unavailable")?;
            let mut blueprints = catalogue
                .blueprints
                .iter()
                .filter(|blueprint| after.as_ref().is_none_or(|after| blueprint.name > *after))
                .collect::<Vec<_>>();
            blueprints.sort_unstable_by(|a, b| a.name.cmp(&b.name));
            blueprints.truncate(limit);
            json!({
                "blueprints": blueprints.iter().map(|blueprint| json!({
                    "name": blueprint.name,
                    "inputs": blueprint.inputs,
                    "energy_j": blueprint.energy_j,
                    "duration_ticks": blueprint.duration_ticks,
                })).collect::<Vec<_>>(),
                "after": blueprints.last().map(|blueprint| &blueprint.name),
            })
        }
    };

    ensure!(
        serde_json::to_vec(&value)?.len() <= RESULT_BYTES,
        "tool result exceeds limit; retry with a smaller page limit"
    );
    Ok(value)
}

pub fn action(world: &mut World, organization: &NpcOrganization, action: &Action) -> Result<Value> {
    authorize(world, organization)?;
    charge(world, organization, QUERY_GAS)?;
    let account = organization.officer;

    match action {
        Action::Queue {
            ship,
            authority_revision,
            travel_revision,
            fuel_priority,
            orders,
        } => {
            ensure!(
                !orders.is_empty() && orders.len() <= 32,
                "queue needs 1–32 orders"
            );
            let orders = orders
                .iter()
                .map(NavigationOrder::decode)
                .collect::<Result<Vec<_>>>()?;
            commands::execute(
                world,
                account,
                id(ship)?,
                *authority_revision,
                ShipCommand::SetTravel {
                    preferences: PlanningPreferences {
                        fuel_priority: *fuel_priority,
                    },
                    engage: true,
                    expected_revision: *travel_revision,
                    orders,
                },
            )?;
        }
        Action::DockServices {
            ship,
            authority_revision,
            cargo,
            power,
        } => {
            commands::execute(
                world,
                account,
                id(ship)?,
                *authority_revision,
                ShipCommand::SetDockServices {
                    cargo: *cargo,
                    power: *power,
                },
            )?;
        }
        Action::Weapons {
            ship,
            authority_revision,
            group,
            track,
            fire,
        } => {
            let ship = id(ship)?;
            if *fire {
                let command = ShipCommand::MarkTarget {
                    group: id(group.as_deref().context("missing group")?)?,
                    track: id(track.as_deref().context("missing track")?)?,
                    maximum_flight_time_s: 2.0,
                };
                commands::execute(world, account, ship, *authority_revision, command)?;
                commands::execute(
                    world,
                    account,
                    ship,
                    *authority_revision,
                    ShipCommand::StartFiring,
                )?;
            } else {
                commands::execute(
                    world,
                    account,
                    ship,
                    *authority_revision,
                    ShipCommand::StopFiring,
                )?;
            }
        }
        Action::Transfer {
            source,
            target,
            kind,
            item,
            quantity,
        } => {
            ensure!(item.len() <= 128, "item name too long");
            let item = match kind.as_str() {
                "resource" => CargoItem::Resource(item.clone()),
                "part" => CargoItem::Part(item.clone()),
                _ => anyhow::bail!("item kind must be resource or part"),
            };
            industry::execute(
                world,
                account,
                IndustryCommand::Transfer {
                    source: id(source)?,
                    target: id(target)?,
                    item,
                    quantity: *quantity,
                },
            )?;
        }
        Action::UnloadProduct {
            source,
            target,
            resource,
            quantity,
        } => {
            industry::execute(
                world,
                account,
                IndustryCommand::UnloadProduct {
                    source: id(source)?,
                    target: id(target)?,
                    resource: resource.clone(),
                    quantity: *quantity,
                },
            )?;
        }
        Action::Refill {
            source,
            ship,
            resource,
            quantity,
        } => {
            industry::execute(
                world,
                account,
                IndustryCommand::Refill {
                    source: id(source)?,
                    ship: id(ship)?,
                    resource: resource.clone(),
                    quantity: *quantity,
                },
            )?;
        }
        Action::Recipe {
            facility,
            recipe,
            batches,
        } => {
            industry::execute(
                world,
                account,
                IndustryCommand::StartRecipe {
                    facility: id(facility)?,
                    recipe: recipe.clone(),
                    batches: *batches,
                },
            )?;
        }
        Action::Build {
            facility,
            blueprint,
        } => {
            let snapshot = industry::snapshot(
                world,
                account,
                &IndustrySubscription {
                    catalogue: true,
                    ..Default::default()
                },
            );
            let catalogue = snapshot
                .catalogue
                .context("industry catalogue unavailable")?;
            let blueprint = catalogue
                .blueprints
                .into_iter()
                .find(|known| known.name == *blueprint)
                .context("unknown public blueprint")?;
            industry::execute(
                world,
                account,
                IndustryCommand::BuildShip {
                    facility: id(facility)?,
                    owner: Principal::Organization(organization.organization),
                    blueprint: blueprint.blueprint,
                },
            )?;
        }
        Action::CancelJob { facility, job } => {
            industry::execute(
                world,
                account,
                IndustryCommand::CancelJob {
                    facility: id(facility)?,
                    job: id(job)?,
                },
            )?;
        }
    }
    Ok(json!({"applied": true}))
}
