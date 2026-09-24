use crate::{
    AccountId, Id,
    ownership::{Principal, Standing},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DeclarationCategory {
    Standing,
    Wanted,
    Embargo,
    Licence,
    Claim,
    Recognition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub source: Principal,
    pub target: Principal,
    pub category: DeclarationCategory,
    pub revision: u64,
    pub standing: Standing,
    pub enabled: bool,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgreementStatus {
    Proposed,
    Active,
    Suspended,
    Terminated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreement {
    pub id: Id,
    pub from: Principal,
    pub to: Principal,
    pub title: String,
    pub terms: Vec<AgreementTerm>,
    pub note: String,
    pub status: AgreementStatus,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AgreementTerm {
    DockingAccess,
    BasingAccess,
    HonorWanted,
    MutualDefence,
    Tariff { basis_points: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoliticalBloc {
    pub id: Id,
    pub name: String,
    pub officers: BTreeSet<AccountId>,
    pub members: BTreeSet<Id>,
    pub applications: BTreeSet<Id>,
    pub withdrawals: BTreeSet<Id>,
    pub posture: BTreeMap<Id, Standing>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diplomacy {
    pub blocs: BTreeMap<Id, PoliticalBloc>,
    pub declarations: BTreeMap<(Principal, DeclarationCategory, Principal), Declaration>,
    pub declaration_history:
        BTreeMap<(Principal, DeclarationCategory, Principal), Vec<Declaration>>,
    pub agreements: BTreeMap<Id, Agreement>,
    pub trust: BTreeMap<(Principal, DeclarationCategory), Vec<Principal>>,
    pub postures: BTreeMap<(Id, Id), Standing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiplomacyCommand {
    Publish(Declaration),
    SetTrust {
        owner: Principal,
        category: DeclarationCategory,
        sources: Vec<Principal>,
    },
    ProposeAgreement {
        from: Principal,
        to: Principal,
        title: String,
        terms: Vec<AgreementTerm>,
        note: String,
    },
    ChangeAgreement {
        id: Id,
        expected_revision: u64,
        status: AgreementStatus,
    },
    CreateBloc {
        name: String,
        founder: Id,
    },
    ApplyToBloc {
        bloc: Id,
        polity: Id,
        apply: bool,
    },
    DecideApplication {
        bloc: Id,
        polity: Id,
        admit: bool,
    },
    RequestBlocWithdrawal {
        bloc: Id,
        polity: Id,
        request: bool,
    },
    DecideBlocWithdrawal {
        bloc: Id,
        polity: Id,
        grant: bool,
    },
    RemoveBlocMember {
        bloc: Id,
        polity: Id,
    },
    SetBlocOfficer {
        bloc: Id,
        account: AccountId,
        officer: bool,
    },
    SetPosture {
        polity: Id,
        target: Id,
        standing: Standing,
    },
    SetBlocPosture {
        bloc: Id,
        target: Id,
        standing: Standing,
    },
}

impl DiplomacyCommand {
    pub fn valid(&self) -> bool {
        let text = |value: &str, maximum| {
            !value.trim().is_empty()
                && value.len() <= maximum
                && !value.chars().any(char::is_control)
        };
        match self {
            Self::Publish(value) => {
                value.note.len() <= 512 && !value.note.chars().any(char::is_control)
            }
            Self::SetTrust { sources, .. } => {
                sources.len() <= 32
                    && sources.iter().collect::<BTreeSet<_>>().len() == sources.len()
            }
            Self::ProposeAgreement { title, terms, note, .. } => {
                text(title, 128)
                    && !terms.is_empty() && terms.len() <= 8
                    && terms.iter().collect::<BTreeSet<_>>().len() == terms.len()
                    && terms.iter().all(|term| !matches!(term, AgreementTerm::Tariff { basis_points } if *basis_points > 10_000))
                    && note.len() <= 2048
                    && !note
                        .chars()
                        .any(|character| character.is_control() && character != '\n')
            }
            Self::CreateBloc { name, .. } => text(name, 128),
            _ => true,
        }
    }
}

impl Diplomacy {
    pub fn active_terms(
        &self,
        party: Principal,
        partner: Principal,
    ) -> impl Iterator<Item = &AgreementTerm> {
        self.agreements
            .values()
            .filter(move |agreement| {
                agreement.status == AgreementStatus::Active
                    && ((agreement.from == party && agreement.to == partner)
                        || (agreement.to == party && agreement.from == partner))
            })
            .flat_map(|agreement| agreement.terms.iter())
    }

    /// Ordered sources resolve categories independently; return the author so
    /// clients can explain where a standing or declaration came from.
    pub fn resolve(
        &self,
        observer: Principal,
        category: DeclarationCategory,
        target: Principal,
    ) -> Option<&Declaration> {
        let treaty_sources = self
            .agreements
            .values()
            .filter(move |agreement| {
                category == DeclarationCategory::Wanted
                    && agreement.status == AgreementStatus::Active
                    && agreement.terms.contains(&AgreementTerm::HonorWanted)
            })
            .filter_map(move |agreement| {
                if agreement.from == observer {
                    Some(agreement.to)
                } else if agreement.to == observer {
                    Some(agreement.from)
                } else {
                    None
                }
            });
        std::iter::once(observer)
            .chain(
                self.trust
                    .get(&(observer, category))
                    .into_iter()
                    .flatten()
                    .copied(),
            )
            .chain(treaty_sources)
            .find_map(|source| {
                self.declarations
                    .get(&(source, category, target))
                    .filter(|declaration| declaration.enabled)
            })
    }

    pub fn valid(&self, directory: &crate::ownership::OwnershipDirectory) -> bool {
        let mut members = BTreeSet::new();
        self.blocs.len() <= 256
            && self.declarations.len() <= 4096
            && self.declaration_history.len() <= 4096
            && self.declaration_history.iter().all(|(key, history)| {
                !history.is_empty()
                    && history.iter().enumerate().all(|(index, declaration)| {
                        *key == (declaration.source, declaration.category, declaration.target)
                            && declaration.revision == index as u64 + 1
                    })
                    && self.declarations.get(key) == history.last()
            })
            && self.agreements.len() <= 1024
            && self.trust.len() <= 4096
            && self.postures.len() <= 65536
            && self.postures.iter().all(|(&(source, target), _)| {
                source != target
                    && directory.sovereignties.contains_key(&source)
                    && directory.sovereignties.contains_key(&target)
            })
            && self.blocs.iter().all(|(id, bloc)| {
                *id == bloc.id
                    && !bloc.name.is_empty()
                    && bloc.name.len() <= 128
                    && bloc.members.iter().all(|member| members.insert(*member))
                    && bloc.withdrawals.is_subset(&bloc.members)
                    && bloc
                        .officers
                        .iter()
                        .all(|id| directory.players.contains_key(id))
                    && bloc
                        .members
                        .iter()
                        .chain(&bloc.applications)
                        .chain(bloc.posture.keys())
                        .all(|id| directory.sovereignties.contains_key(id))
            })
            && self.declarations.iter().all(|(key, declaration)| {
                *key == (declaration.source, declaration.category, declaration.target)
                    && directory.contains(declaration.source)
                    && directory.contains(declaration.target)
                    && declaration.revision > 0
                    && DiplomacyCommand::Publish(declaration.clone()).valid()
            })
            && self.agreements.iter().all(|(id, agreement)| {
                *id == agreement.id
                    && agreement.from != agreement.to
                    && directory.contains(agreement.from)
                    && directory.contains(agreement.to)
                    && agreement.revision > 0
                    && DiplomacyCommand::ProposeAgreement {
                        from: agreement.from,
                        to: agreement.to,
                        title: agreement.title.clone(),
                        terms: agreement.terms.clone(),
                        note: agreement.note.clone(),
                    }
                    .valid()
            })
            && self.trust.iter().all(|((owner, category), sources)| {
                directory.contains(*owner)
                    && sources.iter().all(|source| directory.contains(*source))
                    && DiplomacyCommand::SetTrust {
                        owner: *owner,
                        category: *category,
                        sources: sources.clone(),
                    }
                    .valid()
            })
    }
}
