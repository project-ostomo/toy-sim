use super::*;

#[cfg(test)]
mod tests;

#[derive(Resource, Default)]
pub(super) struct InventoryDirectory(BTreeMap<Id, Entity>);

pub(super) fn refresh(world: &mut World) {
    let entries = world
        .query::<(Entity, &identity::Identity, &hardware::ShipInventory)>()
        .iter(world)
        .map(|(entity, id, _)| (id.0, entity))
        .collect();
    world.insert_resource(InventoryDirectory(entries));
}

fn access(world: &World, account: AccountId, entity: Entity) -> Option<(bool, bool)> {
    if !available(world, entity) {
        return None;
    }
    let manage = ownership::can_access(world, account, entity, Permission::Industry);
    let transfer = ownership::can_access(world, account, entity, Permission::TransferCargo);
    let view = ownership::can_access(world, account, entity, Permission::View);
    (manage || transfer || view).then_some((manage, transfer))
}

fn name(world: &World, entity: Entity) -> String {
    let name = world
        .get::<vessel::Vessel>(entity)
        .map_or_else(|| "Inventory".into(), |ship| ship.vessel_name.to_string());
    let mut bounded = String::new();
    for character in name.chars().filter(|character| !character.is_control()) {
        if bounded.len() + character.len_utf8() > 128 {
            break;
        }
        bounded.push(character);
    }
    if bounded.trim().is_empty() {
        "Inventory".into()
    } else {
        bounded
    }
}

fn capabilities(world: &World, entity: Entity) -> Vec<FacilityCapability> {
    let Some(design) = world.get::<vessel::ShipDesign>(entity) else {
        return Vec::new();
    };
    let devices = world.get::<hardware::PartDevices>(entity);
    design
        .0
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| {
            let (capability, power_per_lane_w, lanes, max_radius_m) =
                match part.definition.equipment {
                    Equipment::Utility {
                        utility:
                            UtilityDef::Factory {
                                capability,
                                power_per_lane_w,
                                lanes,
                            },
                    } => (capability, power_per_lane_w, lanes, None),
                    Equipment::Utility {
                        utility:
                            UtilityDef::Shipyard {
                                power_per_lane_w,
                                lanes,
                                max_radius_m,
                            },
                    } => (
                        IndustryCapability::Shipyard,
                        power_per_lane_w,
                        lanes,
                        Some(max_radius_m),
                    ),
                    _ => return None,
                };
            let operational = devices
                .and_then(|devices| devices.0.get(index))
                .and_then(|device| world.get::<hardware::Device>(*device))
                .is_some_and(|device| device.0.operational)
                && world.get::<travel::Dormant>(entity).is_none();
            Some(FacilityCapability {
                part: part.placed.id,
                capability,
                lanes,
                power_per_lane_w,
                max_radius_m,
                operational,
            })
        })
        .collect()
}

fn summary(
    world: &World,
    entity: Entity,
    can_manage: bool,
    can_transfer: bool,
) -> Option<FacilitySummary> {
    let id = world.get::<identity::Identity>(entity)?.0;
    let owner = world.get::<ownership::AssetOwner>(entity)?.0;
    let capabilities = capabilities(world, entity)
        .into_iter()
        .map(|module| module.capability)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    Some(FacilitySummary {
        entity: id,
        owner,
        name: name(world, entity),
        location: inventory_location(world, entity),
        capabilities,
        can_manage,
        can_transfer,
    })
}

fn hangar(world: &World, account: AccountId, interest: &HangarSubscription) -> Option<HangarView> {
    use osg_model::travel::Presence;

    let ship = crate::sim::commands::observe(world, account, interest.ship).ok()?;
    if !available(world, ship) {
        return None;
    }
    let host = match world.get::<travel::PresenceState>(ship)?.0 {
        Presence::Docked { host, .. } => identity::lookup(world, host).ok()?,
        Presence::Space
            if world.get::<travel::StoredShips>(ship).is_some()
                || world
                    .get::<travel::DockingBays>(ship)
                    .is_some_and(|bays| !bays.0.is_empty()) =>
        {
            ship
        }
        _ => return None,
    };
    if !available(world, host) {
        return None;
    }

    let host_id = world.get::<identity::Identity>(host)?.0;
    let host_inventory = access(world, account, host)
        .filter(|_| world.get::<hardware::ShipInventory>(host).is_some())
        .and_then(|(manage, transfer)| summary(world, host, manage, transfer));
    let mut entries = Vec::new();
    if let Some(stored) = world.get::<travel::StoredShips>(host) {
        for entity in stored.iter() {
            let Some(id) = world.get::<identity::Identity>(entity).map(|id| id.0) else {
                continue;
            };
            if interest.after.is_some_and(|after| id <= after)
                || !available(world, entity)
                || !matches!(
                    world.get::<travel::PresenceState>(entity).map(|presence| &presence.0),
                    Some(Presence::Docked { host, .. }) if *host == host_id
                )
            {
                continue;
            }

            let can_focus = crate::sim::commands::observe(world, account, id).is_ok();
            let inventory_access = access(world, account, entity)
                .filter(|_| world.get::<hardware::ShipInventory>(entity).is_some());
            if !can_focus && inventory_access.is_none() {
                continue;
            }
            let (can_manage, can_transfer) = inventory_access.unwrap_or_default();
            let Some(inventory) = summary(world, entity, can_manage, can_transfer) else {
                continue;
            };
            entries.push(HangarEntry {
                inventory,
                can_focus,
                can_open_inventory: inventory_access.is_some(),
                can_control: can_focus
                    && ownership::can_access(world, account, entity, Permission::Control),
            });
        }
    }

    entries.sort_unstable_by_key(|entry| entry.inventory.entity);
    let more = entries.len() > MAX_DIRECTORY_ENTRIES;
    entries.truncate(MAX_DIRECTORY_ENTRIES);
    let next = more.then(|| entries.last().unwrap().inventory.entity);
    Some(HangarView {
        ship: interest.ship,
        host: host_id,
        host_name: name(world, host),
        host_inventory,
        ships: entries,
        next,
    })
}

pub fn snapshot(
    world: &World,
    account: AccountId,
    subscription: &IndustrySubscription,
) -> IndustrySnapshot {
    let mut snapshot = IndustrySnapshot {
        subscription_revision: subscription.revision,
        hangar: subscription
            .hangar
            .as_ref()
            .and_then(|interest| hangar(world, account, interest)),
        ..Default::default()
    };
    let Some(directory) = world.get_resource::<InventoryDirectory>() else {
        return snapshot;
    };
    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    if subscription.directory {
        for (&id, &entity) in &directory.0 {
            if subscription
                .directory_after
                .is_some_and(|after| id <= after)
            {
                continue;
            }
            let Some((can_manage, can_transfer)) = access(world, account, entity) else {
                continue;
            };
            let Some(summary) = summary(world, entity, can_manage, can_transfer) else {
                continue;
            };
            if snapshot.directory.len() == MAX_DIRECTORY_ENTRIES {
                snapshot.directory_next = snapshot.directory.last().map(|entry| entry.entity);
                break;
            }
            snapshot.directory.push(summary);
        }
    }

    let mut seen = BTreeSet::new();
    for id in subscription
        .inventories
        .iter()
        .take(MAX_SUBSCRIBED_INVENTORIES)
    {
        if !seen.insert(*id) {
            continue;
        }
        let Ok(entity) = identity::lookup(world, *id) else {
            continue;
        };
        let Some((can_manage, can_transfer)) = access(world, account, entity) else {
            continue;
        };
        let (Some(design), Some(inventory), Some(owner)) = (
            world.get::<vessel::ShipDesign>(entity),
            world.get::<hardware::ShipInventory>(entity),
            world.get::<ownership::AssetOwner>(entity),
        ) else {
            continue;
        };
        let items = inventory
            .0
            .cargo_stacks(catalogue)
            .expect("valid authoritative cargo inventory");
        snapshot.facilities.push(FacilityView {
            entity: *id,
            owner: owner.0,
            name: name(world, entity),
            can_manage,
            can_transfer,
            cargo_capacity_m3: design.0.capacity_m3,
            cargo_used_m3: inventory.0.cargo_volume(catalogue),
            items,
            products: inventory
                .0
                .product_stacks(catalogue)
                .expect("valid authoritative product reservoirs"),
            jobs: world
                .get::<IndustryFacility>(entity)
                .map(|queue| queue.jobs.iter().map(|job| job.view.clone()).collect())
                .unwrap_or_default(),
            capabilities: capabilities(world, entity),
            location: inventory_location(world, entity),
        });
    }

    if subscription.catalogue {
        snapshot.catalogue = world
            .get_resource::<CatalogueCache>()
            .map(|cache| cache.0.clone());
    }
    snapshot
}
