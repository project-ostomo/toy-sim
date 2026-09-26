use crate::sim::society::{FIRST_ID, LAST_ID, SocialIndex, search_gram};
use crate::sim::{
    self,
    economy::{DEMURRAGE_EXEMPTION, RATE_SCALE, daily_rate},
    identity,
    ownership::{self},
};
use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use std::collections::BTreeSet;

#[cfg(test)]
mod directory_tests;
mod society;
use osg_model::{
    AccountId, Id, assets::*, diplomacy::DiplomacyView, economy::*, industry::*, market::*,
    ownership::*, rpc::*,
};
pub use society::society_view;

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
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .directory)
            .0
            .administers(account, owner),
        "Account administration required"
    );
    Ok(())
}

pub fn my_affiliation(world: &World, account: AccountId) -> Result<PlayerAffiliation> {
    (&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0
        .players
        .get(&account)
        .cloned()
        .context("Account unavailable")
}

pub fn declaration_history(
    world: &World,
    _: AccountId,
    source: Principal,
    category: osg_model::diplomacy::DeclarationCategory,
    target: Principal,
    before: Option<u64>,
    limit: u16,
) -> Result<Page<osg_model::diplomacy::Declaration, u64>> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
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
            .flat_map(|entries| {
                let end = before.map_or(entries.len(), |before| {
                    usize::try_from(before.saturating_sub(1))
                        .unwrap_or(usize::MAX)
                        .min(entries.len())
                });
                (0..end).rev().map(|index| &entries[index])
            })
            .cloned(),
        limit,
        |entry| entry.revision,
    )
}

pub fn list_blocs(world: &World, _: AccountId) -> Result<Vec<osg_model::diplomacy::PoliticalBloc>> {
    Ok((&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0
        .diplomacy
        .blocs
        .values()
        .cloned()
        .collect())
}

pub fn list_polities(world: &World, _: AccountId) -> Result<Vec<Sovereignty>> {
    Ok((&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0
        .sovereignties
        .values()
        .cloned()
        .collect())
}

pub fn list_organizations(world: &World, _: AccountId, polity: Id) -> Result<Vec<Organization>> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    ensure!(
        directory.sovereignties.contains_key(&polity),
        "Polity unavailable"
    );
    Ok(directory
        .organizations
        .query(
            SocialIndex::Parent(Some(polity), FIRST_ID)
                ..=SocialIndex::Parent(Some(polity), LAST_ID),
        )
        .cloned()
        .collect())
}

/// `None` selects players without an organization.
pub fn list_players(
    world: &World,
    _: AccountId,
    organization: Option<Id>,
) -> Result<Vec<PlayerAffiliation>> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    ensure!(
        organization.is_none_or(|id| directory.organizations.contains_key(&id)),
        "Organization unavailable"
    );
    Ok(directory
        .players
        .query(
            SocialIndex::Parent(organization, FIRST_ID)
                ..=SocialIndex::Parent(organization, LAST_ID),
        )
        .cloned()
        .collect())
}

pub fn search_identities(world: &World, _: AccountId, search: String) -> Result<IdentitySearch> {
    ensure!(search.len() <= 512, "search is too long");
    let search = search.trim().to_lowercase();
    if search.is_empty() {
        return Ok(IdentitySearch::default());
    }
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    let gram = search_gram(&search);
    let range = SocialIndex::Gram(gram.clone(), FIRST_ID)..=SocialIndex::Gram(gram, LAST_ID);
    let matches: Vec<_> = directory
        .sovereignties
        .query(range.clone())
        .cloned()
        .map(IdentityRecord::Sovereignty)
        .chain(
            directory
                .organizations
                .query(range.clone())
                .cloned()
                .map(IdentityRecord::Organization),
        )
        .chain(
            directory
                .players
                .query(range)
                .cloned()
                .map(IdentityRecord::Player),
        )
        .filter(|entry| entry.name().to_lowercase().contains(&search))
        .map(|entry| entry.principal())
        .collect();
    let ancestry: BTreeSet<_> = matches
        .iter()
        .flat_map(|principal| directory.lineage(*principal))
        .collect();
    let mut identities = Vec::new();
    for chunk in ancestry.into_iter().collect::<Vec<_>>().chunks(128) {
        identities.extend(resolve_identities(world, Id([0; 16]), chunk.to_vec())?);
    }
    Ok(IdentitySearch {
        matches,
        identities,
    })
}

pub fn resolve_identities(
    world: &World,
    _: AccountId,
    principals: Vec<Principal>,
) -> Result<Vec<IdentityRecord>> {
    ensure!(principals.len() <= 128, "too many identities");
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
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

pub fn asset_access(world: &World, account: AccountId, asset: Id) -> Result<AssetAccessDetails> {
    ownership::asset_access(world, account, asset)
}

pub fn list_access_profiles(world: &World, account: AccountId) -> Result<Vec<AccessProfile>> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    Ok(directory
        .administered(account)
        .flat_map(|owner| {
            directory
                .access_profiles
                .query(SocialIndex::Owner(owner, FIRST_ID)..=SocialIndex::Owner(owner, LAST_ID))
        })
        .cloned()
        .collect())
}

pub fn diplomacy(world: &World, account: AccountId, principal: Principal) -> Result<DiplomacyView> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    ensure!(directory.contains(principal), "Identity unavailable");
    let lineage = directory.lineage(Principal::Player(account));
    let mut sources: BTreeSet<_> = lineage
        .iter()
        .copied()
        .chain(directory.lineage(principal))
        .collect();
    for source in sources.clone() {
        for (_, trustees) in directory.diplomacy.trust.range(
            (source, osg_model::diplomacy::DeclarationCategory::Standing)
                ..=(
                    source,
                    osg_model::diplomacy::DeclarationCategory::Recognition,
                ),
        ) {
            sources.extend(trustees);
        }
    }
    let mut result = DiplomacyView {
        blocs: directory.diplomacy.blocs.to_map(),
        declarations: sources
            .iter()
            .flat_map(|source| {
                directory.diplomacy.declarations.range(
                    (
                        *source,
                        osg_model::diplomacy::DeclarationCategory::Standing,
                        Principal::Sovereignty(FIRST_ID),
                    )
                        ..=(
                            *source,
                            osg_model::diplomacy::DeclarationCategory::Recognition,
                            Principal::Player(LAST_ID),
                        ),
                )
            })
            .map(|(key, value)| (*key, value.clone()))
            .collect(),
        agreements: sources
            .iter()
            .flat_map(|source| {
                directory.diplomacy.agreements.query(
                    SocialIndex::Party(*source, None, FIRST_ID)
                        ..=SocialIndex::Party(*source, None, LAST_ID),
                )
            })
            .map(|agreement| (agreement.id, agreement.clone()))
            .collect(),
        trust: sources
            .iter()
            .flat_map(|source| {
                directory.diplomacy.trust.range(
                    (*source, osg_model::diplomacy::DeclarationCategory::Standing)
                        ..=(
                            *source,
                            osg_model::diplomacy::DeclarationCategory::Recognition,
                        ),
                )
            })
            .map(|(key, value)| (*key, value.iter().copied().collect()))
            .collect(),
        postures: sources
            .iter()
            .filter_map(|source| {
                if let Principal::Sovereignty(id) = source {
                    Some(*id)
                } else {
                    None
                }
            })
            .flat_map(|source| {
                directory
                    .diplomacy
                    .postures
                    .range((source, FIRST_ID)..=(source, LAST_ID))
            })
            .map(|(key, value)| (*key, *value))
            .collect(),
        ..Default::default()
    };
    result.trust.retain(|(owner, _), _| {
        lineage.contains(owner) || (*owner == principal && directory.administers(account, *owner))
    });
    Ok(result)
}

pub fn standings(
    world: &World,
    account: AccountId,
) -> Result<std::collections::BTreeMap<(Principal, Principal), Standing>> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    let lineage = directory.lineage(Principal::Player(account));
    Ok(lineage
        .into_iter()
        .flat_map(|source| {
            directory.standings.range(
                (source, Principal::Sovereignty(FIRST_ID))..=(source, Principal::Player(LAST_ID)),
            )
        })
        .map(|(key, standing)| (*key, *standing))
        .collect())
}

pub fn resolve_standing(
    world: &World,
    account: AccountId,
    target: Principal,
) -> Result<StandingReport> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    ensure!(directory.contains(target), "Identity unavailable");
    let (standing, source) = directory.standing_with_source(Principal::Player(account), target);
    Ok(StandingReport {
        target,
        standing,
        source,
    })
}

pub fn list_assets(
    world: &World,
    account: AccountId,
    search: String,
    owner: Option<Principal>,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<AssetSummary, Id>> {
    ensure!(search.len() <= 512, "Search is too long");
    let entries = sim::assets::list(world, account, &search, owner, after, limit as usize + 1);
    page(
        entries
            .into_iter()
            .filter(|entry| after.is_none_or(|after| entry.id > after)),
        limit,
        |entry| entry.id,
    )
}

pub fn goods_totals(
    world: &World,
    account: AccountId,
    search: String,
    owner: Option<Principal>,
    after: Option<CargoItem>,
    limit: u16,
) -> Result<Page<GoodsSummary, CargoItem>> {
    ensure!(search.len() <= 512, "Search is too long");
    if let Some(item) = &after {
        validate_item(item)?;
    }
    let entries = sim::assets::goods_totals(world, account, &search, owner);
    let total = entries.len() as u64;
    let mut result = page(
        entries
            .into_iter()
            .filter(|entry| after.as_ref().is_none_or(|after| entry.item > *after)),
        limit,
        |entry| entry.item.clone(),
    )?;
    result.total = Some(total);
    Ok(result)
}

pub fn stock_locations(
    world: &World,
    account: AccountId,
    item: CargoItem,
    owner: Option<Principal>,
    after: Option<StockKey>,
    limit: u16,
) -> Result<Page<StockLocation, StockKey>> {
    validate_item(&item)?;
    let entries = sim::assets::stock_locations(world, account, &item, owner);
    page(
        entries
            .into_iter()
            .filter(|entry| after.as_ref().is_none_or(|after| entry.key > *after)),
        limit,
        |entry| entry.key.clone(),
    )
}

fn validate_item(item: &CargoItem) -> Result<()> {
    let (CargoItem::Resource(id) | CargoItem::Part(id)) = item;
    ensure!(!id.is_empty() && id.len() <= 128, "Invalid item");
    Ok(())
}

fn balance(world: &World, owner: Principal) -> WalletBalance {
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
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

pub fn list_wallets(world: &World, account: AccountId) -> Result<Vec<WalletBalance>> {
    let directory = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .directory)
        .0;
    Ok(directory
        .administered(account)
        .map(|owner| balance(world, owner))
        .collect())
}

pub fn wallet_balance(
    world: &World,
    account: AccountId,
    owner: Principal,
) -> Result<WalletAccount> {
    administers(world, account, owner)?;
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    Ok(WalletAccount {
        balance: balance(world, owner),
        next_charge_ms: (economy.last_day + 1) * DAY_MS,
        market_uec_per_lat: economy
            .exchange
            .trades
            .query((Instrument::Fx, 0)..=(Instrument::Fx, u64::MAX))
            .rev()
            .next()
            .map(|trade| trade.price),
    })
}

pub fn wallet_history(
    world: &World,
    account: AccountId,
    owner: Principal,
    before: Option<u64>,
    limit: u16,
) -> Result<Page<LedgerEntry, u64>> {
    administers(world, account, owner)?;
    page(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .entries
            .query((owner, 0)..(owner, before.unwrap_or(u64::MAX)))
            .rev()
            .cloned(),
        limit,
        |entry| entry.sequence,
    )
}

pub fn gas_balances(world: &World, account: AccountId) -> Result<Vec<GasAccountSnapshot>> {
    let mut balances = ownership::gas_accounts(world, account);
    balances.sort_by_key(|balance| balance.owner);
    Ok(balances)
}

pub fn order_book(
    world: &World,
    _: AccountId,
    instrument: Instrument,
    limit: u16,
) -> Result<OrderBook> {
    ensure!((1..=128).contains(&limit), "invalid order book depth");
    sim::economy::storage::validate_instrument(world, &instrument)?;
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    let bids: Vec<_> = economy
        .exchange
        .orders
        .book(&instrument, Side::Buy)
        .take(limit as usize)
        .cloned()
        .collect();
    let asks: Vec<_> = economy
        .exchange
        .orders
        .book(&instrument, Side::Sell)
        .take(limit as usize)
        .cloned()
        .collect();
    let last_price = economy
        .exchange
        .trades
        .query((instrument.clone(), 0)..=(instrument.clone(), u64::MAX))
        .rev()
        .next()
        .map(|trade| trade.price);
    Ok(OrderBook {
        instrument,
        bids,
        asks,
        last_price,
        backstop_price: economy.official_uec_per_lat,
    })
}

pub fn list_orders(
    world: &World,
    account: AccountId,
    owner: Principal,
    instrument: Option<Instrument>,
    status: Option<OrderStatus>,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<Order, Id>> {
    administers(world, account, owner)?;
    let exchange = &(&world
        .resource::<crate::sim::society::SocietyState>()
        .economy)
        .exchange;
    let mut orders: Vec<_> = exchange
        .orders
        .by_owner(owner, instrument.clone(), status, after)
        .take(limit as usize + 1)
        .chain(
            exchange
                .history
                .by_owner(owner, instrument, status, after)
                .take(limit as usize + 1),
        )
        .cloned()
        .collect();
    orders.sort_by_key(|order| order.id);
    page(orders, limit, |order| order.id)
}

pub fn trade_history(
    world: &World,
    _: AccountId,
    instrument: Instrument,
    before: Option<u64>,
    limit: u16,
) -> Result<Page<Trade, u64>> {
    sim::economy::storage::validate_instrument(world, &instrument)?;
    page(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .exchange
            .trades
            .query((instrument.clone(), 0)..(instrument, before.unwrap_or(u64::MAX)))
            .rev()
            .cloned(),
        limit,
        |trade| trade.sequence,
    )
}

pub fn list_market_stations(
    world: &World,
    account: AccountId,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<MarketStation, Id>> {
    let stations = world
        .resource::<sim::society::AssetRecords>()
        .markets(after)
        .filter_map(|record| {
            let landmark = world.get::<sim::infrastructure::Landmark>(record.entity)?;
            world.get::<sim::hardware::ShipInventory>(record.entity)?;
            ownership::can_access(world, account, record.entity, Permission::View).then(|| {
                MarketStation {
                    id: record.id,
                    name: landmark.name.clone(),
                }
            })
        });
    page(stations, limit, |station| station.id)
}

pub fn compare_commodity_offers(
    world: &World,
    account: AccountId,
    item: CargoItem,
    after: Option<CommodityOfferCursor>,
    limit: u16,
) -> Result<Page<CommodityOffer, CommodityOfferCursor>> {
    ensure!((1..=128).contains(&limit), "invalid page size");
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    let exchange_rate = economy
        .exchange
        .trades
        .query((Instrument::Fx, 0)..=(Instrument::Fx, u64::MAX))
        .rev()
        .next()
        .map(|trade| trade.price);
    let mut rows = std::collections::BTreeMap::new();
    for order in economy.exchange.orders.commodity(&item, after) {
        let Instrument::Commodity {
            station,
            item: order_item,
            currency,
        } = &order.instrument
        else {
            continue;
        };
        let key = (*station, *currency);
        if rows.len() > limit as usize && !rows.contains_key(&key) {
            break;
        }
        if *order_item != item || order.remaining == 0 || after.is_some_and(|after| key <= after) {
            continue;
        }
        let Some(entity) = world
            .resource::<identity::IdentityIndex>()
            .entries()
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

pub fn storage_stock(
    world: &World,
    account: AccountId,
    owner: Principal,
    station: Id,
) -> Result<Vec<StoredStock>> {
    administers(world, account, owner)?;
    let economy = &world
        .resource::<crate::sim::society::SocietyState>()
        .economy;
    Ok(economy
        .storage
        .get(&(station, owner))
        .into_iter()
        .flat_map(|stock| stock.iter())
        .map(|(item, quantity)| StoredStock {
            item: item.clone(),
            quantity: *quantity,
            reserved: economy.stock_reserved(owner, station, item),
        })
        .collect())
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
            identity::register(&mut world, entity, station).unwrap();
            for (offset, side, price, quantity) in [
                (0, Side::Sell, 2 * MONEY_SCALE, 7),
                (1, Side::Sell, 3 * MONEY_SCALE, 4),
                (2, Side::Buy, MONEY_SCALE, 5),
            ] {
                let id = Id([index + offset * 20; 16]);
                world
                    .resource_mut::<crate::sim::society::SocietyState>()
                    .map_unchanged(|state| &mut state.economy)
                    .exchange
                    .orders
                    .insert(
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
        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .exchange
            .trades
            .insert(Trade {
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
            world
                .resource_mut::<crate::sim::society::SocietyState>()
                .map_unchanged(|state| &mut state.economy)
                .exchange
                .trades
                .insert(trade);
        }
        let mut commodity_trade = history.items[0].clone();
        commodity_trade.sequence = 71;
        commodity_trade.instrument = Instrument::Commodity {
            station: Id([10; 16]),
            item: CargoItem::Resource("iron_ore".into()),
            currency: Currency::Uec,
        };
        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .exchange
            .trades
            .insert(commodity_trade);
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
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .exchange
            .orders
            .remove(&archived.id);
        archived.status = OrderStatus::Cancelled;
        archived.remaining = 0;
        archived.closed_ms = Some(100);
        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .exchange
            .history
            .insert(archived.id, archived.clone());
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
