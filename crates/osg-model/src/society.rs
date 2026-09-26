//! Query inputs and server-computed presentation data for society screens.
use crate::{AccountId, Id, IffIdentity, diplomacy::*, ownership::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Branch {
    Blocs,
    Polities,
    Organizations(Id),
    Players(Option<Id>),
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SocietyQuery {
    pub declaration_history: Option<(Principal, DeclarationCategory, Principal)>,
    pub history_before: Option<u64>,
    pub directory: bool,
    pub search: String,
    pub branches: BTreeSet<Branch>,
    pub selected: Option<Principal>,
    pub asset: Option<Id>,
    pub assets_after: Option<Id>,
    pub assets: bool,
    pub profiles: bool,
    pub gas: bool,
    pub advertised: BTreeSet<(Option<AccountId>, Option<Id>)>,
}

/// One replaceable RPC result. Permissions, ancestry and relationships are
/// computed by the server; the client only renders these fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocietyPresentation {
    pub viewer: AccountId,
    pub diplomacy: DiplomacyView,
    pub access_profiles: BTreeMap<Id, AccessProfile>,
    pub access_bindings: BTreeMap<Id, AccessBinding>,
    pub sovereignties: BTreeMap<Id, Sovereignty>,
    pub organizations: BTreeMap<Id, Organization>,
    pub players: BTreeMap<AccountId, PlayerAffiliation>,
    pub standings: BTreeMap<(Principal, Principal), Standing>,
    pub ancestry: BTreeMap<Principal, Vec<Principal>>,
    pub administered: BTreeSet<Principal>,
    pub reports: BTreeMap<(Principal, Principal), (Standing, StandingSource)>,
    pub postures: BTreeMap<(Principal, Principal), Standing>,
    pub advertised: BTreeMap<(Option<AccountId>, Option<Id>), Standing>,
    pub branches: BTreeSet<Branch>,
    pub matches: BTreeSet<Principal>,
}

impl SocietyPresentation {
    pub fn contains(&self, principal: Principal) -> bool {
        self.ancestry.contains_key(&principal)
    }

    pub fn lineage(&self, principal: Principal) -> Vec<Principal> {
        self.ancestry.get(&principal).cloned().unwrap_or_default()
    }

    pub fn administers(&self, account: AccountId, principal: Principal) -> bool {
        account == self.viewer && self.administered.contains(&principal)
    }

    pub fn standing_with_source(
        &self,
        observer: Principal,
        subject: Principal,
    ) -> (Standing, StandingSource) {
        self.reports
            .get(&(observer, subject))
            .cloned()
            .unwrap_or((Standing::Neutral, StandingSource::Default))
    }

    pub fn standing(&self, observer: Principal, subject: Principal) -> Standing {
        self.standing_with_source(observer, subject).0
    }

    pub fn political_posture(&self, observer: Principal, subject: Principal) -> Option<Standing> {
        self.postures.get(&(observer, subject)).copied()
    }

    pub fn advertised_standing(
        &self,
        observer: AccountId,
        owner: Option<AccountId>,
        organization: Option<Id>,
    ) -> Option<Standing> {
        (observer == self.viewer)
            .then(|| self.advertised.get(&(owner, organization)).copied())
            .flatten()
    }

    pub fn contact_standing(
        &self,
        observer: AccountId,
        iff: Option<&IffIdentity>,
    ) -> Option<Standing> {
        let iff = iff.filter(|iff| iff.enabled)?;
        self.advertised_standing(observer, Some(iff.owner), iff.faction)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocietyData {
    pub standing_report: Option<StandingReport>,
    pub account: AccountId,
    pub directory: SocietyPresentation,
    pub assets: Vec<AssetAffiliation>,
    pub gas_accounts: Vec<GasAccountSnapshot>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SocietyView {
    pub history_key: Option<(Principal, DeclarationCategory, Principal)>,
    pub history: Vec<Declaration>,
    pub history_next: Option<u64>,
    pub snapshot: SocietyData,
    pub assets_next: Option<Id>,
    pub loaded_asset: Option<Id>,
}
