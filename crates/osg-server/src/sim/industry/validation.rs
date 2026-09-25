use super::*;
use osg_ships::{Equipment, ShipBlueprint, industry as manufacturing, utilities::UtilityDef};

fn stack_totals(stacks: &[ItemStack], catalogue: &Catalogue) -> Result<BTreeMap<CargoItem, u64>> {
    ensure!(
        !stacks.is_empty() && stacks.len() <= 256,
        "invalid saved industry item count"
    );
    let totals = osg_ships::aggregate_stacks(stacks, catalogue)?;
    ensure!(
        totals.len() == stacks.len(),
        "saved industry items must have unique quantities"
    );
    Ok(totals)
}

fn installed_lane(
    design: &CompiledShipDesign,
    job: &ProductionJob,
    selected_part: Option<u64>,
) -> bool {
    design.parts.iter().any(|part| {
        if selected_part.is_some_and(|id| id != part.placed.id) {
            return false;
        }
        let (capability, power, count, radius) = match part.definition.equipment {
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
            _ => return false,
        };
        count > 0
            && capability == job.work.capability
            && radius >= job.work.required_radius_m
            && u128::from(power)
                >= u128::from(job.work.energy_j.div_ceil(job.work.duration_ticks)) * 10
    })
}

pub fn validate_saved(
    facility: Option<&IndustrialFacilityRecord>,
    mine: Option<&MineSource>,
    design: &CompiledShipDesign,
    inventory: &Inventory,
    catalogue: &Catalogue,
    directory: &OwnershipDirectory,
) -> Result<()> {
    inventory.validate_cargo(catalogue)?;
    let volume = inventory.cargo_volume(catalogue);
    ensure!(
        volume.is_finite()
            && volume <= design.capacity_m3 + design.capacity_m3.max(1.) * 1e-9
            && inventory.mass(catalogue).is_finite(),
        "saved cargo exceeds its physical hold"
    );

    let mut reservations = BTreeMap::<CargoItem, u64>::new();
    let mut identities = BTreeSet::new();
    if let Some(facility) = facility {
        ensure!(facility.policy.valid(), "invalid saved service prices");
        let bytes: usize = facility
            .jobs
            .iter()
            .map(|job| match &job.work.output {
                WorkOutput::Cargo(_) => 0,
                WorkOutput::Ship(bytes) => bytes.len(),
            })
            .sum();
        ensure!(
            bytes <= MAX_QUEUED_BLUEPRINT_BYTES,
            "saved blueprints exceed the 64 MiB facility limit"
        );
        ensure!(
            facility.jobs.len() <= MAX_JOBS,
            "saved industry queue exceeds limit"
        );
        for job in &facility.jobs {
            if let Some(payment) = &job.payment {
                ensure!(
                    directory.contains(payment.payer)
                        && directory.contains(payment.operator)
                        && job.owner == payment.payer
                        && (payment.charged || job.progress_ticks == 0),
                    "invalid saved service payment"
                );
            }
            ensure!(
                identities.insert(job.id)
                    && !job.work.name.trim().is_empty()
                    && job.work.name.len() <= 128
                    && !job.work.name.chars().any(char::is_control)
                    && directory.contains(job.owner)
                    && directory.players.contains_key(&job.created_by),
                "invalid saved industry job identity or owner"
            );
            ensure!(
                job.work.duration_ticks > 0
                    && job.progress_ticks <= job.work.duration_ticks
                    && job.work.energy_j > 0
                    && job.work.stored_energy_j <= job.work.energy_j
                    && job.work.required_radius_m.is_finite()
                    && job.work.required_radius_m >= 0.
                    && job.supplied_power_w <= job.requested_power_w,
                "invalid saved industry progress, power or dimensions"
            );
            ensure!(
                installed_lane(design, job, job.module_part),
                "saved industry job requires unavailable installed capability"
            );
            ensure!(
                u128::from(job.requested_power_w)
                    <= u128::from(job.work.energy_j.div_ceil(job.work.duration_ticks)) * 10
                    && (!matches!(
                        job.status,
                        JobStatus::AwaitingCargoSpace | JobStatus::AwaitingBerth
                    ) || job.progress_ticks == job.work.duration_ticks),
                "saved industry status differs from its progress or energy"
            );
            let inputs = stack_totals(&job.work.inputs, catalogue)?;
            match &job.work.output {
                WorkOutput::Cargo(outputs) => {
                    stack_totals(outputs, catalogue)?;
                    ensure!(
                        job.work.required_radius_m == 0. && job.status != JobStatus::AwaitingBerth,
                        "cargo job has ship dimensions or berth state"
                    );
                    manufacturing::validate_recipe(
                        &Recipe {
                            id: "saved-job".into(),
                            name: job.work.name.clone(),
                            capability: job.work.capability,
                            inputs: job.work.inputs.clone(),
                            outputs: outputs.clone(),
                            duration_ticks: job.work.duration_ticks,
                            energy_j: job.work.energy_j,
                            stored_energy_j: job.work.stored_energy_j,
                        },
                        catalogue,
                    )?;
                }
                WorkOutput::Ship(bytes) => {
                    ensure!(
                        bytes.len() <= osg_ships::MAX_FILE
                            && job.status != JobStatus::AwaitingCargoSpace,
                        "invalid saved construction payload or cargo state"
                    );
                    let blueprint = ShipBlueprint::from_bytes(bytes)?;
                    let hull = blueprint.compile(catalogue)?;
                    let requirements = manufacturing::construction_requirements(&hull, catalogue)?;
                    ensure!(
                        job.work.capability == IndustryCapability::Shipyard
                            && inputs == stack_totals(&requirements.inputs, catalogue)?
                            && job.work.duration_ticks == requirements.duration_ticks
                            && job.work.energy_j == requirements.energy_j
                            && job.work.stored_energy_j == 0
                            && (job.work.required_radius_m - hull.radius).abs()
                                <= hull.radius.max(1.) * 1e-9,
                        "saved construction job differs from its blueprint requirements"
                    );
                }
            }
            for (item, quantity) in inputs {
                let total = reservations.entry(item).or_default();
                *total = total
                    .checked_add(quantity)
                    .context("saved reservation overflow")?;
            }
        }
    }
    ensure!(
        inventory.reservations == reservations,
        "saved cargo reservations differ from pending industry jobs"
    );

    if let Some(mine) = mine {
        manufacturing::item_mass_kg(&mine.output, catalogue)?;
        ensure!(
            mine.units_per_second > 0 && mine.remainder < 10,
            "invalid saved mine rate or fractional remainder"
        );
    }
    Ok(())
}
