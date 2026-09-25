use super::super::{economy::Economy, physics, precision::PreciseTransform};
use super::*;
use access::Asset;
use osg_ships::ShipState;

pub fn advance_production(
    assets: Query<Asset>,
    mut facilities: Query<(
        &mut IndustrialFacility,
        &mut hardware::ShipInventory,
        &mut hardware::ShipThermal,
    )>,
    mut parts: Query<(&mut hardware::Device, &mut hardware::DevicePower)>,
    mut economy: ResMut<Economy>,
    directory: Res<ownership::Directory>,
) {
    let mut order: Vec<_> = assets
        .iter()
        .filter(|asset| facilities.contains(asset.entity))
        .map(|asset| (asset.identity.0, asset.entity))
        .collect();
    order.sort_by_key(|&(id, _)| id);
    for (_, entity) in order {
        let asset = assets.get(entity).unwrap();
        let (mut facility, mut inventory, mut thermal) = facilities.get_mut(entity).unwrap();
        let operational: Vec<_> = facility
            .modules
            .iter()
            .map(|module| {
                asset.active()
                    && asset
                        .parts
                        .0
                        .get(module.part_index)
                        .and_then(|entity| parts.get(*entity).ok())
                        .is_some_and(|(device, _)| device.0.operational)
            })
            .collect();
        for module in &facility.modules {
            if let Some(&part) = asset.parts.0.get(module.part_index)
                && let Ok((mut device, mut power)) = parts.get_mut(part)
            {
                device.0.powered = false;
                device.0.actual = 0.;
                *power = hardware::DevicePower::default();
            }
        }
        facility.assign_lanes(&operational);
        for step in facility.prepare_steps() {
            let part = asset.parts.0[step.part_index];
            let (mut device, mut power) = parts.get_mut(part).unwrap();
            power.requested_w += (step.energy_j * 10) as f64;
            if inventory.0.energy_j < step.energy_j {
                facility.block_job(step.job, JobStatus::AwaitingPower);
                continue;
            }
            if let Some(payment) = &facility.job(step.job).unwrap().payment {
                match economy.charge_service(
                    step.job,
                    payment,
                    &directory.0,
                    &facility.outside_revenue,
                    osg_model::calendar::now_unix_ms(),
                ) {
                    Ok(revenue) => facility.record_charge(step.job, revenue),
                    Err(_) => {
                        facility.block_job(step.job, JobStatus::AwaitingPayment);
                        continue;
                    }
                }
            }
            inventory.0.energy_j -= step.energy_j;
            thermal
                .0
                .add_waste_heat(step.heat_j as f64, osg_model::TICK_SECONDS);
            device.0.powered = true;
            device.0.actual += 1.;
            power.supplied_w += (step.energy_j * 10) as f64;
            facility.commit_step(step);
        }
    }
}

pub fn complete_production(
    mut commands: Commands,
    assets: Query<Asset>,
    mut facilities: Query<(
        &mut IndustrialFacility,
        &mut hardware::ShipInventory,
        &mut travel::StoredMass,
    )>,
    poses: Query<(&PreciseTransform, Option<&physics::Velocity>)>,
    accounts: Query<&identity::Account>,
    identities: Res<identity::IdentityIndex>,
    directory: Res<ownership::Directory>,
    mut economy: ResMut<Economy>,
    catalogue: Res<vessel::ShipCatalogue>,
    mut runtime: ResMut<vessel::WasmRuntime>,
    appearances: Res<identity::AppearanceAssets>,
    gas: Res<super::super::gas::GasLedger>,
    mut events: ResMut<travel::TravelEvents>,
) {
    let mut order: Vec<_> = assets
        .iter()
        .filter(|asset| facilities.contains(asset.entity))
        .map(|asset| (asset.identity.0, asset.entity))
        .collect();
    order.sort_by_key(|&(id, _)| id);
    for (_, entity) in order {
        let asset = assets.get(entity).unwrap();
        if !asset.active() {
            continue;
        }
        let (mut facility, mut inventory, mut stored_mass) = facilities.get_mut(entity).unwrap();
        let ready: Vec<_> = facility.finished_jobs().cloned().collect();
        for job in ready {
            let mut next = inventory.0.clone();
            match &job.work.output {
                WorkOutput::Cargo(outputs) => {
                    let result = (|| {
                        next.complete_cargo(
                            &job.work.inputs,
                            outputs,
                            asset.design.0.capacity_m3,
                            &catalogue.0,
                        )?;
                        let stock = job
                            .payment
                            .as_ref()
                            .map(|payment| {
                                service::storage_credit(
                                    &economy,
                                    &mut next,
                                    asset.identity.0,
                                    payment.payer,
                                    outputs,
                                )
                                .map(|stock| (payment.payer, stock))
                            })
                            .transpose()?;
                        Ok::<_, anyhow::Error>(stock)
                    })();
                    let stock = match result {
                        Ok(stock) => stock,
                        Err(_) => {
                            facility.block_job(job.id, JobStatus::AwaitingCargoSpace);
                            continue;
                        }
                    };
                    if let Some((owner, stock)) = stock {
                        service::write_stock(&mut economy, asset.identity.0, owner, stock);
                    }
                }
                WorkOutput::Ship(_) => {
                    // All validation and allocations that can return an error
                    // precede the commit and ordinary deferred spawn commands.
                    let prepared = (|| {
                        let construction = job
                            .work
                            .construction
                            .as_ref()
                            .context("construction design unavailable")?;
                        let blueprint = &construction.blueprint;
                        let design = construction.design.clone();
                        let state = ShipState::cold(&design, &catalogue.0);
                        let (mass, inertia) = state.mass_properties(&design, &catalogue.0);
                        ensure!(
                            (mass - design.dry_mass).abs() <= 1e-8 * mass.max(1.),
                            "cold construction has unaccounted consumables"
                        );
                        let bay = access::construction_bay(
                            &assets,
                            &asset,
                            &directory.0,
                            job.owner,
                            design.radius,
                            mass,
                        )?;
                        let account = access::lookup(&identities, job.created_by)?;
                        accounts.get(account)?;
                        let faction = directory
                            .0
                            .players
                            .get(&job.created_by)
                            .context("creator unavailable")?
                            .organization;
                        let (pose, velocity) = poses.get(entity)?;
                        let pose = *pose;
                        let velocity =
                            velocity.map_or(bevy::math::DVec3::ZERO, |velocity| velocity.0);
                        let mut controller = runtime.0.instantiate(blueprint.controller_bytes())?;
                        controller.configure_hardware(&design, &catalogue.0);
                        let appearance =
                            osg_ships::appearance::ShipAppearance::from(design.as_ref())
                                .to_bytes()?;
                        next.complete_cargo(
                            &job.work.inputs,
                            &[],
                            asset.design.0.capacity_m3,
                            &catalogue.0,
                        )?;
                        Ok::<_, anyhow::Error>((
                            design, state, mass, inertia, bay, account, faction, pose, velocity,
                            controller, appearance,
                        ))
                    })();
                    let (
                        design,
                        state,
                        mass,
                        inertia,
                        bay,
                        account,
                        faction,
                        pose,
                        velocity,
                        controller,
                        appearance,
                    ) = match prepared {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            if job.status != JobStatus::AwaitingBerth {
                                warn!(job = ?job.id, error = %error, "Construction delivery blocked");
                            }
                            facility.block_job(job.id, JobStatus::AwaitingBerth);
                            continue;
                        }
                    };
                    let appearance_hash = *blake3::hash(&appearance).as_bytes();
                    appearances.insert(appearance_hash, appearance);
                    let spatial = super::super::spatial::SpatialBody {
                        radius_m: design.radius,
                        occludes: true,
                    };
                    let mut ship = commands.spawn(vessel::ship_bundle(
                        design,
                        state,
                        vessel::ShipSoftware::new(controller),
                        pose,
                        velocity,
                        job.work.name.clone(),
                        physics::MassProps {
                            mass,
                            inertia,
                            inertia_inv: inertia.inverse(),
                        },
                    ));
                    ship.insert((
                        identity::Identity(Id::new()),
                        identity::SpatialInstance(Id::new()),
                        identity::Control {
                            account: job.created_by,
                            revision: 1,
                        },
                        identity::ControlledBy(account),
                        ownership::AssetOwner(job.owner),
                        ownership::AssetAccess::default(),
                        identity::Transponder(osg_model::IffIdentity {
                            owner: job.created_by,
                            faction,
                            labels: [job.work.name.clone()].into(),
                            enabled: true,
                        }),
                        identity::Appearance(appearance_hash),
                        travel::Dormant,
                        travel::PresenceState(osg_model::travel::Presence::Docked {
                            host: asset.identity.0,
                            bay,
                        }),
                        travel::DockedIn(entity),
                        travel::DormantMotion {
                            velocity,
                            angular_velocity: bevy::math::DVec3::ZERO,
                            spatial: Some(spatial),
                            rigid_body: true,
                            collision_body: true,
                        },
                    ))
                    .remove::<(
                        physics::Velocity,
                        physics::AngularVelocity,
                        physics::RigidBody,
                        physics::AccumulatedForce,
                        physics::AccumulatedTorque,
                        physics::collision::CollisionBody,
                        super::super::spatial::SpatialBody,
                    )>();
                    events.0.push((ship.id(), "constructed", None));
                    stored_mass.0 += mass;
                    gas.ensure_account(job.owner, 0);
                }
            }
            inventory.0 = next;
            facility.remove_delivered(job.id);
        }
    }
}
