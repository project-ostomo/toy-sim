use crate::{
    economy::WalletBalance,
    market::{Instrument, Order},
    ownership::{
        AccessBinding, AccessProfile, AssetAffiliation, Organization, PlayerAffiliation, Principal,
        Sovereignty,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameError(pub String);

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GameError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T, C> {
    pub items: Vec<T>,
    pub next: Option<C>,
    pub total: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityRecord {
    Sovereignty(Sovereignty),
    Organization(Organization),
    Player(PlayerAffiliation),
}

/// Search matches and the records needed to display their ancestry.
/// These records are not complete child lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySearch {
    pub matches: Vec<Principal>,
    pub identities: Vec<IdentityRecord>,
}

impl IdentityRecord {
    pub fn principal(&self) -> Principal {
        match self {
            Self::Sovereignty(value) => Principal::Sovereignty(value.id),
            Self::Organization(value) => Principal::Organization(value.id),
            Self::Player(value) => Principal::Player(value.account),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Sovereignty(value) => &value.name,
            Self::Organization(value) => &value.name,
            Self::Player(value) => &value.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetAccessDetails {
    pub asset: AssetAffiliation,
    pub binding: Option<AccessBinding>,
    pub profile: Option<AccessProfile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletAccount {
    pub balance: WalletBalance,
    pub next_charge_ms: i64,
    pub market_uec_per_lat: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderBook {
    pub instrument: Instrument,
    pub bids: Vec<Order>,
    pub asks: Vec<Order>,
    pub last_price: Option<u64>,
    pub backstop_price: u64,
}
