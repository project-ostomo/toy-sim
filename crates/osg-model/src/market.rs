//! Authoritative global LAT/UEC exchange. Amounts and prices use micro units.
use crate::{Id, ownership::Principal};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Instrument {
    #[default]
    Fx,
    Commodity {
        station: Id,
        item: crate::industry::CargoItem,
        currency: crate::economy::Currency,
    },
}

impl Instrument {
    pub fn currency(&self) -> crate::economy::Currency {
        match self {
            Self::Fx => crate::economy::Currency::Uec,
            Self::Commodity { currency, .. } => *currency,
        }
    }

    pub fn quantity_scale(&self) -> u64 {
        match self {
            Self::Fx => crate::economy::MONEY_SCALE,
            Self::Commodity { .. } => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    #[default]
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketCommand {
    Limit {
        instrument: Instrument,
        owner: Principal,
        side: Side,
        quantity: u64,
        price: u64,
    },
    /// Execute immediately up to the specified worst price; release any remainder.
    Immediate {
        instrument: Instrument,
        owner: Principal,
        side: Side,
        quantity: u64,
        price: u64,
    },
    Cancel {
        order: Id,
    },
    MoveStorage {
        owner: Principal,
        station: Id,
        ship: Id,
        item: crate::industry::CargoItem,
        quantity: u64,
        deposit: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub status: OrderStatus,
    pub closed_ms: Option<i64>,
    pub original_quantity: u64,
    pub filled_quantity: u64,
    pub instrument: Instrument,
    pub id: Id,
    pub sequence: u64,
    pub owner: Principal,
    pub side: Side,
    pub price: u64,
    pub remaining: u64,
    pub time_ms: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    #[default]
    Open,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
    pub instrument: Instrument,
    pub sequence: u64,
    pub time_ms: i64,
    pub price: u64,
    pub quantity: u64,
    pub buyer: Principal,
    pub seller: Principal,
    pub backstop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

impl MarketQuery {
    pub fn valid(&self) -> bool {
        (1..=100).contains(&self.limit)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketSnapshot {
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
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketStation {
    pub id: Id,
    pub name: String,
}

pub type CommodityOfferCursor = (Id, crate::economy::Currency);

/// Public depth aggregated by station and settlement currency. Prices are per
/// cargo unit; comparable prices use the latest executed LAT/UEC trade.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommodityOffer {
    pub station: MarketStation,
    pub system: Id,
    pub currency: crate::economy::Currency,
    pub price: Option<u64>,
    pub available: u64,
    pub comparable_uec: Option<u64>,
    pub bid_price: Option<u64>,
    pub bid_quantity: u64,
    pub comparable_bid_uec: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredStock {
    pub item: crate::industry::CargoItem,
    pub quantity: u64,
    pub reserved: u64,
}
