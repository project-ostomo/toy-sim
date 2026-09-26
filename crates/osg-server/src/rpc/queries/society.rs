use super::*;
use osg_model::society::*;

pub fn society_view(world: &World, account: AccountId, query: SocietyQuery) -> Result<SocietyView> {
    ensure!(
        query.branches.len() <= 128 && query.advertised.len() <= 512,
        "society query is too large"
    );
    let directory = &world.resource::<sim::society::SocietyState>().directory.0;
    let me = Principal::Player(account);
    let selected = query.selected.unwrap_or(me);
    ensure!(directory.contains(selected), "identity unavailable");
    let mut presentation = SocietyPresentation {
        viewer: account,
        diplomacy: diplomacy(world, account, selected)?,
        standings: standings(world, account)?,
        ..Default::default()
    };
    let mut wanted = BTreeSet::from([me, selected]);
    // Account selectors are returned with their authorization decisions.
    for principal in directory.administered(account) {
        presentation.administered.insert(principal);
        wanted.insert(principal);
    }
    if query.directory {
        presentation.branches = query.branches.clone();
        presentation
            .branches
            .extend([Branch::Blocs, Branch::Polities]);
        for branch in &presentation.branches {
            match *branch {
                Branch::Blocs => {}
                Branch::Polities => wanted.extend(
                    directory
                        .sovereignties
                        .keys()
                        .copied()
                        .map(Principal::Sovereignty),
                ),
                Branch::Organizations(polity) => wanted.extend(
                    list_organizations(world, account, polity)?
                        .into_iter()
                        .map(|org| Principal::Organization(org.id)),
                ),
                Branch::Players(organization) => wanted.extend(
                    list_players(world, account, organization)?
                        .into_iter()
                        .map(|player| Principal::Player(player.account)),
                ),
            }
        }
        if !query.search.trim().is_empty() {
            let search = search_identities(world, account, query.search)?;
            presentation.matches.extend(search.matches);
            wanted.extend(search.identities.into_iter().map(|entry| entry.principal()));
        }
    }
    for (&(source, target), _) in &presentation.standings {
        wanted.extend([source, target]);
    }
    for declaration in presentation.diplomacy.declarations.values() {
        wanted.extend([declaration.source, declaration.target]);
    }
    for agreement in presentation.diplomacy.agreements.values() {
        wanted.extend([agreement.from, agreement.to]);
    }
    for (&(source, _), trustees) in &presentation.diplomacy.trust {
        wanted.insert(source);
        wanted.extend(trustees);
    }
    let mut snapshot = SocietyData {
        account,
        ..Default::default()
    };
    if query.gas {
        snapshot.gas_accounts = gas_balances(world, account)?;
        wanted.extend(snapshot.gas_accounts.iter().map(|balance| balance.owner));
    }
    if query.profiles {
        presentation.access_profiles = list_access_profiles(world, account)?
            .into_iter()
            .map(|profile| (profile.id, profile))
            .collect();
    }
    let mut assets_next = None;
    if query.assets {
        let assets = list_assets(world, account, String::new(), None, query.assets_after, 128)?;
        assets_next = assets.next;
        for asset in assets.items {
            wanted.insert(asset.owner);
            snapshot.assets.push(AssetAffiliation {
                entity: asset.id,
                name: asset.name,
                owner: asset.owner,
                access: Default::default(),
                can_manage: asset.can_manage,
            });
        }
    }
    if let Some(asset) = query.asset {
        let detail = asset_access(world, account, asset)?;
        wanted.insert(detail.asset.owner);
        wanted.extend(
            detail
                .asset
                .access
                .grants
                .iter()
                .map(|grant| grant.principal),
        );
        snapshot.assets.retain(|entry| entry.entity != asset);
        snapshot.assets.push(detail.asset);
        if let Some(binding) = detail.binding {
            presentation.access_bindings.insert(asset, binding);
        }
        if let Some(profile) = detail.profile {
            presentation.access_profiles.insert(profile.id, profile);
        }
    }
    for (owner, organization) in query.advertised {
        wanted.extend(owner.map(Principal::Player));
        wanted.extend(organization.map(Principal::Organization));
        if let Some(standing) = directory.advertised_standing(account, owner, organization) {
            presentation
                .advertised
                .insert((owner, organization), standing);
        }
    }
    wanted = wanted
        .into_iter()
        .flat_map(|principal| directory.lineage(principal))
        .collect();
    for target in wanted {
        let entry = match target {
            Principal::Player(id) => directory
                .players
                .get(&id)
                .cloned()
                .map(IdentityRecord::Player),
            Principal::Organization(id) => directory
                .organizations
                .get(&id)
                .cloned()
                .map(IdentityRecord::Organization),
            Principal::Sovereignty(id) => directory
                .sovereignties
                .get(&id)
                .cloned()
                .map(IdentityRecord::Sovereignty),
        };
        let Some(entry) = entry else { continue };
        match entry {
            IdentityRecord::Player(player) => {
                presentation.players.insert(player.account, player);
            }
            IdentityRecord::Organization(org) => {
                presentation.organizations.insert(org.id, org);
            }
            IdentityRecord::Sovereignty(polity) => {
                presentation.sovereignties.insert(polity.id, polity);
            }
        }
        presentation
            .ancestry
            .insert(target, directory.lineage(target));
        for observer in [me, selected] {
            presentation.reports.insert(
                (observer, target),
                directory.standing_with_source(observer, target),
            );
            if let Some(posture) = directory.political_posture(observer, target) {
                presentation.postures.insert((observer, target), posture);
            }
            for category in [
                osg_model::diplomacy::DeclarationCategory::Standing,
                osg_model::diplomacy::DeclarationCategory::Wanted,
                osg_model::diplomacy::DeclarationCategory::Embargo,
                osg_model::diplomacy::DeclarationCategory::Licence,
                osg_model::diplomacy::DeclarationCategory::Claim,
                osg_model::diplomacy::DeclarationCategory::Recognition,
            ] {
                if let Some(declaration) = directory.diplomacy.resolve(observer, category, target) {
                    presentation
                        .diplomacy
                        .resolved
                        .insert((observer, category, target), declaration.clone());
                }
            }
        }
    }
    snapshot.standing_report = query
        .selected
        .map(|target| resolve_standing(world, account, target))
        .transpose()?;
    snapshot.directory = presentation;
    let history = query
        .declaration_history
        .map(|(source, category, target)| {
            declaration_history(
                world,
                account,
                source,
                category,
                target,
                query.history_before,
                32,
            )
        })
        .transpose()?;
    Ok(SocietyView {
        history_key: query.declaration_history,
        history: history
            .as_ref()
            .map_or_else(Vec::new, |page| page.items.clone()),
        history_next: history.and_then(|page| page.next),
        snapshot,
        assets_next,
        loaded_asset: query.asset,
    })
}
