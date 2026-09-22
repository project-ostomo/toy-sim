use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::ownership::*;
use osg_model::{AccountId, Id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::identity::{self, Control};

#[derive(Resource, Clone, Default)]
pub struct Directory(pub OwnershipDirectory);

#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AssetOwner(pub Principal);

#[derive(Component, Clone, Debug, Default, Serialize, Deserialize)]
pub struct AssetAccess(pub AccessPolicy);

pub fn principal_id(kind: &str, name: &str) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame ownership identity v1");
    hash.update(kind.as_bytes());
    hash.update(&[0]);
    hash.update(name.as_bytes());
    Id(hash.finalize().as_bytes()[..16].try_into().unwrap())
}

pub fn sovereignty_id(name: &str) -> Id {
    principal_id("sovereignty", name)
}

pub fn organization_id(name: &str) -> Id {
    Id(osg_universe::organizations::organization_id(name))
}

pub fn initialize(world: &mut World) {
    world.init_resource::<super::gas::GasLedger>();
    if world.contains_resource::<Directory>() {
        return;
    }
    let mut directory = OwnershipDirectory::default();
    for (name, bloc) in [
        ("USE", Bloc::Union),
        ("Helion Commonwealth", Bloc::League),
        ("Aurora Compact", Bloc::League),
        ("Meridian League", Bloc::League),
        ("St Raphael Commonwealth", Bloc::League),
        ("Vesper Freeports", Bloc::League),
        ("Lyra Research Compact", Bloc::League),
        ("Nova Partenia", Bloc::NonAligned),
        ("Concord Free State", Bloc::NonAligned),
        ("Terminus Protectorate", Bloc::NonAligned),
    ] {
        let id = sovereignty_id(name);
        directory.sovereignties.insert(
            id,
            Sovereignty {
                id,
                name: name.into(),
                bloc,
                officers: BTreeSet::new(),
            },
        );
    }
    for system in &osg_universe::civilization::map().systems {
        let id = sovereignty_id(&system.sovereignty);
        directory
            .sovereignties
            .entry(id)
            .or_insert_with(|| Sovereignty {
                id,
                name: system.sovereignty.clone(),
                bloc: match system.alignment {
                    osg_universe::civilization::Alignment::Use => Bloc::Union,
                    osg_universe::civilization::Alignment::Lfs => Bloc::League,
                    osg_universe::civilization::Alignment::Independent => Bloc::NonAligned,
                },
                officers: BTreeSet::new(),
            });
    }
    for profile in osg_universe::organizations::catalogue() {
        let id = Id(profile.id());
        directory.organizations.insert(
            id,
            Organization {
                id,
                name: profile.name.clone(),
                sovereignty: sovereignty_id(&profile.sovereignty),
                open_membership: profile.open_membership,
                officers: BTreeSet::new(),
            },
        );
    }
    for profile in osg_universe::organizations::catalogue() {
        let source = Principal::Organization(Id(profile.id()));
        for relation in &profile.relations {
            let target = Principal::Organization(organization_id(&relation.organization));
            let standing = match relation.standing {
                osg_universe::organizations::LoreStanding::Friendly => Standing::Friendly,
                osg_universe::organizations::LoreStanding::Neutral => Standing::Neutral,
                osg_universe::organizations::LoreStanding::Hostile => Standing::Hostile,
            };
            directory.standings.insert((source, target), standing);
        }
    }
    let ledger = world.resource::<super::gas::GasLedger>();
    for &id in directory.sovereignties.keys() {
        ledger.ensure_account(Principal::Sovereignty(id), super::gas::STARTING_GAS);
    }
    for &id in directory.organizations.keys() {
        ledger.ensure_account(Principal::Organization(id), super::gas::STARTING_GAS);
    }
    world.insert_resource(Directory(directory));
}

pub fn add_account(world: &mut World, account: AccountId) {
    initialize(world);
    world
        .resource::<super::gas::GasLedger>()
        .ensure_account(Principal::Player(account), super::gas::STARTING_GAS);
    world
        .resource_mut::<Directory>()
        .0
        .players
        .entry(account)
        .or_insert_with(|| PlayerAffiliation {
            account,
            name: format!("Pilot {}", &account.to_string()[..8]),
            organization: Some(organization_id("Helion Flight Cooperative")),
        });
}

pub fn affiliate(world: &mut World, account: AccountId, organization: Option<Id>) -> Result<()> {
    add_account(world, account);
    let mut directory = world.resource_mut::<Directory>();
    ensure!(
        organization.is_none_or(|id| directory.0.organizations.contains_key(&id)),
        "organization unavailable"
    );
    directory.0.players.get_mut(&account).unwrap().organization = organization;
    Ok(())
}

pub fn permits_principal(
    directory: &OwnershipDirectory,
    owner: Principal,
    access: Option<&AccessPolicy>,
    subject: Principal,
    permission: Permission,
) -> bool {
    owner == subject
        || matches!(subject, Principal::Player(account) if directory.administers(account, owner))
        || access.is_some_and(|access| access.permits_principal(directory, subject, permission))
}

pub fn principal_access(
    world: &World,
    subject: Principal,
    entity: Entity,
    permission: Permission,
) -> bool {
    let (Some(directory), Some(owner)) = (
        world.get_resource::<Directory>(),
        world.get::<AssetOwner>(entity),
    ) else {
        return false;
    };
    permits_principal(
        &directory.0,
        owner.0,
        world.get::<AssetAccess>(entity).map(|access| &access.0),
        subject,
        permission,
    )
}

pub fn port_access(
    world: &World,
    subject: Principal,
    host: Entity,
    permission: Permission,
    public: bool,
    allowed: &BTreeSet<AccountId>,
) -> bool {
    public
        || matches!(subject, Principal::Player(account) if allowed.contains(&account))
        || principal_access(world, subject, host, permission)
}

pub fn can_access(
    world: &World,
    account: AccountId,
    entity: Entity,
    permission: Permission,
) -> bool {
    principal_access(world, Principal::Player(account), entity, permission)
}

pub fn authorize(
    world: &World,
    account: AccountId,
    entity: Entity,
    permission: Permission,
) -> Result<()> {
    ensure!(
        can_access(world, account, entity, permission),
        "{permission:?} access denied"
    );
    Ok(())
}

pub fn set_access(
    world: &mut World,
    account: AccountId,
    entity: Entity,
    policy: AccessPolicy,
) -> Result<()> {
    authorize(world, account, entity, Permission::ManageAccess)?;
    ensure!(policy.valid(), "invalid access grants");
    let directory = &world.resource::<Directory>().0;
    ensure!(
        policy
            .grants
            .iter()
            .all(|grant| directory.contains(grant.principal)),
        "grant principal unavailable"
    );
    world.entity_mut(entity).insert(AssetAccess(policy));
    Ok(())
}

pub fn capture_control(world: &mut World, entity: Entity, account: AccountId) -> Result<()> {
    let player = identity::add_account(world, account, false);
    let revision = world
        .get::<Control>(entity)
        .context("ship control unavailable")?
        .revision
        .checked_add(1)
        .context("control revision exhausted")?;
    world.entity_mut(entity).insert((
        Control { account, revision },
        identity::ControlledBy(player),
        AssetOwner(Principal::Player(account)),
        AssetAccess::default(),
    ));
    Ok(())
}

pub fn apply(world: &mut World, account: AccountId, command: SocietyCommand) -> Result<()> {
    match command {
        SocietyCommand::CreateOrganization { name } => {
            let name = name.trim();
            ensure!(
                !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
                "invalid organization name"
            );
            let directory = &world.resource::<Directory>().0;
            ensure!(directory.organizations.len() < 16384, "organization limit");
            ensure!(
                !directory
                    .organizations
                    .values()
                    .any(|organization| organization.name.eq_ignore_ascii_case(name)),
                "organization name already registered"
            );
            let previous = directory
                .players
                .get(&account)
                .and_then(|player| player.organization)
                .context("join a sovereign organization before founding one")?;
            let sovereignty = directory.organizations[&previous].sovereignty;
            let id = Id::new();
            let mut directory = world.resource_mut::<Directory>();
            directory
                .0
                .organizations
                .get_mut(&previous)
                .unwrap()
                .officers
                .remove(&account);
            directory.0.organizations.insert(
                id,
                Organization {
                    id,
                    name: name.into(),
                    sovereignty,
                    open_membership: true,
                    officers: BTreeSet::from([account]),
                },
            );
            directory.0.players.get_mut(&account).unwrap().organization = Some(id);
            world
                .resource::<super::gas::GasLedger>()
                .ensure_account(Principal::Organization(id), super::gas::STARTING_GAS);
        }
        SocietyCommand::SetOfficer {
            organization,
            account: member,
            officer,
        } => {
            let mut directory = world.resource_mut::<Directory>();
            ensure!(
                directory
                    .0
                    .administers(account, Principal::Organization(organization)),
                "officer access denied"
            );
            ensure!(
                directory
                    .0
                    .players
                    .get(&member)
                    .is_some_and(|player| player.organization == Some(organization)),
                "officer must be an organization member"
            );
            let organization = directory.0.organizations.get_mut(&organization).unwrap();
            if officer {
                organization.officers.insert(member);
            } else {
                organization.officers.remove(&member);
            }
        }
        SocietyCommand::SetStanding { target, standing } => {
            let mut directory = world.resource_mut::<Directory>();
            ensure!(directory.0.contains(target), "standing target unavailable");
            ensure!(
                target != Principal::Player(account),
                "cannot change standing toward yourself"
            );
            let key = (Principal::Player(account), target);
            if let Some(standing) = standing {
                ensure!(
                    directory.0.standings.contains_key(&key)
                        || directory
                            .0
                            .standings
                            .keys()
                            .filter(|(source, _)| *source == key.0)
                            .count()
                            < 256,
                    "personal standing limit"
                );
                directory.0.standings.insert(key, standing);
            } else {
                directory.0.standings.remove(&key);
            }
        }
        SocietyCommand::SetMembership {
            account: member,
            organization,
        } => {
            let directory = &world.resource::<Directory>().0;
            let current = directory
                .players
                .get(&member)
                .context("player unavailable")?
                .organization;
            let authorized = match organization {
                Some(id) => {
                    let organization = directory
                        .organizations
                        .get(&id)
                        .context("organization unavailable")?;
                    member == account
                        && (organization.open_membership
                            || organization.officers.contains(&account))
                }
                None => {
                    member == account
                        || current.is_some_and(|id| {
                            directory.administers(account, Principal::Organization(id))
                        })
                }
            };
            ensure!(authorized, "membership access denied");
            if let Some(current) = current.filter(|current| Some(*current) != organization) {
                world
                    .resource_mut::<Directory>()
                    .0
                    .organizations
                    .get_mut(&current)
                    .unwrap()
                    .officers
                    .remove(&member);
            }
            affiliate(world, member, organization)?;
        }
        SocietyCommand::SetAssetAccess { asset, policy } => {
            let entity = identity::lookup(world, asset)?;
            set_access(world, account, entity, policy)?;
        }
        SocietyCommand::TransferAsset { asset, owner } => {
            let entity = identity::lookup(world, asset)?;
            let old_owner = world
                .get::<AssetOwner>(entity)
                .context("asset owner unavailable")?
                .0;
            let directory = &world.resource::<Directory>().0;
            ensure!(
                directory.administers(account, old_owner),
                "ownership transfer access denied"
            );
            ensure!(directory.contains(owner), "recipient unavailable");
            ensure!(
                directory.administers(account, owner),
                "recipient must accept ownership"
            );
            world
                .entity_mut(entity)
                .insert((AssetOwner(owner), AssetAccess::default()));
        }
    }
    Ok(())
}

pub fn snapshot(world: &World, account: AccountId) -> SocietySnapshot {
    let source = &world.resource::<Directory>().0;
    let lineage = source.lineage(Principal::Player(account));
    let mut directory = source.clone();
    directory
        .standings
        .retain(|(observer, _), _| lineage.contains(observer));
    let mut administrators = std::collections::BTreeMap::new();
    let mut assets: Vec<_> = world
        .resource::<identity::IdentityIndex>()
        .0
        .iter()
        .filter_map(|(id, entity)| {
            let owner = world.get::<AssetOwner>(*entity)?.0;
            let administers = *administrators
                .entry(owner)
                .or_insert_with(|| source.administers(account, owner));
            let access = world.get::<AssetAccess>(*entity);
            let permits = |permission| {
                administers
                    || access.is_some_and(|access| access.0.permits_lineage(&lineage, permission))
            };
            let can_manage = permits(Permission::ManageAccess);
            if !can_manage && !permits(Permission::View) {
                return None;
            }
            Some(AssetAffiliation {
                entity: *id,
                name: world
                    .get::<super::vessel::Vessel>(*entity)
                    .map_or_else(|| id.to_string(), |vessel| vessel.vessel_name.to_string()),
                owner,
                access: access.map(|access| access.0.clone()).unwrap_or_default(),
                can_manage,
            })
        })
        .collect();
    assets.sort_by_key(|asset| asset.entity);
    let ledger = world.resource::<super::gas::GasLedger>();
    let gas_accounts = std::iter::once(Principal::Player(account))
        .chain(
            source
                .organizations
                .keys()
                .copied()
                .map(Principal::Organization),
        )
        .chain(
            source
                .sovereignties
                .keys()
                .copied()
                .map(Principal::Sovereignty),
        )
        .filter(|owner| source.administers(account, *owner))
        .filter_map(|owner| ledger.account(owner))
        .collect();

    SocietySnapshot {
        account,
        directory,
        gas_accounts,
        assets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_organization_profiles_seed_exact_owners_and_directed_standings() {
        let mut world = World::new();
        initialize(&mut world);
        let directory = &world.resource::<Directory>().0;
        let profiles = osg_universe::organizations::catalogue();
        assert!(profiles.len() >= 100);
        assert_eq!(directory.organizations.len(), profiles.len());
        for profile in profiles {
            let id = Id(profile.id());
            assert_eq!(id, principal_id("organization", &profile.name));
            let organization = &directory.organizations[&id];
            assert_eq!(organization.name, profile.name);
            assert_eq!(
                organization.sovereignty,
                sovereignty_id(&profile.sovereignty)
            );
            assert_eq!(organization.open_membership, profile.open_membership);
            assert!(organization.officers.is_empty());
            for relation in &profile.relations {
                let expected = match relation.standing {
                    osg_universe::organizations::LoreStanding::Friendly => Standing::Friendly,
                    osg_universe::organizations::LoreStanding::Neutral => Standing::Neutral,
                    osg_universe::organizations::LoreStanding::Hostile => Standing::Hostile,
                };
                assert_eq!(
                    directory.standings.get(&(
                        Principal::Organization(id),
                        Principal::Organization(organization_id(&relation.organization)),
                    )),
                    Some(&expected)
                );
            }
        }
        let nova = &directory.sovereignties[&sovereignty_id("Nova Partenia")];
        assert_eq!(nova.bloc, Bloc::NonAligned);
    }

    #[test]
    fn gas_balances_are_visible_only_to_the_owner_or_current_administrators() {
        let mut world = World::new();
        let owner = Id([1; 16]);
        let member = Id([2; 16]);
        identity::initialize(&mut world, &[owner, member]);
        let organization = organization_id("Helion Flight Cooperative");
        world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&organization)
            .unwrap()
            .officers
            .insert(owner);

        let owned = snapshot(&world, owner);
        assert!(owned.valid());
        assert_eq!(owned.gas_accounts.len(), 2);
        assert!(
            owned
                .gas_accounts
                .iter()
                .any(|account| account.owner == Principal::Organization(organization))
        );
        let ordinary = snapshot(&world, member);
        assert!(ordinary.valid());
        assert_eq!(ordinary.gas_accounts.len(), 1);
        assert_eq!(ordinary.gas_accounts[0].owner, Principal::Player(member));

        world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&organization)
            .unwrap()
            .officers
            .remove(&owner);
        assert_eq!(snapshot(&world, owner).gas_accounts.len(), 1);
    }

    #[test]
    fn captured_iff_does_not_grant_access_to_previous_owner() {
        let mut world = World::new();
        let old = Id([1; 16]);
        let new = Id([2; 16]);
        identity::initialize(&mut world, &[old, new]);
        let ship = world
            .spawn((
                Control {
                    account: old,
                    revision: 1,
                },
                AssetOwner(Principal::Player(old)),
                identity::Transponder(osg_model::IffIdentity {
                    owner: old,
                    faction: Some(organization_id("Unifleet Defense")),
                    labels: BTreeSet::new(),
                    enabled: true,
                }),
            ))
            .id();
        identity::register(&mut world, ship, Id([3; 16]));
        capture_control(&mut world, ship, new).unwrap();
        assert_eq!(
            world.get::<identity::Transponder>(ship).unwrap().0.owner,
            old
        );
        assert_eq!(
            world.get::<identity::Transponder>(ship).unwrap().0.faction,
            Some(organization_id("Unifleet Defense"))
        );
        assert!(!can_access(&world, old, ship, Permission::Industry));
        assert!(can_access(&world, new, ship, Permission::Industry));
        assert!(snapshot(&world, old).assets.is_empty());
        assert_eq!(
            snapshot(&world, new).assets[0].owner,
            Principal::Player(new)
        );
    }

    #[test]
    fn membership_and_access_changes_require_authority() {
        let mut world = World::new();
        let account = Id([1; 16]);
        let stranger = Id([2; 16]);
        identity::initialize(&mut world, &[account, stranger]);
        let defense = organization_id("Unifleet Defense");
        assert!(
            apply(
                &mut world,
                account,
                SocietyCommand::SetMembership {
                    account,
                    organization: Some(defense)
                }
            )
            .is_err()
        );
        assert!(
            apply(
                &mut world,
                account,
                SocietyCommand::SetMembership {
                    account: stranger,
                    organization: None
                }
            )
            .is_err()
        );
        let services = organization_id("Unifleet Station Services");
        apply(
            &mut world,
            account,
            SocietyCommand::SetMembership {
                account,
                organization: Some(services),
            },
        )
        .unwrap();
        assert_eq!(
            world.resource::<Directory>().0.players[&account].organization,
            Some(services)
        );

        let asset = Id([3; 16]);
        let ship = world
            .spawn((
                AssetOwner(Principal::Player(account)),
                Control {
                    account,
                    revision: 1,
                },
            ))
            .id();
        identity::register(&mut world, ship, asset);
        let policy = AccessPolicy {
            public: BTreeSet::new(),
            grants: vec![AccessGrant {
                principal: Principal::Player(stranger),
                permissions: BTreeSet::from([Permission::Control]),
            }],
        };
        assert!(set_access(&mut world, stranger, ship, policy.clone()).is_err());
        set_access(&mut world, account, ship, policy).unwrap();
        assert!(can_access(&world, stranger, ship, Permission::Control));
        assert!(!can_access(&world, stranger, ship, Permission::Configure));
        assert!(!can_access(
            &world,
            stranger,
            ship,
            Permission::ManageAccess
        ));
        assert!(
            apply(
                &mut world,
                stranger,
                SocietyCommand::TransferAsset {
                    asset,
                    owner: Principal::Player(stranger)
                }
            )
            .is_err()
        );
    }

    #[test]
    fn founders_administer_organization_assets_and_keep_office_when_reselecting_membership() {
        let mut world = World::new();
        let account = Id([1; 16]);
        let recruit = Id([2; 16]);
        identity::initialize(&mut world, &[account, recruit]);
        apply(
            &mut world,
            account,
            SocietyCommand::CreateOrganization {
                name: "Neris Independent Haulers".into(),
            },
        )
        .unwrap();
        let organization = world.resource::<Directory>().0.players[&account]
            .organization
            .unwrap();
        assert!(
            world
                .resource::<Directory>()
                .0
                .administers(account, Principal::Organization(organization))
        );
        apply(
            &mut world,
            account,
            SocietyCommand::SetMembership {
                account,
                organization: Some(organization),
            },
        )
        .unwrap();
        assert!(
            world
                .resource::<Directory>()
                .0
                .administers(account, Principal::Organization(organization))
        );
        assert!(
            apply(
                &mut world,
                recruit,
                SocietyCommand::CreateOrganization {
                    name: "neris independent haulers".into()
                }
            )
            .is_err()
        );
        let asset = Id([3; 16]);
        let ship = world
            .spawn((
                AssetOwner(Principal::Player(account)),
                Control {
                    account,
                    revision: 1,
                },
            ))
            .id();
        identity::register(&mut world, ship, asset);
        apply(
            &mut world,
            account,
            SocietyCommand::TransferAsset {
                asset,
                owner: Principal::Organization(organization),
            },
        )
        .unwrap();
        assert_eq!(
            world.get::<AssetOwner>(ship).unwrap().0,
            Principal::Organization(organization)
        );
        assert!(can_access(&world, account, ship, Permission::ManageAccess));
        apply(
            &mut world,
            recruit,
            SocietyCommand::SetMembership {
                account: recruit,
                organization: Some(organization),
            },
        )
        .unwrap();
        assert!(!can_access(&world, recruit, ship, Permission::ManageAccess));
        apply(
            &mut world,
            account,
            SocietyCommand::SetOfficer {
                organization,
                account: recruit,
                officer: true,
            },
        )
        .unwrap();
        apply(
            &mut world,
            account,
            SocietyCommand::SetMembership {
                account,
                organization: None,
            },
        )
        .unwrap();
        assert!(!can_access(&world, account, ship, Permission::ManageAccess));
        assert!(can_access(&world, recruit, ship, Permission::ManageAccess));
    }

    #[test]
    fn last_officer_can_leave_without_taking_organization_assets_or_gas() {
        let mut world = World::new();
        let account = Id([1; 16]);
        identity::initialize(&mut world, &[account]);
        apply(
            &mut world,
            account,
            SocietyCommand::CreateOrganization {
                name: "Independent Test Cooperative".into(),
            },
        )
        .unwrap();
        let organization = world.resource::<Directory>().0.players[&account]
            .organization
            .unwrap();
        let owner = Principal::Organization(organization);
        let ship = world.spawn(AssetOwner(owner)).id();
        let balance = world
            .resource::<super::super::gas::GasLedger>()
            .account(owner)
            .unwrap();

        apply(
            &mut world,
            account,
            SocietyCommand::SetMembership {
                account,
                organization: None,
            },
        )
        .unwrap();

        let directory = &world.resource::<Directory>().0;
        assert_eq!(directory.players[&account].organization, None);
        assert!(directory.organizations[&organization].officers.is_empty());
        assert_eq!(world.get::<AssetOwner>(ship).unwrap().0, owner);
        assert!(!can_access(&world, account, ship, Permission::ManageAccess));
        assert_eq!(
            world
                .resource::<super::super::gas::GasLedger>()
                .account(owner),
            Some(balance)
        );
        assert!(
            !snapshot(&world, account)
                .gas_accounts
                .iter()
                .any(|account| account.owner == owner)
        );
    }

    #[test]
    fn personal_overrides_are_private_and_membership_does_not_rewrite_iff() {
        let mut world = World::new();
        let account = Id([1; 16]);
        let other = Id([2; 16]);
        identity::initialize(&mut world, &[account, other]);
        let target = Principal::Organization(organization_id("Terminus Privateers"));
        assert_eq!(
            world
                .resource::<Directory>()
                .0
                .standing(Principal::Player(account), target),
            Standing::Hostile
        );
        apply(
            &mut world,
            account,
            SocietyCommand::SetStanding {
                target,
                standing: Some(Standing::Friendly),
            },
        )
        .unwrap();
        assert_eq!(
            snapshot(&world, account)
                .directory
                .standing(Principal::Player(account), target),
            Standing::Friendly
        );
        assert!(
            !snapshot(&world, other)
                .directory
                .standings
                .contains_key(&(Principal::Player(account), target))
        );
        apply(
            &mut world,
            account,
            SocietyCommand::SetStanding {
                target,
                standing: None,
            },
        )
        .unwrap();
        assert_eq!(
            world
                .resource::<Directory>()
                .0
                .standing(Principal::Player(account), target),
            Standing::Hostile
        );
    }
}
