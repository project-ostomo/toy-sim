use super::*;
use crate::sim::{hardware, identity, industry, infrastructure, ownership, travel, vessel};
use osg_model::{Id, industry::CargoItem, market::*};

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

impl Economy {
    pub(crate) fn stock_reserved(&self, owner: Principal, station: Id, item: &CargoItem) -> u64 {
        self.exchange.orders.values().filter(|order| {
            order.owner == owner && order.side == Side::Sell && matches!(&order.instrument,
                Instrument::Commodity { station: location, item: stock, .. } if *location == station && stock == item)
        }).fold(0_u64, |total, order| total.saturating_add(order.remaining))
    }

    pub(super) fn stock_available(&self, owner: Principal, instrument: &Instrument) -> Result<u64> {
        let Instrument::Commodity { station, item, .. } = instrument else {
            anyhow::bail!("commodity instrument required");
        };
        self.storage
            .get(&(*station, owner))
            .and_then(|stock| stock.get(item))
            .copied()
            .unwrap_or(0)
            .checked_sub(self.stock_reserved(owner, *station, item))
            .context("stock reservations exceed custody")
    }
}

impl Transaction<'_> {
    pub(super) fn move_stock(
        &mut self,
        from: Principal,
        to: Principal,
        instrument: &Instrument,
        quantity: u64,
    ) -> Result<()> {
        let Instrument::Commodity { station, item, .. } = instrument else {
            anyhow::bail!("commodity instrument required");
        };
        ensure!(
            from != to && quantity > 0 && self.stock_available(from, instrument)? >= quantity,
            "insufficient available stock"
        );
        let recipient = self.stock_mut((*station, to));
        ensure!(
            recipient.contains_key(item) || recipient.len() < 1024,
            "storage item limit"
        );
        let amount = recipient.entry(item.clone()).or_default();
        *amount = amount.checked_add(quantity).context("stock overflow")?;
        let source = self.stock_mut((*station, from));
        *source.get_mut(item).unwrap() -= quantity;
        source.retain(|_, quantity| *quantity > 0);
        self.remove_empty_stock((*station, from));
        Ok(())
    }
}

impl Economy {
    pub(super) fn stock_snapshot(
        &self,
        owner: Principal,
        instrument: &Instrument,
    ) -> Vec<StoredStock> {
        let Instrument::Commodity { station, .. } = instrument else {
            return Vec::new();
        };
        self.storage
            .get(&(*station, owner))
            .into_iter()
            .flat_map(|stock| stock.iter())
            .map(|(item, &quantity)| StoredStock {
                item: item.clone(),
                quantity,
                reserved: self.stock_reserved(owner, *station, item),
            })
            .collect()
    }

    pub fn custody_totals(&self, station: Id) -> Result<BTreeMap<CargoItem, u64>> {
        let mut totals = BTreeMap::<CargoItem, u64>::new();
        for ((location, _), stock) in &self.storage {
            if *location != station {
                continue;
            }
            ensure!(
                stock.len() <= 1024 && stock.values().all(|quantity| *quantity > 0),
                "invalid stored stock"
            );
            for (item, quantity) in stock {
                let total = totals.entry(item.clone()).or_default();
                *total = total
                    .checked_add(*quantity)
                    .context("stock custody overflow")?;
            }
        }
        Ok(totals)
    }
}

pub fn apply(world: &mut World, account: AccountId, id: Id, command: MarketCommand) -> Result<()> {
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
        world.resource::<Directory>().0.administers(account, owner),
        "storage account administration required"
    );
    let instrument = Instrument::Commodity {
        station,
        item: item.clone(),
        currency: Currency::Uec,
    };
    validate_instrument(world, &instrument)?;
    let station_entity = identity::lookup(world, station)?;
    let ship_entity = identity::lookup(world, ship)?;
    ownership::authorize(
        world,
        account,
        ship_entity,
        osg_model::ownership::Permission::TransferCargo,
    )?;
    ensure!(
        station == ship
            || matches!(world.get::<travel::PresenceState>(ship_entity).map(|presence| &presence.0),
        Some(osg_model::travel::Presence::Docked { host, .. }) if *host == station),
        "ship must be docked at the station"
    );
    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    let mut station_inventory = world
        .get::<hardware::ShipInventory>(station_entity)
        .context("station inventory unavailable")?
        .0
        .clone();
    let mut ship_inventory = world
        .get::<hardware::ShipInventory>(ship_entity)
        .context("ship inventory unavailable")?
        .0
        .clone();
    let economy = world.resource::<Economy>();
    let current = economy
        .storage
        .get(&(station, owner))
        .and_then(|stock| stock.get(&item))
        .copied()
        .unwrap_or(0);
    let next = if deposit {
        if station != ship {
            let capacity = world
                .get::<vessel::ShipDesign>(station_entity)
                .context("station design unavailable")?
                .0
                .capacity_m3;
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
            let capacity = world
                .get::<vessel::ShipDesign>(ship_entity)
                .context("ship design unavailable")?
                .0
                .capacity_m3;
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

    world
        .get_mut::<hardware::ShipInventory>(station_entity)
        .unwrap()
        .0 = station_inventory;
    if station != ship {
        world
            .get_mut::<hardware::ShipInventory>(ship_entity)
            .unwrap()
            .0 = ship_inventory;
    }
    let mut economy = world.resource_mut::<Economy>();
    let stock = economy.storage.entry((station, owner)).or_default();
    if next > 0 {
        stock.insert(item, next);
    } else {
        stock.remove(&item);
    }
    if stock.is_empty() {
        economy.storage.remove(&(station, owner));
    }
    economy.completed.insert((account, id));
    crate::sim::hardware::synchronize_mass(world, &[station_entity, ship_entity]);
    industry::refresh_publication(world);
    Ok(())
}
