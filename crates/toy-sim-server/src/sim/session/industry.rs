use anyhow::{Result, ensure};
use bevy::prelude::World;
use toy_sim_model::{AccountId, industry::*};

#[derive(Default)]
pub(super) struct IndustrySession {
    subscription: Option<IndustrySubscription>,
    last_revision: Option<u64>,
    last_catalogue: Option<[u8; 32]>,
    last_snapshot: Option<[u8; 32]>,
}

impl IndustrySession {
    pub(super) fn subscribe(&mut self, subscription: IndustrySubscription) -> Result<()> {
        ensure!(
            self.last_revision
                .is_none_or(|revision| subscription.revision > revision),
            "stale industry subscription revision"
        );
        self.last_revision = Some(subscription.revision);
        self.subscription = Some(subscription);
        self.last_catalogue = None;
        self.last_snapshot = None;
        Ok(())
    }

    pub(super) fn unsubscribe(&mut self) {
        self.subscription = None;
        self.last_catalogue = None;
        self.last_snapshot = None;
    }

    pub(super) fn frame(
        &mut self,
        world: &World,
        account: AccountId,
    ) -> Result<Option<IndustrySnapshot>> {
        let Some(subscription) = &self.subscription else {
            return Ok(None);
        };
        let mut snapshot = super::super::industry::snapshot(world, account, subscription);
        toy_sim_protocol::validate_industry_snapshot_content(&snapshot)
            .expect("internal server bug: invalid industry snapshot");
        if snapshot
            .catalogue
            .as_ref()
            .is_some_and(|catalogue| Some(catalogue.revision) == self.last_catalogue)
        {
            snapshot.catalogue = None;
        }
        snapshot = fit_budget(snapshot, subscription)?;

        let catalogue = snapshot.catalogue.take();
        let catalogue_revision = catalogue
            .as_ref()
            .map(|catalogue| catalogue.revision)
            .or(self.last_catalogue);
        let digest =
            *blake3::hash(&postcard::to_allocvec(&(&snapshot, catalogue_revision))?).as_bytes();
        snapshot.catalogue = catalogue;
        if self.last_snapshot == Some(digest) && snapshot.catalogue.is_none() {
            return Ok(None);
        }

        self.last_catalogue = catalogue_revision;
        self.last_snapshot = Some(digest);
        Ok(Some(snapshot))
    }
}

fn fit_budget(
    mut snapshot: IndustrySnapshot,
    subscription: &IndustrySubscription,
) -> Result<IndustrySnapshot> {
    let error = |reason: &str| IndustrySnapshot {
        subscription_revision: subscription.revision,
        error: Some(reason.into()),
        ..Default::default()
    };
    let mut facilities = std::mem::take(&mut snapshot.facilities);
    facilities.sort_by_key(|facility| {
        subscription
            .inventories
            .iter()
            .position(|id| *id == facility.entity)
    });

    let overhead = postcard::to_allocvec(&snapshot)?.len() + 32 * MAX_SUBSCRIBED_INVENTORIES;
    if overhead > MAX_SNAPSHOT_BYTES {
        return Ok(error(
            "Industry directory, hangar or catalogue exceeds the subscription byte limit.",
        ));
    }
    let mut used = overhead;
    for facility in facilities {
        let bytes = postcard::to_allocvec(&facility)?.len();
        if bytes + 32 > MAX_SNAPSHOT_BYTES {
            return Ok(error(
                "One inventory exceeds the subscription byte limit; its contents were not sent.",
            ));
        }
        if used + bytes > MAX_SNAPSHOT_BYTES {
            snapshot.omitted_inventories.push(facility.entity);
        } else {
            used += bytes;
            snapshot.facilities.push(facility);
        }
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::{Id, ownership::Principal};

    fn large_facility(id: u8, count: usize) -> FacilityView {
        FacilityView {
            entity: Id([id; 16]),
            owner: Principal::Player(Id([1; 16])),
            name: "Cargo warehouse".into(),
            can_manage: true,
            can_transfer: true,
            cargo_capacity_m3: 1e8,
            cargo_used_m3: count as f64,
            items: (0..count)
                .map(|index| CargoStack {
                    item: CargoItem::Part(format!("{index:0128}")),
                    quantity: 1,
                    reserved: 0,
                    name: "N".repeat(128),
                    unit_mass_kg: 1.,
                    unit_volume_m3: 1.,
                })
                .collect(),
            products: Vec::new(),
            jobs: Vec::new(),
            capabilities: Vec::new(),
            location: None,
        }
    }

    #[test]
    fn industry_byte_budget_omits_whole_inventories_in_requested_priority_order() {
        let subscription = IndustrySubscription {
            revision: 5,
            inventories: vec![Id([2; 16]), Id([1; 16])],
            ..Default::default()
        };
        let snapshot = IndustrySnapshot {
            subscription_revision: 5,
            facilities: vec![large_facility(1, 1024), large_facility(2, 1024)],
            ..Default::default()
        };
        let bounded = fit_budget(snapshot, &subscription).unwrap();
        assert!(bounded.error.is_none());
        assert_eq!(bounded.facilities.len(), 1);
        assert_eq!(bounded.facilities[0].entity, Id([2; 16]));
        assert_eq!(bounded.facilities[0].items.len(), 1024);
        assert_eq!(bounded.omitted_inventories, vec![Id([1; 16])]);
        assert!(postcard::to_allocvec(&bounded).unwrap().len() <= MAX_SNAPSHOT_BYTES);
    }

    #[test]
    fn hangar_summaries_count_toward_the_shared_snapshot_byte_budget() {
        let summary = |number| FacilitySummary {
            entity: Id([number; 16]),
            owner: Principal::Player(Id([1; 16])),
            name: "H".repeat(128),
            location: Some(Id([254; 16])),
            capabilities: Vec::new(),
            can_manage: false,
            can_transfer: true,
        };
        let hangar = HangarView {
            ship: Id([1; 16]),
            host: Id([254; 16]),
            host_name: "Local warehouse".into(),
            host_inventory: Some(FacilitySummary {
                entity: Id([254; 16]),
                ..summary(254)
            }),
            ships: (1..=128)
                .map(|number| HangarEntry {
                    inventory: summary(number),
                    can_focus: true,
                    can_open_inventory: true,
                    can_control: false,
                })
                .collect(),
            next: None,
        };
        let subscription = IndustrySubscription {
            revision: 6,
            inventories: vec![Id([2; 16]), Id([1; 16])],
            ..Default::default()
        };
        let mut snapshot = IndustrySnapshot {
            subscription_revision: 6,
            hangar: Some(hangar.clone()),
            facilities: vec![large_facility(1, 900), large_facility(2, 900)],
            ..Default::default()
        };
        toy_sim_protocol::validate_industry_snapshot_content(&snapshot).unwrap();
        let with_hangar = fit_budget(snapshot.clone(), &subscription).unwrap();
        snapshot.hangar = None;
        let without_hangar = fit_budget(snapshot, &subscription).unwrap();
        assert_eq!(without_hangar.facilities.len(), 2);
        assert_eq!(with_hangar.facilities.len(), 1);
        assert_eq!(with_hangar.omitted_inventories, vec![Id([1; 16])]);
        assert_eq!(with_hangar.hangar, Some(hangar));
        assert!(postcard::to_allocvec(&with_hangar).unwrap().len() <= MAX_SNAPSHOT_BYTES);
    }

    #[test]
    fn oversized_catalogue_returns_a_bounded_revisioned_error() {
        let subscription = IndustrySubscription {
            revision: 9,
            inventories: vec![Id([1; 16])],
            ..Default::default()
        };
        let bounded = fit_budget(
            IndustrySnapshot {
                subscription_revision: 9,
                catalogue: Some(IndustryCatalogue {
                    revision: [1; 32],
                    recipes: Vec::new(),
                    blueprints: (0..16)
                        .map(|index| BlueprintView {
                            name: format!("Blueprint {index}"),
                            blueprint: vec![0; 48 * 1024],
                            inputs: Vec::new(),
                            duration_ticks: 1,
                            energy_j: 1,
                        })
                        .collect(),
                }),
                ..Default::default()
            },
            &subscription,
        )
        .unwrap();
        assert_eq!(bounded.subscription_revision, 9);
        assert!(bounded.error.is_some());
        assert!(bounded.facilities.is_empty());
        assert!(postcard::to_allocvec(&bounded).unwrap().len() < 512);
    }
}
