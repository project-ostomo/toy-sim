use super::*;
use crate::sim::{hardware, identity, infrastructure, ownership, vessel};

#[cfg(test)]
mod tests;

pub fn validate_instrument(world: &World, instrument: &Instrument) -> Result<()> {
    let Instrument::Commodity { station, item, .. } = instrument else {
        return Ok(());
    };
    let entity = identity::lookup(world, *station)?;
    ensure!(
        world.get::<infrastructure::Landmark>(entity).is_some(),
        "station market unavailable"
    );
    ensure!(
        world
            .get::<hardware::Hull>(entity)
            .is_some_and(|hull| hull.0 > 0.),
        "station unavailable"
    );
    osg_ships::industry::item_mass_kg(item, &world.resource::<vessel::ShipCatalogue>().0)?;
    Ok(())
}

pub fn apply(
    economy: &mut Economy,
    directory: &crate::sim::society::OwnershipDirectory,
    assets: &mut super::super::society::MarketAssets,
    account: AccountId,
    command: MarketCommand,
) -> Result<()> {
    let MarketCommand::MoveStorage {
        owner,
        station,
        ship,
        item,
        quantity,
        deposit,
    } = command
    else {
        anyhow::bail!("storage command required");
    };
    ensure!(quantity > 0, "positive cargo quantity required");
    ensure!(
        directory.administers(account, owner),
        "storage account administration required"
    );
    let instrument = Instrument::Commodity {
        station,
        item: item.clone(),
        currency: Currency::Uec,
    };
    assets.validate_instrument(&instrument)?;
    let station_entity = assets.lookup(station)?;
    let ship_entity = assets.lookup(ship)?;
    let (_, ship_owner, ship_access, presence, _, _) = assets.assets.get(ship_entity)?;
    ensure!(
        ownership::permits_principal(
            directory,
            ship_owner.0,
            ship_access.map(|access| &access.0),
            Principal::Player(account),
            osg_model::ownership::Permission::TransferCargo
        ),
        "cargo access denied"
    );
    ensure!(
        station == ship
            || matches!(presence.map(|presence| &presence.0),
            Some(osg_model::travel::Presence::Docked { host, .. }) if *host == station),
        "ship must be docked at the station"
    );
    let catalogue = &assets.catalogue.0;
    let mut station_inventory = assets.inventories.get(station_entity)?.0.clone();
    let mut ship_inventory = assets.inventories.get(ship_entity)?.0.clone();
    let current = economy
        .storage
        .get(&(station, owner))
        .and_then(|stock| stock.get(&item))
        .copied()
        .unwrap_or(0);
    let next = if deposit {
        if station != ship {
            let capacity = assets.assets.get(station_entity)?.0.0.capacity_m3;
            ship_inventory.transfer_item(
                &mut station_inventory,
                &item,
                quantity,
                capacity,
                catalogue,
            )?;
        } else {
            ensure!(
                station_inventory.cargo_available(&item, catalogue)? >= quantity,
                "insufficient station cargo"
            );
        }
        let held = station_inventory.custody.entry(item.clone()).or_default();
        *held = held.checked_add(quantity).context("custody overflow")?;
        current.checked_add(quantity).context("stock overflow")?
    } else {
        ensure!(
            economy.stock_available(owner, &instrument)? >= quantity,
            "stock is reserved or unavailable"
        );
        let held = station_inventory
            .custody
            .get_mut(&item)
            .context("custody unavailable")?;
        *held = held.checked_sub(quantity).context("custody shortage")?;
        station_inventory
            .custody
            .retain(|_, quantity| *quantity > 0);
        if station != ship {
            let capacity = assets.assets.get(ship_entity)?.0.0.capacity_m3;
            station_inventory.transfer_item(
                &mut ship_inventory,
                &item,
                quantity,
                capacity,
                catalogue,
            )?;
        }
        current - quantity
    };
    let stock = economy.storage.get(&(station, owner));
    ensure!(
        stock.is_none_or(|stock| stock.contains_key(&item) || stock.len() < 1024),
        "storage item limit"
    );
    station_inventory.validate_cargo(catalogue)?;
    ship_inventory.validate_cargo(catalogue)?;

    assets.inventories.get_mut(station_entity)?.0 = station_inventory;
    if station != ship {
        assets.inventories.get_mut(ship_entity)?.0 = ship_inventory;
    }
    let mut stock = economy
        .storage
        .get(&(station, owner))
        .cloned()
        .unwrap_or_default();
    if next > 0 {
        stock.insert(item, next);
    } else {
        stock.remove(&item);
    }
    economy.storage.insert((station, owner), stock);
    Ok(())
}
