use crate::{AccountId, Id, IffIdentity};
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipDirectory {
    pub diplomacy: crate::diplomacy::Diplomacy,
    pub access_profiles: BTreeMap<Id, AccessProfile>,
    pub access_bindings: BTreeMap<Id, AccessBinding>,
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

    pub fn political_posture(&self, observer: Principal, subject: Principal) -> Option<Standing> {
        let polity = |principal| {
            self.lineage(principal).into_iter().find_map(|value| {
                if let Principal::Sovereignty(id) = value {
                    Some(id)
                } else {
                    None
                }
            })
        };
        self.diplomacy
            .postures
            .get(&(polity(observer)?, polity(subject)?))
            .copied()
    }

    pub fn iff_standing(&self, observer: AccountId, iff: &IffIdentity) -> Option<Standing> {
        iff.enabled.then(|| {
            self.advertised_standing(observer, Some(iff.owner), iff.faction)
                .unwrap_or_default()
        })
    }

    pub fn contact_standing(
        &self,
        observer: AccountId,
        iff: Option<&IffIdentity>,
    ) -> Option<Standing> {
        let iff = iff.filter(|iff| iff.enabled)?;
        self.advertised_standing(observer, Some(iff.owner), iff.faction)
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
        self.resolve_standing(observer, subject).0
    }

    pub fn standing_with_source(
        &self,
        observer: Principal,
        subject: Principal,
    ) -> (Standing, StandingSource) {
        self.resolve_standing(&self.lineage(observer), &self.lineage(subject))
    }

    fn resolve_standing(
        &self,
        observer: &[Principal],
        subject: &[Principal],
    ) -> (Standing, StandingSource) {
        for &source in observer {
            for &target in subject {
                if let Some(declaration) = self.diplomacy.resolve(
                    source,
                    crate::diplomacy::DeclarationCategory::Standing,
                    target,
                ) {
                    if declaration.enabled {
                        return (
                            declaration.standing,
                            StandingSource::Declaration {
                                source: declaration.source,
                                target: declaration.target,
                                revision: declaration.revision,
                            },
                        );
                    }
                }
                if let Some(&standing) = self.standings.get(&(source, target)) {
                    return (standing, StandingSource::Override { source, target });
                }
            }
        }
        for &source in observer {
            for agreement in self.diplomacy.agreements.values() {
                if agreement.status != crate::diplomacy::AgreementStatus::Active
                    || !agreement
                        .terms
                        .contains(&crate::diplomacy::AgreementTerm::MutualDefence)
                {
                    continue;
                }
                let ally = if agreement.from == source {
                    agreement.to
                } else if agreement.to == source {
                    agreement.from
                } else {
                    continue;
                };
                for &target in subject {
                    if self
                        .diplomacy
                        .resolve(
                            ally,
                            crate::diplomacy::DeclarationCategory::Standing,
                            target,
                        )
                        .is_some_and(|declaration| declaration.standing == Standing::Hostile)
                        || self.standings.get(&(ally, target)) == Some(&Standing::Hostile)
                    {
                        return (
                            Standing::Hostile,
                            StandingSource::MutualDefence {
                                agreement: agreement.id,
                                ally,
                            },
                        );
                    }
                }
            }
        }
        if observer.iter().any(|principal| subject.contains(principal)) {
            (Standing::Friendly, StandingSource::SharedAffiliation)
        } else {
            (Standing::Neutral, StandingSource::Default)
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
            && self.diplomacy.valid(self)
            && self.access_profiles.len() <= 1024
            && self.access_bindings.values().all(|binding| {
                self.access_profiles.contains_key(&binding.profile)
                    && binding.overrides.valid()
                    && binding
                        .overrides
                        .grants
                        .iter()
                        .all(|grant| self.contains(grant.principal))
            })
            && self.access_profiles.iter().all(|(id, profile)| {
                *id == profile.id
                    && self.contains(profile.owner)
                    && valid_name(&profile.name)
                    && profile.policy.valid()
                    && profile
                        .policy
                        .grants
                        .iter()
                        .all(|grant| self.contains(grant.principal))
            })
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

impl AccessBinding {
    pub fn effective(&self, profile: &AccessPolicy) -> AccessPolicy {
        let mut result = profile.clone();
        result.public.extend(self.overrides.public.iter().copied());
        for grant in &self.overrides.grants {
            if let Some(existing) = result
                .grants
                .iter_mut()
                .find(|entry| entry.principal == grant.principal)
            {
                existing
                    .permissions
                    .extend(grant.permissions.iter().copied());
            } else {
                result.grants.push(grant.clone());
            }
        }
        result
            .public
            .retain(|permission| !self.denied.contains(permission));
        for grant in &mut result.grants {
            grant
                .permissions
                .retain(|permission| !self.denied.contains(permission));
        }
        result.grants.retain(|grant| !grant.permissions.is_empty());
        result
    }
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
        assert_eq!(
            directory.standing_with_source(source, target),
            (
                Standing::Friendly,
                StandingSource::Override { source, target },
            )
        );
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
        let iff = IffIdentity {
            owner: Id([3; 16]),
            faction: None,
            labels: BTreeSet::new(),
            enabled: true,
        };
        assert_eq!(
            directory.contact_standing(Id([6; 16]), Some(&iff)),
            Some(Standing::Neutral)
        );
        assert_eq!(directory.contact_standing(Id([6; 16]), None), None);
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
    fn gas_balances_reject_overflow() {
        let balance = GasAccountSnapshot {
            owner: Principal::Player(Id([3; 16])),
            available: 900,
            reserved: 50,
            spent: 50,
        };
        assert!(balance.valid());
        assert!(
            !GasAccountSnapshot {
                available: u64::MAX,
                ..balance
            }
            .valid()
        );
    }
}
