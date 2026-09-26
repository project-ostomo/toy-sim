//! Client query parameters and views composed from independent RPC responses.
use super::QueryState;
use osg_model::{Id, assets::*, economy::*, industry::*, market::*, ownership::*};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndustryQuery {
    pub directory: bool,
    pub directory_after: Option<Id>,
    pub hangar: Option<HangarQuery>,
    pub inventories: Vec<Id>,
    pub catalogue: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HangarQuery {
    pub ship: Id,
    pub after: Option<Id>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndustryView {
    pub directory: QueryState<osg_model::rpc::Page<FacilitySummary, Id>>,
    pub hangar: QueryState<HangarView>,
    pub facilities: std::collections::BTreeMap<Id, QueryState<FacilityView>>,
    pub hangar_query: Option<HangarQuery>,
    pub catalogue: QueryState<IndustryCatalogue>,
}

impl IndustryView {
    pub fn inventory(&self, id: Id) -> &QueryState<FacilityView> {
        self.facilities.get(&id).unwrap_or(&QueryState::Loading)
    }

    pub fn summaries(&self) -> impl Iterator<Item = &FacilitySummary> {
        self.directory
            .as_ref()
            .into_iter()
            .flat_map(|page| &page.items)
    }

    pub fn directory_next(&self) -> Option<Id> {
        self.directory.as_ref().and_then(|page| page.next)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetsQuery {
    pub search: String,
    pub owner: Option<Principal>,
    pub after: Option<Id>,
    pub goods_after: Option<CargoItem>,
    pub item: Option<CargoItem>,
    pub sources_after: Option<StockKey>,
    pub limit: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetsView {
    pub query: AssetsQuery,
    pub assets: Vec<AssetSummary>,
    pub goods: Vec<GoodsSummary>,
    pub sources: Vec<StockLocation>,
    pub next: Option<Id>,
    pub goods_next: Option<CargoItem>,
    pub sources_next: Option<StockKey>,
    pub total_assets: u64,
    pub total_goods: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletQuery {
    pub owner: Principal,
    pub before: Option<u64>,
    pub limit: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalletView {
    /// Latest executed FX trades, newest first, for market valuation history.
    pub fx_trades: Vec<Trade>,
    pub owner: Option<Principal>,
    pub balances: Vec<WalletBalance>,
    pub entries: Vec<LedgerEntry>,
    pub next_before: Option<u64>,
    pub next_charge_ms: i64,
    /// Price of the latest executed trade, in micro UEC per LAT.
    pub market_uec_per_lat: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketQuery {
    pub order_status: Option<OrderStatus>,
    pub offers_after: Option<CommodityOfferCursor>,
    pub orders_after: Option<Id>,
    pub stations_after: Option<Id>,
    pub instrument: Instrument,
    pub owner: Principal,
    pub before: Option<u64>,
    pub limit: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketView {
    pub offers: Vec<CommodityOffer>,
    pub offers_next: Option<CommodityOfferCursor>,
    pub stations: Vec<MarketStation>,
    pub stations_next: Option<Id>,
    pub instrument: Instrument,
    pub stock: Vec<StoredStock>,
    pub owner: Option<Principal>,
    pub available_uec: u64,
    pub available_lat: u64,
    pub reserved_uec: u64,
    pub reserved_lat: u64,
    pub lat_restricted: bool,
    pub last_price: Option<u64>,
    pub backstop_price: u64,
    pub bids: Vec<Order>,
    pub asks: Vec<Order>,
    pub orders: Vec<Order>,
    pub orders_next: Option<Id>,
    pub trades: Vec<Trade>,
    pub next_before: Option<u64>,
}

pub use osg_model::society::SocietyData;
