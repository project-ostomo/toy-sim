use crate::sim::society::{FIRST_ID, LAST_ID, SocialIndex};
use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::{AccountId, Id, diplomacy::*, ownership::Principal};
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

pub fn apply(
    directory: &mut crate::sim::society::OwnershipDirectory,
    account: AccountId,
    command: DiplomacyCommand,
) -> Result<()> {
    ensure!(command.valid(), "invalid diplomacy command");
    let administer = |principal| -> Result<()> {
        ensure!(
            directory.administers(account, principal),
            "officer authority required"
        );
        Ok(())
    };
    let known = |principal| -> Result<()> {
        ensure!(directory.contains(principal), "principal unavailable");
        Ok(())
    };

    match command {
        DiplomacyCommand::Publish(mut declaration) => {
            administer(declaration.source)?;
            known(declaration.target)?;
            let key = (declaration.source, declaration.category, declaration.target);
            let revision = directory
                .diplomacy
                .declarations
                .get(&key)
                .map_or(0, |d| d.revision);
            ensure!(
                declaration.revision == revision,
                "declaration changed; refresh before publishing"
            );
            declaration.revision = revision.checked_add(1).context("revision exhausted")?;
            directory
                .diplomacy
                .declaration_history
                .entry(key)
                .or_default()
                .push_back(declaration.clone());
            directory.diplomacy.declarations.insert(key, declaration);
        }
        DiplomacyCommand::SetTrust {
            owner,
            category,
            sources,
        } => {
            administer(owner)?;
            for source in &sources {
                known(*source)?;
                ensure!(*source != owner, "own declarations already take priority");
            }
            directory
                .diplomacy
                .trust
                .insert((owner, category), sources.into());
        }
        DiplomacyCommand::ProposeAgreement {
            from,
            to,
            title,
            terms,
            note,
        } => {
            administer(from)?;
            known(to)?;
            ensure!(from != to, "agreement needs two parties");
            let id = Id::new();
            directory.diplomacy.agreements.insert(
                id,
                Agreement {
                    id,
                    from,
                    to,
                    title,
                    terms,
                    note,
                    status: AgreementStatus::Proposed,
                    revision: 1,
                },
            );
        }
        DiplomacyCommand::ChangeAgreement {
            id,
            expected_revision,
            status,
        } => {
            let mut agreement = directory
                .diplomacy
                .agreements
                .get(&id)
                .cloned()
                .context("agreement unavailable")?;
            ensure!(
                agreement.revision == expected_revision,
                "agreement changed; refresh before acting"
            );
            let from = directory.administers(account, agreement.from);
            let to = directory.administers(account, agreement.to);
            ensure!(from || to, "agreement party authority required");
            let allowed = match (agreement.status, status) {
                (AgreementStatus::Proposed, AgreementStatus::Active) => to,
                (AgreementStatus::Proposed, AgreementStatus::Terminated)
                | (AgreementStatus::Active, AgreementStatus::Suspended)
                | (AgreementStatus::Active, AgreementStatus::Terminated)
                | (AgreementStatus::Suspended, AgreementStatus::Terminated) => true,
                // Resuming requires a fresh proposal and counterparty consent.
                _ => false,
            };
            ensure!(allowed, "invalid agreement transition");
            agreement.status = status;
            agreement.revision = agreement
                .revision
                .checked_add(1)
                .context("revision exhausted")?;
            directory.diplomacy.agreements.insert(id, agreement);
        }
        DiplomacyCommand::CreateBloc { name, founder } => {
            administer(Principal::Sovereignty(founder))?;
            ensure!(
                directory
                    .diplomacy
                    .blocs
                    .query(
                        SocialIndex::Member(founder, FIRST_ID)
                            ..=SocialIndex::Member(founder, LAST_ID)
                    )
                    .next()
                    .is_none(),
                "leave the current bloc first"
            );
            ensure!(
                directory
                    .diplomacy
                    .blocs
                    .query(
                        SocialIndex::Name(name.to_lowercase(), FIRST_ID)
                            ..=SocialIndex::Name(name.to_lowercase(), LAST_ID)
                    )
                    .next()
                    .is_none(),
                "bloc name already registered"
            );
            let id = Id::new();
            let posture = directory
                .diplomacy
                .postures
                .range((founder, FIRST_ID)..=(founder, LAST_ID))
                .map(|(&(_, target), &standing)| (target, standing))
                .collect();
            directory.diplomacy.blocs.insert(
                id,
                PoliticalBloc {
                    id,
                    name,
                    officers: BTreeSet::from([account]),
                    members: BTreeSet::from([founder]),
                    applications: BTreeSet::new(),
                    withdrawals: BTreeSet::new(),
                    posture,
                },
            );
        }
        DiplomacyCommand::ApplyToBloc {
            bloc,
            polity,
            apply,
        } => {
            administer(Principal::Sovereignty(polity))?;
            ensure!(
                !apply
                    || directory
                        .diplomacy
                        .blocs
                        .query(
                            SocialIndex::Member(polity, FIRST_ID)
                                ..=SocialIndex::Member(polity, LAST_ID)
                        )
                        .next()
                        .is_none(),
                "already a bloc member"
            );
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            if apply {
                bloc.applications.insert(polity);
            } else {
                ensure!(bloc.applications.remove(&polity), "application unavailable");
            }
            directory.diplomacy.blocs.insert(bloc.id, bloc);
        }
        DiplomacyCommand::DecideApplication {
            bloc,
            polity,
            admit,
        } => {
            ensure!(
                !admit
                    || directory
                        .diplomacy
                        .blocs
                        .query(
                            SocialIndex::Member(polity, FIRST_ID)
                                ..=SocialIndex::Member(polity, LAST_ID)
                        )
                        .next()
                        .is_none(),
                "already a bloc member"
            );
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            ensure!(bloc.applications.remove(&polity), "application unavailable");
            if admit {
                bloc.members.insert(polity);
                bloc.posture.remove(&polity);
                for &member in &bloc.members {
                    directory.diplomacy.postures.remove(&(member, polity));
                }
                for (&target, &standing) in &bloc.posture {
                    if target != polity {
                        directory
                            .diplomacy
                            .postures
                            .insert((polity, target), standing);
                    }
                }
            }
            directory.diplomacy.blocs.insert(bloc.id, bloc);
            if admit {
                let applicants: Vec<_> = directory
                    .diplomacy
                    .blocs
                    .query(
                        super::society::SocialIndex::Application(polity, super::society::FIRST_ID)
                            ..=super::society::SocialIndex::Application(
                                polity,
                                super::society::LAST_ID,
                            ),
                    )
                    .map(|bloc| bloc.id)
                    .collect();
                for id in applicants {
                    let mut applicant = directory.diplomacy.blocs.get(&id).unwrap().clone();
                    applicant.applications.remove(&polity);
                    directory.diplomacy.blocs.insert(id, applicant);
                }
            }
        }
        DiplomacyCommand::RequestBlocWithdrawal {
            bloc,
            polity,
            request,
        } => {
            administer(Principal::Sovereignty(polity))?;
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            ensure!(bloc.members.contains(&polity), "membership unavailable");
            if request {
                ensure!(
                    bloc.withdrawals.insert(polity),
                    "withdrawal already requested"
                );
            } else {
                ensure!(
                    bloc.withdrawals.remove(&polity),
                    "withdrawal request unavailable"
                );
            }
            directory.diplomacy.blocs.insert(bloc.id, bloc);
        }
        DiplomacyCommand::DecideBlocWithdrawal {
            bloc,
            polity,
            grant,
        } => {
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            ensure!(
                bloc.withdrawals.remove(&polity),
                "withdrawal request unavailable"
            );
            if grant {
                ensure!(bloc.members.remove(&polity), "membership unavailable");
            }
            // The copied posture deliberately survives departure.
            directory.diplomacy.blocs.insert(bloc.id, bloc);
        }
        DiplomacyCommand::RemoveBlocMember { bloc, polity } => {
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            ensure!(bloc.members.remove(&polity), "membership unavailable");
            bloc.withdrawals.remove(&polity);
            // The copied posture deliberately survives departure.
            directory.diplomacy.blocs.insert(bloc.id, bloc);
        }
        DiplomacyCommand::SetBlocOfficer {
            bloc,
            account: officer,
            officer: enabled,
        } => {
            known(Principal::Player(officer))?;
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            if enabled {
                bloc.officers.insert(officer);
            } else {
                ensure!(
                    bloc.officers.len() > 1 || !bloc.officers.contains(&officer),
                    "cannot remove the final officer"
                );
                bloc.officers.remove(&officer);
            }
            directory.diplomacy.blocs.insert(bloc.id, bloc);
        }
        DiplomacyCommand::SetPosture {
            polity,
            target,
            standing,
        } => {
            administer(Principal::Sovereignty(polity))?;
            known(Principal::Sovereignty(target))?;
            ensure!(polity != target, "invalid posture target");
            ensure!(
                directory
                    .diplomacy
                    .blocs
                    .query(
                        SocialIndex::Member(polity, FIRST_ID)
                            ..=SocialIndex::Member(polity, LAST_ID)
                    )
                    .next()
                    .is_none(),
                "bloc members follow bloc posture"
            );
            directory
                .diplomacy
                .postures
                .insert((polity, target), standing);
        }
        DiplomacyCommand::SetBlocPosture {
            bloc,
            target,
            standing,
        } => {
            known(Principal::Sovereignty(target))?;
            let mut bloc = directory
                .diplomacy
                .blocs
                .get(&bloc)
                .cloned()
                .context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            ensure!(
                !bloc.members.contains(&target),
                "cannot target a member polity"
            );
            bloc.posture.insert(target, standing);
            for &member in &bloc.members {
                directory
                    .diplomacy
                    .postures
                    .insert((member, target), standing);
            }
            directory.diplomacy.blocs.insert(bloc.id, bloc);
        }
    }
    ensure!(
        directory.diplomacy.valid(directory),
        "diplomacy capacity or validation failure"
    );
    let polities: Vec<_> = directory.sovereignties.keys().copied().collect();
    for id in polities {
        let mut polity = directory.sovereignties.get(&id).unwrap().clone();
        polity.bloc = if directory
            .diplomacy
            .blocs
            .get(&super::ownership::principal_id(
                "bloc",
                "Union State of Earth",
            ))
            .is_some_and(|bloc| bloc.members.contains(&polity.id))
        {
            osg_model::ownership::Bloc::Union
        } else if directory
            .diplomacy
            .blocs
            .get(&super::ownership::principal_id(
                "bloc",
                "League of Free States",
            ))
            .is_some_and(|bloc| bloc.members.contains(&polity.id))
        {
            osg_model::ownership::Bloc::League
        } else {
            osg_model::ownership::Bloc::NonAligned
        };
        directory.sovereignties.insert(id, polity);
    }
    Ok(())
}
