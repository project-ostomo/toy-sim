use crate::sim::economy::{Record, Records};
use osg_model::{
    Id,
    ownership::{AccessBinding, AccessPolicy},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessBindings {
    records: Records<Arc<(Id, AccessBinding)>>,
}

impl AccessBindings {
    pub fn get(&self, asset: &Id) -> Option<&AccessBinding> {
        self.records.get(asset).map(|row| &row.1)
    }

    pub fn insert(&mut self, asset: Id, binding: AccessBinding) {
        self.records.insert(Arc::new((asset, binding)));
    }

    pub fn remove(&mut self, asset: &Id) {
        self.records.remove(asset);
    }

    pub fn values(&self) -> impl Iterator<Item = &AccessBinding> {
        self.records.values().map(|row| &row.1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Id, &AccessBinding)> {
        self.records.values().map(|row| (&row.0, &row.1))
    }

    pub fn by_profile(&self, profile: Id) -> impl Iterator<Item = (Id, &AccessBinding)> {
        self.records
            .query((profile, super::FIRST_ID)..=(profile, super::LAST_ID))
            .map(|row| (row.0, &row.1))
    }

    pub fn remove_profile(&mut self, profile: Id) {
        let assets: Vec<_> = self.by_profile(profile).map(|(asset, _)| asset).collect();
        for asset in assets {
            self.remove(&asset);
        }
    }
}

impl Record for (Id, AccessBinding) {
    type Key = Id;
    type Index = (Id, Id);

    fn key(&self) -> Id {
        self.0
    }
    fn indexes(&self) -> Vec<Self::Index> {
        vec![(self.1.profile, self.0)]
    }
    fn index_key(index: &Self::Index) -> Id {
        index.1
    }
}

pub fn effective_access(binding: &AccessBinding, profile: &AccessPolicy) -> AccessPolicy {
    let mut result = profile.clone();
    result
        .public
        .extend(binding.overrides.public.iter().copied());
    for grant in &binding.overrides.grants {
        if let Some(existing) = result
            .grants
            .iter_mut()
            .find(|entry| entry.principal == grant.principal)
        {
            existing
                .permissions
                .extend(grant.permissions.iter().copied());
        } else {
            result.grants.push(grant.clone());
        }
    }
    result
        .public
        .retain(|permission| !binding.denied.contains(permission));
    for grant in &mut result.grants {
        grant
            .permissions
            .retain(|permission| !binding.denied.contains(permission));
    }
    result.grants.retain(|grant| !grant.permissions.is_empty());
    result
}
