use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::{AccountId, Id, diplomacy::*, ownership::Principal};
use std::collections::BTreeSet;

use super::ownership::Directory;

#[cfg(test)]
mod tests;

pub fn apply(world: &mut World, account: AccountId, command: DiplomacyCommand) -> Result<()> {
    ensure!(command.valid(), "invalid diplomacy command");
    let directory = &world.resource::<Directory>().0;
    let mut diplomacy = directory.diplomacy.clone();
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
            let revision = diplomacy.declarations.get(&key).map_or(0, |d| d.revision);
            ensure!(
                declaration.revision == revision,
                "declaration changed; refresh before publishing"
            );
            declaration.revision = revision.checked_add(1).context("revision exhausted")?;
            diplomacy
                .declaration_history
                .entry(key)
                .or_default()
                .push(declaration.clone());
            diplomacy.declarations.insert(key, declaration);
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
            diplomacy.trust.insert((owner, category), sources);
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
            diplomacy.agreements.insert(
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
            let agreement = diplomacy
                .agreements
                .get_mut(&id)
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
        }
        DiplomacyCommand::CreateBloc { name, founder } => {
            administer(Principal::Sovereignty(founder))?;
            ensure!(
                !diplomacy
                    .blocs
                    .values()
                    .any(|bloc| bloc.members.contains(&founder)),
                "leave the current bloc first"
            );
            ensure!(
                !diplomacy
                    .blocs
                    .values()
                    .any(|bloc| bloc.name.eq_ignore_ascii_case(&name)),
                "bloc name already registered"
            );
            let id = Id::new();
            let posture = diplomacy
                .postures
                .iter()
                .filter_map(|(&(source, target), &standing)| {
                    (source == founder).then_some((target, standing))
                })
                .collect();
            diplomacy.blocs.insert(
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
                    || !diplomacy
                        .blocs
                        .values()
                        .any(|bloc| bloc.members.contains(&polity)),
                "already a bloc member"
            );
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
            if apply {
                bloc.applications.insert(polity);
            } else {
                ensure!(bloc.applications.remove(&polity), "application unavailable");
            }
        }
        DiplomacyCommand::DecideApplication {
            bloc,
            polity,
            admit,
        } => {
            ensure!(
                !admit
                    || !diplomacy
                        .blocs
                        .values()
                        .any(|bloc| bloc.members.contains(&polity)),
                "already a bloc member"
            );
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            ensure!(bloc.applications.remove(&polity), "application unavailable");
            if admit {
                bloc.members.insert(polity);
                bloc.posture.remove(&polity);
                for &member in &bloc.members {
                    diplomacy.postures.remove(&(member, polity));
                }
                for (&target, &standing) in &bloc.posture {
                    if target != polity {
                        diplomacy.postures.insert((polity, target), standing);
                    }
                }
                for bloc in diplomacy.blocs.values_mut() {
                    bloc.applications.remove(&polity);
                }
            }
        }
        DiplomacyCommand::RequestBlocWithdrawal {
            bloc,
            polity,
            request,
        } => {
            administer(Principal::Sovereignty(polity))?;
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
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
        }
        DiplomacyCommand::DecideBlocWithdrawal {
            bloc,
            polity,
            grant,
        } => {
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
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
        }
        DiplomacyCommand::RemoveBlocMember { bloc, polity } => {
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
            ensure!(
                bloc.officers.contains(&account),
                "bloc officer authority required"
            );
            ensure!(bloc.members.remove(&polity), "membership unavailable");
            bloc.withdrawals.remove(&polity);
            // The copied posture deliberately survives departure.
        }
        DiplomacyCommand::SetBlocOfficer {
            bloc,
            account: officer,
            officer: enabled,
        } => {
            known(Principal::Player(officer))?;
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
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
                !diplomacy
                    .blocs
                    .values()
                    .any(|bloc| bloc.members.contains(&polity)),
                "bloc members follow bloc posture"
            );
            diplomacy.postures.insert((polity, target), standing);
        }
        DiplomacyCommand::SetBlocPosture {
            bloc,
            target,
            standing,
        } => {
            known(Principal::Sovereignty(target))?;
            let bloc = diplomacy.blocs.get_mut(&bloc).context("bloc unavailable")?;
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
                diplomacy.postures.insert((member, target), standing);
            }
        }
    }
    ensure!(
        diplomacy.valid(directory),
        "diplomacy capacity or validation failure"
    );
    let mut directory = world.resource_mut::<Directory>();
    for polity in directory.0.sovereignties.values_mut() {
        polity.bloc = if diplomacy
            .blocs
            .get(&super::ownership::principal_id(
                "bloc",
                "Union State of Earth",
            ))
            .is_some_and(|bloc| bloc.members.contains(&polity.id))
        {
            osg_model::ownership::Bloc::Union
        } else if diplomacy
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
    }
    directory.0.diplomacy = diplomacy;
    Ok(())
}
