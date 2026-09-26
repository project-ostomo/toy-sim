use super::{hardware, identity, ownership, travel, vessel};
use bevy::prelude::*;
use osg_model::{
    AccountId, Id, assets::*, industry::CargoItem, ownership::Permission, travel::Presence,
};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

fn name(world: &World, id: Id) -> String {
    let Ok(entity) = identity::lookup(world, id) else {
        return id.to_string();
    };
    if let Some(vessel) = world.get::<vessel::Vessel>(entity) {
        return vessel.vessel_name.to_string();
    }
    world
        .get::<super::infrastructure::Landmark>(entity)
        .map_or_else(|| id.to_string(), |landmark| landmark.name.clone())
}

fn space_location(world: &World, entity: Entity) -> String {
    system_name(world, entity).map_or_else(|| "In space".into(), |name| format!("Near {name}"))
}

fn system_name(world: &World, entity: Entity) -> Option<String> {
    world
        .get_resource::<super::orrery::Universe>()
        .and_then(|universe| {
            let position = world
                .get::<super::precision::PreciseTransform>(entity)?
                .translation_um;
            let index = universe.index().nearest(position)?;
            Some(universe.systems()[index].name.to_string())
        })
}

fn telemetry(world: &World, entity: Entity) -> Option<AssetTelemetry> {
    let inventory = &world.get::<hardware::ShipInventory>(entity)?.0;
    let design = &world.get::<vessel::ShipDesign>(entity)?.0;
    let catalogue = &world.get_resource::<vessel::ShipCatalogue>()?.0;
    let consumables = catalogue
        .resources
        .iter()
        .enumerate()
        .filter_map(|(index, resource)| {
            let volume = inventory
                .tank_capacities_m3
                .get(index)
                .copied()
                .unwrap_or(0.);
            if volume <= 0. {
                return None;
            }
            Some(AssetConsumable {
                resource: resource.id.clone(),
                name: resource.title.clone(),
                quantity: inventory.quantities.get(index).copied().unwrap_or(0),
                capacity: (volume / resource.volume_m3).floor() as u64,
            })
        })
        .collect();
    Some(AssetTelemetry {
        cargo_used_m3: inventory.cargo_volume(catalogue),
        cargo_capacity_m3: design.capacity_m3,
        energy_j: inventory.energy_j,
        energy_capacity_j: design.battery_j,
        consumables,
    })
}

fn accessible(
    world: &World,
    account: AccountId,
    owner_filter: Option<osg_model::ownership::Principal>,
    after: Option<Id>,
) -> impl Iterator<Item = (Id, Entity, osg_model::ownership::Principal, bool, bool)> + '_ {
    let society = world.resource::<super::society::SocietyState>();
    world
        .resource::<super::society::AssetRecords>()
        .visible(&society.directory.0, account, owner_filter, after)
        .filter_map(move |record| {
            let (id, entity) = (record.id, record.entity);
            let Some(owner) = world
                .get::<ownership::AssetOwner>(entity)
                .map(|owner| owner.0)
            else {
                return None;
            };
            if owner_filter.is_some_and(|selected| selected != owner) {
                return None;
            }
            let permits = |permission| ownership::can_access(world, account, entity, permission);
            let can_manage = permits(Permission::ManageAccess);
            let can_open = permits(Permission::View)
                || permits(Permission::TransferCargo)
                || permits(Permission::Industry);
            if !can_manage && !can_open {
                return None;
            }

            Some((id, entity, owner, can_manage, can_open))
        })
}

pub fn list(
    world: &World,
    account: AccountId,
    search: &str,
    owner_filter: Option<osg_model::ownership::Principal>,
    after: Option<Id>,
    limit: usize,
) -> Vec<AssetSummary> {
    let search = search.to_lowercase();
    let mut assets = Vec::new();
    for (id, entity, owner, can_manage, can_open) in accessible(world, account, owner_filter, after)
    {
        if assets.len() >= limit {
            break;
        }
        let permits = |permission| ownership::can_access(world, account, entity, permission);
        let presence = world
            .get::<travel::PresenceState>(entity)
            .map(|state| &state.0);
        let (location, status) = match presence {
            Some(Presence::Docked { host, .. }) => (name(world, *host), "Docked"),
            Some(Presence::StoredInWreck(host)) => (name(world, *host), "In wreck"),
            Some(Presence::SlipTransit(_)) => ("In slip transit".into(), "In transit"),
            Some(Presence::Destroyed) => ("Wrecks".into(), "Destroyed"),
            _ => (space_location(world, entity), "In space"),
        };
        let host = match presence {
            Some(Presence::Docked { host, .. } | Presence::StoredInWreck(host)) => Some(*host),
            _ => None,
        };
        let location_entity = host
            .and_then(|id| identity::lookup(world, id).ok())
            .unwrap_or(entity);
        let kind = if world
            .get::<super::infrastructure::Landmark>(entity)
            .is_some()
        {
            AssetKind::Installation
        } else {
            AssetKind::Ship
        };
        let asset_name = name(world, id);
        let matches = format!("{asset_name} {location} {status} {kind:?}")
            .to_lowercase()
            .contains(&search);
        if matches {
            assets.push(AssetSummary {
                id,
                name: asset_name,
                owner,
                kind,
                location,
                system: if matches!(
                    presence,
                    Some(Presence::SlipTransit(_) | Presence::Destroyed)
                ) {
                    None
                } else {
                    system_name(world, location_entity)
                },
                host,
                telemetry: permits(Permission::View)
                    .then(|| telemetry(world, entity))
                    .flatten(),
                status: status.into(),
                can_manage,
                can_open: can_open && world.get::<hardware::ShipInventory>(entity).is_some(),
                can_focus: permits(Permission::Control) && kind == AssetKind::Ship,
            });
        }
    }
    assets
}

fn visit_stock(
    world: &World,
    account: AccountId,
    owner_filter: Option<osg_model::ownership::Principal>,
    mut add_stock: impl FnMut(CargoItem, String, StockLocation),
) {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    let catalogue = world.get_resource::<vessel::ShipCatalogue>();
    let mut public_reserved =
        BTreeMap::<(Id, osg_model::ownership::Principal, CargoItem), u64>::new();
    let assets = world.resource::<super::society::AssetRecords>();
    let visible = assets.visible(directory, account, owner_filter, None);
    let relevant: BTreeMap<_, _> = visible
        .into_iter()
        .chain(
            directory
                .administered(account)
                .flat_map(|owner| assets.customers(owner)),
        )
        .map(|record| (record.id, record))
        .collect();
    for (station, record) in relevant {
        for ((owner, item), quantity) in &record.inputs {
            public_reserved.insert((station, *owner, item.clone()), *quantity);
        }
    }

    for (id, entity, owner, _, can_open) in accessible(world, account, owner_filter, None) {
        if can_open {
            if let (Some(inventory), Some(catalogue)) =
                (world.get::<hardware::ShipInventory>(entity), catalogue)
            {
                if let Ok(stacks) = inventory.0.cargo_stacks(&catalogue.0) {
                    for stack in stacks {
                        // Custody belongs to storage principals, not the station owner.
                        let custody = inventory.0.custody.get(&stack.item).copied().unwrap_or(0);
                        let held_for_customers: u64 = public_reserved
                            .iter()
                            .filter(|((station, _, item), _)| *station == id && item == &stack.item)
                            .map(|(_, quantity)| *quantity)
                            .sum();
                        add_stock(
                            stack.item,
                            stack.name,
                            StockLocation {
                                key: StockKey {
                                    entity: id,
                                    owner,
                                    storage: false,
                                },
                                name: name(world, id),
                                quantity: stack
                                    .quantity
                                    .saturating_sub(custody)
                                    .saturating_sub(held_for_customers),
                                reserved: stack
                                    .reserved
                                    .saturating_sub(custody)
                                    .saturating_sub(held_for_customers),
                            },
                        );
                    }
                }
            }
        }
    }

    let mut storage: BTreeMap<_, _> = directory
        .administered(account)
        .filter(|owner| owner_filter.is_none_or(|selected| *owner == selected))
        .flat_map(|owner| economy.storage.by_owner(owner, None))
        .map(|(key, items)| (*key, items.clone()))
        .collect();
    for ((station, owner, item), quantity) in &public_reserved {
        *storage
            .entry((*station, *owner))
            .or_default()
            .entry(item.clone())
            .or_default() += quantity;
    }
    for (&(station, owner), stock) in &storage {
        if !directory.administers(account, owner)
            || owner_filter.is_some_and(|selected| selected != owner)
        {
            continue;
        }
        for (item, &quantity) in stock {
            let label = catalogue
                .and_then(|catalogue| osg_ships::industry::item_name(item, &catalogue.0).ok())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{item:?}"));
            add_stock(
                item.clone(),
                label,
                StockLocation {
                    key: StockKey {
                        entity: station,
                        owner,
                        storage: true,
                    },
                    name: name(world, station),
                    quantity,
                    reserved: economy.stock_reserved(owner, station, item).saturating_add(
                        public_reserved
                            .get(&(station, owner, item.clone()))
                            .copied()
                            .unwrap_or(0),
                    ),
                },
            );
        }
    }
}

pub fn goods_totals(
    world: &World,
    account: AccountId,
    search: &str,
    owner: Option<osg_model::ownership::Principal>,
) -> Vec<GoodsSummary> {
    let mut goods = BTreeMap::<CargoItem, GoodsSummary>::new();
    visit_stock(world, account, owner, |item, name, source| {
        if source.quantity == 0 {
            return;
        }
        let total = goods.entry(item.clone()).or_insert_with(|| GoodsSummary {
            item,
            name,
            quantity: 0,
            reserved: 0,
            locations: 0,
        });
        total.quantity += source.quantity as u128;
        total.reserved += source.reserved as u128;
        total.locations += 1;
    });
    let search = search.to_lowercase();
    goods
        .into_values()
        .filter(|goods| goods.name.to_lowercase().contains(&search))
        .collect()
}

pub fn stock_locations(
    world: &World,
    account: AccountId,
    item: &CargoItem,
    owner: Option<osg_model::ownership::Principal>,
) -> Vec<StockLocation> {
    let mut sources = BTreeMap::new();
    visit_stock(world, account, owner, |found, _, source| {
        if &found == item && source.quantity > 0 {
            sources.insert(source.key.clone(), source);
        }
    });
    sources.into_values().collect()
}
