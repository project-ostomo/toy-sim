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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
pub struct DiplomacyView {
    pub resolved: BTreeMap<(Principal, DeclarationCategory, Principal), Declaration>,
    pub blocs: BTreeMap<Id, PoliticalBloc>,
    pub declarations: BTreeMap<(Principal, DeclarationCategory, Principal), Declaration>,
    pub declaration_history:
        BTreeMap<(Principal, DeclarationCategory, Principal), Vec<Declaration>>,
    pub agreements: BTreeMap<Id, Agreement>,
    pub trust: BTreeMap<(Principal, DeclarationCategory), Vec<Principal>>,
    pub postures: BTreeMap<(Id, Id), Standing>,
}

impl DiplomacyView {
    pub fn resolve(
        &self,
        observer: Principal,
        category: DeclarationCategory,
        target: Principal,
    ) -> Option<&Declaration> {
        self.resolved.get(&(observer, category, target))
    }
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
