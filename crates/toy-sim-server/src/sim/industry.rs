use super::{hardware, identity, ownership, travel, vessel};
use anyhow::{Context, Result, ensure};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use toy_sim_model::{
    AccountId, Id,
    industry::*,
    ownership::{OwnershipDirectory, Permission, Principal},
};
use toy_sim_ships::{Catalogue, CompiledShipDesign, Equipment, Inventory, utilities::UtilityDef};

mod construction;
mod publication;
mod validation;

pub use publication::snapshot;
pub use validation::validate_saved;

#[cfg(test)]
mod acceptance_tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod product_tests;

const MAX_JOBS: usize = 128;
const MAX_QUEUED_BLUEPRINT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Component, Clone, Default, Debug, Serialize, Deserialize)]
pub struct IndustryFacility {
    pub jobs: Vec<IndustryJob>,
    pub seeded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndustryJob {
    pub view: JobView,
    pub inputs: Vec<ItemStack>,
    pub output: JobOutput,
    pub energy_j: u64,
    pub stored_energy_j: u64,
    pub required_radius_m: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobOutput {
    Cargo(Vec<ItemStack>),
    Ship(Vec<u8>),
}

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct MineSource {
    pub output: CargoItem,
    pub units_per_second: u64,
    pub remainder: u64,
    pub last_recipient: Option<Id>,
}

#[derive(Resource)]
struct CatalogueCache(IndustryCatalogue);

#[derive(Resource)]
struct PhysicalCatalogue(std::sync::Arc<Catalogue>);

#[derive(Clone)]
struct Lane {
    part: u64,
    device: Entity,
    capability: IndustryCapability,
    power_w: u64,
    radius_m: f64,
}

pub fn initialize(world: &mut World) -> Result<()> {
    if world.contains_resource::<CatalogueCache>() {
        return Ok(());
    }
    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    let recipes = toy_sim_ships::industry::recipes(catalogue)?;
    let blueprint = toy_sim_ships::industry::starter_ship();
    let design = blueprint.compile(catalogue)?;
    let requirements = toy_sim_ships::industry::construction_requirements(&design, catalogue)?;
    let blueprints = vec![BlueprintView {
        name: blueprint.name.clone(),
        blueprint: blueprint.to_bytes()?,
        inputs: requirements.inputs,
        duration_ticks: requirements.duration_ticks,
        energy_j: requirements.energy_j,
    }];
    let revision = *blake3::hash(&postcard::to_stdvec(&(&recipes, &blueprints))?).as_bytes();
    let physical = std::sync::Arc::new(catalogue.clone());
    world.insert_resource(CatalogueCache(IndustryCatalogue {
        revision,
        recipes,
        blueprints,
    }));
    world.insert_resource(PhysicalCatalogue(physical));
    publication::refresh(world);
    Ok(())
}

pub fn refresh_publication(world: &mut World) {
    publication::refresh(world);
}

pub fn seed_demo(world: &mut World, station: Entity, account: AccountId) -> Result<()> {
    initialize(world)?;
    if world
        .get::<IndustryFacility>(station)
        .is_some_and(|facility| facility.seeded)
    {
        return Ok(());
    }

    world
        .run_system_once(hardware::initialize)
        .map_err(|error| anyhow::anyhow!("hardware initialization failed: {error:?}"))?;

    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let capacity = world
        .get::<vessel::ShipDesign>(station)
        .context("starter station design missing")?
        .0
        .capacity_m3;
    let mut inventory = world
        .get::<hardware::ShipInventory>(station)
        .context("starter station inventory missing")?
        .0
        .clone();
    for stack in toy_sim_ships::industry::starter_stock(&catalogue)? {
        inventory.insert_item(&stack.item, stack.quantity, capacity, &catalogue)?;
    }

    let mut policy = world
        .get::<ownership::AssetAccess>(station)
        .map(|access| access.0.clone())
        .unwrap_or_default();
    policy.grants.push(toy_sim_model::ownership::AccessGrant {
        principal: Principal::Player(account),
        permissions: [
            Permission::View,
            Permission::Industry,
            Permission::TransferCargo,
        ]
        .into(),
    });
    world.get_mut::<hardware::ShipInventory>(station).unwrap().0 = inventory;
    world.entity_mut(station).insert((
        ownership::AssetAccess(policy),
        MineSource {
            output: CargoItem::Resource("industrial_ore".into()),
            units_per_second: 10,
            remainder: 0,
            last_recipient: None,
        },
    ));
    if let Some(mut facility) = world.get_mut::<IndustryFacility>(station) {
        facility.seeded = true;
    } else {
        world.entity_mut(station).insert(IndustryFacility {
            seeded: true,
            ..Default::default()
        });
    }
    synchronize_mass(world, &[station]);
    publication::refresh(world);
    Ok(())
}

pub fn inventory_location(world: &World, entity: Entity) -> Option<Id> {
    match &world.get::<travel::PresenceState>(entity)?.0 {
        toy_sim_model::travel::Presence::Docked { host, .. } => Some(*host),
        toy_sim_model::travel::Presence::Space => {
            world.get::<identity::Identity>(entity).map(|id| id.0)
        }
        _ => None,
    }
}

fn available(world: &World, entity: Entity) -> bool {
    world
        .get::<hardware::Hull>(entity)
        .is_some_and(|hull| hull.0 > 0.)
        && inventory_location(world, entity).is_some()
}

fn colocated(world: &World, source: Entity, target: Entity) -> Result<()> {
    ensure!(
        available(world, source) && available(world, target),
        "inventory unavailable"
    );
    let source_location =
        inventory_location(world, source).context("source inventory unavailable")?;
    let target_location =
        inventory_location(world, target).context("target inventory unavailable")?;
    let source_id = world
        .get::<identity::Identity>(source)
        .context("source identity missing")?
        .0;
    let target_id = world
        .get::<identity::Identity>(target)
        .context("target identity missing")?
        .0;
    ensure!(
        source_location == target_location
            || source_location == target_id
            || target_location == source_id,
        "cargo transfers require a shared dock"
    );
    Ok(())
}

pub fn execute(
    world: &mut World,
    account: AccountId,
    command: IndustryCommand,
    uploads: Option<&crate::blueprint_uploads::BlueprintUploads>,
) -> Result<()> {
    initialize(world)?;
    match command {
        IndustryCommand::UnloadProduct {
            source,
            target,
            resource,
            quantity,
        } => unload_product(
            world,
            account,
            identity::lookup(world, source)?,
            identity::lookup(world, target)?,
            &resource,
            quantity,
        ),
        IndustryCommand::Transfer {
            source,
            target,
            item,
            quantity,
        } => {
            ensure!(quantity > 0 && source != target, "invalid cargo transfer");
            let source = identity::lookup(world, source)?;
            let target = identity::lookup(world, target)?;
            transfer(world, account, source, target, item, quantity)
        }
        IndustryCommand::Refill {
            source,
            ship,
            resource,
            quantity,
        } => refill(
            world,
            account,
            identity::lookup(world, source)?,
            identity::lookup(world, ship)?,
            &resource,
            quantity,
        ),
        IndustryCommand::CancelJob { facility, job } => {
            let facility = identity::lookup(world, facility)?;
            ownership::authorize(world, account, facility, Permission::Industry)?;
            let queue = world
                .get::<IndustryFacility>(facility)
                .context("facility has no jobs")?;
            let index = queue
                .jobs
                .iter()
                .position(|value| value.view.id == job)
                .context("job unavailable")?;
            let inputs = queue.jobs[index].inputs.clone();
            let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
            let mut inventory = world
                .get::<hardware::ShipInventory>(facility)
                .context("inventory missing")?
                .0
                .clone();
            inventory.release_cargo(&inputs, &catalogue)?;
            world
                .get_mut::<hardware::ShipInventory>(facility)
                .unwrap()
                .0 = inventory;
            world
                .get_mut::<IndustryFacility>(facility)
                .unwrap()
                .jobs
                .remove(index);
            Ok(())
        }
        IndustryCommand::StartRecipe {
            facility,
            recipe,
            batches,
        } => {
            ensure!(
                (1..=MAX_RECIPE_BATCHES).contains(&batches),
                "invalid batch count"
            );
            let facility = identity::lookup(world, facility)?;
            let recipe = world
                .resource::<CatalogueCache>()
                .0
                .recipes
                .iter()
                .find(|value| value.id == recipe)
                .context("unknown recipe")?
                .clone();
            let scale = |stacks: Vec<ItemStack>| -> Result<Vec<ItemStack>> {
                stacks
                    .into_iter()
                    .map(|mut stack| {
                        stack.quantity = stack
                            .quantity
                            .checked_mul(u64::from(batches))
                            .context("quantity overflow")?;
                        Ok(stack)
                    })
                    .collect()
            };
            let owner = world
                .get::<ownership::AssetOwner>(facility)
                .context("owner missing")?
                .0;
            let job = IndustryJob {
                view: JobView {
                    id: Id::new(),
                    name: recipe.name,
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
                },
                inputs: scale(recipe.inputs)?,
                output: JobOutput::Cargo(scale(recipe.outputs)?),
                energy_j: recipe
                    .energy_j
                    .checked_mul(u64::from(batches))
                    .context("energy overflow")?,
                stored_energy_j: recipe
                    .stored_energy_j
                    .checked_mul(u64::from(batches))
                    .context("energy overflow")?,
                required_radius_m: 0.,
            };
            admit(world, account, facility, job)
        }
        IndustryCommand::BuildShip {
            facility,
            owner,
            blueprint_hash,
        } => {
            let blueprint = uploads
                .context("ship construction requires an uploaded blueprint")?
                .get(blueprint_hash)?;
            build_ship(world, account, facility, owner, blueprint.as_ref().as_ref())
        }
    }
}

pub fn build_ship(
    world: &mut World,
    account: AccountId,
    facility: Id,
    owner: Principal,
    blueprint: &[u8],
) -> Result<()> {
    initialize(world)?;
    let facility = identity::lookup(world, facility)?;
    let job = construction::job(world, account, facility, owner, blueprint)?;
    admit(world, account, facility, job)
}

fn validate_blueprint_budget(jobs: &[IndustryJob], additional: usize) -> Result<()> {
    let bytes = jobs.iter().try_fold(additional, |total, job| {
        let size = match &job.output {
            JobOutput::Ship(bytes) => bytes.len(),
            JobOutput::Cargo(_) => 0,
        };
        total
            .checked_add(size)
            .context("queued blueprint byte overflow")
    })?;
    ensure!(
        bytes <= MAX_QUEUED_BLUEPRINT_BYTES,
        "queued ship blueprints exceed the 64 MiB facility limit"
    );
    Ok(())
}

pub fn transfer(
    world: &mut World,
    account: AccountId,
    source: Entity,
    target: Entity,
    item: CargoItem,
    quantity: u64,
) -> Result<()> {
    ensure!(source != target && quantity > 0, "invalid cargo transfer");
    ownership::authorize(world, account, source, Permission::TransferCargo)?;
    ownership::authorize(world, account, target, Permission::TransferCargo)?;
    colocated(world, source, target)?;
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let capacity = world
        .get::<vessel::ShipDesign>(target)
        .context("target design missing")?
        .0
        .capacity_m3;
    let mut from = world
        .get::<hardware::ShipInventory>(source)
        .context("source inventory missing")?
        .0
        .clone();
    let mut to = world
        .get::<hardware::ShipInventory>(target)
        .context("target inventory missing")?
        .0
        .clone();
    from.transfer_item(&mut to, &item, quantity, capacity, &catalogue)?;
    world.get_mut::<hardware::ShipInventory>(source).unwrap().0 = from;
    world.get_mut::<hardware::ShipInventory>(target).unwrap().0 = to;
    synchronize_mass(world, &[source, target]);
    Ok(())
}

fn unload_product(
    world: &mut World,
    account: AccountId,
    source: Entity,
    target: Entity,
    resource: &str,
    quantity: u64,
) -> Result<()> {
    ensure!(quantity > 0, "invalid product quantity");
    ownership::authorize(world, account, source, Permission::TransferCargo)?;
    ownership::authorize(world, account, target, Permission::TransferCargo)?;
    colocated(world, source, target)?;

    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    let index = catalogue
        .resources
        .iter()
        .position(|value| value.id == resource)
        .context("unknown resource")?;
    let capacity = world
        .get::<vessel::ShipDesign>(target)
        .context("target design missing")?
        .0
        .capacity_m3;
    let mut from = world
        .get::<hardware::ShipInventory>(source)
        .context("source inventory missing")?
        .0
        .clone();

    if source == target {
        from.package_product(index, quantity, capacity, catalogue)?;
        world.get_mut::<hardware::ShipInventory>(source).unwrap().0 = from;
    } else {
        let mut to = world
            .get::<hardware::ShipInventory>(target)
            .context("target inventory missing")?
            .0
            .clone();
        from.unload_product(&mut to, index, quantity, capacity, catalogue)?;

        world.get_mut::<hardware::ShipInventory>(source).unwrap().0 = from;
        world.get_mut::<hardware::ShipInventory>(target).unwrap().0 = to;
    }

    synchronize_mass(world, &[source, target]);
    Ok(())
}

fn refill(
    world: &mut World,
    account: AccountId,
    source: Entity,
    target: Entity,
    resource: &str,
    quantity: u64,
) -> Result<()> {
    ensure!(quantity > 0, "invalid refill quantity");
    ownership::authorize(world, account, source, Permission::TransferCargo)?;
    ownership::authorize(world, account, target, Permission::TransferCargo)?;
    colocated(world, source, target)?;
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let index = catalogue
        .resources
        .iter()
        .position(|value| value.id == resource)
        .context("unknown resource")?;
    let mut from = world
        .get::<hardware::ShipInventory>(source)
        .context("source inventory missing")?
        .0
        .clone();
    if resource == "shield_coolant" {
        let design = &world
            .get::<vessel::ShipDesign>(target)
            .context("target design missing")?
            .0;
        let capacity_mg = toy_sim_ships::industry::mass_mg(design.shield_reserve_capacity_kg)?;
        let unit_mg = toy_sim_ships::industry::mass_mg(catalogue.resources[index].mass_kg)?;
        let delivered_mg = unit_mg
            .checked_mul(quantity)
            .context("coolant quantity overflow")?;
        let mut thermal = world
            .get::<hardware::ShipThermal>(target)
            .context("target thermal storage missing")?
            .0;
        ensure!(
            delivered_mg <= capacity_mg.saturating_sub(thermal.shield_reserve_mg),
            "shield coolant reservoir full"
        );

        from.withdraw_cargo(&CargoItem::Resource(resource.into()), quantity, &catalogue)?;
        thermal.shield_reserve_mg = thermal
            .shield_reserve_mg
            .checked_add(delivered_mg)
            .context("coolant quantity overflow")?;
        world.get_mut::<hardware::ShipInventory>(source).unwrap().0 = from;
        world.get_mut::<hardware::ShipThermal>(target).unwrap().0 = thermal;
        synchronize_mass(world, &[source, target]);
        return Ok(());
    }
    if source == target {
        from.refill_cargo(index, quantity, &catalogue)?;
        world.get_mut::<hardware::ShipInventory>(source).unwrap().0 = from;
    } else {
        let mut to = world
            .get::<hardware::ShipInventory>(target)
            .context("target inventory missing")?
            .0
            .clone();
        from.withdraw_cargo(&CargoItem::Resource(resource.into()), quantity, &catalogue)?;
        to.insert_consumable(index, quantity, &catalogue)?;
        world.get_mut::<hardware::ShipInventory>(source).unwrap().0 = from;
        world.get_mut::<hardware::ShipInventory>(target).unwrap().0 = to;
    }
    synchronize_mass(world, &[source, target]);
    Ok(())
}

fn admit(world: &mut World, account: AccountId, facility: Entity, job: IndustryJob) -> Result<()> {
    ownership::authorize(world, account, facility, Permission::Industry)?;
    ensure!(available(world, facility), "facility unavailable");
    ensure!(
        world
            .get::<IndustryFacility>(facility)
            .is_none_or(|queue| queue.jobs.len() < MAX_JOBS),
        "facility queue full"
    );
    ensure!(
        lanes(world, facility)
            .iter()
            .any(|lane| suitable(lane, &job)),
        "no operational module can perform this job"
    );
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let mut inventory = world
        .get::<hardware::ShipInventory>(facility)
        .context("inventory missing")?
        .0
        .clone();
    inventory.reserve_cargo(&job.inputs, &catalogue)?;
    let mut queue = world
        .get::<IndustryFacility>(facility)
        .cloned()
        .unwrap_or_default();
    queue.jobs.push(job);
    validate_saved(
        Some(&queue),
        world.get::<MineSource>(facility),
        &world.get::<vessel::ShipDesign>(facility).unwrap().0,
        &inventory,
        &catalogue,
        &world.resource::<ownership::Directory>().0,
    )?;

    world
        .get_mut::<hardware::ShipInventory>(facility)
        .unwrap()
        .0 = inventory;
    world.entity_mut(facility).insert(queue);
    Ok(())
}

fn suitable(lane: &Lane, job: &IndustryJob) -> bool {
    lane.capability == job.view.capability
        && lane.radius_m >= job.required_radius_m
        && u128::from(lane.power_w)
            >= u128::from(job.energy_j.div_ceil(job.view.duration_ticks)) * 10
}

fn lanes(world: &World, facility: Entity) -> Vec<Lane> {
    if !available(world, facility) || world.get::<travel::Dormant>(facility).is_some() {
        return Vec::new();
    }
    let (Some(design), Some(parts)) = (
        world.get::<vessel::ShipDesign>(facility),
        world.get::<hardware::PartDevices>(facility),
    ) else {
        return Vec::new();
    };
    let mut lanes = Vec::new();
    for (index, &device) in parts.0.iter().enumerate() {
        if !world
            .get::<hardware::Device>(device)
            .is_some_and(|value| value.0.operational)
        {
            continue;
        }
        let part = &design.0.parts[index];
        let (capability, power_w, count, radius_m) = match part.definition.equipment {
            Equipment::Utility {
                utility:
                    UtilityDef::Factory {
                        capability,
                        power_per_lane_w,
                        lanes,
                    },
            } => (capability, power_per_lane_w, lanes, f64::INFINITY),
            Equipment::Utility {
                utility:
                    UtilityDef::Shipyard {
                        power_per_lane_w,
                        lanes,
                        max_radius_m,
                    },
            } => (
                IndustryCapability::Shipyard,
                power_per_lane_w,
                lanes,
                max_radius_m,
            ),
            _ => continue,
        };
        lanes.extend((0..count).map(|_| Lane {
            part: part.placed.id,
            device,
            capability,
            power_w,
            radius_m,
        }));
    }
    lanes
}

fn cumulative_energy(energy: u64, ticks: u64, duration: u64) -> u64 {
    (u128::from(energy) * u128::from(ticks) / u128::from(duration)) as u64
}

pub fn advance(world: &mut World) {
    let Some(catalogue) = world
        .get_resource::<PhysicalCatalogue>()
        .map(|catalogue| catalogue.0.clone())
    else {
        return;
    };
    let facilities: Vec<_> = world
        .query_filtered::<Entity, With<IndustryFacility>>()
        .iter(world)
        .collect();
    let mut mass_changed = BTreeSet::new();
    for entity in facilities {
        let mut queue = world.get::<IndustryFacility>(entity).unwrap().clone();
        let mut available_lanes = lanes(world, entity);
        for lane in &available_lanes {
            if let Some(mut device) = world.get_mut::<hardware::Device>(lane.device) {
                device.0.powered = false;
                device.0.actual = 0.;
            }
            if let Some(mut power) = world.get_mut::<hardware::DevicePower>(lane.device) {
                *power = hardware::DevicePower::default();
            }
        }
        let mut completed = Vec::new();
        for (index, job) in queue.jobs.iter_mut().enumerate() {
            let was_awaiting_berth = job.view.status == JobStatus::AwaitingBerth;
            job.view.module_part = None;
            job.view.requested_power_w = 0;
            job.view.supplied_power_w = 0;
            if !available(world, entity) || world.get::<travel::Dormant>(entity).is_some() {
                job.view.status = JobStatus::ModuleUnavailable;
                continue;
            }
            let Some(lane_index) = available_lanes.iter().position(|lane| suitable(lane, job))
            else {
                job.view.status = if lanes(world, entity).iter().any(|lane| suitable(lane, job)) {
                    JobStatus::Queued
                } else {
                    JobStatus::ModuleUnavailable
                };
                continue;
            };
            let lane = available_lanes.remove(lane_index);
            job.view.module_part = Some(lane.part);
            if job.view.progress_ticks < job.view.duration_ticks {
                let paid = cumulative_energy(
                    job.energy_j,
                    job.view.progress_ticks,
                    job.view.duration_ticks,
                );
                let next = cumulative_energy(
                    job.energy_j,
                    job.view.progress_ticks + 1,
                    job.view.duration_ticks,
                );
                let energy = next - paid;
                job.view.requested_power_w = energy.saturating_mul(10);
                if let Some(mut power) = world.get_mut::<hardware::DevicePower>(lane.device) {
                    power.requested_w += job.view.requested_power_w as f64;
                }
                let Some(mut inventory) = world.get_mut::<hardware::ShipInventory>(entity) else {
                    job.view.status = JobStatus::ModuleUnavailable;
                    continue;
                };
                if inventory.0.energy_j < energy {
                    job.view.status = JobStatus::AwaitingPower;
                    continue;
                }
                inventory.0.energy_j -= energy;
                job.view.progress_ticks += 1;
                job.view.supplied_power_w = job.view.requested_power_w;
                job.view.status = JobStatus::Running;
                let heat_total = job.energy_j - job.stored_energy_j;
                let heat = if job.energy_j == 0 {
                    0
                } else {
                    ((u128::from(next) * u128::from(heat_total) / u128::from(job.energy_j))
                        - (u128::from(paid) * u128::from(heat_total) / u128::from(job.energy_j)))
                        as u64
                };
                hardware::add_travel_heat(world, entity, heat as f64, 0.1);
                if let Some(mut device) = world.get_mut::<hardware::Device>(lane.device) {
                    device.0.powered = true;
                    device.0.actual += 1.;
                }
                if let Some(mut power) = world.get_mut::<hardware::DevicePower>(lane.device) {
                    power.supplied_w += job.view.supplied_power_w as f64;
                }
            }
            if job.view.progress_ticks != job.view.duration_ticks {
                continue;
            }
            let capacity = world
                .get::<vessel::ShipDesign>(entity)
                .unwrap()
                .0
                .capacity_m3;
            let mut inventory = world
                .get::<hardware::ShipInventory>(entity)
                .unwrap()
                .0
                .clone();
            match &job.output {
                JobOutput::Cargo(outputs) => {
                    if inventory
                        .complete_cargo(&job.inputs, outputs, capacity, &catalogue)
                        .is_err()
                    {
                        job.view.status = JobStatus::AwaitingCargoSpace;
                        continue;
                    }
                }
                JobOutput::Ship(_) => {
                    let completion = inventory
                        .complete_cargo(&job.inputs, &[], capacity, &catalogue)
                        .context("construction input settlement failed")
                        .and_then(|()| construction::finish(world, entity, job).map(|_| ()));
                    if let Err(error) = completion {
                        if !was_awaiting_berth {
                            warn!(
                                facility = ?entity,
                                job = ?job.view.id,
                                error = %format!("{error:#}"),
                                "Ship construction completion blocked"
                            );
                        }
                        job.view.status = JobStatus::AwaitingBerth;
                        continue;
                    }
                }
            }
            world.get_mut::<hardware::ShipInventory>(entity).unwrap().0 = inventory;
            completed.push(index);
            mass_changed.insert(entity);
        }
        for index in completed.into_iter().rev() {
            queue.jobs.remove(index);
        }
        world.entity_mut(entity).insert(queue);
    }
    advance_mines(world, &catalogue, &mut mass_changed);
    synchronize_mass(world, &mass_changed.into_iter().collect::<Vec<_>>());
    publication::refresh(world);
}

fn advance_mines(world: &mut World, catalogue: &Catalogue, changed: &mut BTreeSet<Entity>) {
    let mines: Vec<_> = world
        .query::<(Entity, &MineSource)>()
        .iter(world)
        .map(|(entity, mine)| (entity, mine.clone()))
        .collect();
    for (host, mut mine) in mines {
        if !available(world, host) || world.get::<travel::Dormant>(host).is_some() {
            continue;
        }
        let total = u128::from(mine.units_per_second) + u128::from(mine.remainder);
        let production = (total / 10) as u64;
        mine.remainder = (total % 10) as u64;
        let unit_volume = toy_sim_ships::industry::item_volume_m3(&mine.output, catalogue)
            .unwrap_or(f64::INFINITY);
        let mut recipients: Vec<_> = world
            .get::<travel::StoredShips>(host)
            .into_iter()
            .flat_map(|ships| ships.iter())
            .filter_map(|entity| {
                let id = world.get::<identity::Identity>(entity)?.0;
                let owner = world.get::<ownership::AssetOwner>(entity)?.0;
                let request = world.get::<hardware::utilities::DockServiceRequest>(entity)?;
                let capacity = world.get::<vessel::ShipDesign>(entity)?.0.capacity_m3;
                let inventory = &world.get::<hardware::ShipInventory>(entity)?.0;
                let room = ((capacity - inventory.cargo_volume(catalogue)).max(0.) / unit_volume)
                    .floor() as u64;
                (room > 0
                    && request.cargo
                    && available(world, entity)
                    && ownership::principal_access(world, owner, host, Permission::TransferCargo))
                .then_some((id, entity, room, capacity))
            })
            .collect();
        recipients.sort_by_key(|&(id, ..)| id);
        if let Some(cursor) = mine.last_recipient {
            let start = recipients.partition_point(|&(id, ..)| id <= cursor);
            let length = recipients.len();
            if length > 0 {
                recipients.rotate_left(start % length);
            }
        }
        let rooms = recipients.iter().map(|&(_, _, room, _)| room).collect();
        let (equal_share, mut extra) = loading_shares(rooms, production);
        let mut next_cursor = None;
        for (id, entity, room, capacity) in recipients {
            let mut amount = room.min(equal_share);
            if room > equal_share && extra > 0 {
                amount += 1;
                extra -= 1;
                next_cursor = Some(id);
            }
            if amount == 0 {
                continue;
            }

            world
                .get_mut::<hardware::ShipInventory>(entity)
                .unwrap()
                .0
                .insert_item(&mine.output, amount, capacity, catalogue)
                .expect("mine allocation fits its validated inventory");
            changed.insert(entity);
        }
        if next_cursor.is_some() {
            mine.last_recipient = next_cursor;
        }
        world.entity_mut(host).insert(mine);
    }
}

fn loading_shares(mut rooms: Vec<u64>, production: u64) -> (u64, u64) {
    rooms.sort_unstable();
    let mut remaining = production;
    let mut recipients = rooms.len() as u64;
    let mut level = 0;
    for room in rooms {
        let cost = u128::from(room - level) * u128::from(recipients);
        if cost > u128::from(remaining) {
            return (level + remaining / recipients, remaining % recipients);
        }
        remaining -= cost as u64;
        level = room;
        recipients -= 1;
    }
    (level, 0)
}

pub fn synchronize_mass(world: &mut World, affected: &[Entity]) {
    let mut visited_ships = BTreeSet::new();
    for &entity in affected {
        if !visited_ships.insert(entity) {
            continue;
        }
        let updated = {
            let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
            let (Some(design), Some(inventory), Some(thermal)) = (
                world.get::<vessel::ShipDesign>(entity),
                world.get::<hardware::ShipInventory>(entity),
                world.get::<hardware::ShipThermal>(entity),
            ) else {
                continue;
            };
            design.0.dry_mass
                + inventory.0.mass(catalogue)
                + thermal.0.shield_deployed_kg
                + thermal.0.shield_reserve_kg()
                + world
                    .get::<travel::StoredMass>(entity)
                    .map_or(0., |stored| stored.0)
        };
        let previous = world
            .get::<super::physics::MassProps>(entity)
            .map_or(updated, |mass| mass.mass);
        update_mass(world, entity, updated);
        let delta = updated - previous;
        if delta == 0. {
            continue;
        }

        let mut ancestor = containing_ship(world, entity);
        let mut seen = BTreeSet::from([entity]);
        for _ in 0..8 {
            let Some(parent) = ancestor else {
                break;
            };
            assert!(
                seen.insert(parent),
                "validated containment hierarchy has no cycle"
            );
            let stored = world
                .get::<travel::StoredMass>(parent)
                .map_or(0., |stored| stored.0);
            world
                .entity_mut(parent)
                .insert(travel::StoredMass((stored + delta).max(0.)));
            if let Some(mass) = world.get::<super::physics::MassProps>(parent) {
                update_mass(world, parent, (mass.mass + delta).max(0.));
            }
            ancestor = containing_ship(world, parent);
        }
    }
}

fn containing_ship(world: &World, entity: Entity) -> Option<Entity> {
    let presence = &world.get::<travel::PresenceState>(entity)?.0;
    let host = match presence {
        toy_sim_model::travel::Presence::Docked { host, .. }
        | toy_sim_model::travel::Presence::StoredInWreck(host) => *host,
        _ => return None,
    };
    identity::lookup(world, host).ok()
}

fn update_mass(world: &mut World, entity: Entity, total: f64) {
    let Some(design) = world
        .get::<vessel::ShipDesign>(entity)
        .map(|design| design.0.clone())
    else {
        return;
    };
    if let Some(mut mass) = world.get_mut::<super::physics::MassProps>(entity) {
        mass.mass = total;
        mass.inertia = design.inertia * (total / design.dry_mass);
        mass.inertia_inv = mass.inertia.inverse();
    }
}
