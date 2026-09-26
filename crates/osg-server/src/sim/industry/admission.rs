use super::*;
use access::{Asset, construction_bay, lookup};
use requests::*;

pub fn configure_services(
    mut society: ResMut<crate::sim::society::SocietyState>,
    mut queue: ResMut<ServicePolicyQueue>,
    epoch: Res<identity::WorldEpoch>,
    index: Res<identity::IdentityIndex>,
    assets: Query<Asset>,
    mut facilities: Query<&mut IndustrialFacility>,
) {
    let state = &mut *society;
    let directory = &state.directory;

    while let Some(request) = queue.0.pop_front() {
        if request.reply.is_closed() {
            continue;
        }
        if request.wrong_world(epoch.0) {
            request.finish(Err(anyhow::anyhow!("World changed; refresh state")));
            continue;
        }
        let result = (|| {
            let entity = lookup(&index, request.arguments.facility)?;
            let asset = assets.get(entity)?;
            ensure!(
                asset.operator(&directory.0, request.account),
                "facility owner authority required"
            );
            ensure!(
                asset.landmark.is_some(),
                "public services require station storage"
            );
            for tier in &request.arguments.policy.tiers {
                ensure!(
                    match tier.customer {
                        CustomerMatch::Principal(principal) => directory.0.contains(principal),
                        CustomerMatch::Bloc(bloc) =>
                            directory.0.diplomacy.blocs.contains_key(&bloc),
                        CustomerMatch::Declaration { source, .. } => directory.0.contains(source),
                        CustomerMatch::Everyone => true,
                    },
                    "unknown customer tier"
                );
            }
            facilities
                .get_mut(entity)?
                .replace_policy(request.arguments.policy.clone())
        })();
        request.finish(result);
    }
}

pub fn cancel_work(
    mut society: ResMut<crate::sim::society::SocietyState>,
    mut queue: ResMut<CancellationQueue>,
    epoch: Res<identity::WorldEpoch>,
    index: Res<identity::IdentityIndex>,
    catalogue: Res<vessel::ShipCatalogue>,
    assets: Query<Asset>,
    mut facilities: Query<(&mut IndustrialFacility, &mut hardware::ShipInventory)>,
) {
    while let Some(request) = queue.0.pop_front() {
        if request.reply.is_closed() {
            continue;
        }
        if request.wrong_world(epoch.0) {
            request.finish(Err(anyhow::anyhow!("World changed; refresh state")));
            continue;
        }
        let mut state = society.clone();
        let directory = &state.directory;
        let mut economy = &mut state.economy;
        let result = (|| {
            let entity = lookup(&index, request.arguments.facility)?;
            let asset = assets.get(entity)?;
            let (mut facility, mut inventory) = facilities.get_mut(entity)?;
            let job = facility
                .job(request.arguments.job)
                .context("job unavailable")?
                .clone();
            let mut next = inventory.0.clone();
            let credit = if let Some(payment) = &job.payment {
                ensure!(
                    asset.operator(&directory.0, request.account)
                        || directory.0.administers(request.account, payment.payer),
                    "job authority required"
                );
                ensure!(
                    !payment.charged && job.progress_ticks == 0,
                    "started jobs cannot be refunded"
                );
                Some((
                    payment.payer,
                    service::storage_credit(
                        &economy,
                        &mut next,
                        asset.identity.0,
                        payment.payer,
                        &job.work.inputs,
                    )?,
                ))
            } else {
                asset.authorize(&directory.0, request.account, Permission::Industry)?;
                None
            };
            let mut next_facility = facility.clone();
            next_facility.cancel(job.id, &mut next, &catalogue.0)?;
            if let Some(payment) = &job.payment {
                economy.release(
                    payment.payer,
                    payment.currency,
                    payment.amount,
                    job.id,
                    osg_model::calendar::now_unix_ms(),
                )?;
            }
            *facility = next_facility;
            inventory.0 = next;
            if let Some((owner, stock)) = credit {
                service::write_stock(&mut economy, asset.identity.0, owner, stock);
            }
            Ok(())
        })();
        if result.is_ok() {
            *society = state;
        }
        request.finish(result);
    }
}

pub fn start_work(
    mut society: ResMut<crate::sim::society::SocietyState>,
    mut queue: ResMut<WorkQueue>,
    epoch: Res<identity::WorldEpoch>,
    index: Res<identity::IdentityIndex>,
    catalogue: Res<vessel::ShipCatalogue>,
    manufacturing: Res<ManufacturingCatalogue>,
    mut runtime: ResMut<vessel::WasmRuntime>,
    assets: Query<Asset>,
    parts: Query<&hardware::Device>,
    mut facilities: Query<(&mut IndustrialFacility, &mut hardware::ShipInventory)>,
) {
    while let Some(item) = queue.0.pop_front() {
        let request = item.request;
        if request.reply.is_closed() {
            continue;
        }
        if request.wrong_world(epoch.0) {
            request.finish(Err(anyhow::anyhow!("World changed; refresh state")));
            continue;
        }
        let mut state = society.clone();
        let directory = &state.directory;
        let mut economy = &mut state.economy;
        let result = (|| {
            let id = match &request.arguments {
                StartWork::Recipe { facility, .. } | StartWork::Ship { facility, .. } => *facility,
                StartWork::Public(quote) => quote.facility,
            };
            let entity = lookup(&index, id)?;
            let asset = assets.get(entity)?;
            ensure!(asset.available(), "facility unavailable");
            let (mut facility, mut inventory) = facilities.get_mut(entity)?;
            let operational = asset.operational(&facility, &parts);
            let (work, owner, public) = match &request.arguments {
                StartWork::Recipe {
                    recipe, batches, ..
                } => (
                    ServiceWork::Recipe {
                        recipe: recipe.clone(),
                        batches: *batches,
                    },
                    asset.owner.0,
                    None,
                ),
                StartWork::Ship {
                    owner,
                    blueprint_hash,
                    ..
                } => (
                    ServiceWork::Ship {
                        blueprint_hash: *blueprint_hash,
                    },
                    *owner,
                    None,
                ),
                StartWork::Public(quote) => (quote.work.clone(), quote.payer, Some(quote)),
            };
            if public.is_some() {
                ensure!(
                    directory.0.administers(request.account, owner),
                    "payer authority required"
                );
            } else {
                asset.authorize(&directory.0, request.account, Permission::Industry)?;
                ensure!(
                    !facility.policy.accepting || asset.operator(&directory.0, request.account),
                    "outside customers must request a service quote"
                );
                ensure!(directory.0.contains(owner), "unknown output owner");
                if owner != asset.owner.0 {
                    asset.authorize(&directory.0, request.account, Permission::TransferCargo)?;
                    ensure!(
                        directory.0.administers(request.account, owner),
                        "output owner not administered by requester"
                    );
                }
            }

            let plan = construction::prepare_work(
                &work,
                public.is_some(),
                &item.uploads,
                &manufacturing,
                &catalogue.0,
                &mut runtime.0,
            )?;
            if let WorkOutput::Ship(_) = &plan.output {
                construction_bay(
                    &assets,
                    &asset,
                    &directory.0,
                    owner,
                    plan.required_radius_m,
                    construction::dry_mass(&plan, &catalogue.0)?,
                )?;
            }
            let quote = public
                .map(|expected| {
                    let current =
                        facility.quote(id, asset.owner.0, owner, work, &plan, &directory.0)?;
                    ensure!(current == *expected, "quote changed; request a new quote");
                    Ok::<_, anyhow::Error>(current)
                })
                .transpose()?;
            let mut job = plan.job(request.account, owner);
            let mut next = inventory.0.clone();
            let mut stock = None;
            if let Some(quote) = quote {
                economy.settle(osg_model::calendar::now_unix_ms());
                ensure!(
                    economy.available(owner, quote.currency) >= quote.total,
                    "insufficient available funds"
                );
                let mut remaining = economy
                    .storage
                    .get(&(id, owner))
                    .cloned()
                    .unwrap_or_default();
                for input in &job.work.inputs {
                    let quantity = remaining
                        .get_mut(&input.item)
                        .context("deliver required inputs to your station storage first")?;
                    ensure!(
                        quantity.saturating_sub(economy.stock_reserved(owner, id, &input.item))
                            >= input.quantity,
                        "deliver required inputs to your station storage first"
                    );
                    *quantity -= input.quantity;
                    let custody = next
                        .custody
                        .get_mut(&input.item)
                        .context("input custody missing")?;
                    *custody = custody
                        .checked_sub(input.quantity)
                        .context("input custody shortage")?;
                }
                remaining = remaining
                    .into_iter()
                    .filter(|(_, quantity)| *quantity > 0)
                    .collect();
                next.custody.retain(|_, quantity| *quantity > 0);
                stock = Some(remaining);
                job.payment = Some(ServicePayment {
                    payer: owner,
                    operator: quote.operator,
                    currency: quote.currency,
                    amount: quote.total,
                    charged: false,
                });
            }
            let payment = job.payment.clone();
            let mut next_facility = facility.clone();
            let job_id = next_facility.admit(job, &mut next, &catalogue.0, &operational)?;
            if let Some(payment) = payment {
                economy.reserve(
                    payment.payer,
                    payment.currency,
                    payment.amount,
                    job_id,
                    osg_model::calendar::now_unix_ms(),
                )?;
            }
            *facility = next_facility;
            inventory.0 = next;
            if let Some(stock) = stock {
                service::write_stock(&mut economy, id, owner, stock);
            }
            Ok(())
        })();
        if result.is_ok() {
            *society = state;
        }
        request.finish(result);
    }
}
