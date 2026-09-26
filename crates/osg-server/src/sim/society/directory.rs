use super::super::economy::IndexedMap;
use super::{FIRST_ID, LAST_ID, SocialIndex};
use imbl::OrdMap;
use osg_model::{AccountId, Id, IffIdentity, ownership::*};
use serde::{Deserialize, Serialize};

pub trait AccessRules {
    fn permits(
        &self,
        directory: &OwnershipDirectory,
        account: AccountId,
        permission: Permission,
    ) -> bool;
    fn permits_principal(
        &self,
        directory: &OwnershipDirectory,
        subject: Principal,
        permission: Permission,
    ) -> bool;
}

impl AccessRules for AccessPolicy {
    fn permits(
        &self,
        directory: &OwnershipDirectory,
        account: AccountId,
        permission: Permission,
    ) -> bool {
        self.permits_principal(directory, Principal::Player(account), permission)
    }

    fn permits_principal(
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
        self.grants.iter().any(|grant| {
            grant.permissions.contains(&permission) && lineage.contains(&grant.principal)
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipDirectory {
    pub diplomacy: super::Diplomacy,
    pub access_profiles: IndexedMap<AccessProfile>,
    pub access_bindings: super::AccessBindings,
    pub sovereignties: IndexedMap<Sovereignty>,
    pub organizations: IndexedMap<Organization>,
    pub players: IndexedMap<PlayerAffiliation>,

    pub standings: OrdMap<(Principal, Principal), Standing>,
}

impl OwnershipDirectory {
    pub fn administered(&self, account: AccountId) -> impl Iterator<Item = Principal> + '_ {
        std::iter::once(Principal::Player(account))
            .chain(
                self.organizations
                    .query(
                        SocialIndex::Officer(account, FIRST_ID)
                            ..=SocialIndex::Officer(account, LAST_ID),
                    )
                    .map(|org| Principal::Organization(org.id)),
            )
            .chain(
                self.sovereignties
                    .query(
                        SocialIndex::Officer(account, FIRST_ID)
                            ..=SocialIndex::Officer(account, LAST_ID),
                    )
                    .map(|polity| Principal::Sovereignty(polity.id)),
            )
    }
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
                    osg_model::diplomacy::DeclarationCategory::Standing,
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
            for agreement in self.diplomacy.agreements.query(
                SocialIndex::Party(
                    source,
                    Some(osg_model::diplomacy::AgreementStatus::Active),
                    FIRST_ID,
                )
                    ..=SocialIndex::Party(
                        source,
                        Some(osg_model::diplomacy::AgreementStatus::Active),
                        LAST_ID,
                    ),
            ) {
                if agreement.status != osg_model::diplomacy::AgreementStatus::Active
                    || !agreement
                        .terms
                        .contains(&osg_model::diplomacy::AgreementTerm::MutualDefence)
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
                            osg_model::diplomacy::DeclarationCategory::Standing,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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
