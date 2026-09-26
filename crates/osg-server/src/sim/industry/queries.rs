use super::*;
use access::{Asset, AssetItem, lookup};
use bevy::ecs::system::SystemParam;
use osg_model::{economy::Currency, rpc::Page, travel::Presence};
use requests::{IndustryQueryQueue, IndustryQueryRequest};

#[derive(SystemParam)]
pub struct IndustryReads<'w, 's> {
    assets: Query<'w, 's, Asset>,
    inventories: Query<'w, 's, &'static hardware::ShipInventory>,
    facilities: Query<'w, 's, &'static IndustrialFacility>,
    parts: Query<'w, 's, &'static hardware::Device>,
    society: Res<'w, crate::sim::society::SocietyState>,
    identities: Res<'w, identity::IdentityIndex>,
    catalogue: Res<'w, vessel::ShipCatalogue>,
    manufacturing: Res<'w, ManufacturingCatalogue>,
}

impl IndustryReads<'_, '_> {
    fn capabilities(&self, asset: &AssetItem<'_, '_>) -> Vec<FacilityCapability> {
        let Ok(facility) = self.facilities.get(asset.entity) else {
            return Vec::new();
        };
        let operational = asset.operational(facility, &self.parts);
        facility
            .modules
            .iter()
            .zip(operational)
            .map(|(module, operational)| FacilityCapability {
                part: module.part_id,
                capability: module.capability,
                lanes: module.lanes.len() as u32,
                power_per_lane_w: module.power_per_lane_w,
                max_radius_m: module.max_radius_m,
                operational,
            })
            .collect()
    }

    fn summary(&self, asset: &AssetItem<'_, '_>, manage: bool, transfer: bool) -> FacilitySummary {
        let modules = self.capabilities(asset);
        let mut metrics = FacilityMetrics {
            system: asset.landmark.map(|landmark| landmark.system),
            power_generated_w: asset.power.map(|power| power.generated_w),
            power_consumed_w: asset.power.map(|power| power.supplied_w),
            total_lanes: modules.iter().map(|module| module.lanes).sum(),
            berths_used: asset.bays.map(|_| {
                asset
                    .stored
                    .map_or(0, |stored| stored.iter().count() as u32)
            }),
            berths_total: asset.bays.map(|bays| bays.0.len() as u32),
            ..Default::default()
        };
        if let Ok(facility) = self.facilities.get(asset.entity) {
            metrics.outside_revenue = Some(vec![
                (Currency::Uec, facility.outside_revenue.uec),
                (Currency::Lat, facility.outside_revenue.lat),
            ]);
            for job in &facility.jobs {
                metrics.busy_lanes += u32::from(job.module_part.is_some());
                metrics.outside_jobs += u32::from(job.payment.is_some());
                match job.status {
                    JobStatus::Queued => metrics.queued_jobs += 1,
                    JobStatus::Running => {}
                    _ => metrics.stalled_jobs += 1,
                }
            }
        }
        FacilitySummary {
            metrics,
            entity: asset.identity.0,
            owner: asset.owner.0,
            name: asset.name(),
            location: asset.location(),
            capabilities: modules
                .into_iter()
                .map(|module| module.capability)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            can_manage: manage,
            can_transfer: transfer,
        }
    }

    fn list(
        &self,
        account: AccountId,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<FacilitySummary, Id>> {
        ensure!((1..=128).contains(&limit), "Invalid page size");
        let mut items: Vec<_> = self
            .assets
            .iter()
            .filter_map(|asset| {
                if !self.inventories.contains(asset.entity)
                    || after.is_some_and(|after| asset.identity.0 <= after)
                {
                    return None;
                }
                let (manage, transfer) =
                    asset.inventory_access(&self.society.directory.0, account)?;
                Some(self.summary(&asset, manage, transfer))
            })
            .collect();
        items.sort_by_key(|entry| entry.entity);
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next = more.then(|| items.last().unwrap().entity);
        Ok(Page {
            items,
            next,
            total: None,
        })
    }

    fn facility(&self, account: AccountId, id: Id) -> Result<FacilityView> {
        let asset = self.assets.get(lookup(&self.identities, id)?)?;
        let (can_manage, can_transfer) = asset
            .inventory_access(&self.society.directory.0, account)
            .context("Facility access unavailable")?;
        let inventory = &self.inventories.get(asset.entity)?.0;
        let summary = self.summary(&asset, can_manage, can_transfer);
        let facility = self.facilities.get(asset.entity).ok();
        Ok(FacilityView {
            metrics: summary.metrics,
            entity: id,
            owner: asset.owner.0,
            name: summary.name,
            can_manage,
            can_transfer,
            cargo_capacity_m3: asset.design.0.capacity_m3,
            cargo_used_m3: inventory.cargo_volume(&self.catalogue.0),
            items: inventory.cargo_stacks(&self.catalogue.0)?,
            products: inventory.product_stacks(&self.catalogue.0)?,
            jobs: facility.map_or_else(Vec::new, IndustrialFacility::job_views),
            capabilities: self.capabilities(&asset),
            location: asset.location(),
            service: facility
                .map(|facility| facility.policy.clone())
                .unwrap_or_default(),
            can_configure_service: asset.operator(&self.society.directory.0, account),
        })
    }

    fn can_focus(&self, asset: &AssetItem<'_, '_>, account: AccountId) -> bool {
        asset.control.is_some()
            && [Permission::View, Permission::Control]
                .into_iter()
                .any(|permission| {
                    asset.permits(
                        &self.society.directory.0,
                        Principal::Player(account),
                        permission,
                    )
                })
    }

    fn hangar(&self, account: AccountId, ship: Id, after: Option<Id>) -> Result<HangarView> {
        let vessel = self.assets.get(lookup(&self.identities, ship)?)?;
        ensure!(
            vessel.available() && self.can_focus(&vessel, account),
            "Hangar access unavailable"
        );
        let host = match vessel.presence.0 {
            Presence::Docked { host, .. } => self.assets.get(lookup(&self.identities, host)?)?,
            Presence::Space
                if vessel.stored.is_some()
                    || vessel.bays.is_some_and(|bays| !bays.0.is_empty()) =>
            {
                vessel
            }
            _ => anyhow::bail!("Hangar access unavailable"),
        };
        ensure!(host.available(), "Hangar access unavailable");
        let host_inventory = host
            .inventory_access(&self.society.directory.0, account)
            .filter(|_| self.inventories.contains(host.entity))
            .map(|(manage, transfer)| self.summary(&host, manage, transfer));
        let mut ships: Vec<_> = host.stored.into_iter().flat_map(|stored| stored.iter()).filter_map(|entity| {
            let asset = self.assets.get(entity).ok()?;
            if !asset.available() || after.is_some_and(|after| asset.identity.0 <= after)
                || !matches!(asset.presence.0, Presence::Docked { host: id, .. } if id == host.identity.0) { return None; }
            let can_focus = self.can_focus(&asset, account);
            let access = asset.inventory_access(&self.society.directory.0, account).filter(|_| self.inventories.contains(entity));
            if !can_focus && access.is_none() { return None; }
            let (manage, transfer) = access.unwrap_or_default();
            Some(HangarEntry {
                inventory: self.summary(&asset, manage, transfer),
                can_focus,
                can_open_inventory: access.is_some(),
                can_control: can_focus && asset.permits(&self.society.directory.0, Principal::Player(account), Permission::Control),
            })
        }).collect();
        ships.sort_by_key(|ship| ship.inventory.entity);
        let more = ships.len() > MAX_DIRECTORY_ENTRIES;
        ships.truncate(MAX_DIRECTORY_ENTRIES);
        let next = more.then(|| ships.last().unwrap().inventory.entity);
        Ok(HangarView {
            berths_used: host
                .bays
                .map(|_| host.stored.map_or(0, |stored| stored.iter().count() as u32)),
            berths_total: host.bays.map(|bays| bays.0.len() as u32),
            ship,
            host: host.identity.0,
            host_name: host.name(),
            host_inventory,
            ships,
            next,
        })
    }

    fn public(
        &self,
        account: AccountId,
        search: &str,
        after: Option<Id>,
        limit: u16,
    ) -> Result<Page<PublicFacility, Id>> {
        ensure!(
            (1..=128).contains(&limit) && search.len() <= 512,
            "invalid facility query"
        );
        let search = search.to_lowercase();
        let mut items: Vec<_> = self
            .assets
            .iter()
            .filter_map(|asset| {
                if !asset.available() || after.is_some_and(|after| asset.identity.0 <= after) {
                    return None;
                }
                let facility = self.facilities.get(asset.entity).ok()?;
                if !facility.policy.accepting || !asset.name().to_lowercase().contains(&search) {
                    return None;
                }
                Some(PublicFacility {
                    summary: self.summary(
                        &asset,
                        asset.operator(&self.society.directory.0, account),
                        false,
                    ),
                    policy: facility.policy.clone(),
                    capabilities: self.capabilities(&asset),
                    queued_jobs: facility
                        .jobs
                        .iter()
                        .filter(|job| job.progress_ticks == 0)
                        .count(),
                    running_jobs: facility
                        .jobs
                        .iter()
                        .filter(|job| job.progress_ticks > 0)
                        .count(),
                })
            })
            .collect();
        items.sort_by_key(|entry| entry.summary.entity);
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next = more.then(|| items.last().unwrap().summary.entity);
        Ok(Page {
            items,
            next,
            total: None,
        })
    }

    fn jobs(&self, account: AccountId, id: Id) -> Result<Vec<JobView>> {
        let asset = self.assets.get(lookup(&self.identities, id)?)?;
        let operator = asset.operator(&self.society.directory.0, account);
        Ok(self
            .facilities
            .get(asset.entity)
            .into_iter()
            .flat_map(|facility| &facility.jobs)
            .filter(|job| {
                operator
                    || job.payment.as_ref().is_some_and(|payment| {
                        self.society.directory.0.administers(account, payment.payer)
                    })
            })
            .map(ProductionJob::view)
            .collect())
    }

    fn quote(
        &self,
        account: AccountId,
        id: Id,
        payer: Principal,
        work: ServiceWork,
        uploads: &crate::blueprint_uploads::BlueprintUploads,
        runtime: &mut osg_ship_wasm::ControllerRuntime,
    ) -> Result<ServiceQuote> {
        let asset = self.assets.get(lookup(&self.identities, id)?)?;
        ensure!(asset.available(), "facility unavailable");
        ensure!(
            self.society.directory.0.administers(account, payer),
            "payer authority required"
        );
        let facility = self.facilities.get(asset.entity)?;
        let plan = construction::prepare_work(
            &work,
            true,
            uploads,
            &self.manufacturing,
            &self.catalogue.0,
            runtime,
        )?;
        ensure!(
            facility.supports(&plan, &asset.operational(facility, &self.parts)),
            "no operational lane can perform this job"
        );
        if let WorkOutput::Ship(bytes) = &plan.output {
            facility.check_budget(bytes.len())?;
            access::construction_bay(
                &self.assets,
                &asset,
                &self.society.directory.0,
                payer,
                plan.required_radius_m,
                construction::dry_mass(&plan, &self.catalogue.0)?,
            )?;
        }
        facility.quote(
            id,
            asset.owner.0,
            payer,
            work,
            &plan,
            &self.society.directory.0,
        )
    }
}

pub fn answer_queries(
    mut queue: ResMut<IndustryQueryQueue>,
    epoch: Res<identity::WorldEpoch>,
    reads: IndustryReads,
    mut runtime: ResMut<vessel::WasmRuntime>,
) {
    while let Some(item) = queue.0.pop_front() {
        match item.request {
            IndustryQueryRequest::Directory(request) => request
                .answer(epoch.0, |(after, limit)| {
                    reads.list(item.account, after, limit)
                }),
            IndustryQueryRequest::Facility(request) => {
                request.answer(epoch.0, |id| reads.facility(item.account, id))
            }
            IndustryQueryRequest::Hangar(request) => request.answer(epoch.0, |(ship, after)| {
                reads.hangar(item.account, ship, after)
            }),
            IndustryQueryRequest::Catalogue(request) => {
                request.answer(epoch.0, |()| Ok(reads.manufacturing.0.clone()))
            }
            IndustryQueryRequest::PublicFacilities(request) => request
                .answer(epoch.0, |(search, after, limit)| {
                    reads.public(item.account, &search, after, limit)
                }),
            IndustryQueryRequest::Jobs(request) => {
                request.answer(epoch.0, |id| reads.jobs(item.account, id))
            }
            IndustryQueryRequest::Quote(request) => request.answer(epoch.0, |(id, payer, work)| {
                reads.quote(item.account, id, payer, work, &item.uploads, &mut runtime.0)
            }),
        }
    }
}

#[cfg(test)]
mod tests;
