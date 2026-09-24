use super::*;
use crate::{diplomacy::DeclarationCategory, economy::Currency};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServicePolicy {
    pub revision: u64,
    pub accepting: bool,
    pub currency: Currency,
    pub rates: Vec<ServiceRate>,
    pub tiers: Vec<CustomerTier>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRate {
    pub capability: IndustryCapability,
    pub energy_per_mj: u64,
    pub time_per_hour: u64,
    pub public_lanes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CustomerMatch {
    Principal(Principal),
    Bloc(Id),
    Declaration {
        source: Principal,
        category: DeclarationCategory,
    },
    Everyone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerTier {
    pub customer: CustomerMatch,
    /// 10,000 is list price, zero is free; None refuses service.
    pub price_basis_points: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceWork {
    Recipe { recipe: String, batches: u32 },
    Ship { blueprint_hash: [u8; 32] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceQuote {
    pub facility: Id,
    pub payer: Principal,
    pub operator: Principal,
    pub work: ServiceWork,
    pub policy_revision: u64,
    pub currency: Currency,
    pub energy_charge: u64,
    pub time_charge: u64,
    pub price_basis_points: u16,
    pub total: u64,
    pub inputs: Vec<ItemStack>,
    pub outputs: Vec<ItemStack>,
    pub duration_ticks: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServicePayment {
    pub payer: Principal,
    pub operator: Principal,
    pub currency: Currency,
    pub amount: u64,
    pub charged: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublicFacility {
    pub summary: FacilitySummary,
    pub policy: ServicePolicy,
    pub capabilities: Vec<FacilityCapability>,
    pub queued_jobs: usize,
    pub running_jobs: usize,
}

impl ServicePolicy {
    pub fn valid(&self) -> bool {
        self.rates.len() <= 4
            && self.tiers.len() <= 32
            && self
                .rates
                .iter()
                .map(|rate| rate.capability)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.rates.len()
            && self
                .tiers
                .iter()
                .all(|tier| tier.price_basis_points.is_none_or(|rate| rate <= 10_000))
    }
}
