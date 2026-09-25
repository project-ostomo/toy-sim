use super::*;
use osg_model::{diplomacy::DeclarationCategory, ownership::Standing};

fn fixture() -> (World, AccountId, Id, Id, Id) {
    let mut world = World::new();
    let account = Id([1; 16]);
    super::super::identity::initialize(&mut world, &[account]);
    world.resource_mut::<Directory>().0.diplomacy.blocs.clear();
    let first = super::super::ownership::sovereignty_id("Helion Commonwealth");
    let second = super::super::ownership::sovereignty_id("Aurora Compact");
    let target = super::super::ownership::sovereignty_id("USE");
    for polity in [first, second] {
        world
            .resource_mut::<Directory>()
            .0
            .sovereignties
            .get_mut(&polity)
            .unwrap()
            .officers
            .insert(account);
    }
    (world, account, first, second, target)
}

#[test]
fn active_terms_grant_navigation_and_inherit_wanted_and_defence() {
    use osg_model::diplomacy::{Agreement, AgreementStatus, AgreementTerm, Declaration};
    let (mut world, _account, first, second, target) = fixture();
    let owner = Principal::Sovereignty(first);
    let partner = Principal::Sovereignty(second);
    let subject = Principal::Sovereignty(target);
    let mut directory = world.resource_mut::<Directory>();
    let id = Id([8; 16]);
    directory.0.diplomacy.agreements.insert(
        id,
        Agreement {
            id,
            from: owner,
            to: partner,
            title: "Mutual pact".into(),
            terms: vec![
                AgreementTerm::DockingAccess,
                AgreementTerm::BasingAccess,
                AgreementTerm::HonorWanted,
                AgreementTerm::MutualDefence,
            ],
            note: String::new(),
            status: AgreementStatus::Active,
            revision: 2,
        },
    );
    let declaration = Declaration {
        source: partner,
        target: subject,
        category: DeclarationCategory::Wanted,
        revision: 1,
        standing: Standing::Hostile,
        enabled: true,
        note: "Wanted".into(),
    };
    directory
        .0
        .diplomacy
        .declarations
        .insert((partner, DeclarationCategory::Wanted, subject), declaration);
    directory
        .0
        .standings
        .insert((partner, subject), Standing::Hostile);
    assert!(
        directory
            .0
            .diplomacy
            .resolve(owner, DeclarationCategory::Wanted, subject)
            .is_some()
    );
    assert_eq!(directory.0.standing(owner, subject), Standing::Hostile);
    drop(directory);
    let host = world
        .spawn((
            super::super::ownership::AssetOwner(owner),
            super::super::ownership::AssetAccess::default(),
        ))
        .id();
    assert!(super::super::ownership::principal_access(
        &world,
        partner,
        host,
        osg_model::ownership::Permission::Dock
    ));
    assert!(super::super::ownership::principal_access(
        &world,
        partner,
        host,
        osg_model::ownership::Permission::Navigate
    ));
    assert!(!super::super::ownership::principal_access(
        &world,
        partner,
        host,
        osg_model::ownership::Permission::TransferCargo
    ));
}

#[test]
fn declaration_revisions_keep_ordered_public_history() {
    let (mut world, account, first, _, target) = fixture();
    let source = Principal::Sovereignty(first);
    let target = Principal::Sovereignty(target);
    for (revision, enabled) in [(0, true), (1, false)] {
        apply(
            &mut world,
            account,
            DiplomacyCommand::Publish(Declaration {
                source,
                target,
                category: DeclarationCategory::Embargo,
                revision,
                standing: Standing::Neutral,
                enabled,
                note: format!("revision {revision}"),
            }),
        )
        .unwrap();
    }
    let history = &world
        .resource::<Directory>()
        .0
        .diplomacy
        .declaration_history[&(source, DeclarationCategory::Embargo, target)];
    assert_eq!(
        history
            .iter()
            .map(|entry| entry.revision)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        !world.resource::<Directory>().0.diplomacy.declarations
            [&(source, DeclarationCategory::Embargo, target)]
            .enabled
    );
}

#[test]
fn application_requires_officer_decision_and_departure_retains_posture() {
    let (mut world, account, first, second, target) = fixture();
    apply(
        &mut world,
        account,
        DiplomacyCommand::CreateBloc {
            name: "Test federation".into(),
            founder: first,
        },
    )
    .unwrap();
    let bloc = *world
        .resource::<Directory>()
        .0
        .diplomacy
        .blocs
        .keys()
        .next()
        .unwrap();
    apply(
        &mut world,
        account,
        DiplomacyCommand::SetBlocPosture {
            bloc,
            target,
            standing: Standing::Hostile,
        },
    )
    .unwrap();
    apply(
        &mut world,
        account,
        DiplomacyCommand::ApplyToBloc {
            bloc,
            polity: second,
            apply: true,
        },
    )
    .unwrap();
    assert!(
        !world.resource::<Directory>().0.diplomacy.blocs[&bloc]
            .members
            .contains(&second)
    );
    assert!(
        apply(
            &mut world,
            Id([9; 16]),
            DiplomacyCommand::DecideApplication {
                bloc,
                polity: second,
                admit: true
            }
        )
        .is_err()
    );
    apply(
        &mut world,
        account,
        DiplomacyCommand::DecideApplication {
            bloc,
            polity: second,
            admit: true,
        },
    )
    .unwrap();
    apply(
        &mut world,
        account,
        DiplomacyCommand::RequestBlocWithdrawal {
            bloc,
            polity: second,
            request: true,
        },
    )
    .unwrap();
    assert!(
        world.resource::<Directory>().0.diplomacy.blocs[&bloc]
            .members
            .contains(&second)
    );
    assert!(
        apply(
            &mut world,
            account,
            DiplomacyCommand::SetPosture {
                polity: second,
                target,
                standing: Standing::Neutral,
            },
        )
        .is_err()
    );
    apply(
        &mut world,
        account,
        DiplomacyCommand::DecideBlocWithdrawal {
            bloc,
            polity: second,
            grant: true,
        },
    )
    .unwrap();
    assert_eq!(
        world.resource::<Directory>().0.diplomacy.postures[&(second, target)],
        Standing::Hostile
    );
    apply(
        &mut world,
        account,
        DiplomacyCommand::SetPosture {
            polity: second,
            target,
            standing: Standing::Neutral,
        },
    )
    .unwrap();
    assert_eq!(
        world.resource::<Directory>().0.diplomacy.postures[&(second, target)],
        Standing::Neutral
    );
}

#[test]
fn withdrawals_require_member_authority_and_officer_resolution() {
    let (mut world, account, first, second, _) = fixture();
    apply(
        &mut world,
        account,
        DiplomacyCommand::CreateBloc {
            name: "Withdrawal test".into(),
            founder: first,
        },
    )
    .unwrap();
    let bloc = *world
        .resource::<Directory>()
        .0
        .diplomacy
        .blocs
        .keys()
        .next()
        .unwrap();
    let polity_officer = Id([7; 16]);
    world
        .resource_mut::<Directory>()
        .0
        .sovereignties
        .get_mut(&first)
        .unwrap()
        .officers
        .insert(polity_officer);

    let request = DiplomacyCommand::RequestBlocWithdrawal {
        bloc,
        polity: first,
        request: true,
    };
    let cancel = DiplomacyCommand::RequestBlocWithdrawal {
        bloc,
        polity: first,
        request: false,
    };
    let grant = DiplomacyCommand::DecideBlocWithdrawal {
        bloc,
        polity: first,
        grant: true,
    };
    let refuse = DiplomacyCommand::DecideBlocWithdrawal {
        bloc,
        polity: first,
        grant: false,
    };
    assert!(apply(&mut world, Id([9; 16]), request.clone()).is_err());
    assert!(
        apply(
            &mut world,
            account,
            DiplomacyCommand::RequestBlocWithdrawal {
                bloc,
                polity: second,
                request: true
            }
        )
        .is_err()
    );
    assert!(apply(&mut world, account, grant.clone()).is_err());

    apply(&mut world, polity_officer, request.clone()).unwrap();
    assert!(apply(&mut world, polity_officer, request.clone()).is_err());
    assert!(apply(&mut world, polity_officer, grant.clone()).is_err());
    assert!(
        apply(
            &mut world,
            polity_officer,
            DiplomacyCommand::RemoveBlocMember {
                bloc,
                polity: first
            }
        )
        .is_err()
    );
    assert!(
        world.resource::<Directory>().0.diplomacy.blocs[&bloc]
            .withdrawals
            .contains(&first)
    );
    apply(&mut world, polity_officer, cancel.clone()).unwrap();
    assert!(apply(&mut world, account, grant).is_err());

    apply(&mut world, polity_officer, request.clone()).unwrap();
    apply(&mut world, account, refuse).unwrap();
    assert!(
        world.resource::<Directory>().0.diplomacy.blocs[&bloc]
            .members
            .contains(&first)
    );
    assert!(
        world.resource::<Directory>().0.diplomacy.blocs[&bloc]
            .withdrawals
            .is_empty()
    );
    assert!(apply(&mut world, polity_officer, cancel).is_err());

    apply(&mut world, polity_officer, request).unwrap();
    apply(
        &mut world,
        account,
        DiplomacyCommand::RemoveBlocMember {
            bloc,
            polity: first,
        },
    )
    .unwrap();
    let bloc = &world.resource::<Directory>().0.diplomacy.blocs[&bloc];
    assert!(bloc.members.is_empty());
    assert!(bloc.withdrawals.is_empty());
}

#[test]
fn declarations_use_revisions_and_trust_preserves_provenance() {
    let (mut world, account, first, second, _) = fixture();
    let source = Principal::Sovereignty(first);
    let target = Principal::Sovereignty(second);
    let observer = Principal::Player(account);
    let declaration = Declaration {
        source,
        target,
        category: DeclarationCategory::Standing,
        revision: 0,
        standing: Standing::Hostile,
        enabled: true,
        note: "Public declaration".into(),
    };
    apply(
        &mut world,
        account,
        DiplomacyCommand::Publish(declaration.clone()),
    )
    .unwrap();
    assert!(apply(&mut world, account, DiplomacyCommand::Publish(declaration)).is_err());
    apply(
        &mut world,
        account,
        DiplomacyCommand::SetTrust {
            owner: observer,
            category: DeclarationCategory::Standing,
            sources: vec![source],
        },
    )
    .unwrap();
    let directory = &world.resource::<Directory>().0;
    let resolved = directory
        .diplomacy
        .resolve(observer, DeclarationCategory::Standing, target)
        .unwrap();
    assert_eq!(resolved.source, source);
    assert_eq!(resolved.revision, 1);
    assert_eq!(directory.standing(observer, target), Standing::Hostile);
    let private = crate::rpc::diplomacy(&world, Id([9; 16]), observer).unwrap();
    assert!(private.trust.is_empty());
}

#[test]
fn proposer_cannot_accept_for_counterparty_and_terminated_agreement_stays_closed() {
    let (mut world, account, first, _, target) = fixture();
    let from = Principal::Sovereignty(first);
    let to = Principal::Sovereignty(target);
    apply(
        &mut world,
        account,
        DiplomacyCommand::ProposeAgreement {
            from,
            to,
            title: "Accord".into(),
            terms: vec![AgreementTerm::DockingAccess],
            note: "Mutual recognition".into(),
        },
    )
    .unwrap();
    let id = *world
        .resource::<Directory>()
        .0
        .diplomacy
        .agreements
        .keys()
        .next()
        .unwrap();
    assert!(
        apply(
            &mut world,
            account,
            DiplomacyCommand::ChangeAgreement {
                id,
                expected_revision: 1,
                status: AgreementStatus::Active
            }
        )
        .is_err()
    );
    apply(
        &mut world,
        account,
        DiplomacyCommand::ChangeAgreement {
            id,
            expected_revision: 1,
            status: AgreementStatus::Terminated,
        },
    )
    .unwrap();
    assert!(
        apply(
            &mut world,
            account,
            DiplomacyCommand::ChangeAgreement {
                id,
                expected_revision: 2,
                status: AgreementStatus::Active
            }
        )
        .is_err()
    );
}
