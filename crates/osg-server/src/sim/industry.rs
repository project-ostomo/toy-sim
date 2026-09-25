use super::{hardware, identity, ownership, travel, vessel};
use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::{
    AccountId, Id,
    industry::*,
    ownership::{OwnershipDirectory, Permission, Principal},
};
use osg_ships::{Catalogue, CompiledShipDesign, Equipment, Inventory, utilities::UtilityDef};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

mod access;
mod admission;
mod cargo;
mod construction;
mod production;
mod queries;
mod requests;
mod service;
mod validation;

pub use requests::{
    CancelWork, CancellationQueue, CargoAction, CargoQueue, ConfigureService, Incoming,
    IndustryQueryQueue, IndustryQueryRequest, MAX_QUEUED_REQUESTS, Mutation, QueryItem,
    ReadRequest, ServicePolicyQueue, StartWork, WorkItem, WorkQueue,
};
pub use validation::validate_saved;

const MAX_JOBS: usize = 128;
const MAX_QUEUED_BLUEPRINT_BYTES: usize = 64 * 1024 * 1024;

/// Production state belonging to one vessel. Physical part condition and stock
/// remain in the vessel's shared hardware.
#[derive(Component, Clone, Debug, Default)]
pub struct IndustrialFacility {
    modules: Vec<IndustrialModule>,
    jobs: Vec<ProductionJob>,
    policy: ServicePolicy,
    outside_revenue: super::economy::Balance,
    mine: Option<MineSource>,
}

impl IndustrialFacility {
    pub fn jobs(&self) -> &[ProductionJob] {
        &self.jobs
    }

    pub fn from_design(design: &CompiledShipDesign) -> Self {
        let mut facility = Self::default();
        facility.rebuild_modules(design);
        facility
    }

    fn rebuild_modules(&mut self, design: &CompiledShipDesign) {
        self.modules = design
            .parts
            .iter()
            .enumerate()
            .filter_map(|(part_index, part)| {
                let (capability, power_per_lane_w, lanes, max_radius_m) =
                    match part.definition.equipment {
                        Equipment::Utility {
                            utility:
                                UtilityDef::Factory {
                                    capability,
                                    power_per_lane_w,
                                    lanes,
                                },
                        } => (capability, power_per_lane_w, lanes, None),
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
                            Some(max_radius_m),
                        ),
                        _ => return None,
                    };
                Some(IndustrialModule {
                    part_id: part.placed.id,
                    part_index,
                    capability,
                    power_per_lane_w,
                    max_radius_m,
                    lanes: vec![None; lanes as usize],
                })
            })
            .collect();
    }

    pub fn job_views(&self) -> Vec<JobView> {
        self.jobs.iter().map(ProductionJob::view).collect()
    }

    pub fn to_record(&self) -> IndustrialFacilityRecord {
        IndustrialFacilityRecord {
            jobs: self.jobs.clone(),
            policy: self.policy.clone(),
            outside_revenue: self.outside_revenue.clone(),
            mine: self.mine.clone(),
        }
    }

    pub fn from_record(
        record: IndustrialFacilityRecord,
        design: &CompiledShipDesign,
        catalogue: &Catalogue,
    ) -> Result<Self> {
        let mut facility = Self::from_design(design);
        facility.jobs = record.jobs;
        for job in &mut facility.jobs {
            if let WorkOutput::Ship(bytes) = &job.work.output {
                let blueprint = Arc::new(osg_ships::ShipBlueprint::from_bytes(bytes)?);
                let design = Arc::new(blueprint.compile(catalogue)?);
                job.work.construction = Some(ConstructionDesign { blueprint, design });
            }
        }
        facility.policy = record.policy;
        facility.outside_revenue = record.outside_revenue;
        facility.mine = record.mine;
        Ok(facility)
    }

    pub fn set_mine(&mut self, mine: MineSource) {
        self.mine = Some(mine);
    }

    fn supports(&self, work: &WorkPlan, operational: &[bool]) -> bool {
        self.modules.iter().enumerate().any(|(index, module)| {
            operational.get(index) == Some(&true)
                && !module.lanes.is_empty()
                && module.suitable(work)
        })
    }

    fn check_budget(&self, additional: usize) -> Result<()> {
        let bytes = self.jobs.iter().try_fold(additional, |total, job| {
            total
                .checked_add(match &job.work.output {
                    WorkOutput::Ship(bytes) => bytes.len(),
                    _ => 0,
                })
                .context("blueprint budget overflow")
        })?;
        ensure!(
            bytes <= MAX_QUEUED_BLUEPRINT_BYTES,
            "queued ship blueprints exceed the 64 MiB facility limit"
        );
        Ok(())
    }

    fn admit(
        &mut self,
        job: ProductionJob,
        inventory: &mut Inventory,
        catalogue: &Catalogue,
        operational: &[bool],
    ) -> Result<Id> {
        ensure!(self.jobs.len() < MAX_JOBS, "facility queue full");
        ensure!(
            self.supports(&job.work, operational),
            "no operational module can perform this job"
        );
        self.check_budget(match &job.work.output {
            WorkOutput::Ship(bytes) => bytes.len(),
            _ => 0,
        })?;
        inventory.reserve_cargo(&job.work.inputs, catalogue)?;
        let id = job.id;
        self.jobs.push(job);
        Ok(id)
    }

    fn cancel(
        &mut self,
        id: Id,
        inventory: &mut Inventory,
        catalogue: &Catalogue,
    ) -> Result<ProductionJob> {
        let index = self
            .jobs
            .iter()
            .position(|job| job.id == id)
            .context("job unavailable")?;
        let job = &self.jobs[index];
        ensure!(
            job.payment
                .as_ref()
                .is_none_or(|payment| !payment.charged && job.progress_ticks == 0),
            "started jobs cannot be refunded"
        );
        inventory.release_cargo(&job.work.inputs, catalogue)?;
        Ok(self.jobs.remove(index))
    }

    fn assign_lanes(&mut self, operational: &[bool]) {
        for module in &mut self.modules {
            module.lanes.fill(None);
        }
        let mut public = BTreeMap::<IndustryCapability, u32>::new();
        for job in &mut self.jobs {
            job.module_part = None;
            job.requested_power_w = 0;
            job.supplied_power_w = 0;
            if job.payment.is_some() {
                let limit = self
                    .policy
                    .rates
                    .iter()
                    .find(|rate| rate.capability == job.work.capability)
                    .map_or(1, |rate| rate.public_lanes.max(1));
                let used = public.entry(job.work.capability).or_default();
                if *used >= limit {
                    job.status = JobStatus::Queued;
                    continue;
                }
                *used += 1;
            }

            let mut suitable = false;
            for (index, module) in self.modules.iter_mut().enumerate() {
                if operational.get(index) != Some(&true) || !module.suitable(&job.work) {
                    continue;
                }
                suitable = true;
                if let Some(lane) = module.lanes.iter_mut().find(|lane| lane.is_none()) {
                    *lane = Some(job.id);
                    job.module_part = Some(module.part_id);
                    break;
                }
            }
            if job.module_part.is_none() {
                job.status = if suitable {
                    JobStatus::Queued
                } else {
                    JobStatus::ModuleUnavailable
                };
            }
        }
    }

    fn prepare_steps(&mut self) -> Vec<ProductionStep> {
        let mut steps = Vec::new();
        for job in &mut self.jobs {
            let Some(part) = job.module_part else {
                continue;
            };
            if job.progress_ticks == job.work.duration_ticks {
                continue;
            }
            let cumulative = |energy: u64, ticks: u64| {
                (u128::from(energy) * u128::from(ticks) / u128::from(job.work.duration_ticks))
                    as u64
            };
            let paid = cumulative(job.work.energy_j, job.progress_ticks);
            let next = cumulative(job.work.energy_j, job.progress_ticks + 1);
            let heat_total = job.work.energy_j - job.work.stored_energy_j;
            let heat = |energy: u64| {
                (u128::from(energy) * u128::from(heat_total) / u128::from(job.work.energy_j)) as u64
            };
            job.requested_power_w = (next - paid) * 10;
            steps.push(ProductionStep {
                job: job.id,
                part_index: self
                    .modules
                    .iter()
                    .find(|module| module.part_id == part)
                    .unwrap()
                    .part_index,
                energy_j: next - paid,
                heat_j: heat(next) - heat(paid),
            });
        }
        steps
    }

    fn job(&self, id: Id) -> Option<&ProductionJob> {
        self.jobs.iter().find(|job| job.id == id)
    }

    fn block_job(&mut self, id: Id, reason: JobStatus) {
        self.jobs
            .iter_mut()
            .find(|job| job.id == id)
            .unwrap()
            .status = reason;
    }

    fn record_charge(&mut self, id: Id, revenue: super::economy::Balance) {
        if let Some(payment) = &mut self
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .unwrap()
            .payment
        {
            payment.charged = true;
        }
        self.outside_revenue = revenue;
    }

    fn commit_step(&mut self, step: ProductionStep) {
        let job = self.jobs.iter_mut().find(|job| job.id == step.job).unwrap();
        job.progress_ticks += 1;
        job.supplied_power_w = job.requested_power_w;
        job.status = JobStatus::Running;
    }

    fn finished_jobs(&self) -> impl Iterator<Item = &ProductionJob> {
        self.jobs.iter().filter(|job| {
            job.module_part.is_some() && job.progress_ticks == job.work.duration_ticks
        })
    }

    fn remove_delivered(&mut self, id: Id) {
        self.jobs.retain(|job| job.id != id);
    }

    fn replace_policy(&mut self, mut policy: ServicePolicy) -> Result<()> {
        ensure!(policy.valid(), "invalid service policy");
        ensure!(
            policy.revision == self.policy.revision,
            "prices changed; reload before publishing"
        );
        for rate in &policy.rates {
            let count: usize = self
                .modules
                .iter()
                .filter(|module| module.capability == rate.capability)
                .map(|module| module.lanes.len())
                .sum();
            ensure!(
                rate.public_lanes as usize <= count,
                "public lane allocation exceeds installed lanes"
            );
        }
        policy.revision = policy
            .revision
            .checked_add(1)
            .context("price revision exhausted")?;
        self.policy = policy;
        Ok(())
    }

    fn quote(
        &self,
        facility: Id,
        operator: Principal,
        payer: Principal,
        work: ServiceWork,
        plan: &WorkPlan,
        directory: &OwnershipDirectory,
    ) -> Result<ServiceQuote> {
        ensure!(self.policy.accepting, "outside jobs are closed");
        let rate = self
            .policy
            .rates
            .iter()
            .find(|rate| rate.capability == plan.capability && rate.public_lanes > 0)
            .context("module closed to outside jobs")?;
        let price_basis_points = self
            .policy
            .tiers
            .iter()
            .find(|tier| service::tier_matches(directory, payer, &tier.customer))
            .map_or(Some(10_000), |tier| tier.price_basis_points)
            .context("customer tier refuses service")?;
        let energy_charge = u64::try_from(
            (u128::from(plan.energy_j) * u128::from(rate.energy_per_mj)).div_ceil(1_000_000),
        )
        .context("price overflow")?;
        let time_charge = u64::try_from(
            (u128::from(plan.duration_ticks) * u128::from(rate.time_per_hour)).div_ceil(36_000),
        )
        .context("price overflow")?;
        let total = u64::try_from(
            ((u128::from(energy_charge) + u128::from(time_charge))
                * u128::from(price_basis_points))
            .div_ceil(10_000),
        )
        .context("price overflow")?;

        Ok(ServiceQuote {
            facility,
            payer,
            operator,
            work,
            policy_revision: self.policy.revision,
            currency: self.policy.currency,
            energy_charge,
            time_charge,
            price_basis_points,
            total,
            inputs: plan.inputs.clone(),
            outputs: match &plan.output {
                WorkOutput::Cargo(outputs) => outputs.clone(),
                WorkOutput::Ship(_) => Vec::new(),
            },
            duration_ticks: plan.duration_ticks,
        })
    }
}

#[derive(Clone, Debug)]
struct IndustrialModule {
    pub part_id: u64,
    pub part_index: usize,
    pub capability: IndustryCapability,
    pub power_per_lane_w: u64,
    pub max_radius_m: Option<f64>,
    pub lanes: Vec<Option<Id>>,
}

impl IndustrialModule {
    fn suitable(&self, work: &WorkPlan) -> bool {
        self.capability == work.capability
            && self
                .max_radius_m
                .is_none_or(|maximum| maximum >= work.required_radius_m)
            && u128::from(self.power_per_lane_w)
                >= u128::from(work.energy_j.div_ceil(work.duration_ticks)) * 10
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProductionJob {
    pub id: Id,
    pub owner: Principal,
    pub created_by: AccountId,
    pub work: WorkPlan,
    pub progress_ticks: u64,
    pub status: JobStatus,
    pub payment: Option<ServicePayment>,
    #[serde(skip)]
    pub requested_power_w: u64,
    #[serde(skip)]
    pub supplied_power_w: u64,
    #[serde(skip)]
    pub module_part: Option<u64>,
}

impl ProductionJob {
    pub fn view(&self) -> JobView {
        JobView {
            id: self.id,
            name: self.work.name.clone(),
            capability: self.work.capability,
            progress_ticks: self.progress_ticks,
            duration_ticks: self.work.duration_ticks,
            status: self.status.clone(),
            owner: self.owner,
            created_by: self.created_by,
            module_part: self.module_part,
            requested_power_w: self.requested_power_w,
            supplied_power_w: self.supplied_power_w,
            payment: self.payment.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkPlan {
    pub name: String,
    pub capability: IndustryCapability,
    pub inputs: Vec<ItemStack>,
    pub output: WorkOutput,
    pub duration_ticks: u64,
    pub energy_j: u64,
    pub stored_energy_j: u64,
    pub required_radius_m: f64,
    #[serde(skip)]
    construction: Option<ConstructionDesign>,
}

impl WorkPlan {
    fn recipe(recipe: &Recipe, batches: u32) -> Result<Self> {
        ensure!(
            (1..=MAX_RECIPE_BATCHES).contains(&batches),
            "invalid batch count"
        );
        let multiply = |amount: u64| {
            amount
                .checked_mul(u64::from(batches))
                .context("work quantity overflow")
        };
        let scale = |stacks: &[ItemStack]| -> Result<Vec<ItemStack>> {
            stacks
                .iter()
                .map(|stack| {
                    Ok(ItemStack {
                        item: stack.item.clone(),
                        quantity: multiply(stack.quantity)?,
                    })
                })
                .collect()
        };

        Ok(Self {
            name: recipe.name.clone(),
            capability: recipe.capability,
            inputs: scale(&recipe.inputs)?,
            output: WorkOutput::Cargo(scale(&recipe.outputs)?),
            duration_ticks: multiply(recipe.duration_ticks)?,
            energy_j: multiply(recipe.energy_j)?,
            stored_energy_j: multiply(recipe.stored_energy_j)?,
            required_radius_m: 0.,
            construction: None,
        })
    }

    fn job(self, account: AccountId, owner: Principal) -> ProductionJob {
        ProductionJob {
            id: Id::new(),
            owner,
            created_by: account,
            work: self,
            progress_ticks: 0,
            status: JobStatus::Queued,
            payment: None,
            requested_power_w: 0,
            supplied_power_w: 0,
            module_part: None,
        }
    }
}

#[derive(Clone, Debug)]
struct ConstructionDesign {
    pub blueprint: Arc<osg_ships::ShipBlueprint>,
    pub design: Arc<CompiledShipDesign>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkOutput {
    Cargo(Vec<ItemStack>),
    Ship(Arc<[u8]>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MineSource {
    pub output: CargoItem,
    pub units_per_second: u64,
    pub remainder: u64,
    pub last_recipient: Option<Id>,
}

/// Stable save data contains no device entities or derived module assignments.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndustrialFacilityRecord {
    pub jobs: Vec<ProductionJob>,
    pub policy: ServicePolicy,
    pub outside_revenue: super::economy::Balance,
    pub mine: Option<MineSource>,
}

struct ProductionStep {
    pub job: Id,
    pub part_index: usize,
    pub energy_j: u64,
    pub heat_j: u64,
}

#[derive(Resource)]
struct ManufacturingCatalogue(IndustryCatalogue);

pub struct IndustryPlugin;

impl Plugin for IndustryPlugin {
    fn build(&self, app: &mut App) {
        let catalogue = &app.world().resource::<vessel::ShipCatalogue>().0;
        let recipes = osg_ships::industry::recipes(catalogue).expect("industry recipes");
        let blueprint = osg_ships::industry::starter_ship();
        let design = blueprint.compile(catalogue).expect("starter design");
        let requirements = osg_ships::industry::construction_requirements(&design, catalogue)
            .expect("starter bill");
        let blueprints = vec![BlueprintView {
            name: blueprint.name.clone(),
            blueprint: blueprint.to_bytes().expect("starter blueprint"),
            inputs: requirements.inputs,
            duration_ticks: requirements.duration_ticks,
            energy_j: requirements.energy_j,
        }];
        let revision =
            *blake3::hash(&postcard::to_stdvec(&(&recipes, &blueprints)).unwrap()).as_bytes();
        app.insert_resource(ManufacturingCatalogue(IndustryCatalogue {
            revision,
            recipes,
            blueprints,
        }))
        .init_resource::<requests::ServicePolicyQueue>()
        .init_resource::<requests::CancellationQueue>()
        .init_resource::<requests::CargoQueue>()
        .init_resource::<requests::WorkQueue>()
        .init_resource::<requests::IndustryQueryQueue>()
        .init_resource::<crate::OperationHistory>()
        .add_systems(
            FixedPreUpdate,
            initialize_facilities.in_set(hardware::HardwareSystems::Initialize),
        )
        .add_systems(
            FixedPreUpdate,
            (
                admission::configure_services,
                admission::cancel_work,
                cargo::process_cargo,
                admission::start_work,
                hardware::publish_mass,
            )
                .chain()
                .after(hardware::HardwareSystems::Initialize),
        )
        .add_systems(
            FixedUpdate,
            (
                production::advance_production,
                production::complete_production,
                cargo::advance_mines,
            )
                .chain()
                .after(hardware::utilities::service_docked)
                .before(hardware::cooling::run)
                .in_set(hardware::HardwareSystems::Run),
        )
        .add_systems(
            FixedLast,
            queries::answer_queries.after(super::simulation::SimulationSystems::Observations),
        );
    }
}

fn initialize_facilities(
    mut commands: Commands,
    mut ships: Query<
        (
            Entity,
            &vessel::ShipDesign,
            Option<&mut IndustrialFacility>,
            Has<super::infrastructure::Landmark>,
        ),
        Or<(Changed<vessel::ShipDesign>, Without<IndustrialFacility>)>,
    >,
) {
    for (entity, design, facility, landmark) in &mut ships {
        if let Some(mut facility) = facility {
            facility.rebuild_modules(&design.0);
        } else {
            let facility = IndustrialFacility::from_design(&design.0);
            if landmark || !facility.modules.is_empty() {
                commands.entity(entity).insert(facility);
            }
        }
    }
}

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod product_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
use bevy::ecs::system::RunSystemOnce;
#[cfg(test)]
use test_support::{
    cumulative_energy, enqueue_blueprint, install, lanes, read_directory, read_facility,
    read_hangar,
};
#[cfg(test)]
pub use test_support::{enqueue_command, service_client, tick};
