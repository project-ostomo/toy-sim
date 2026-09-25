use super::*;

fn id(value: u128) -> Id {
    Id(value.to_be_bytes())
}

#[test]
fn hierarchy_queries_return_complete_scoped_lists_and_search_ancestry() {
    let account = id(1);
    let polity = id(2);
    let organization = id(3);
    let mut world = World::new();
    let mut directory = OwnershipDirectory::default();
    directory.sovereignties.insert(
        polity,
        Sovereignty {
            id: polity,
            name: "Home polity".into(),
            bloc: Bloc::NonAligned,
            officers: BTreeSet::new(),
        },
    );
    for number in 0..140 {
        let id = id(1000 + number);
        directory.sovereignties.insert(
            id,
            Sovereignty {
                id,
                name: format!("Polity {number}"),
                bloc: Bloc::NonAligned,
                officers: BTreeSet::new(),
            },
        );
        directory.organizations.insert(
            id,
            Organization {
                id,
                name: format!("Organization {number}"),
                sovereignty: polity,
                open_membership: true,
                officers: BTreeSet::new(),
            },
        );
        directory.players.insert(
            id,
            PlayerAffiliation {
                account: id,
                name: format!("Pilot {number}"),
                organization: Some(organization),
            },
        );
    }
    directory.organizations.insert(
        organization,
        Organization {
            id: organization,
            name: "Other organization".into(),
            sovereignty: id(1000),
            open_membership: true,
            officers: BTreeSet::new(),
        },
    );
    directory.players.insert(
        account,
        PlayerAffiliation {
            account,
            name: "Unaffiliated pilot".into(),
            organization: None,
        },
    );
    world.insert_resource(Directory(directory));

    assert_eq!(list_polities(&world, account).unwrap().len(), 141);
    assert_eq!(
        list_organizations(&world, account, polity).unwrap().len(),
        140
    );
    assert!(
        list_organizations(&world, account, id(1001))
            .unwrap()
            .is_empty()
    );
    assert!(list_organizations(&world, account, id(9000)).is_err());
    assert_eq!(
        list_players(&world, account, Some(organization))
            .unwrap()
            .len(),
        140
    );
    assert_eq!(
        list_players(&world, account, None).unwrap()[0].account,
        account
    );
    assert!(list_players(&world, account, Some(id(9000))).is_err());

    let search = search_identities(&world, account, "PILOT 139".into()).unwrap();
    assert_eq!(search.matches, [Principal::Player(id(1139))]);
    let records: BTreeSet<_> = search
        .identities
        .iter()
        .map(IdentityRecord::principal)
        .collect();
    assert_eq!(
        records,
        BTreeSet::from([
            Principal::Player(id(1139)),
            Principal::Organization(organization),
            Principal::Sovereignty(id(1000)),
        ])
    );
    assert_eq!(
        search_identities(&world, account, "Pilot ".into())
            .unwrap()
            .matches
            .len(),
        141
    );
    assert!(
        search_identities(&world, account, " ".into())
            .unwrap()
            .matches
            .is_empty()
    );
}

#[test]
fn scoped_account_lists_are_complete_and_recheck_authority() {
    let account = id(1);
    let other = id(2);
    let mut world = World::new();
    identity::initialize(&mut world, &[account, other]);
    let polity = *world
        .resource::<Directory>()
        .0
        .sovereignties
        .keys()
        .next()
        .unwrap();
    let station = id(999);
    for number in 0..140 {
        let org = id(1000 + number);
        let owner = Principal::Organization(org);
        let mut directory = world.resource_mut::<Directory>();
        directory.0.organizations.insert(
            org,
            Organization {
                id: org,
                name: format!("Managed {number}"),
                sovereignty: polity,
                open_membership: true,
                officers: BTreeSet::from([account]),
            },
        );
        directory.0.access_profiles.insert(
            org,
            AccessProfile {
                id: org,
                owner,
                name: format!("Profile {number}"),
                policy: AccessPolicy::default(),
            },
        );
        world
            .resource::<sim::gas::GasLedger>()
            .ensure_account(owner, 100);
        world
            .resource_mut::<Economy>()
            .storage
            .entry((station, Principal::Player(account)))
            .or_default()
            .insert(CargoItem::Resource(format!("resource-{number}")), 1);
    }

    assert!(list_wallets(&world, account).unwrap().len() >= 140);
    assert!(gas_balances(&world, account).unwrap().len() >= 140);
    assert_eq!(list_access_profiles(&world, account).unwrap().len(), 140);
    assert_eq!(
        storage_stock(&world, account, Principal::Player(account), station)
            .unwrap()
            .len(),
        140
    );
    assert!(storage_stock(&world, other, Principal::Player(account), station).is_err());

    let org = id(1000);
    world
        .resource_mut::<Directory>()
        .0
        .organizations
        .get_mut(&org)
        .unwrap()
        .officers
        .clear();
    assert!(
        !list_wallets(&world, account)
            .unwrap()
            .iter()
            .any(|balance| balance.owner == Principal::Organization(org))
    );
    assert!(
        !gas_balances(&world, account)
            .unwrap()
            .iter()
            .any(|balance| balance.owner == Principal::Organization(org))
    );
    assert!(
        !list_access_profiles(&world, account)
            .unwrap()
            .iter()
            .any(|profile| profile.id == org)
    );
}
