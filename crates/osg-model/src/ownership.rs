use crate::{AccountId, Id, IffIdentity, Tag};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipDirectory {
    pub sovereignties: BTreeMap<Id, Sovereignty>,
    pub organizations: BTreeMap<Id, Organization>,
    pub players: BTreeMap<AccountId, PlayerAffiliation>,
    pub standings: BTreeMap<(Principal, Principal), Standing>,
}

impl OwnershipDirectory {
    pub fn contains(&self, principal: Principal) -> bool {
        match principal {
            Principal::Sovereignty(id) => self.sovereignties.contains_key(&id),
            Principal::Organization(id) => self.organizations.contains_key(&id),
            Principal::Player(id) => self.players.contains_key(&id),
        }
    }

    pub fn lineage(&self, principal: Principal) -> Vec<Principal> {
        let mut result = vec![principal];
        let organization = match principal {
            Principal::Player(account) => self.players.get(&account).and_then(|p| p.organization),
            Principal::Organization(id) => Some(id),
            Principal::Sovereignty(_) => None,
        };
        if let Some(organization) = organization {
            if matches!(principal, Principal::Player(_)) {
                result.push(Principal::Organization(organization));
            }
            if let Some(organization) = self.organizations.get(&organization) {
                result.push(Principal::Sovereignty(organization.sovereignty));
            }
        }
        result
    }

    pub fn belongs_to(&self, account: AccountId, principal: Principal) -> bool {
        self.lineage(Principal::Player(account))
            .contains(&principal)
    }

    pub fn administers(&self, account: AccountId, principal: Principal) -> bool {
        match principal {
            Principal::Player(owner) => account == owner,
            Principal::Organization(id) => self
                .organizations
                .get(&id)
                .is_some_and(|organization| organization.officers.contains(&account)),
            Principal::Sovereignty(id) => self
                .sovereignties
                .get(&id)
                .is_some_and(|sovereignty| sovereignty.officers.contains(&account)),
        }
    }

    pub fn standing(&self, observer: Principal, subject: Principal) -> Standing {
        self.standing_for_lineages(&self.lineage(observer), &self.lineage(subject))
    }

    pub fn iff_standing(&self, observer: AccountId, iff: &IffIdentity) -> Option<Standing> {
        iff.enabled.then(|| {
            self.advertised_standing(observer, Some(iff.owner), iff.faction)
                .unwrap_or_default()
        })
    }

    pub fn track_standing(&self, observer: AccountId, tags: &BTreeSet<Tag>) -> Option<Standing> {
        let owner = tags.iter().find_map(|tag| match tag {
            Tag::IffOwner(owner) => Some(*owner),
            _ => None,
        });
        let organization = tags.iter().find_map(|tag| match tag {
            Tag::IffFaction(organization) => Some(*organization),
            _ => None,
        });
        self.advertised_standing(observer, owner, organization)
    }

    pub fn advertised_standing(
        &self,
        observer: AccountId,
        owner: Option<AccountId>,
        organization: Option<Id>,
    ) -> Option<Standing> {
        if owner.is_none() && organization.is_none() {
            return None;
        }
        let mut subject = Vec::new();
        if let Some(owner) = owner {
            subject.push(Principal::Player(owner));
        }
        if let Some(organization) = organization {
            subject.extend(self.lineage(Principal::Organization(organization)));
        }
        Some(self.standing_for_lineages(&self.lineage(Principal::Player(observer)), &subject))
    }

    fn standing_for_lineages(&self, observer: &[Principal], subject: &[Principal]) -> Standing {
        for &source in observer {
            for &target in subject {
                if let Some(&standing) = self.standings.get(&(source, target)) {
                    return standing;
                }
            }
        }
        if observer.iter().any(|principal| subject.contains(principal)) {
            Standing::Friendly
        } else {
            Standing::Neutral
        }
    }

    pub fn can_advertise(&self, account: AccountId, organization: Option<Id>) -> bool {
        organization.is_none_or(|organization| {
            self.belongs_to(account, Principal::Organization(organization))
                || self.administers(account, Principal::Organization(organization))
        })
    }

    pub fn valid(&self) -> bool {
        let valid_name = |name: &str| {
            !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
        };
        self.sovereignties.len() <= 4096
            && self.organizations.len() <= 16384
            && self.players.len() <= 65536
            && self.standings.len() <= 16384
            && self
                .standings
                .keys()
                .all(|(source, target)| self.contains(*source) && self.contains(*target))
            && self
                .sovereignties
                .iter()
                .all(|(id, item)| *id == item.id && valid_name(&item.name))
            && self.organizations.iter().all(|(id, item)| {
                *id == item.id
                    && valid_name(&item.name)
                    && self.sovereignties.contains_key(&item.sovereignty)
            })
            && self.players.iter().all(|(id, item)| {
                *id == item.account
                    && valid_name(&item.name)
                    && item
                        .organization
                        .is_none_or(|org| self.organizations.contains_key(&org))
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Permission {
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

    pub fn permits(
        &self,
        directory: &OwnershipDirectory,
        account: AccountId,
        permission: Permission,
    ) -> bool {
        self.permits_principal(directory, Principal::Player(account), permission)
    }

    pub fn permits_principal(
        &self,
        directory: &OwnershipDirectory,
        subject: Principal,
        permission: Permission,
    ) -> bool {
        if self.public.contains(&permission) {
            return true;
        }
        if self.grants.is_empty() {
            return false;
        }
        let lineage = directory.lineage(subject);
        self.permits_lineage(&lineage, permission)
    }

    pub fn permits_lineage(&self, lineage: &[Principal], permission: Permission) -> bool {
        self.public.contains(&permission)
            || self.grants.iter().any(|grant| {
                grant.permissions.contains(&permission) && lineage.contains(&grant.principal)
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocietySnapshot {
    pub account: AccountId,
    pub directory: OwnershipDirectory,
    pub assets: Vec<AssetAffiliation>,
    pub gas_accounts: Vec<GasAccountSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasAccountSnapshot {
    pub owner: Principal,
    pub available: u64,
    pub reserved: u64,
    pub spent: u64,
}

impl GasAccountSnapshot {
    pub fn valid(&self) -> bool {
        self.available
            .checked_add(self.reserved)
            .and_then(|total| total.checked_add(self.spent))
            .is_some()
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

impl SocietySnapshot {
    pub fn valid(&self) -> bool {
        let mut assets = BTreeSet::new();
        let mut gas_accounts = BTreeSet::new();
        self.directory.valid()
            && self.gas_accounts.len() <= 1 + 16384 + 4096
            && self.gas_accounts.iter().all(|balance| {
                gas_accounts.insert(balance.owner)
                    && self.directory.contains(balance.owner)
                    && self.directory.administers(self.account, balance.owner)
                    && balance.valid()
            })
            && self.assets.len() <= 4096
            && self.assets.iter().all(|asset| {
                assets.insert(asset.entity)
                    && !asset.name.is_empty()
                    && asset.name.len() <= 256
                    && self.directory.contains(asset.owner)
                    && asset.access.valid()
                    && asset
                        .access
                        .grants
                        .iter()
                        .all(|grant| self.directory.contains(grant.principal))
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocietyCommand {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> OwnershipDirectory {
        let mut directory = OwnershipDirectory::default();
        for (sovereignty, organization, account) in [(1, 2, 3), (4, 5, 6)] {
            let sovereignty = Id([sovereignty; 16]);
            let organization = Id([organization; 16]);
            let account = Id([account; 16]);
            directory.sovereignties.insert(
                sovereignty,
                Sovereignty {
                    id: sovereignty,
                    name: "State".into(),
                    bloc: Bloc::League,
                    officers: BTreeSet::new(),
                },
            );
            directory.organizations.insert(
                organization,
                Organization {
                    id: organization,
                    name: "Organization".into(),
                    sovereignty,
                    open_membership: false,
                    officers: BTreeSet::new(),
                },
            );
            directory.players.insert(
                account,
                PlayerAffiliation {
                    account,
                    name: "Player".into(),
                    organization: Some(organization),
                },
            );
        }
        directory
    }

    #[test]
    fn explicit_player_and_organization_standings_override_sovereignty() {
        let mut directory = directory();
        let source = Principal::Player(Id([3; 16]));
        let target = Principal::Player(Id([6; 16]));
        directory.standings.insert(
            (
                Principal::Sovereignty(Id([1; 16])),
                Principal::Sovereignty(Id([4; 16])),
            ),
            Standing::Hostile,
        );
        assert_eq!(directory.standing(source, target), Standing::Hostile);
        directory.standings.insert(
            (
                Principal::Organization(Id([2; 16])),
                Principal::Organization(Id([5; 16])),
            ),
            Standing::Neutral,
        );
        assert_eq!(directory.standing(source, target), Standing::Neutral);
        directory
            .standings
            .insert((source, target), Standing::Friendly);
        assert_eq!(directory.standing(source, target), Standing::Friendly);
    }

    #[test]
    fn league_members_remain_distinct_and_hidden_affiliation_is_not_revealed() {
        let directory = directory();
        assert_eq!(
            directory.standing(
                Principal::Player(Id([3; 16])),
                Principal::Player(Id([6; 16]))
            ),
            Standing::Neutral
        );
        let tags = BTreeSet::from([Tag::IffOwner(Id([3; 16]))]);
        assert_eq!(
            directory.track_standing(Id([6; 16]), &tags),
            Some(Standing::Neutral)
        );
        assert_eq!(
            directory.track_standing(Id([6; 16]), &BTreeSet::new()),
            None
        );
    }

    #[test]
    fn organization_grants_follow_membership_without_granting_management() {
        let directory = directory();
        let policy = AccessPolicy {
            public: BTreeSet::from([Permission::View]),
            grants: vec![AccessGrant {
                principal: Principal::Organization(Id([2; 16])),
                permissions: BTreeSet::from([Permission::Industry]),
            }],
        };
        assert!(policy.permits(&directory, Id([3; 16]), Permission::Industry));
        assert!(!policy.permits(&directory, Id([6; 16]), Permission::Industry));
        assert!(!policy.permits(&directory, Id([3; 16]), Permission::ManageAccess));
    }

    #[test]
    fn gas_balances_require_ownership_or_administration_and_valid_totals() {
        let account = Id([3; 16]);
        let own = GasAccountSnapshot {
            owner: Principal::Player(account),
            available: 900,
            reserved: 50,
            spent: 50,
        };
        let mut snapshot = SocietySnapshot {
            account,
            directory: directory(),
            assets: Vec::new(),
            gas_accounts: vec![own],
        };
        assert!(snapshot.valid());

        snapshot.gas_accounts.push(GasAccountSnapshot {
            owner: Principal::Player(Id([6; 16])),
            ..own
        });
        assert!(!snapshot.valid());

        snapshot.gas_accounts[1].owner = Principal::Organization(Id([2; 16]));
        assert!(
            !snapshot.valid(),
            "membership alone cannot read the account"
        );
        snapshot
            .directory
            .organizations
            .get_mut(&Id([2; 16]))
            .unwrap()
            .officers
            .insert(account);
        assert!(snapshot.valid());

        snapshot.gas_accounts.push(own);
        assert!(!snapshot.valid(), "duplicate billing principals");
        snapshot.gas_accounts.pop();
        snapshot.gas_accounts[0].available = u64::MAX;
        assert!(
            !snapshot.valid(),
            "available plus reserved and spent overflow"
        );
        snapshot.gas_accounts[0] = own;

        snapshot.gas_accounts[1].owner = Principal::Sovereignty(Id([1; 16]));
        assert!(!snapshot.valid());
        snapshot
            .directory
            .sovereignties
            .get_mut(&Id([1; 16]))
            .unwrap()
            .officers
            .insert(account);
        assert!(snapshot.valid());
        snapshot.gas_accounts[1].owner = Principal::Organization(Id([9; 16]));
        assert!(!snapshot.valid(), "unknown billing principal");
    }
}
