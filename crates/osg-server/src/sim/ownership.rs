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
    world.init_resource::<super::economy::Economy>();
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
    for (name, alignment) in [
        ("Union State of Earth", Bloc::Union),
        ("League of Free States", Bloc::League),
    ] {
        let id = principal_id("bloc", name);
        directory.diplomacy.blocs.insert(
            id,
            osg_model::diplomacy::PoliticalBloc {
                id,
                name: name.into(),
                officers: BTreeSet::new(),
                members: directory
                    .sovereignties
                    .values()
                    .filter(|polity| polity.bloc == alignment)
                    .map(|polity| polity.id)
                    .collect(),
                applications: BTreeSet::new(),
                withdrawals: BTreeSet::new(),
                posture: Default::default(),
            },
        );
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
    treaty_permits(&directory.0, owner.0, subject, permission)
        || permits_principal(
            &directory.0,
            owner.0,
            world.get::<AssetAccess>(entity).map(|access| &access.0),
            subject,
            permission,
        )
}

pub fn treaty_permits(
    directory: &OwnershipDirectory,
    owner: Principal,
    subject: Principal,
    permission: Permission,
) -> bool {
    let terms = directory.lineage(owner).into_iter().flat_map(|party| {
        directory
            .lineage(subject)
            .into_iter()
            .flat_map(move |partner| directory.diplomacy.active_terms(party, partner))
    });
    terms.into_iter().any(|term| match permission {
        Permission::Dock => matches!(
            term,
            osg_model::diplomacy::AgreementTerm::DockingAccess
                | osg_model::diplomacy::AgreementTerm::BasingAccess
        ),
        Permission::Navigate => matches!(term, osg_model::diplomacy::AgreementTerm::BasingAccess),
        _ => false,
    })
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
    let asset = world
        .get::<identity::Identity>(entity)
        .context("asset identity unavailable")?
        .0;
    let binding = directory.access_bindings.get(&asset).cloned();
    let policy = if let Some(mut binding) = binding {
        binding.overrides = policy;
        let effective = binding.effective(&directory.access_profiles[&binding.profile].policy);
        ensure!(
            effective.valid(),
            "combined access policy exceeds grant limit"
        );
        world
            .resource_mut::<Directory>()
            .0
            .access_bindings
            .insert(asset, binding);
        effective
    } else {
        policy
    };
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
    if let Some(asset) = world
        .get::<identity::Identity>(entity)
        .map(|identity| identity.0)
    {
        world
            .resource_mut::<Directory>()
            .0
            .access_bindings
            .remove(&asset);
    }
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
        SocietyCommand::UnlinkAccessProfile { asset } => {
            let entity = identity::lookup(world, asset)?;
            authorize(world, account, entity, Permission::ManageAccess)?;
            world
                .resource_mut::<Directory>()
                .0
                .access_bindings
                .remove(&asset);
        }
        SocietyCommand::SetAccessDenied { asset, denied } => {
            let entity = identity::lookup(world, asset)?;
            authorize(world, account, entity, Permission::ManageAccess)?;
            let directory = &world.resource::<Directory>().0;
            let mut binding = directory
                .access_bindings
                .get(&asset)
                .context("asset has no linked profile")?
                .clone();
            binding.denied = denied;
            let policy = binding.effective(&directory.access_profiles[&binding.profile].policy);
            world
                .resource_mut::<Directory>()
                .0
                .access_bindings
                .insert(asset, binding);
            world.entity_mut(entity).insert(AssetAccess(policy));
        }
        SocietyCommand::SaveAccessProfile(profile) => {
            let mut directory = world.resource_mut::<Directory>();
            ensure!(
                directory.0.administers(account, profile.owner),
                "profile administration required"
            );
            if let Some(previous) = directory.0.access_profiles.get(&profile.id) {
                ensure!(
                    previous.owner == profile.owner,
                    "profile owner cannot change"
                );
            }
            ensure!(
                directory.0.access_profiles.contains_key(&profile.id)
                    || directory.0.access_profiles.len() < 1024,
                "access profile limit"
            );
            ensure!(
                !profile.name.trim().is_empty()
                    && profile.name.len() <= 128
                    && !profile.name.chars().any(char::is_control),
                "invalid profile name"
            );
            ensure!(
                profile.policy.valid()
                    && profile
                        .policy
                        .grants
                        .iter()
                        .all(|grant| directory.0.contains(grant.principal)),
                "invalid profile policy"
            );
            let mut updates = Vec::new();
            for (&asset, binding) in &directory.0.access_bindings {
                if binding.profile == profile.id {
                    let policy = binding.effective(&profile.policy);
                    ensure!(policy.valid(), "combined access policy exceeds grant limit");
                    updates.push((asset, policy));
                }
            }
            drop(directory);
            let updates: Vec<_> = updates
                .into_iter()
                .map(|(asset, policy)| {
                    identity::lookup(world, asset).map(|entity| (entity, policy))
                })
                .collect::<Result<_>>()?;
            world
                .resource_mut::<Directory>()
                .0
                .access_profiles
                .insert(profile.id, profile);
            for (entity, policy) in updates {
                world.entity_mut(entity).insert(AssetAccess(policy));
            }
        }
        SocietyCommand::DeleteAccessProfile { id } => {
            let mut directory = world.resource_mut::<Directory>();
            let profile = directory
                .0
                .access_profiles
                .get(&id)
                .context("profile unavailable")?;
            ensure!(
                directory.0.administers(account, profile.owner),
                "profile administration required"
            );
            directory.0.access_profiles.remove(&id);
            directory
                .0
                .access_bindings
                .retain(|_, binding| binding.profile != id);
        }
        SocietyCommand::ApplyAccessProfile { asset, profile } => {
            let directory = &world.resource::<Directory>().0;
            let profile = directory
                .access_profiles
                .get(&profile)
                .context("profile unavailable")?;
            ensure!(
                directory.administers(account, profile.owner),
                "profile administration required"
            );
            let binding = AccessBinding {
                profile: profile.id,
                overrides: AccessPolicy::default(),
                denied: BTreeSet::new(),
            };
            let policy = profile.policy.clone();
            let entity = identity::lookup(world, asset)?;
            authorize(world, account, entity, Permission::ManageAccess)?;
            world.entity_mut(entity).insert(AssetAccess(policy));
            world
                .resource_mut::<Directory>()
                .0
                .access_bindings
                .insert(asset, binding);
        }
        SocietyCommand::Diplomacy(command) => super::diplomacy::apply(world, account, command)?,
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
            world
                .resource_mut::<Directory>()
                .0
                .access_bindings
                .remove(&asset);
        }
    }
    Ok(())
}

pub fn asset_access(
    world: &World,
    account: AccountId,
    id: Id,
) -> Result<osg_model::rpc::AssetAccessDetails> {
    let entity = identity::lookup(world, id)?;
    let owner = world
        .get::<AssetOwner>(entity)
        .context("Asset owner unavailable")?
        .0;
    let can_manage = can_access(world, account, entity, Permission::ManageAccess);
    ensure!(
        can_manage || can_access(world, account, entity, Permission::View),
        "Asset access unavailable"
    );

    let directory = &world.resource::<Directory>().0;
    let binding = can_manage
        .then(|| directory.access_bindings.get(&id).cloned())
        .flatten();
    let profile = binding
        .as_ref()
        .and_then(|binding| directory.access_profiles.get(&binding.profile))
        .cloned();
    Ok(osg_model::rpc::AssetAccessDetails {
        asset: AssetAffiliation {
            entity: id,
            name: world
                .get::<super::vessel::Vessel>(entity)
                .map_or_else(|| id.to_string(), |vessel| vessel.vessel_name.to_string()),
            owner,
            access: world
                .get::<AssetAccess>(entity)
                .map(|access| access.0.clone())
                .unwrap_or_default(),
            can_manage,
        },
        binding,
        profile,
    })
}

pub(crate) fn gas_accounts(world: &World, account: AccountId) -> Vec<GasAccountSnapshot> {
    let source = &world.resource::<Directory>().0;
    let ledger = world.resource::<super::gas::GasLedger>();
    std::iter::once(Principal::Player(account))
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_profiles_require_both_profile_and_asset_authority() {
        let mut world = World::new();
        let owner = Id([1; 16]);
        let other = Id([2; 16]);
        identity::initialize(&mut world, &[owner, other]);
        let entity = world
            .spawn((AssetOwner(Principal::Player(other)), AssetAccess::default()))
            .id();
        let asset = Id::new();
        identity::register(&mut world, entity, asset).unwrap();
        let profile = AccessProfile {
            id: Id::new(),
            owner: Principal::Player(owner),
            name: "Public observation".into(),
            policy: AccessPolicy {
                public: BTreeSet::from([Permission::View]),
                grants: Vec::new(),
            },
        };
        apply(
            &mut world,
            owner,
            SocietyCommand::SaveAccessProfile(profile.clone()),
        )
        .unwrap();
        let command = SocietyCommand::ApplyAccessProfile {
            asset,
            profile: profile.id,
        };
        assert!(apply(&mut world, owner, command.clone()).is_err());
        assert!(apply(&mut world, other, command.clone()).is_err());
        world
            .entity_mut(entity)
            .insert(AssetOwner(Principal::Player(owner)));
        apply(&mut world, owner, command).unwrap();
        assert_eq!(world.get::<AssetAccess>(entity).unwrap().0, profile.policy);
        assert!(
            crate::rpc::list_access_profiles(&world, other)
                .unwrap()
                .is_empty()
        );

        let mut changed = profile.clone();
        changed.policy.public.insert(Permission::Dock);
        apply(
            &mut world,
            owner,
            SocietyCommand::SaveAccessProfile(changed.clone()),
        )
        .unwrap();
        assert!(can_access(&world, other, entity, Permission::Dock));
        apply(
            &mut world,
            owner,
            SocietyCommand::SetAccessDenied {
                asset,
                denied: BTreeSet::from([Permission::Dock]),
            },
        )
        .unwrap();
        assert!(!can_access(&world, other, entity, Permission::Dock));
        changed.policy.public.insert(Permission::TransferCargo);
        apply(
            &mut world,
            owner,
            SocietyCommand::SaveAccessProfile(changed),
        )
        .unwrap();
        assert!(!can_access(&world, other, entity, Permission::Dock));
        assert!(can_access(&world, other, entity, Permission::TransferCargo));
        assert!(can_access(&world, owner, entity, Permission::Dock));
        apply(
            &mut world,
            owner,
            SocietyCommand::UnlinkAccessProfile { asset },
        )
        .unwrap();
        apply(
            &mut world,
            owner,
            SocietyCommand::SaveAccessProfile(profile),
        )
        .unwrap();
        assert!(can_access(&world, other, entity, Permission::TransferCargo));
    }

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

        let owned = gas_accounts(&world, owner);
        assert!(owned.iter().all(GasAccountSnapshot::valid));
        assert_eq!(owned.len(), 2);
        assert!(
            owned
                .iter()
                .any(|account| account.owner == Principal::Organization(organization))
        );
        let ordinary = gas_accounts(&world, member);
        assert!(ordinary.iter().all(GasAccountSnapshot::valid));
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0].owner, Principal::Player(member));

        world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&organization)
            .unwrap()
            .officers
            .remove(&owner);
        assert_eq!(gas_accounts(&world, owner).len(), 1);
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
        identity::register(&mut world, ship, Id([3; 16])).unwrap();
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
        assert!(crate::sim::assets::list(&world, old, "", None).is_empty());
        assert_eq!(
            crate::sim::assets::list(&world, new, "", None)[0].owner,
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
        identity::register(&mut world, ship, asset).unwrap();
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
        identity::register(&mut world, ship, asset).unwrap();
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
            !gas_accounts(&world, account)
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
            crate::rpc::resolve_standing(&world, account, target)
                .unwrap()
                .standing,
            Standing::Friendly
        );
        assert!(
            !crate::rpc::standings(&world, other)
                .unwrap()
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
