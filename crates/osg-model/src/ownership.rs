use crate::{AccountId, Id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Principal {
    Sovereignty(Id),
    Organization(Id),
    Player(AccountId),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bloc {
    Union,
    League,
    #[default]
    NonAligned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sovereignty {
    pub id: Id,
    pub name: String,
    pub bloc: Bloc,
    pub officers: BTreeSet<AccountId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub id: Id,
    pub name: String,
    pub sovereignty: Id,
    pub open_membership: bool,
    pub officers: BTreeSet<AccountId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerAffiliation {
    pub account: AccountId,
    pub name: String,
    pub organization: Option<Id>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Standing {
    Friendly,
    #[default]
    Neutral,
    Hostile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandingSource {
    Declaration {
        source: Principal,
        target: Principal,
        revision: u64,
    },
    Override {
        source: Principal,
        target: Principal,
    },
    MutualDefence {
        agreement: Id,
        ally: Principal,
    },
    SharedAffiliation,
    Default,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandingReport {
    pub target: Principal,
    pub standing: Standing,
    pub source: StandingSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Permission {
    Navigate,
    Dock,
    View,
    Control,
    Configure,
    TransferCargo,
    Industry,
    ManageAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGrant {
    pub principal: Principal,
    pub permissions: BTreeSet<Permission>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessPolicy {
    pub public: BTreeSet<Permission>,
    pub grants: Vec<AccessGrant>,
}

impl AccessPolicy {
    pub fn valid(&self) -> bool {
        let mut principals = BTreeSet::new();
        self.grants.len() <= 256
            && self
                .grants
                .iter()
                .all(|grant| !grant.permissions.is_empty() && principals.insert(grant.principal))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessProfile {
    pub id: Id,
    pub owner: Principal,
    pub name: String,
    pub policy: AccessPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessBinding {
    pub profile: Id,
    pub overrides: AccessPolicy,
    pub denied: BTreeSet<Permission>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasAccountSnapshot {
    pub owner: Principal,
    pub available: u64,
    pub spent: u64,
}

impl GasAccountSnapshot {
    pub fn valid(&self) -> bool {
        self.available.checked_add(self.spent).is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetAffiliation {
    pub entity: Id,
    pub name: String,
    pub owner: Principal,
    pub access: AccessPolicy,
    pub can_manage: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocietyCommand {
    UnlinkAccessProfile {
        asset: Id,
    },
    SetAccessDenied {
        asset: Id,
        denied: BTreeSet<Permission>,
    },
    SaveAccessProfile(AccessProfile),
    DeleteAccessProfile {
        id: Id,
    },
    ApplyAccessProfile {
        asset: Id,
        profile: Id,
    },
    Diplomacy(crate::diplomacy::DiplomacyCommand),
    CreateOrganization {
        name: String,
    },
    SetOfficer {
        organization: Id,
        account: AccountId,
        officer: bool,
    },
    SetStanding {
        target: Principal,
        standing: Option<Standing>,
    },
    SetMembership {
        account: AccountId,
        organization: Option<Id>,
    },
    SetAssetAccess {
        asset: Id,
        policy: AccessPolicy,
    },
    TransferAsset {
        asset: Id,
        owner: Principal,
    },
}
