use super::*;
use access::{Asset, colocated, lookup};
use requests::{CargoAction, CargoQueue};

pub fn process_cargo(
    mut society: ResMut<crate::sim::society::SocietyState>,
    mut queue: ResMut<CargoQueue>,
    epoch: Res<identity::WorldEpoch>,
    index: Res<identity::IdentityIndex>,
    catalogue: Res<vessel::ShipCatalogue>,
    assets: Query<Asset>,
    mut inventories: Query<&mut hardware::ShipInventory>,
    mut thermals: Query<&mut hardware::ShipThermal>,
) {
    let state = &mut *society;
    let directory = &state.directory;

    while let Some(request) = queue.0.pop_front() {
        if request.reply.is_closed() {
            continue;
        }
        if request.wrong_world(epoch.0) {
            request.finish(Err(anyhow::anyhow!("World changed; refresh state")));
            continue;
        }
        let result = (|| {
            let (source, target, quantity) = match &request.arguments {
                CargoAction::Transfer {
                    source,
                    target,
                    quantity,
                    ..
                }
                | CargoAction::Refill {
                    source,
                    target,
                    quantity,
                    ..
                }
                | CargoAction::UnloadProduct {
                    source,
                    target,
                    quantity,
                    ..
                } => (*source, *target, *quantity),
            };
            ensure!(quantity > 0, "invalid cargo quantity");
            let source = lookup(&index, source)?;
            let target = lookup(&index, target)?;
            let from_asset = assets.get(source)?;
            let to_asset = assets.get(target)?;
            from_asset.authorize(&directory.0, request.account, Permission::TransferCargo)?;
            to_asset.authorize(&directory.0, request.account, Permission::TransferCargo)?;
            colocated(&from_asset, &to_asset)?;
            let mut from = inventories.get(source)?.0.clone();
            let mut to = inventories.get(target)?.0.clone();
            let capacity = to_asset.design.0.capacity_m3;
            let mut thermal = None;

            match &request.arguments {
                CargoAction::Transfer { item, .. } => {
                    ensure!(source != target, "invalid cargo transfer");
                    from.transfer_item(&mut to, item, quantity, capacity, &catalogue.0)?;
                }
                CargoAction::Refill { resource, .. } => {
                    let resource_index = catalogue
                        .0
                        .resources
                        .iter()
                        .position(|entry| entry.id == *resource)
                        .context("unknown resource")?;
                    if resource == "shield_coolant" {
                        let capacity = osg_ships::industry::mass_mg(
                            to_asset.design.0.shield_reserve_capacity_kg,
                        )?;
                        let unit = osg_ships::industry::mass_mg(
                            catalogue.0.resources[resource_index].mass_kg,
                        )?;
                        let delivered = unit
                            .checked_mul(quantity)
                            .context("coolant quantity overflow")?;
                        let mut next = thermals.get(target)?.0;
                        ensure!(
                            delivered <= capacity.saturating_sub(next.shield_reserve_mg),
                            "shield coolant reservoir full"
                        );
                        from.withdraw_cargo(
                            &CargoItem::Resource(resource.clone()),
                            quantity,
                            &catalogue.0,
                        )?;
                        next.shield_reserve_mg = next
                            .shield_reserve_mg
                            .checked_add(delivered)
                            .context("coolant quantity overflow")?;
                        thermal = Some(next);
                    } else if source == target {
                        from.refill_cargo(resource_index, quantity, &catalogue.0)?;
                    } else {
                        from.withdraw_cargo(
                            &CargoItem::Resource(resource.clone()),
                            quantity,
                            &catalogue.0,
                        )?;
                        to.insert_consumable(resource_index, quantity, &catalogue.0)?;
                    }
                }
                CargoAction::UnloadProduct { resource, .. } => {
                    let resource = catalogue
                        .0
                        .resources
                        .iter()
                        .position(|entry| entry.id == *resource)
                        .context("unknown resource")?;
                    if source == target {
                        from.package_product(resource, quantity, capacity, &catalogue.0)?;
                    } else {
                        from.unload_product(&mut to, resource, quantity, capacity, &catalogue.0)?;
                    }
                }
            }

            // These components were borrowed successfully above, and this
            // system makes no structural changes between validation and commit.
            inventories.get_mut(source).unwrap().0 = from;
            if source != target {
                inventories.get_mut(target).unwrap().0 = to;
            }
            if let Some(thermal) = thermal {
                thermals.get_mut(target).unwrap().0 = thermal;
            }
            Ok(())
        })();
        request.finish(result);
    }
}

pub fn advance_mines(
    society: Res<crate::sim::society::SocietyState>,
    assets: Query<Asset>,
    mut facilities: Query<&mut IndustrialFacility>,
    mut inventories: Query<&mut hardware::ShipInventory>,
    dock_requests: Query<&hardware::utilities::DockServiceRequest>,
    catalogue: Res<vessel::ShipCatalogue>,
) {
    let state = &*society;
    let directory = &state.directory;

    let mut order: Vec<_> = assets
        .iter()
        .filter(|asset| facilities.contains(asset.entity))
        .map(|asset| (asset.identity.0, asset.entity))
        .collect();
    order.sort_by_key(|&(id, _)| id);
    for (_, host) in order {
        let asset = assets.get(host).unwrap();
        if !asset.active() {
            continue;
        }
        let mut facility = facilities.get_mut(host).unwrap();
        let Some(mine) = &mut facility.mine else {
            continue;
        };
        let total = u128::from(mine.units_per_second) + u128::from(mine.remainder);
        let production = (total / 10) as u64;
        mine.remainder = (total % 10) as u64;
        let unit_volume = osg_ships::industry::item_volume_m3(&mine.output, &catalogue.0)
            .unwrap_or(f64::INFINITY);
        let mut recipients: Vec<_> = asset
            .stored
            .into_iter()
            .flat_map(|stored| stored.iter())
            .filter_map(|entity| {
                let recipient = assets.get(entity).ok()?;
                let inventory = inventories.get(entity).ok()?;
                let request = dock_requests.get(entity).ok()?;
                let capacity = recipient.design.0.capacity_m3;
                let room = ((capacity - inventory.0.cargo_volume(&catalogue.0)).max(0.)
                    / unit_volume)
                    .floor() as u64;
                (room > 0
                    && request.cargo
                    && recipient.available()
                    && asset.permits(&directory.0, recipient.owner.0, Permission::TransferCargo))
                .then_some((recipient.identity.0, entity, room, capacity))
            })
            .collect();
        recipients.sort_by_key(|&(id, ..)| id);
        if let Some(cursor) = mine.last_recipient {
            let start = recipients.partition_point(|&(id, ..)| id <= cursor);
            let length = recipients.len();
            if length > 0 {
                recipients.rotate_left(start % length);
            }
        }
        let (share, mut extra) = loading_shares(
            recipients.iter().map(|&(_, _, room, _)| room).collect(),
            production,
        );
        for (id, entity, room, capacity) in recipients {
            let mut amount = room.min(share);
            if room > share && extra > 0 {
                amount += 1;
                extra -= 1;
                mine.last_recipient = Some(id);
            }
            if amount > 0 {
                // Unloadable production is discarded rather than stored for a burst.
                let _ = inventories.get_mut(entity).unwrap().0.insert_item(
                    &mine.output,
                    amount,
                    capacity,
                    &catalogue.0,
                );
            }
        }
    }
}

fn loading_shares(mut rooms: Vec<u64>, production: u64) -> (u64, u64) {
    rooms.sort_unstable();
    let mut remaining = production;
    let mut recipients = rooms.len() as u64;
    let mut level = 0;
    for room in rooms {
        let cost = u128::from(room - level) * u128::from(recipients);
        if cost > u128::from(remaining) {
            return (level + remaining / recipients, remaining % recipients);
        }
        remaining -= cost as u64;
        level = room;
        recipients -= 1;
    }
    (level, 0)
}
