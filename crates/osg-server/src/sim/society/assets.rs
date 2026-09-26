use super::OwnershipDirectory;
use crate::sim::{
    economy::{Record, Records},
    hardware, identity, industry, infrastructure, ownership,
};
use bevy::prelude::*;
use osg_model::{
    AccountId, Id,
    industry::CargoItem,
    ownership::{Permission, Principal},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct AssetRecord {
    pub id: Id,
    pub entity: Entity,
    pub owner: Principal,
    access: osg_model::ownership::AccessPolicy,
    market: bool,
    pub inputs: BTreeMap<(Principal, CargoItem), u64>,
}

impl Record for AssetRecord {
    type Key = Id;
    type Index = AssetIndex;

    fn key(&self) -> Id {
        self.id
    }

    fn indexes(&self) -> Vec<AssetIndex> {
        let mut indexes = vec![
            AssetIndex::Owner(self.owner, self.id),
            AssetIndex::Entity(self.entity, self.id),
        ];
        let visible = |permission: &Permission| {
            matches!(
                permission,
                Permission::View
                    | Permission::TransferCargo
                    | Permission::Industry
                    | Permission::ManageAccess
            )
        };
        if self.access.public.iter().any(visible) {
            indexes.push(AssetIndex::Grant(None, self.id));
        }
        for grant in &self.access.grants {
            if grant.permissions.iter().any(visible) {
                indexes.push(AssetIndex::Grant(Some(grant.principal), self.id));
            }
        }
        if self.market {
            indexes.push(AssetIndex::Market(self.id));
        }
        indexes.extend(
            self.inputs
                .keys()
                .map(|(owner, _)| AssetIndex::Customer(*owner, self.id)),
        );
        indexes
    }

    fn index_key(index: &AssetIndex) -> Id {
        match index {
            AssetIndex::Owner(_, id)
            | AssetIndex::Entity(_, id)
            | AssetIndex::Grant(_, id)
            | AssetIndex::Customer(_, id)
            | AssetIndex::Market(id) => *id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssetIndex {
    Owner(Principal, Id),
    Entity(Entity, Id),
    Grant(Option<Principal>, Id),
    Customer(Principal, Id),
    Market(Id),
}

#[derive(Resource, Clone, Debug, Default)]
pub struct AssetRecords {
    records: Records<AssetRecord>,
}

impl AssetRecords {
    pub fn visible<'a>(
        &'a self,
        directory: &OwnershipDirectory,
        account: AccountId,
        owner: Option<Principal>,
        after: Option<Id>,
    ) -> impl Iterator<Item = &'a AssetRecord> {
        let first = after.unwrap_or(Id([0; 16]));
        let last = Id([255; 16]);
        let mut ranges = Vec::new();
        if let Some(owner) = owner {
            ranges.push(AssetIndex::Owner(owner, first)..=AssetIndex::Owner(owner, last));
        } else {
            for owner in directory.administered(account) {
                ranges.push(AssetIndex::Owner(owner, first)..=AssetIndex::Owner(owner, last));
            }
            for principal in std::iter::once(None).chain(
                directory
                    .lineage(Principal::Player(account))
                    .into_iter()
                    .map(Some),
            ) {
                ranges
                    .push(AssetIndex::Grant(principal, first)..=AssetIndex::Grant(principal, last));
            }
        }
        let mut streams: Vec<_> = ranges
            .into_iter()
            .map(|range| self.records.query(range).peekable())
            .collect();
        // Merge ordered index ranges lazily so a page only visits its candidates.
        std::iter::from_fn(move || {
            let id = streams
                .iter_mut()
                .filter_map(|stream| stream.peek().map(|row| row.id))
                .min()?;
            for stream in &mut streams {
                if stream.peek().is_some_and(|row| row.id == id) {
                    stream.next();
                }
            }
            self.records.get(&id)
        })
        .filter(move |record| after.is_none_or(|after| record.id > after))
    }

    pub fn customers(&self, owner: Principal) -> impl Iterator<Item = &AssetRecord> {
        self.records.query(
            AssetIndex::Customer(owner, Id([0; 16]))..=AssetIndex::Customer(owner, Id([255; 16])),
        )
    }

    pub fn markets(&self, after: Option<Id>) -> impl Iterator<Item = &AssetRecord> {
        self.records
            .query(
                AssetIndex::Market(after.unwrap_or(Id([0; 16])))
                    ..=AssetIndex::Market(Id([255; 16])),
            )
            .filter(move |record| after.is_none_or(|after| record.id > after))
    }
}

/// Publication is a normal schedule phase. Changes to physical components
/// become queryable at this boundary, without hooks or deferred domain work.
pub fn publish_assets(
    mut assets_index: ResMut<AssetRecords>,
    assets: Query<(
        Entity,
        &identity::Identity,
        &ownership::AssetOwner,
        Option<&ownership::AssetAccess>,
        Option<&infrastructure::Landmark>,
        Option<&hardware::ShipInventory>,
        Option<&industry::IndustrialFacility>,
    )>,
    changed: Query<
        Entity,
        Or<(
            Changed<identity::Identity>,
            Changed<ownership::AssetOwner>,
            Changed<ownership::AssetAccess>,
            Changed<infrastructure::Landmark>,
            Changed<hardware::ShipInventory>,
            Changed<industry::IndustrialFacility>,
        )>,
    >,
    mut removed: RemovedComponents<identity::Identity>,
    mut removed_owner: RemovedComponents<ownership::AssetOwner>,
    mut removed_access: RemovedComponents<ownership::AssetAccess>,
    mut removed_landmark: RemovedComponents<infrastructure::Landmark>,
    mut removed_inventory: RemovedComponents<hardware::ShipInventory>,
    mut removed_facility: RemovedComponents<industry::IndustrialFacility>,
) {
    let records = &mut assets_index.records;
    let affected: BTreeSet<_> = changed
        .iter()
        .chain(removed.read())
        .chain(removed_owner.read())
        .chain(removed_access.read())
        .chain(removed_landmark.read())
        .chain(removed_inventory.read())
        .chain(removed_facility.read())
        .collect();
    for entity in affected {
        let ids: Vec<_> = records
            .query(
                AssetIndex::Entity(entity, Id([0; 16]))..=AssetIndex::Entity(entity, Id([255; 16])),
            )
            .map(|record| record.id)
            .collect();
        for id in ids {
            records.remove(&id);
        }
        let Ok((entity, identity, owner, access, landmark, inventory, facility)) =
            assets.get(entity)
        else {
            continue;
        };
        let mut inputs = BTreeMap::<_, u64>::new();
        for job in facility.into_iter().flat_map(|facility| facility.jobs()) {
            if let Some(payment) = &job.payment {
                for stack in &job.work.inputs {
                    let quantity = inputs
                        .entry((payment.payer, stack.item.clone()))
                        .or_default();
                    *quantity = quantity.saturating_add(stack.quantity);
                }
            }
        }
        records.insert(AssetRecord {
            id: identity.0,
            entity,
            owner: owner.0,
            access: access.map(|access| access.0.clone()).unwrap_or_default(),
            market: landmark.is_some() && inventory.is_some(),
            inputs,
        });
    }
}
