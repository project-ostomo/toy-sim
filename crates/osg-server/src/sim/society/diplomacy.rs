use super::super::economy::IndexedMap;
use super::{FIRST_ID, LAST_ID, SocialIndex};
use imbl::{OrdMap, Vector};
use osg_model::{Id, diplomacy::*, ownership::*};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diplomacy {
    pub blocs: IndexedMap<PoliticalBloc>,

    pub declarations: OrdMap<(Principal, DeclarationCategory, Principal), Declaration>,

    pub declaration_history:
        OrdMap<(Principal, DeclarationCategory, Principal), Vector<Declaration>>,
    pub agreements: IndexedMap<Agreement>,

    pub trust: OrdMap<(Principal, DeclarationCategory), Vector<Principal>>,

    pub postures: OrdMap<(Id, Id), Standing>,
}

impl Diplomacy {
    pub fn active_terms(
        &self,
        party: Principal,
        partner: Principal,
    ) -> impl Iterator<Item = &AgreementTerm> {
        self.agreements
            .query(
                SocialIndex::Party(party, Some(AgreementStatus::Active), FIRST_ID)
                    ..=SocialIndex::Party(party, Some(AgreementStatus::Active), LAST_ID),
            )
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
            .query(
                SocialIndex::Party(observer, Some(AgreementStatus::Active), FIRST_ID)
                    ..=SocialIndex::Party(observer, Some(AgreementStatus::Active), LAST_ID),
            )
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

    pub fn valid(&self, directory: &super::OwnershipDirectory) -> bool {
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
                    && self.declarations.get(key) == history.back()
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
                        sources: sources.iter().copied().collect(),
                    }
                    .valid()
            })
    }
}
