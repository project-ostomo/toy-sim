use super::*;
use crate::sim::economy::Economy;
use osg_model::{economy::Currency, rpc::Page};

pub fn operator(world: &World, account: AccountId, facility: Entity) -> bool {
    world
        .get::<ownership::AssetOwner>(facility)
        .is_some_and(|owner| {
            world
                .resource::<ownership::Directory>()
                .0
                .administers(account, owner.0)
        })
}

pub fn publish(
    world: &mut World,
    account: AccountId,
    facility: Id,
    mut policy: ServicePolicy,
) -> Result<()> {
    let entity = identity::lookup(world, facility)?;
    ensure!(
        operator(world, account, entity),
        "facility owner authority required"
    );
    ensure!(policy.valid(), "invalid service policy");
    ensure!(
        world
            .get::<super::super::infrastructure::Landmark>(entity)
            .is_some(),
        "public services require station storage"
    );
    let mut queue = world
        .get::<IndustryFacility>(entity)
        .cloned()
        .unwrap_or_default();
    ensure!(
        policy.revision == queue.service.revision,
        "prices changed; reload before publishing"
    );
    let capabilities = publication::capabilities(world, entity);
    for rate in &policy.rates {
        let installed: u32 = capabilities
            .iter()
            .filter(|module| module.capability == rate.capability)
            .map(|module| module.lanes)
            .sum();
        ensure!(
            rate.public_lanes <= installed,
            "public lane allocation exceeds installed lanes"
        );
    }
    let directory = &world.resource::<ownership::Directory>().0;
    for tier in &policy.tiers {
        ensure!(
            match tier.customer {
                CustomerMatch::Principal(principal) => directory.contains(principal),
                CustomerMatch::Bloc(bloc) => directory.diplomacy.blocs.contains_key(&bloc),
                CustomerMatch::Declaration { source, .. } => directory.contains(source),
                CustomerMatch::Everyone => true,
            },
            "unknown customer tier"
        );
    }
    policy.revision = policy
        .revision
        .checked_add(1)
        .context("price revision exhausted")?;
    queue.service = policy;
    world.entity_mut(entity).insert(queue);
    Ok(())
}

pub fn list(
    world: &World,
    account: AccountId,
    search: String,
    after: Option<Id>,
    limit: u16,
) -> Result<Page<PublicFacility, Id>> {
    ensure!(
        (1..=128).contains(&limit) && search.len() <= 512,
        "invalid facility query"
    );
    let search = search.to_lowercase();
    let mut items = Vec::new();
    for (&id, &entity) in world.resource::<identity::IdentityIndex>().entries() {
        if after.is_some_and(|after| id <= after) || !available(world, entity) {
            continue;
        }
        let Some(queue) = world
            .get::<IndustryFacility>(entity)
            .filter(|queue| queue.service.accepting)
        else {
            continue;
        };
        let Some(summary) =
            publication::summary(world, entity, operator(world, account, entity), false)
        else {
            continue;
        };
        if !summary.name.to_lowercase().contains(&search) {
            continue;
        }
        items.push(PublicFacility {
            summary,
            policy: queue.service.clone(),
            capabilities: publication::capabilities(world, entity),
            queued_jobs: queue
                .jobs
                .iter()
                .filter(|job| job.view.progress_ticks == 0)
                .count(),
            running_jobs: queue
                .jobs
                .iter()
                .filter(|job| job.view.progress_ticks > 0)
                .count(),
        });
    }
    items.sort_by_key(|facility| facility.summary.entity);
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(Page {
        next: more.then(|| items.last().unwrap().summary.entity),
        items,
        total: None,
    })
}

fn tier_matches(directory: &OwnershipDirectory, customer: Principal, rule: &CustomerMatch) -> bool {
    let lineage = directory.lineage(customer);
    match rule {
        CustomerMatch::Principal(principal) => lineage.contains(principal),
        CustomerMatch::Bloc(id) => directory.diplomacy.blocs.get(id).is_some_and(|bloc| {
            lineage.iter().any(|principal| matches!(principal, Principal::Sovereignty(id) if bloc.members.contains(id)))
        }),
        CustomerMatch::Declaration { source, category } => lineage.iter().any(|target| {
            directory.diplomacy.declarations.get(&(*source, *category, *target)).is_some_and(|declaration| declaration.enabled)
        }),
        CustomerMatch::Everyone => true,
    }
}

fn prepare(
    world: &mut World,
    account: AccountId,
    facility: Entity,
    payer: Principal,
    work: &ServiceWork,
    uploads: &crate::blueprint_uploads::BlueprintUploads,
) -> Result<IndustryJob> {
    match work {
        ServiceWork::Recipe { recipe, batches } => {
            recipe_job(world, account, payer, recipe, *batches)
        }
        ServiceWork::Ship { blueprint_hash } => {
            let bytes = world
                .resource::<CatalogueCache>()
                .0
                .blueprints
                .iter()
                .find(|blueprint| blake3::hash(&blueprint.blueprint).as_bytes() == blueprint_hash)
                .map(|blueprint| blueprint.blueprint.clone());
            if let Some(bytes) = bytes {
                construction::prepare(world, account, facility, payer, &bytes)
            } else {
                let bytes = uploads.get(*blueprint_hash)?;
                construction::prepare(world, account, facility, payer, bytes.as_ref().as_ref())
            }
        }
    }
}

pub(super) fn recipe_job(
    world: &World,
    account: AccountId,
    owner: Principal,
    recipe: &str,
    batches: u32,
) -> Result<IndustryJob> {
    ensure!(
        (1..=MAX_RECIPE_BATCHES).contains(&batches),
        "invalid batch count"
    );
    let recipe = world
        .resource::<CatalogueCache>()
        .0
        .recipes
        .iter()
        .find(|entry| entry.id == recipe)
        .context("recipe unavailable")?;
    let scale = |stacks: &[ItemStack]| -> Result<Vec<ItemStack>> {
        stacks
            .iter()
            .map(|stack| {
                Ok(ItemStack {
                    item: stack.item.clone(),
                    quantity: stack
                        .quantity
                        .checked_mul(u64::from(batches))
                        .context("quantity overflow")?,
                })
            })
            .collect()
    };
    Ok(IndustryJob {
        view: JobView {
            id: Id::new(),
            name: recipe.name.clone(),
            capability: recipe.capability,
            progress_ticks: 0,
            duration_ticks: recipe
                .duration_ticks
                .checked_mul(u64::from(batches))
                .context("duration overflow")?,
            status: JobStatus::Queued,
            owner,
            created_by: account,
            module_part: None,
            requested_power_w: 0,
            supplied_power_w: 0,
            payment: None,
        },
        inputs: scale(&recipe.inputs)?,
        output: JobOutput::Cargo(scale(&recipe.outputs)?),
        energy_j: recipe
            .energy_j
            .checked_mul(u64::from(batches))
            .context("energy overflow")?,
        stored_energy_j: recipe
            .stored_energy_j
            .checked_mul(u64::from(batches))
            .context("energy overflow")?,
        required_radius_m: 0.,
    })
}

pub fn quote(
    world: &mut World,
    account: AccountId,
    facility: Id,
    payer: Principal,
    work: ServiceWork,
    uploads: &crate::blueprint_uploads::BlueprintUploads,
) -> Result<ServiceQuote> {
    initialize(world)?;
    let entity = identity::lookup(world, facility)?;
    ensure!(available(world, entity), "facility unavailable");
    ensure!(
        world
            .resource::<ownership::Directory>()
            .0
            .administers(account, payer),
        "payer authority required"
    );
    let job = prepare(world, account, entity, payer, &work, uploads)?;
    let policy = &world
        .get::<IndustryFacility>(entity)
        .context("service unavailable")?
        .service;
    ensure!(policy.accepting, "outside jobs are closed");
    let rate = policy
        .rates
        .iter()
        .find(|rate| rate.capability == job.view.capability && rate.public_lanes > 0)
        .context("module closed to outside jobs")?;
    ensure!(
        lanes(world, entity).iter().any(|lane| suitable(lane, &job)),
        "no operational lane can perform this job"
    );
    let directory = &world.resource::<ownership::Directory>().0;
    let price_basis_points = policy
        .tiers
        .iter()
        .find(|tier| tier_matches(directory, payer, &tier.customer))
        .map_or(Some(10_000), |tier| tier.price_basis_points)
        .context("customer tier refuses service")?;
    let energy_charge = u64::try_from(
        (u128::from(job.energy_j) * u128::from(rate.energy_per_mj)).div_ceil(1_000_000),
    )
    .context("price overflow")?;
    let time_charge = u64::try_from(
        (u128::from(job.view.duration_ticks) * u128::from(rate.time_per_hour)).div_ceil(36_000),
    )
    .context("price overflow")?;
    let total = u64::try_from(
        ((u128::from(energy_charge) + u128::from(time_charge)) * u128::from(price_basis_points))
            .div_ceil(10_000),
    )
    .context("price overflow")?;
    let operator = world
        .get::<ownership::AssetOwner>(entity)
        .context("owner missing")?
        .0;
    Ok(ServiceQuote {
        facility,
        payer,
        operator,
        work,
        policy_revision: policy.revision,
        currency: policy.currency,
        energy_charge,
        time_charge,
        price_basis_points,
        total,
        inputs: job.inputs,
        outputs: match job.output {
            JobOutput::Cargo(outputs) => outputs,
            JobOutput::Ship(_) => Vec::new(),
        },
        duration_ticks: job.view.duration_ticks,
    })
}

pub fn order(
    world: &mut World,
    account: AccountId,
    expected: ServiceQuote,
    uploads: &crate::blueprint_uploads::BlueprintUploads,
) -> Result<()> {
    let current = quote(
        world,
        account,
        expected.facility,
        expected.payer,
        expected.work.clone(),
        uploads,
    )?;
    ensure!(current == expected, "quote changed; request a new quote");
    let entity = identity::lookup(world, current.facility)?;
    let mut job = prepare(
        world,
        account,
        entity,
        current.payer,
        &current.work,
        uploads,
    )?;
    let id = job.view.id;
    let mut queue = world
        .get::<IndustryFacility>(entity)
        .cloned()
        .unwrap_or_default();
    ensure!(queue.jobs.len() < MAX_JOBS, "facility queue full");
    world
        .resource_mut::<Economy>()
        .settle(osg_model::calendar::now_unix_ms());
    let economy = world.resource::<Economy>();
    ensure!(
        economy.available(current.payer, current.currency) >= current.total,
        "insufficient available funds"
    );
    let payment = ServicePayment {
        payer: current.payer,
        operator: current.operator,
        currency: current.currency,
        amount: current.total,
        charged: false,
    };
    job.view.payment = Some(payment.clone());
    let storage_key = (current.facility, current.payer);
    let mut stock = economy
        .storage
        .get(&storage_key)
        .cloned()
        .unwrap_or_default();
    let mut inventory = world
        .get::<hardware::ShipInventory>(entity)
        .context("inventory unavailable")?
        .0
        .clone();
    for stack in &job.inputs {
        let available_stock = economy
            .storage
            .get(&(current.facility, current.payer))
            .and_then(|stock| stock.get(&stack.item))
            .copied()
            .unwrap_or(0);
        ensure!(
            available_stock.saturating_sub(economy.stock_reserved(
                current.payer,
                current.facility,
                &stack.item
            )) >= stack.quantity,
            "deliver required inputs to your station storage first"
        );
        let amount = stock.get_mut(&stack.item).context("input stock missing")?;
        *amount = amount
            .checked_sub(stack.quantity)
            .context("input stock shortage")?;
        let custody = inventory
            .custody
            .get_mut(&stack.item)
            .context("input custody missing")?;
        *custody = custody
            .checked_sub(stack.quantity)
            .context("input custody shortage")?;
    }
    stock.retain(|_, quantity| *quantity > 0);
    inventory.custody.retain(|_, quantity| *quantity > 0);
    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    inventory.reserve_cargo(&job.inputs, catalogue)?;
    queue.jobs.push(job);
    validate_saved(
        Some(&queue),
        world.get::<MineSource>(entity),
        &world
            .get::<vessel::ShipDesign>(entity)
            .context("design unavailable")?
            .0,
        &inventory,
        catalogue,
        &world.resource::<ownership::Directory>().0,
    )?;
    world.get_mut::<hardware::ShipInventory>(entity).unwrap().0 = inventory;
    world.entity_mut(entity).insert(queue);
    let mut economy = world.resource_mut::<Economy>();
    economy.service_holds.insert(id, payment);
    if stock.is_empty() {
        economy.storage.remove(&storage_key);
    } else {
        economy.storage.insert(storage_key, stock);
    }
    Ok(())
}

pub(super) fn credit_storage(
    economy: &mut Economy,
    inventory: &mut Inventory,
    station: Id,
    owner: Principal,
    stacks: &[ItemStack],
) -> Result<()> {
    let mut stock = economy
        .storage
        .get(&(station, owner))
        .cloned()
        .unwrap_or_default();
    let mut custody = inventory.custody.clone();
    for stack in stacks {
        let amount = stock.entry(stack.item.clone()).or_default();
        *amount = amount
            .checked_add(stack.quantity)
            .context("storage overflow")?;
        let amount = custody.entry(stack.item.clone()).or_default();
        *amount = amount
            .checked_add(stack.quantity)
            .context("custody overflow")?;
    }
    ensure!(stock.len() <= 1024, "storage item limit");
    economy.storage.insert((station, owner), stock);
    inventory.custody = custody;
    Ok(())
}

#[cfg(test)]
#[test]
fn storage_credit_rolls_back_all_items_when_later_custody_overflows() {
    let mut economy = Economy::default();
    let mut inventory = Inventory::empty(&Catalogue::builtin());
    let station = Id::new();
    let owner = Principal::Player(Id::new());
    let first = CargoItem::Resource("first".into());
    let second = CargoItem::Resource("second".into());
    inventory.custody.insert(second.clone(), u64::MAX);
    let before = inventory.custody.clone();
    let result = credit_storage(
        &mut economy,
        &mut inventory,
        station,
        owner,
        &[
            ItemStack {
                item: first,
                quantity: 1,
            },
            ItemStack {
                item: second,
                quantity: 1,
            },
        ],
    );
    assert!(result.is_err());
    assert!(economy.storage.is_empty());
    assert_eq!(inventory.custody, before);
}

pub fn cancel(world: &mut World, account: AccountId, facility: Id, job: Id) -> Result<()> {
    let entity = identity::lookup(world, facility)?;
    let mut queue = world
        .get::<IndustryFacility>(entity)
        .context("facility unavailable")?
        .clone();
    let index = queue
        .jobs
        .iter()
        .position(|entry| entry.view.id == job)
        .context("job unavailable")?;
    let job = &queue.jobs[index];
    let payment = job.view.payment.as_ref().context("not a public job")?;
    ensure!(
        operator(world, account, entity)
            || world
                .resource::<ownership::Directory>()
                .0
                .administers(account, payment.payer),
        "job authority required"
    );
    ensure!(
        !payment.charged && job.view.progress_ticks == 0,
        "started jobs cannot be refunded"
    );
    let mut inventory = world
        .get::<hardware::ShipInventory>(entity)
        .context("inventory unavailable")?
        .0
        .clone();
    inventory.release_cargo(&job.inputs, &world.resource::<vessel::ShipCatalogue>().0)?;
    credit_storage(
        &mut world.resource_mut::<Economy>(),
        &mut inventory,
        facility,
        payment.payer,
        &job.inputs,
    )?;
    world
        .resource_mut::<Economy>()
        .service_holds
        .remove(&job.view.id);
    queue.jobs.remove(index);
    world.get_mut::<hardware::ShipInventory>(entity).unwrap().0 = inventory;
    world.entity_mut(entity).insert(queue);
    Ok(())
}

pub fn jobs(world: &World, account: AccountId, facility: Id) -> Result<Vec<JobView>> {
    let entity = identity::lookup(world, facility)?;
    let directory = &world.resource::<ownership::Directory>().0;
    Ok(world
        .get::<IndustryFacility>(entity)
        .into_iter()
        .flat_map(|queue| &queue.jobs)
        .filter(|job| {
            operator(world, account, entity)
                || job
                    .view
                    .payment
                    .as_ref()
                    .is_some_and(|payment| directory.administers(account, payment.payer))
        })
        .map(|job| job.view.clone())
        .collect())
}

pub(super) fn charge(
    world: &mut World,
    job: &mut IndustryJob,
    revenue: &mut crate::sim::economy::Balance,
) -> Result<()> {
    let Some(payment) = &mut job.view.payment else {
        return Ok(());
    };
    if payment.charged {
        return Ok(());
    }
    world.resource_scope(|world, mut economy: Mut<Economy>| {
        economy.transaction(|economy| {
            ensure!(
                economy.release_service_hold(job.view.id).as_ref() == Some(payment),
                "payment reservation no longer funded"
            );
            if payment.amount > 0 && payment.payer != payment.operator {
                let before = economy
                    .balances
                    .get(&payment.operator)
                    .cloned()
                    .unwrap_or_default();
                let directory = &world.resource::<ownership::Directory>().0;
                let restricted = payment.currency == Currency::Lat
                    && economy.restricted(directory, payment.operator);
                let start = economy.entries.len();
                economy.transfer(
                    Some(directory),
                    payment.payer,
                    payment.operator,
                    payment.currency,
                    payment.amount,
                    restricted,
                    osg_model::calendar::now_unix_ms(),
                )?;
                economy.reference_payment(start, job.view.id);
                let after = economy
                    .balances
                    .get(&payment.operator)
                    .cloned()
                    .unwrap_or_default();
                let received_uec = after
                    .uec
                    .checked_sub(before.uec)
                    .context("service receipt underflow")?;
                let received_lat = after
                    .lat
                    .checked_sub(before.lat)
                    .context("service receipt underflow")?;
                let uec = revenue
                    .uec
                    .checked_add(received_uec)
                    .context("service revenue overflow")?;
                let lat = revenue
                    .lat
                    .checked_add(received_lat)
                    .context("service revenue overflow")?;
                *revenue = crate::sim::economy::Balance { uec, lat };
            }
            payment.charged = true;
            Ok(())
        })
    })
}
