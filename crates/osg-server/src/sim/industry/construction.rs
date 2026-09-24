use super::*;
use osg_ships::{ShipBlueprint, ShipState};
use std::sync::Arc;

pub(super) fn job(
    world: &mut World,
    account: AccountId,
    facility: Entity,
    owner: Principal,
    blueprint_bytes: &[u8],
) -> Result<IndustryJob> {
    ownership::authorize(world, account, facility, Permission::Industry)?;
    let facility_owner = world
        .get::<ownership::AssetOwner>(facility)
        .context("facility owner missing")?
        .0;
    let directory = &world.resource::<ownership::Directory>().0;
    ensure!(directory.contains(owner), "unknown output owner");
    if owner != facility_owner {
        ownership::authorize(world, account, facility, Permission::TransferCargo)?;
        ensure!(
            directory.administers(account, owner),
            "output owner not administered by requester"
        );
    }
    prepare(world, account, facility, owner, blueprint_bytes)
}

pub(super) fn prepare(
    world: &mut World,
    account: AccountId,
    facility: Entity,
    owner: Principal,
    blueprint_bytes: &[u8],
) -> Result<IndustryJob> {
    ensure!(
        blueprint_bytes.len() <= osg_ships::MAX_FILE,
        "ship blueprint too large"
    );
    let jobs = world
        .get::<IndustryFacility>(facility)
        .map_or(&[][..], |facility| facility.jobs.as_slice());
    validate_blueprint_budget(jobs, blueprint_bytes.len())?;

    let blueprint = ShipBlueprint::from_bytes(blueprint_bytes)?;
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let design = blueprint.compile(&catalogue)?;
    world
        .resource_mut::<vessel::WasmRuntime>()
        .0
        .validate_program(blueprint.controller_bytes())?;
    let requirements = osg_ships::industry::construction_requirements(&design, &catalogue)?;
    ensure!(
        world
            .get::<travel::DockingBays>(facility)
            .is_some_and(|bays| {
                bays.0.iter().any(|bay| {
                    bay.radius_m >= design.radius && bay.mass_capacity_kg >= design.dry_mass
                })
            }),
        "no docking aperture fits this design"
    );
    travel::construction_bay(world, facility, owner, design.radius, design.dry_mass)?;
    Ok(IndustryJob {
        view: JobView {
            id: Id::new(),
            name: blueprint.name.clone(),
            capability: IndustryCapability::Shipyard,
            progress_ticks: 0,
            duration_ticks: requirements.duration_ticks,
            status: JobStatus::Queued,
            owner,
            created_by: account,
            module_part: None,
            requested_power_w: 0,
            supplied_power_w: 0,
            payment: None,
        },
        inputs: requirements.inputs,
        output: JobOutput::Ship(blueprint_bytes.into()),
        energy_j: requirements.energy_j,
        stored_energy_j: 0,
        required_radius_m: design.radius,
    })
}

pub(super) fn finish(world: &mut World, facility: Entity, job: &IndustryJob) -> Result<Entity> {
    let JobOutput::Ship(bytes) = &job.output else {
        anyhow::bail!("job does not construct a ship");
    };
    let blueprint = ShipBlueprint::from_bytes(bytes)?;
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let design = Arc::new(blueprint.compile(&catalogue)?);
    let state = ShipState::cold(&design, &catalogue);
    let (mass, inertia) = state.mass_properties(&design, &catalogue);
    ensure!(
        (mass - design.dry_mass).abs() <= 1e-8 * mass.max(1.),
        "cold construction has unaccounted consumables"
    );
    let bay = travel::construction_bay(world, facility, job.view.owner, design.radius, mass)?;
    let pose =
        super::super::session::ship_pose(world, facility).context("shipyard pose unavailable")?;
    let mut controller = world
        .resource_mut::<vessel::WasmRuntime>()
        .0
        .instantiate(blueprint.controller_bytes())?;
    controller.configure_hardware(&design, &catalogue);
    let ship = world
        .spawn(vessel::ship_bundle(
            design,
            state,
            vessel::ShipSoftware::new(controller),
            super::super::precision::PreciseTransform {
                translation_um: pose.position,
                rotation: bevy::math::DQuat::from_array(pose.rotation),
            },
            bevy::math::DVec3::from_array(pose.velocity),
            blueprint.name,
            super::super::physics::MassProps {
                mass,
                inertia,
                inertia_inv: inertia.inverse(),
            },
        ))
        .id();
    let result = (|| {
        world
            .run_system_once(hardware::initialize)
            .map_err(|error| anyhow::anyhow!("hardware initialization failed: {error:?}"))?;
        identity::attach_ship(world, ship, job.view.created_by, Id::new())?;
        world
            .entity_mut(ship)
            .insert(ownership::AssetOwner(job.view.owner));
        travel::store_constructed(world, ship, facility, bay)?;
        Ok(())
    })();
    if let Err(error) = result {
        world.despawn(ship);
        return Err(error);
    }
    world
        .resource::<super::super::gas::GasLedger>()
        .ensure_account(job.view.owner, 0);
    Ok(ship)
}
