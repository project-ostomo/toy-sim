use super::super::economy::Record;
use osg_model::{AccountId, Id, diplomacy::*, ownership::*};
use std::collections::BTreeSet;

pub const FIRST_ID: Id = Id([0; 16]);
pub const LAST_ID: Id = Id([255; 16]);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SocialIndex {
    Parent(Option<Id>, Id),
    Officer(AccountId, Id),
    Name(String, Id),
    Gram(String, Id),
    Party(Principal, Option<AgreementStatus>, Id),
    Owner(Principal, Id),
    Member(Id, Id),
    Application(Id, Id),
}

pub fn search_gram(search: &str) -> String {
    search.chars().take(3).collect()
}

fn named(name: &str, id: Id) -> Vec<SocialIndex> {
    let name = name.to_lowercase();
    let characters: Vec<_> = name.chars().collect();
    let mut grams = BTreeSet::new();
    for size in 1..=3.min(characters.len()) {
        for window in characters.windows(size) {
            grams.insert(window.iter().collect::<String>());
        }
    }
    std::iter::once(SocialIndex::Name(name, id))
        .chain(grams.into_iter().map(|gram| SocialIndex::Gram(gram, id)))
        .collect()
}

macro_rules! record {
    ($ty:ty, $row:ident, $key:expr, $indexes:expr) => {
        impl Record for $ty {
            type Key = Id;
            type Index = SocialIndex;

            fn key(&self) -> Id {
                let $row = self;
                $key
            }

            fn indexes(&self) -> Vec<SocialIndex> {
                let $row = self;
                $indexes
            }

            fn index_key(index: &SocialIndex) -> Id {
                match index {
                    SocialIndex::Parent(_, id)
                    | SocialIndex::Officer(_, id)
                    | SocialIndex::Name(_, id)
                    | SocialIndex::Gram(_, id)
                    | SocialIndex::Party(_, _, id)
                    | SocialIndex::Owner(_, id)
                    | SocialIndex::Member(_, id)
                    | SocialIndex::Application(_, id) => *id,
                }
            }
        }
    };
}

record!(Sovereignty, row, row.id, {
    let mut indexes = named(&row.name, row.id);
    indexes.extend(
        row.officers
            .iter()
            .map(|officer| SocialIndex::Officer(*officer, row.id)),
    );
    indexes
});

record!(Organization, row, row.id, {
    let mut indexes = named(&row.name, row.id);
    indexes.push(SocialIndex::Parent(Some(row.sovereignty), row.id));
    indexes.extend(
        row.officers
            .iter()
            .map(|officer| SocialIndex::Officer(*officer, row.id)),
    );
    indexes
});

record!(PlayerAffiliation, row, row.account, {
    let mut indexes = named(&row.name, row.account);
    indexes.push(SocialIndex::Parent(row.organization, row.account));
    indexes
});

record!(Agreement, row, row.id, {
    let mut indexes = Vec::new();
    for party in [row.from, row.to] {
        for status in [None, Some(row.status)] {
            indexes.push(SocialIndex::Party(party, status, row.id));
        }
    }
    indexes
});

record!(PoliticalBloc, row, row.id, {
    let mut indexes = named(&row.name, row.id);
    indexes.extend(
        row.officers
            .iter()
            .map(|officer| SocialIndex::Officer(*officer, row.id)),
    );
    indexes.extend(
        row.members
            .iter()
            .map(|member| SocialIndex::Member(*member, row.id)),
    );
    indexes.extend(
        row.applications
            .iter()
            .map(|member| SocialIndex::Application(*member, row.id)),
    );
    indexes
});

record!(
    AccessProfile,
    row,
    row.id,
    vec![SocialIndex::Owner(row.owner, row.id)]
);
