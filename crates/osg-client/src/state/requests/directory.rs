use super::*;
use osg_model::{diplomacy::PoliticalBloc, ownership::*, rpc::*};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Branch {
    Blocs,
    Polities,
    Organizations(Id),
    Players(Option<Id>),
}

#[derive(Clone, Default)]
pub struct Status {
    pub loaded: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct Entry {
    load: Load<Branch, Records>,
    status: Status,
}

enum Records {
    Blocs(Vec<PoliticalBloc>),
    Identities(Vec<IdentityRecord>),
}

#[derive(Default)]
pub struct State {
    entries: BTreeMap<Branch, Entry>,
    search: Load<String, IdentitySearch>,
    pub matches: BTreeSet<Principal>,
    pub search_status: Status,
}

impl State {
    pub fn loading(&self) -> bool {
        self.entries.values().any(|entry| {
            entry.load.loading() && !entry.status.loaded && entry.status.error.is_none()
        }) || (self.search.loading()
            && !self.search_status.loaded
            && self.search_status.error.is_none())
    }

    pub fn statuses(&self) -> BTreeMap<Branch, Status> {
        self.entries
            .iter()
            .map(|(key, entry)| (*key, entry.status.clone()))
            .collect()
    }

    pub fn update(
        &mut self,
        net: &OsgNetClient,
        context: Option<SessionKey>,
        query: &SocietyQuery,
        now: Duration,
        revision: u64,
        directory: &mut OwnershipDirectory,
    ) {
        let mut active = query.branches.clone();
        if query.directory {
            active.extend([Branch::Blocs, Branch::Polities]);
        } else {
            active.clear();
        }
        for branch in &active {
            self.entries.entry(*branch).or_default();
        }
        for (branch, entry) in &mut self.entries {
            entry.load.refresh(revision);
            let wanted = context
                .filter(|_| active.contains(branch))
                .map(|key| (key, *branch));
            let net = net.clone();
            let (_, result) = entry
                .load
                .update(wanted, now, move |world, branch| fetch(net, world, branch));
            if let Some(result) = result {
                match result {
                    Ok(records) => {
                        replace_branch(directory, *branch, records);
                        entry.status = Status {
                            loaded: true,
                            error: None,
                        };
                    }
                    Err(error) => entry.status.error = Some(error),
                }
            }
        }

        self.search.refresh(revision);
        let search = query.search.trim().to_lowercase();
        let wanted = context
            .filter(|_| query.directory && !search.is_empty())
            .map(|key| (key, search));
        let net = net.clone();
        let (changed, result) = self
            .search
            .update(wanted, now, move |world, search| async move {
                call(net.search_identities(world, search)).await
            });
        if changed {
            self.matches.clear();
            self.search_status = Status::default();
        }
        if let Some(result) = result {
            match result {
                Ok(result) => {
                    self.matches = result.matches.into_iter().collect();
                    merge(directory, result.identities);
                    self.search_status = Status {
                        loaded: true,
                        error: None,
                    };
                }
                Err(error) => self.search_status.error = Some(error),
            }
        }
    }
}

async fn fetch(net: OsgNetClient, world: Id, branch: Branch) -> Result<Records, String> {
    Ok(match branch {
        Branch::Blocs => Records::Blocs(call(net.list_blocs(world)).await?),
        Branch::Polities => Records::Identities(
            call(net.list_polities(world))
                .await?
                .into_iter()
                .map(IdentityRecord::Sovereignty)
                .collect(),
        ),
        Branch::Organizations(polity) => Records::Identities(
            call(net.list_organizations(world, polity))
                .await?
                .into_iter()
                .map(IdentityRecord::Organization)
                .collect(),
        ),
        Branch::Players(organization) => Records::Identities(
            call(net.list_players(world, organization))
                .await?
                .into_iter()
                .map(IdentityRecord::Player)
                .collect(),
        ),
    })
}

fn replace_branch(directory: &mut OwnershipDirectory, branch: Branch, records: Records) {
    match records {
        Records::Blocs(blocs) => {
            directory.diplomacy.blocs = blocs.into_iter().map(|bloc| (bloc.id, bloc)).collect()
        }
        Records::Identities(records) => {
            match branch {
                Branch::Polities => directory.sovereignties.clear(),
                Branch::Organizations(polity) => directory
                    .organizations
                    .retain(|_, org| org.sovereignty != polity),
                Branch::Players(organization) => directory
                    .players
                    .retain(|_, player| player.organization != organization),
                Branch::Blocs => unreachable!(),
            }
            merge(directory, records);
        }
    }
}

pub fn merge(
    directory: &mut OwnershipDirectory,
    records: impl IntoIterator<Item = IdentityRecord>,
) {
    for record in records {
        match record {
            IdentityRecord::Sovereignty(value) => {
                directory.sovereignties.insert(value.id, value);
            }
            IdentityRecord::Organization(value) => {
                directory.organizations.insert(value.id, value);
            }
            IdentityRecord::Player(value) => {
                directory.players.insert(value.account, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_child_lists_replace_membership_without_erasing_other_branches() {
        let first = Id([1; 16]);
        let second = Id([2; 16]);
        let account = Id([3; 16]);
        let other = Id([4; 16]);
        let mut directory = OwnershipDirectory::default();
        merge(
            &mut directory,
            [
                IdentityRecord::Player(PlayerAffiliation {
                    account,
                    name: "Moving pilot".into(),
                    organization: Some(first),
                }),
                IdentityRecord::Player(PlayerAffiliation {
                    account: other,
                    name: "Other pilot".into(),
                    organization: Some(second),
                }),
            ],
        );

        replace_branch(
            &mut directory,
            Branch::Players(Some(first)),
            Records::Identities(Vec::new()),
        );
        assert!(!directory.players.contains_key(&account));
        assert!(directory.players.contains_key(&other));

        replace_branch(
            &mut directory,
            Branch::Players(Some(second)),
            Records::Identities(vec![IdentityRecord::Player(PlayerAffiliation {
                account,
                name: "Moving pilot".into(),
                organization: Some(second),
            })]),
        );
        assert_eq!(directory.players.len(), 1);
        assert_eq!(directory.players[&account].organization, Some(second));
    }

    #[test]
    fn resolving_search_ancestry_does_not_claim_complete_membership() {
        let mut directory = OwnershipDirectory::default();
        let state = State::default();
        let organization = Id([1; 16]);
        merge(
            &mut directory,
            [IdentityRecord::Organization(Organization {
                id: organization,
                name: "Found organization".into(),
                sovereignty: Id([2; 16]),
                open_membership: true,
                officers: BTreeSet::new(),
            })],
        );
        assert!(directory.organizations.contains_key(&organization));
        assert!(
            !state
                .statuses()
                .contains_key(&Branch::Players(Some(organization)))
        );
    }
}
