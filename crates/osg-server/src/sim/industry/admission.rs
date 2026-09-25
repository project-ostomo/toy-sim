use super::super::economy::Economy;
use super::*;
use access::{Asset, construction_bay, lookup};
use requests::*;

pub fn configure_services(
    mut queue: ResMut<ServicePolicyQueue>,
    epoch: Res<identity::WorldEpoch>,
    mut history: ResMut<crate::OperationHistory>,
    index: Res<identity::IdentityIndex>,
    directory: Res<ownership::Directory>,
    assets: Query<Asset>,
    mut facilities: Query<&mut IndustrialFacility>,
) {
    while let Some(request) = queue.0.pop_front() {
        if request.reply.is_closed() {
            continue;
        }
        if let Some(result) = request.previous(epoch.0, &history) {
            let _ = request.reply.send(result);
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
        request.finish(&mut history, result);
    }
}

pub fn cancel_work(
    mut queue: ResMut<CancellationQueue>,
    epoch: Res<identity::WorldEpoch>,
    mut history: ResMut<crate::OperationHistory>,
    index: Res<identity::IdentityIndex>,
    directory: Res<ownership::Directory>,
    catalogue: Res<vessel::ShipCatalogue>,
    mut economy: ResMut<Economy>,
    assets: Query<Asset>,
    mut facilities: Query<(&mut IndustrialFacility, &mut hardware::ShipInventory)>,
) {
    while let Some(request) = queue.0.pop_front() {
        if request.reply.is_closed() {
            continue;
        }
        if let Some(result) = request.previous(epoch.0, &history) {
            let _ = request.reply.send(result);
            continue;
        }
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
            facility.cancel(job.id, &mut next, &catalogue.0)?;
            inventory.0 = next;
            if let Some((owner, stock)) = credit {
                service::write_stock(&mut economy, asset.identity.0, owner, stock);
                economy.service_holds.remove(&job.id);
            }
            Ok(())
        })();
        request.finish(&mut history, result);
    }
}

pub fn start_work(
    mut queue: ResMut<WorkQueue>,
    epoch: Res<identity::WorldEpoch>,
    mut history: ResMut<crate::OperationHistory>,
    index: Res<identity::IdentityIndex>,
    directory: Res<ownership::Directory>,
    catalogue: Res<vessel::ShipCatalogue>,
    manufacturing: Res<ManufacturingCatalogue>,
    mut runtime: ResMut<vessel::WasmRuntime>,
    mut economy: ResMut<Economy>,
    assets: Query<Asset>,
    parts: Query<&hardware::Device>,
    mut facilities: Query<(&mut IndustrialFacility, &mut hardware::ShipInventory)>,
) {
    while let Some(item) = queue.0.pop_front() {
        let request = item.request;
        if request.reply.is_closed() {
            continue;
        }
        if let Some(result) = request.previous(epoch.0, &history) {
            let _ = request.reply.send(result);
            continue;
        }
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
                remaining.retain(|_, quantity| *quantity > 0);
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
            let job_id = facility.admit(job, &mut next, &catalogue.0, &operational)?;
            inventory.0 = next;
            if let Some(stock) = stock {
                service::write_stock(&mut economy, id, owner, stock);
            }
            if let Some(payment) = payment {
                economy.service_holds.insert(job_id, payment);
            }
            Ok(())
        })();
        request.finish(&mut history, result);
    }
}
