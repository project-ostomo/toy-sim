use crate::sim::{
    self,
    economy::Economy,
    identity,
    ownership::{self, Directory},
};
use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::{
    AccountId, Id, assets::*, diplomacy::Diplomacy, economy::*, industry::*, market::*,
    ownership::*, rpc::*,
};

fn page<T, C>(
    items: impl IntoIterator<Item = T>,
    limit: u16,
    key: impl Fn(&T) -> C,
) -> Result<Page<T, C>> {
    ensure!(
        (1..=128).contains(&limit),
        "page size must be between 1 and 128"
    );
    let mut items: Vec<_> = items.into_iter().take(limit as usize + 1).collect();
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next = if more { items.last().map(key) } else { None };
    Ok(Page {
        items,
        next,
        total: None,
    })
}

fn administers(world: &World, account: AccountId, owner: Principal) -> Result<()> {
    ensure!(
        world.resource::<Directory>().0.administers(account, owner),
        "Account administration required"
    );
    Ok(())
}

fn identities(directory: &OwnershipDirectory) -> impl Iterator<Item = IdentityRecord> + '_ {
    directory
        .sovereignties
        .values()
        .cloned()
        .map(IdentityRecord::Sovereignty)
        .chain(
            directory
                .organizations
                .values()
                .cloned()
                .map(IdentityRecord::Organization),
        )
        .chain(
            directory
                .players
                .values()
                .cloned()
                .map(IdentityRecord::Player),
        )
}

pub(super) fn my_affiliation(world: &World, account: AccountId) -> Result<PlayerAffiliation> {
    world
        .resource::<Directory>()
        .0
        .players
        .get(&account)
        .cloned()
        .context("Account unavailable")
}

pub(super) fn declaration_history(
    world: &World,
    _: AccountId,
    source: Principal,
    category: osg_model::diplomacy::DeclarationCategory,
    target: Principal,
    before: Option<u64>,
    limit: u16,
) -> Result<Page<osg_model::diplomacy::Declaration, u64>> {
    let directory = &world.resource::<Directory>().0;
    ensure!(
        directory.contains(source) && directory.contains(target),
        "principal unavailable"
    );
    page(
        directory
            .diplomacy
            .declaration_history
            .get(&(source, category, target))
            .into_iter()
            .flat_map(|entries| entries.iter().rev())
            .filter(|entry| before.is_none_or(|before| entry.revision < before))
            .cloned(),
        limit,
        |entry| entry.revision,
    )
}

pub(super) fn list_identities(
    world: &World,
    _: AccountId,
    search: String,
    after: Option<Principal>,
    limit: u16,
) -> Result<Page<IdentityRecord, Principal>> {
    ensure!(search.len() <= 512, "search is too long");
    let search = search.to_lowercase();
    let entries = identities(&world.resource::<Directory>().0)
        .filter(|entry| after.is_none_or(|after| entry.principal() > after))
        .filter(|entry| entry.name().to_lowercase().contains(&search));
    page(entries, limit, IdentityRecord::principal)
}

pub(super) fn resolve_identities(
    world: &World,
    _: AccountId,
    principals: Vec<Principal>,
) -> Result<Vec<IdentityRecord>> {
    ensure!(principals.len() <= 128, "too many identities");
    let directory = &world.resource::<Directory>().0;
    Ok(principals
        .into_iter()
        .filter_map(|principal| match principal {
            Principal::Sovereignty(id) => directory
                .sovereignties
                .get(&id)
                .cloned()
                .map(IdentityRecord::Sovereignty),
            Principal::Organization(id) => directory
                .organizations
                .get(&id)
                .cloned()
                .map(IdentityRecord::Organization),
            Principal::Player(id) => directory
                .players
                .get(&id)
                .cloned()
                .map(IdentityRecord::Player),
        })
        .collect())
}

pub(super) fn asset_access(
    world: &World,
    account: AccountId,
    asset: Id,
) -> Result<AssetAccessDetails> {
    let snapshot = ownership::snapshot(world, account);
    let asset = snapshot
        .assets
        .into_iter()
        .find(|entry| entry.entity == asset)
        .context("Asset access unavailable")?;
    let binding = snapshot
        .directory
        .access_bindings
        .get(&asset.entity)
        .cloned();
    let profile = binding
        .as_ref()
        .and_then(|binding| snapshot.directory.access_profiles.get(&binding.profile))
        .cloned();
    Ok(AssetAccessDetails {
        asset,
        binding,
        profile,
    })
}

pub(super) fn list_access_profiles(
    world: &World,
    account: AccountId,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<AccessProfile, Id>> {
    let directory = &world.resource::<Directory>().0;
    page(
        directory
            .access_profiles
            .values()
            .filter(|profile| directory.administers(account, profile.owner))
            .filter(|profile| after.is_none_or(|after| profile.id > after))
            .cloned(),
        limit,
        |profile| profile.id,
    )
}

pub(super) fn diplomacy(
    world: &World,
    account: AccountId,
    principal: Principal,
) -> Result<Diplomacy> {
    let directory = &world.resource::<Directory>().0;
    ensure!(directory.contains(principal), "Identity unavailable");
    let mut result = Diplomacy {
        blocs: directory.diplomacy.blocs.clone(),
        declarations: directory.diplomacy.declarations.clone(),
        agreements: directory.diplomacy.agreements.clone(),
        trust: directory.diplomacy.trust.clone(),
        postures: directory.diplomacy.postures.clone(),
        ..Default::default()
    };
    let lineage = directory.lineage(Principal::Player(account));
    result.trust.retain(|(owner, _), _| {
        lineage.contains(owner) || (*owner == principal && directory.administers(account, *owner))
    });
    Ok(result)
}

pub(super) fn standings(
    world: &World,
    account: AccountId,
) -> Result<std::collections::BTreeMap<(Principal, Principal), Standing>> {
    let directory = &world.resource::<Directory>().0;
    let lineage = directory.lineage(Principal::Player(account));
    Ok(directory
        .standings
        .iter()
        .filter(|((source, _), _)| lineage.contains(source))
        .map(|(key, standing)| (*key, *standing))
        .collect())
}

pub(super) fn resolve_standing(
    world: &World,
    account: AccountId,
    target: Principal,
) -> Result<StandingReport> {
    let directory = &world.resource::<Directory>().0;
    ensure!(directory.contains(target), "Identity unavailable");
    let (standing, source) = directory.standing_with_source(Principal::Player(account), target);
    Ok(StandingReport {
        target,
        standing,
        source,
    })
}

pub(super) fn list_assets(
    world: &World,
    account: AccountId,
    search: String,
    owner: Option<Principal>,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<AssetSummary, Id>> {
    let query = AssetsQuery {
        search,
        owner,
        after,
        limit,
        ..Default::default()
    };
    ensure!(query.valid(), "Invalid asset query");
    let snapshot = sim::assets::snapshot(world, account, &query);
    Ok(Page {
        items: snapshot.assets,
        next: snapshot.next,
        total: Some(snapshot.total_assets),
    })
}

pub(super) fn goods_totals(
    world: &World,
    account: AccountId,
    search: String,
    owner: Option<Principal>,
    after: Option<CargoItem>,
    limit: u16,
) -> Result<Page<GoodsSummary, CargoItem>> {
    let query = AssetsQuery {
        search,
        owner,
        goods_after: after,
        limit,
        ..Default::default()
    };
    ensure!(query.valid(), "Invalid goods query");
    let snapshot = sim::assets::snapshot(world, account, &query);
    Ok(Page {
        items: snapshot.goods,
        next: snapshot.goods_next,
        total: Some(snapshot.total_goods),
    })
}

pub(super) fn stock_locations(
    world: &World,
    account: AccountId,
    item: CargoItem,
    owner: Option<Principal>,
    after: Option<StockKey>,
    limit: u16,
) -> Result<Page<StockLocation, StockKey>> {
    let query = AssetsQuery {
        item: Some(item),
        owner,
        sources_after: after,
        limit,
        ..Default::default()
    };
    ensure!(query.valid(), "Invalid stock query");
    let snapshot = sim::assets::snapshot(world, account, &query);
    Ok(Page {
        items: snapshot.sources,
        next: snapshot.sources_next,
        total: None,
    })
}

fn balance(world: &World, owner: Principal) -> WalletBalance {
    let economy = world.resource::<Economy>();
    let directory = &world.resource::<Directory>().0;
    let balance = economy.balances.get(&owner).cloned().unwrap_or_default();
    WalletBalance {
        owner,
        turnover_tax_bps: economy
            .tax_rate(directory, owner)
            .map_or(0, |(_, rate)| rate),
        uec: balance.uec,
        lat: balance.lat,
        reserved_uec: economy.reserved(owner, Currency::Uec),
        reserved_lat: economy.reserved(owner, Currency::Lat),
        next_demurrage: (balance.uec.saturating_sub(DEMURRAGE_EXEMPTION) as u128
            * daily_rate(economy.last_day))
        .div_ceil(RATE_SCALE) as u64,
        lat_restricted: economy.restricted(directory, owner),
    }
}

pub(super) fn list_wallets(
    world: &World,
    account: AccountId,
    after: Option<Principal>,
    limit: u16,
) -> Result<Page<WalletBalance, Principal>> {
    let directory = &world.resource::<Directory>().0;
    page(
        identities(directory)
            .map(|entry| entry.principal())
            .filter(|owner| {
                after.is_none_or(|after| *owner > after) && directory.administers(account, *owner)
            })
            .map(|owner| balance(world, owner)),
        limit,
        |balance| balance.owner,
    )
}

pub(super) fn wallet_balance(
    world: &World,
    account: AccountId,
    owner: Principal,
) -> Result<WalletAccount> {
    administers(world, account, owner)?;
    let economy = world.resource::<Economy>();
    Ok(WalletAccount {
        balance: balance(world, owner),
        next_charge_ms: (economy.last_day + 1) * DAY_MS,
        market_uec_per_lat: economy
            .exchange
            .trades
            .iter()
            .rev()
            .find(|trade| trade.instrument == Instrument::Fx)
            .map(|trade| trade.price),
    })
}

pub(super) fn wallet_history(
    world: &World,
    account: AccountId,
    owner: Principal,
    before: Option<u64>,
    limit: u16,
) -> Result<Page<LedgerEntry, u64>> {
    administers(world, account, owner)?;
    page(
        world
            .resource::<Economy>()
            .entries
            .iter()
            .rev()
            .filter(|entry| {
                entry.owner == owner && before.is_none_or(|before| entry.sequence < before)
            })
            .cloned(),
        limit,
        |entry| entry.sequence,
    )
}

pub(super) fn gas_balances(
    world: &World,
    account: AccountId,
    after: Option<Principal>,
    limit: u16,
) -> Result<Page<GasAccountSnapshot, Principal>> {
    let mut balances = ownership::gas_accounts(world, account);
    balances.sort_by_key(|balance| balance.owner);
    page(
        balances
            .into_iter()
            .filter(|balance| after.is_none_or(|after| balance.owner > after)),
        limit,
        |balance| balance.owner,
    )
}

pub(super) fn order_book(
    world: &World,
    _: AccountId,
    instrument: Instrument,
    limit: u16,
) -> Result<OrderBook> {
    ensure!((1..=128).contains(&limit), "invalid order book depth");
    sim::economy::storage::validate_instrument(world, &instrument)?;
    let economy = world.resource::<Economy>();
    let mut bids: Vec<_> = economy
        .exchange
        .orders
        .values()
        .filter(|order| order.instrument == instrument && order.side == Side::Buy)
        .cloned()
        .collect();
    let mut asks: Vec<_> = economy
        .exchange
        .orders
        .values()
        .filter(|order| order.instrument == instrument && order.side == Side::Sell)
        .cloned()
        .collect();
    bids.sort_by_key(|order| (u64::MAX - order.price, order.sequence));
    asks.sort_by_key(|order| (order.price, order.sequence));
    bids.truncate(limit as usize);
    asks.truncate(limit as usize);
    let last_price = economy
        .exchange
        .trades
        .iter()
        .rev()
        .find(|trade| trade.instrument == instrument)
        .map(|trade| trade.price);
    Ok(OrderBook {
        instrument,
        bids,
        asks,
        last_price,
        backstop_price: economy.official_uec_per_lat,
    })
}

pub(super) fn list_orders(
    world: &World,
    account: AccountId,
    owner: Principal,
    instrument: Option<Instrument>,
    status: Option<OrderStatus>,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<Order, Id>> {
    administers(world, account, owner)?;
    let exchange = &world.resource::<Economy>().exchange;
    let mut orders: Vec<_> = exchange
        .orders
        .values()
        .chain(exchange.history.iter())
        .filter(|order| {
            order.owner == owner
                && status.is_none_or(|status| order.status == status)
                && instrument
                    .as_ref()
                    .is_none_or(|instrument| order.instrument == *instrument)
                && after.is_none_or(|after| order.id > after)
        })
        .cloned()
        .collect();
    orders.sort_by_key(|order| order.id);
    page(orders, limit, |order| order.id)
}

pub(super) fn trade_history(
    world: &World,
    _: AccountId,
    instrument: Instrument,
    before: Option<u64>,
    limit: u16,
) -> Result<Page<Trade, u64>> {
    sim::economy::storage::validate_instrument(world, &instrument)?;
    page(
        world
            .resource::<Economy>()
            .exchange
            .trades
            .iter()
            .rev()
            .filter(|trade| {
                trade.instrument == instrument
                    && before.is_none_or(|before| trade.sequence < before)
            })
            .cloned(),
        limit,
        |trade| trade.sequence,
    )
}

pub(super) fn list_market_stations(
    world: &World,
    account: AccountId,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<MarketStation, Id>> {
    let mut stations: Vec<_> = world
        .resource::<identity::IdentityIndex>()
        .0
        .iter()
        .filter(|(id, _)| after.is_none_or(|after| **id > after))
        .filter_map(|(id, entity)| {
            let landmark = world.get::<sim::infrastructure::Landmark>(*entity)?;
            world.get::<sim::hardware::ShipInventory>(*entity)?;
            ownership::can_access(world, account, *entity, Permission::View).then(|| {
                MarketStation {
                    id: *id,
                    name: landmark.name.clone(),
                }
            })
        })
        .collect();
    stations.sort_by_key(|station| station.id);
    page(stations, limit, |station| station.id)
}

pub(super) fn compare_commodity_offers(
    world: &World,
    account: AccountId,
    item: CargoItem,
    after: Option<CommodityOfferCursor>,
    limit: u16,
) -> Result<Page<CommodityOffer, CommodityOfferCursor>> {
    ensure!((1..=128).contains(&limit), "invalid page size");
    let economy = world.resource::<Economy>();
    let exchange_rate = economy
        .exchange
        .trades
        .iter()
        .rev()
        .find(|trade| trade.instrument == Instrument::Fx)
        .map(|trade| trade.price);
    let mut rows = std::collections::BTreeMap::new();
    for order in economy.exchange.orders.values() {
        let Instrument::Commodity {
            station,
            item: order_item,
            currency,
        } = &order.instrument
        else {
            continue;
        };
        let key = (*station, *currency);
        if *order_item != item || order.remaining == 0 || after.is_some_and(|after| key <= after) {
            continue;
        }
        let Some(entity) = world
            .resource::<identity::IdentityIndex>()
            .0
            .get(station)
            .copied()
        else {
            continue;
        };
        let Some(landmark) = world.get::<sim::infrastructure::Landmark>(entity) else {
            continue;
        };
        if world.get::<sim::hardware::ShipInventory>(entity).is_none()
            || !ownership::can_access(world, account, entity, Permission::View)
        {
            continue;
        }
        let row = rows.entry(key).or_insert_with(|| CommodityOffer {
            station: MarketStation {
                id: *station,
                name: landmark.name.clone(),
            },
            system: landmark.system,
            currency: *currency,
            price: None,
            available: 0,
            comparable_uec: None,
            bid_price: None,
            bid_quantity: 0,
            comparable_bid_uec: None,
        });
        match order.side {
            Side::Sell => {
                row.price = Some(
                    row.price
                        .map_or(order.price, |price| price.min(order.price)),
                );
                row.available = row.available.saturating_add(order.remaining);
            }
            Side::Buy => {
                row.bid_price = Some(
                    row.bid_price
                        .map_or(order.price, |price| price.max(order.price)),
                );
                row.bid_quantity = row.bid_quantity.saturating_add(order.remaining);
            }
        }
    }
    for row in rows.values_mut() {
        let comparable = |price: u64| match row.currency {
            Currency::Uec => Some(price),
            Currency::Lat => exchange_rate.and_then(|rate| {
                u64::try_from((price as u128 * rate as u128).div_ceil(MONEY_SCALE as u128)).ok()
            }),
        };
        row.comparable_uec = row.price.and_then(comparable);
        row.comparable_bid_uec = row.bid_price.and_then(comparable);
    }
    page(rows.into_values(), limit, |row| {
        (row.station.id, row.currency)
    })
}

pub(super) fn storage_stock(
    world: &World,
    account: AccountId,
    owner: Principal,
    station: Id,
    after: Option<CargoItem>,
    limit: u16,
) -> Result<Page<StoredStock, CargoItem>> {
    administers(world, account, owner)?;
    let economy = world.resource::<Economy>();
    page(
        economy
            .storage
            .get(&(station, owner))
            .into_iter()
            .flat_map(|stock| stock.iter())
            .filter(|(item, _)| after.as_ref().is_none_or(|after| *item > after))
            .map(|(item, quantity)| StoredStock {
                item: item.clone(),
                quantity: *quantity,
                reserved: economy.stock_reserved(owner, station, item),
            }),
        limit,
        |stock| stock.item.clone(),
    )
}

pub(super) fn list_facilities(
    world: &World,
    account: AccountId,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<FacilitySummary, Id>> {
    ensure!((1..=128).contains(&limit), "invalid page size");
    let snapshot = sim::industry::snapshot(
        world,
        account,
        &IndustryQuery {
            directory: true,
            directory_after: after,
            ..Default::default()
        },
    );
    let next = snapshot.directory_next;
    let mut result = page(snapshot.directory, limit, |entry| entry.entity)?;
    if result.next.is_none() {
        result.next = next;
    }
    Ok(result)
}

pub(super) fn facility(world: &World, account: AccountId, facility: Id) -> Result<FacilityView> {
    sim::industry::snapshot(
        world,
        account,
        &IndustryQuery {
            inventories: vec![facility],
            ..Default::default()
        },
    )
    .facilities
    .into_iter()
    .next()
    .context("Facility access unavailable")
}

pub(super) fn hangar(
    world: &World,
    account: AccountId,
    ship: Id,
    after: Option<Id>,
) -> Result<HangarView> {
    sim::industry::snapshot(
        world,
        account,
        &IndustryQuery {
            hangar: Some(HangarQuery { ship, after }),
            ..Default::default()
        },
    )
    .hangar
    .context("Hangar access unavailable")
}

pub(super) fn industry_catalogue(world: &World, account: AccountId) -> Result<IndustryCatalogue> {
    sim::industry::snapshot(
        world,
        account,
        &IndustryQuery {
            catalogue: true,
            ..Default::default()
        },
    )
    .catalogue
    .context("Industry catalogue unavailable")
}

#[cfg(test)]
mod market_comparison_tests {
    use super::*;

    #[test]
    fn comparison_filters_private_stations_pages_books_and_uses_market_fx() {
        let mut world = World::new();
        let account = Id([1; 16]);
        let owner = Principal::Player(account);
        identity::initialize(&mut world, &[account, Id([2; 16])]);
        let item = CargoItem::Resource("iron_ore".into());
        for (index, station_owner) in [
            (10, owner),
            (11, owner),
            (12, Principal::Player(Id([2; 16]))),
        ] {
            let station = Id([index; 16]);
            let entity = world
                .spawn((
                    sim::infrastructure::Landmark {
                        system: Id([9; 16]),
                        name: format!("Station {index}"),
                    },
                    ownership::AssetOwner(station_owner),
                    sim::hardware::ShipInventory(osg_ships::Inventory {
                        quantities: Vec::new(),
                        cargo: Vec::new(),
                        packaged_parts: Default::default(),
                        reservations: Default::default(),
                        custody: Default::default(),
                        tank_capacities_m3: Vec::new(),
                        energy_j: 0,
                    }),
                ))
                .id();
            world
                .resource_mut::<identity::IdentityIndex>()
                .0
                .insert(station, entity);
            for (offset, side, price, quantity) in [
                (0, Side::Sell, 2 * MONEY_SCALE, 7),
                (1, Side::Sell, 3 * MONEY_SCALE, 4),
                (2, Side::Buy, MONEY_SCALE, 5),
            ] {
                let id = Id([index + offset * 20; 16]);
                world.resource_mut::<Economy>().exchange.orders.insert(
                    id,
                    Order {
                        status: OrderStatus::Open,
                        closed_ms: None,
                        original_quantity: quantity,
                        filled_quantity: 0,
                        id,
                        instrument: Instrument::Commodity {
                            station,
                            item: item.clone(),
                            currency: Currency::Lat,
                        },
                        owner: station_owner,
                        side,
                        price,
                        remaining: quantity,
                        sequence: index as u64,
                        time_ms: 0,
                    },
                );
            }
        }
        let first = compare_commodity_offers(&world, account, item.clone(), None, 1).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].available, 11);
        assert_eq!(first.items[0].bid_quantity, 5);
        assert_eq!(first.items[0].price, Some(2 * MONEY_SCALE));
        assert_eq!(first.items[0].comparable_uec, None);
        assert!(first.next.is_some());
        world.resource_mut::<Economy>().exchange.trades.push(Trade {
            instrument: Instrument::Fx,
            sequence: 1,
            time_ms: 0,
            price: 4 * MONEY_SCALE,
            quantity: MONEY_SCALE,
            buyer: owner,
            seller: owner,
            backstop: false,
        });
        let second = compare_commodity_offers(&world, account, item, first.next, 1).unwrap();
        assert_eq!(second.items[0].station.id, Id([11; 16]));
        assert_eq!(second.items[0].comparable_uec, Some(8 * MONEY_SCALE));
        assert_eq!(second.items[0].comparable_bid_uec, Some(4 * MONEY_SCALE));
        assert_eq!(second.next, None);
        let history = trade_history(&world, account, Instrument::Fx, None, 60).unwrap();
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].price, 4 * MONEY_SCALE);
        for sequence in 2..=70 {
            let mut trade = history.items[0].clone();
            trade.sequence = sequence;
            world.resource_mut::<Economy>().exchange.trades.push(trade);
        }
        let mut commodity_trade = history.items[0].clone();
        commodity_trade.sequence = 71;
        commodity_trade.instrument = Instrument::Commodity {
            station: Id([10; 16]),
            item: CargoItem::Resource("iron_ore".into()),
            currency: Currency::Uec,
        };
        world
            .resource_mut::<Economy>()
            .exchange
            .trades
            .push(commodity_trade);
        let history = trade_history(&world, account, Instrument::Fx, None, 60).unwrap();
        assert_eq!(history.items.len(), 60);
        assert_eq!(history.items[0].sequence, 70);
        assert_eq!(history.next, Some(11));
        let first = list_orders(
            &world,
            account,
            owner,
            None,
            Some(OrderStatus::Open),
            None,
            1,
        )
        .unwrap();
        assert_eq!(first.items.len(), 1);
        assert!(first.next.is_some());
        let second = list_orders(
            &world,
            account,
            owner,
            None,
            Some(OrderStatus::Open),
            first.next,
            1,
        )
        .unwrap();
        assert_ne!(first.items[0].id, second.items[0].id);
        let mut archived = first.items[0].clone();
        world
            .resource_mut::<Economy>()
            .exchange
            .orders
            .remove(&archived.id);
        archived.status = OrderStatus::Cancelled;
        archived.remaining = 0;
        archived.closed_ms = Some(100);
        world
            .resource_mut::<Economy>()
            .exchange
            .history
            .push(archived.clone());
        let cancelled = list_orders(
            &world,
            account,
            owner,
            None,
            Some(OrderStatus::Cancelled),
            None,
            128,
        )
        .unwrap();
        assert_eq!(cancelled.items, vec![archived]);
        assert!(list_orders(&world, Id([2; 16]), owner, None, None, None, 128).is_err());
    }
}
