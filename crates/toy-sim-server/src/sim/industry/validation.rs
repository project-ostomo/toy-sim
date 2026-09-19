use super::*;
use toy_sim_ships::{ShipBlueprint, industry as manufacturing};

fn stack_totals(stacks: &[ItemStack], catalogue: &Catalogue) -> Result<BTreeMap<CargoItem, u64>> {
    ensure!(
        !stacks.is_empty() && stacks.len() <= 256,
        "invalid saved industry item count"
    );
    let totals = toy_sim_ships::aggregate_stacks(stacks, catalogue)?;
    ensure!(
        totals.len() == stacks.len(),
        "saved industry items must have unique quantities"
    );
    Ok(totals)
}

fn installed_lane(
    design: &CompiledShipDesign,
    job: &IndustryJob,
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
            && capability == job.view.capability
            && radius >= job.required_radius_m
            && u128::from(power) >= u128::from(job.energy_j.div_ceil(job.view.duration_ticks)) * 10
    })
}

pub fn validate_saved(
    facility: Option<&IndustryFacility>,
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
        ensure!(
            facility.jobs.len() <= MAX_JOBS,
            "saved industry queue exceeds limit"
        );
        for job in &facility.jobs {
            ensure!(
                identities.insert(job.view.id)
                    && !job.view.name.trim().is_empty()
                    && job.view.name.len() <= 128
                    && !job.view.name.chars().any(char::is_control)
                    && directory.contains(job.view.owner)
                    && directory.players.contains_key(&job.view.created_by),
                "invalid saved industry job identity or owner"
            );
            ensure!(
                job.view.duration_ticks > 0
                    && job.view.progress_ticks <= job.view.duration_ticks
                    && job.energy_j > 0
                    && job.stored_energy_j <= job.energy_j
                    && job.required_radius_m.is_finite()
                    && job.required_radius_m >= 0.
                    && job.view.supplied_power_w <= job.view.requested_power_w,
                "invalid saved industry progress, power or dimensions"
            );
            ensure!(
                installed_lane(design, job, job.view.module_part),
                "saved industry job requires unavailable installed capability"
            );
            ensure!(
                u128::from(job.view.requested_power_w)
                    <= u128::from(job.energy_j.div_ceil(job.view.duration_ticks)) * 10
                    && (!matches!(
                        job.view.status,
                        JobStatus::AwaitingCargoSpace | JobStatus::AwaitingBerth
                    ) || job.view.progress_ticks == job.view.duration_ticks),
                "saved industry status differs from its progress or energy"
            );
            let inputs = stack_totals(&job.inputs, catalogue)?;
            match &job.output {
                JobOutput::Cargo(outputs) => {
                    stack_totals(outputs, catalogue)?;
                    ensure!(
                        job.required_radius_m == 0. && job.view.status != JobStatus::AwaitingBerth,
                        "cargo job has ship dimensions or berth state"
                    );
                    manufacturing::validate_recipe(
                        &Recipe {
                            id: "saved-job".into(),
                            name: job.view.name.clone(),
                            capability: job.view.capability,
                            inputs: job.inputs.clone(),
                            outputs: outputs.clone(),
                            duration_ticks: job.view.duration_ticks,
                            energy_j: job.energy_j,
                            stored_energy_j: job.stored_energy_j,
                        },
                        catalogue,
                    )?;
                }
                JobOutput::Ship(bytes) => {
                    ensure!(
                        bytes.len() <= 2_000_000
                            && job.view.status != JobStatus::AwaitingCargoSpace,
                        "invalid saved construction payload or cargo state"
                    );
                    let blueprint = ShipBlueprint::from_bytes(bytes)?;
                    let hull = blueprint.compile(catalogue)?;
                    let requirements = manufacturing::construction_requirements(&hull, catalogue)?;
                    ensure!(
                        job.view.capability == IndustryCapability::Shipyard
                            && inputs == stack_totals(&requirements.inputs, catalogue)?
                            && job.view.duration_ticks == requirements.duration_ticks
                            && job.energy_j == requirements.energy_j
                            && job.stored_energy_j == 0
                            && (job.required_radius_m - hull.radius).abs()
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
