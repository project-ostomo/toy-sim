use crate::sim::{
    hardware, identity, infrastructure, intelligence, orrery, ownership, physics, precision,
    registry, simulation, spatial, travel, vessel,
};
use anyhow::{Context, Result, ensure};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use toy_sim_model::travel::{Presence, TravelState};
use toy_sim_model::{Id, IffIdentity, InfoGroupKey, Pose, Track};

#[derive(Serialize, Deserialize)]
struct WorldRecord {
    epoch: Id,
    directory: toy_sim_model::ownership::OwnershipDirectory,
    rate: f64,
    sensor_seed: [u8; 32],
    catalogue: [u8; 32],
    resource_ids: Vec<String>,
    elapsed_ns: u64,
    tick: u64,
    groups: Vec<GroupRecord>,
    accounts: Vec<AccountRecord>,
    ships: Vec<ShipRecord>,
    gates: Vec<GateRecord>,
    tracks: Vec<TrackRecord>,
    programs: BTreeMap<[u8; 32], Vec<u8>>,
    projectiles: Vec<ProjectileRecord>,
}

#[derive(Serialize, Deserialize)]
struct GroupRecord {
    id: Id,
    key: Option<InfoGroupKey>,
}

#[derive(Serialize, Deserialize)]
struct AccountRecord {
    id: Id,
    group: Id,
}

#[derive(Serialize, Deserialize)]
struct TrackRecord {
    group: Id,
    physical: Id,
    track: Track,
}

#[derive(Serialize, Deserialize)]
struct ControlRecord {
    account: Id,
    revision: u64,
}

#[derive(Serialize, Deserialize)]
struct SoftwareRecord {
    persistent_data: Vec<u8>,
    request_id: u64,
    hull_energy_j: f64,
    shield_energy_j: f64,
}

#[derive(Serialize, Deserialize)]
struct ShipRecord {
    id: Id,
    spatial_instance: Option<Id>,
    name: String,
    blueprint: String,
    program: [u8; 32],
    pose: Pose,
    hardware: toy_sim_ships::ShipState,
    parts: Vec<PartRecord>,
    software: Option<SoftwareRecord>,
    control: Option<ControlRecord>,
    iff: Option<IffIdentity>,
    group: Option<Id>,
    travel: TravelState,
    presence: Presence,
    stored_mass: f64,
    dormant_thermal_s: f64,
    beacon: bool,
    fixed: bool,
    controlled: bool,
    bays: Option<Vec<travel::Bay>>,
    drive: Option<travel::SlipDrive>,
    transit: Option<travel::Transit>,
    landmark: Option<infrastructure::Landmark>,
    crew: Option<(u32, u32, f64)>,
    sensor_override: Option<(f64, bool)>,
    owner: ownership::AssetOwner,
    access: ownership::AssetAccess,
    dock_services: (bool, bool),
}

#[derive(Serialize, Deserialize)]
struct PartRecord {
    index: usize,
    reactor: Option<(f64, f64, bool)>,
    thermal_engine_decay_j: Option<f64>,
}

#[derive(Serialize, Deserialize)]
struct GateRecord {
    id: Id,
    pose: Pose,
    gate: travel::Gate,
    orbit: Option<infrastructure::GateOrbit>,
    landmark: Option<infrastructure::Landmark>,
    control: Option<ControlRecord>,
    iff: Option<IffIdentity>,
    radius_m: f64,
    owner: ownership::AssetOwner,
    access: ownership::AssetAccess,
}

#[derive(Serialize, Deserialize)]
struct ProjectileRecord {
    owner: Option<Id>,
    pose: Pose,
    remaining_s: f64,
    radius_m: f64,
    hull_hp: f64,
    thermal: toy_sim_ships::thermal::ThermalState,
    mass_kg: f64,
    inertia: [f64; 9],
}

fn id(world: &World, entity: Entity) -> Result<Id> {
    Ok(world
        .get::<identity::Identity>(entity)
        .context("persistent relation has no stable identity")?
        .0)
}

fn control(world: &World, entity: Entity) -> Option<ControlRecord> {
    world
        .get::<identity::Control>(entity)
        .map(|control| ControlRecord {
            account: control.account,
            revision: control.revision,
        })
}

fn pose(world: &World, entity: Entity) -> Result<Pose> {
    let mut pose = intelligence::pose(
        world
            .get::<precision::PreciseTransform>(entity)
            .context("persistent body has no pose")?,
        world.get::<physics::Velocity>(entity),
        world.get::<physics::AngularVelocity>(entity),
    );
    if let Some(motion) = world.get::<travel::DormantMotion>(entity) {
        pose.velocity = motion.velocity.to_array();
        pose.angular_velocity = motion.angular_velocity.to_array();
    }
    Ok(pose)
}

pub fn capture(world: &World) -> Result<Vec<u8>> {
    let mut record = WorldRecord {
        epoch: world.resource::<identity::WorldEpoch>().0,
        directory: world.resource::<ownership::Directory>().0.clone(),
        rate: world.resource::<crate::sim::session::Clock>().rate,
        sensor_seed: world.resource::<identity::SensorSeed>().0,
        catalogue: world.resource::<registry::UniverseRegistry>().catalogue,
        resource_ids: world
            .resource::<vessel::ShipCatalogue>()
            .0
            .resources
            .iter()
            .map(|resource| resource.id.clone())
            .collect(),
        elapsed_ns: world
            .resource::<Time<Fixed>>()
            .elapsed()
            .as_nanos()
            .try_into()?,
        tick: world.resource::<simulation::SimulationCounters>().ticks,
        groups: Vec::new(),
        accounts: Vec::new(),
        ships: Vec::new(),
        gates: Vec::new(),
        tracks: Vec::new(),
        programs: BTreeMap::new(),
        projectiles: Vec::new(),
    };
    let mut identities = world
        .resource::<identity::IdentityIndex>()
        .0
        .iter()
        .collect::<Vec<_>>();
    identities.sort_unstable_by_key(|(id, _)| **id);
    for (&stable_id, &entity) in identities {
        if let Some(group) = world.get::<intelligence::Group>(entity) {
            record.groups.push(GroupRecord {
                id: stable_id,
                key: group.key,
            });
        }
        if let Some(account) = world.get::<identity::Account>(entity) {
            record.accounts.push(AccountRecord {
                id: stable_id,
                group: id(world, account.group)?,
            });
        }
        if let Some(design) = world.get::<vessel::ShipDesign>(entity) {
            let mut blueprint = design.0.blueprint.clone();
            blueprint.firmware = toy_sim_ships::Firmware::Standard;
            let controller = world
                .get::<vessel::ShipSoftware>(entity)
                .map(|software| software.controller.checkpoint());
            let program_bytes = controller.as_ref().map_or_else(
                || design.0.blueprint.controller_bytes().to_vec(),
                |checkpoint| checkpoint.program.clone(),
            );
            let program = *blake3::hash(&program_bytes).as_bytes();
            record.programs.entry(program).or_insert(program_bytes);
            let software =
                world
                    .get::<vessel::ShipSoftware>(entity)
                    .map(|software| SoftwareRecord {
                        persistent_data: controller.as_ref().unwrap().persistent_data.clone(),
                        request_id: software.request_id,
                        hull_energy_j: software.hull_energy_j,
                        shield_energy_j: software.shield_energy_j,
                    });
            record.ships.push(ShipRecord {
                id: stable_id,
                spatial_instance: world
                    .get::<identity::SpatialInstance>(entity)
                    .map(|instance| instance.0),
                name: world
                    .get::<vessel::Vessel>(entity)
                    .context("ship has no vessel")?
                    .vessel_name
                    .to_string(),
                blueprint: toml::to_string(&blueprint)?,
                program,
                pose: pose(world, entity)?,
                hardware: hardware::snapshot(world, entity)
                    .context("ship hardware unavailable during checkpoint")?,
                parts: world
                    .get::<hardware::PartDevices>(entity)
                    .into_iter()
                    .flat_map(|parts| parts.0.iter().enumerate())
                    .filter_map(|(index, &part)| {
                        let reactor =
                            world
                                .get::<hardware::reactors::Reactor>(part)
                                .map(|reactor| {
                                    (
                                        reactor.core_energy_j,
                                        reactor.decay_energy_j,
                                        reactor.shutdown,
                                    )
                                });
                        let thermal_engine_decay_j = world
                            .get::<hardware::devices::ThermalEngine>(part)
                            .map(|engine| engine.decay_energy_j);
                        (reactor.is_some() || thermal_engine_decay_j.is_some()).then_some(
                            PartRecord {
                                index,
                                reactor,
                                thermal_engine_decay_j,
                            },
                        )
                    })
                    .collect(),
                software,
                control: control(world, entity),
                iff: world
                    .get::<identity::Transponder>(entity)
                    .map(|iff| iff.0.clone()),
                group: world
                    .get::<identity::Membership>(entity)
                    .map(|membership| id(world, membership.0))
                    .transpose()?,
                travel: world
                    .get::<travel::Travel>(entity)
                    .map(|travel| travel.0.clone())
                    .unwrap_or_default(),
                presence: world
                    .get::<travel::PresenceState>(entity)
                    .map(|presence| presence.0.clone())
                    .unwrap_or(Presence::Space),
                stored_mass: world
                    .get::<travel::StoredMass>(entity)
                    .map_or(0.0, |mass| mass.0),
                dormant_thermal_s: world
                    .get::<hardware::DormantThermalElapsed>(entity)
                    .map_or(0.0, |elapsed| elapsed.0),
                beacon: world.get::<identity::BeaconEmitter>(entity).is_some(),
                fixed: world.get::<identity::FixedBeacon>(entity).is_some(),
                controlled: world.get::<vessel::ControlledVessel>(entity).is_some(),
                bays: world
                    .get::<travel::DockingBays>(entity)
                    .map(|bays| bays.0.clone()),
                drive: world.get::<travel::SlipDrive>(entity).cloned(),
                transit: world.get::<travel::Transit>(entity).cloned(),
                landmark: world.get::<infrastructure::Landmark>(entity).cloned(),
                crew: world
                    .get::<hardware::utilities::Crew>(entity)
                    .map(|crew| (crew.people, crew.capacity, crew.support_fraction)),
                sensor_override: world
                    .get::<hardware::SensorOverride>(entity)
                    .map(|sensor| (sensor.range_m, sensor.occlusion)),
                owner: *world
                    .get::<ownership::AssetOwner>(entity)
                    .context("ship owner unavailable")?,
                access: world
                    .get::<ownership::AssetAccess>(entity)
                    .cloned()
                    .unwrap_or_default(),
                dock_services: world
                    .get::<hardware::utilities::DockServiceRequest>(entity)
                    .map_or((false, false), |request| (request.cargo, request.power)),
            });
        } else if let Some(gate) = world.get::<travel::Gate>(entity) {
            record.gates.push(GateRecord {
                id: stable_id,
                pose: pose(world, entity)?,
                gate: gate.clone(),
                orbit: world.get::<infrastructure::GateOrbit>(entity).cloned(),
                landmark: world.get::<infrastructure::Landmark>(entity).cloned(),
                control: control(world, entity),
                iff: world
                    .get::<identity::Transponder>(entity)
                    .map(|iff| iff.0.clone()),
                radius_m: world
                    .get::<spatial::SpatialBody>(entity)
                    .map_or(gate.radius_m, |body| body.radius_m),
                owner: *world
                    .get::<ownership::AssetOwner>(entity)
                    .context("gate owner unavailable")?,
                access: world
                    .get::<ownership::AssetAccess>(entity)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
    }
    for (&(group, physical), &entity) in &world.resource::<intelligence::AssociationIndex>().0 {
        if let Some(track) = world.get::<intelligence::TrackEstimate>(entity) {
            record.tracks.push(TrackRecord {
                group: id(world, group)?,
                physical,
                track: track.0.clone(),
            });
        }
    }
    record
        .tracks
        .sort_by_key(|track| (track.group, track.physical));
    for entity in world.iter_entities() {
        if let Some(projectile) = entity.get::<physics::collision::Projectile>() {
            let mass = entity
                .get::<physics::MassProps>()
                .context("projectile mass unavailable")?;
            record.projectiles.push(ProjectileRecord {
                owner: projectile.launch_owner.and_then(|owner| {
                    world
                        .get::<identity::Identity>(owner)
                        .map(|identity| identity.0)
                }),
                pose: pose(world, entity.id())?,
                remaining_s: projectile.remaining_s,
                radius_m: projectile.radius_m,
                hull_hp: projectile.hull_hp,
                thermal: projectile.thermal,
                mass_kg: mass.mass,
                inertia: mass.inertia.to_cols_array(),
            });
        }
    }
    Ok(postcard::to_stdvec(&record)?)
}

fn validate(world: &mut World, record: &WorldRecord) -> Result<()> {
    use std::collections::BTreeSet;
    use toy_sim_model::{PUBLIC_GROUP, ownership::Principal};

    ensure!(record.directory.valid(), "invalid ownership directory");
    ensure!(
        record.rate.is_finite() && (0.0..=100.0).contains(&record.rate),
        "invalid saved clock rate"
    );
    let mut identities = BTreeSet::new();
    for value in record
        .groups
        .iter()
        .map(|record| record.id)
        .chain(record.accounts.iter().map(|record| record.id))
        .chain(record.ships.iter().map(|record| record.id))
        .chain(record.gates.iter().map(|record| record.id))
    {
        ensure!(identities.insert(value), "duplicate persistent identity");
    }
    let groups: BTreeSet<_> = record.groups.iter().map(|group| group.id).collect();
    let accounts: BTreeSet<_> = record.accounts.iter().map(|account| account.id).collect();
    let ships: BTreeMap<_, _> = record.ships.iter().map(|ship| (ship.id, ship)).collect();
    let gates: BTreeMap<_, _> = record.gates.iter().map(|gate| (gate.id, gate)).collect();
    ensure!(
        groups.contains(&PUBLIC_GROUP),
        "public information group unavailable"
    );
    let mut keys = BTreeSet::new();
    for group in &record.groups {
        ensure!(
            (group.id == PUBLIC_GROUP) == group.key.is_none(),
            "invalid information group key"
        );
        if let Some(key) = group.key {
            ensure!(keys.insert(key.0), "duplicate information group secret");
        }
    }
    for account in &record.accounts {
        ensure!(groups.contains(&account.group), "account group unavailable");
        ensure!(
            record.directory.players.contains_key(&account.id),
            "account affiliation unavailable"
        );
    }
    let access_valid = |owner: &ownership::AssetOwner, access: &ownership::AssetAccess| {
        record.directory.contains(owner.0)
            && access.0.valid()
            && access
                .0
                .grants
                .iter()
                .all(|grant| record.directory.contains(grant.principal))
    };
    let control_valid = |control: &Option<ControlRecord>| {
        control
            .as_ref()
            .is_none_or(|control| accounts.contains(&control.account))
    };
    let iff_valid = |iff: &Option<IffIdentity>| {
        iff.as_ref().is_none_or(|iff| {
            record.directory.contains(Principal::Player(iff.owner))
                && iff
                    .faction
                    .is_none_or(|id| record.directory.organizations.contains_key(&id))
                && iff.range_m.is_finite()
                && iff.range_m >= 0.
        })
    };
    for (hash, program) in &record.programs {
        ensure!(
            *blake3::hash(program).as_bytes() == *hash,
            "saved program checksum mismatch"
        );
        world
            .resource_mut::<vessel::WasmRuntime>()
            .0
            .validate_program(program)?;
    }
    let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
    for ship in &record.ships {
        ensure!(valid_pose(&ship.pose), "invalid saved ship pose");
        ensure!(
            access_valid(&ship.owner, &ship.access),
            "invalid ship access policy"
        );
        ensure!(
            control_valid(&ship.control) && iff_valid(&ship.iff),
            "ship identity reference unavailable"
        );
        ensure!(
            ship.group.is_none_or(|group| groups.contains(&group)),
            "ship group unavailable"
        );
        ensure!(
            ship.stored_mass.is_finite() && ship.stored_mass >= 0.,
            "invalid contained mass"
        );
        ensure!(
            ship.dormant_thermal_s.is_finite() && (0.0..1.0).contains(&ship.dormant_thermal_s),
            "invalid dormant thermal clock"
        );
        let blueprint: toy_sim_ships::ShipBlueprint = toml::from_str(&ship.blueprint)?;
        let design = blueprint.compile(catalogue)?;
        let hardware = &ship.hardware;
        ensure!(
            hardware.inventory.quantities.len() == catalogue.resources.len()
                && hardware.inventory.cargo.len() == catalogue.resources.len()
                && hardware.inventory.tank_capacities_m3.len() == catalogue.resources.len()
                && hardware.devices.len() == design.parts.len()
                && hardware.weapons.len() == design.weapon_parts.len()
                && hardware.settings.len() == design.device_catalogue.len(),
            "saved hardware shape differs from design"
        );
        ensure!(
            hardware
                .settings
                .iter()
                .flatten()
                .all(toy_sim_ships::DeviceSetting::finite)
                && hardware
                    .inventory
                    .tank_capacities_m3
                    .iter()
                    .all(|value| value.is_finite() && *value >= 0.)
                && hardware.inventory.energy_j <= design.battery_j
                && hardware.hull.is_finite()
                && hardware.hull >= 0.
                && hardware.thermal.hull_energy_j.is_finite()
                && hardware.thermal.hull_energy_j >= 0.
                && hardware.thermal.shield_energy_j.is_finite()
                && hardware.thermal.shield_energy_j >= 0.,
            "invalid saved hardware values"
        );
        let mut thermal_indices = BTreeSet::new();
        for part in &ship.parts {
            ensure!(
                part.index < design.parts.len() && thermal_indices.insert(part.index),
                "invalid thermal part index"
            );
            let equipment = &design.parts[part.index].definition.equipment;
            if let Some((core, decay, _)) = part.reactor {
                ensure!(
                    matches!(equipment, toy_sim_ships::Equipment::Reactor { .. })
                        && core.is_finite()
                        && core >= 0.
                        && decay.is_finite()
                        && decay >= 0.,
                    "invalid saved reactor state"
                );
            }
            if let Some(decay) = part.thermal_engine_decay_j {
                ensure!(
                    matches!(equipment, toy_sim_ships::Equipment::ThermalEngine { .. })
                        && decay.is_finite()
                        && decay >= 0.,
                    "invalid saved engine decay heat"
                );
            }
        }
        ensure!(
            record.programs.contains_key(&ship.program),
            "saved ship program unavailable"
        );
        if let Some(software) = &ship.software {
            ensure!(
                record.programs.contains_key(&ship.program),
                "saved program unavailable"
            );
            ensure!(
                software.persistent_data.len() <= 65536,
                "saved program data exceeds limit"
            );
        }
        ensure!(
            ship.transit.is_some() == matches!(ship.presence, Presence::SlipTransit(_)),
            "slip transit state is inconsistent"
        );
        if let Some(transit) = &ship.transit {
            ensure!(
                transit.departed <= transit.next_attempt,
                "invalid transit clock"
            );
        }
        if let Some(drive) = &ship.drive {
            ensure!(
                drive.power_w.is_finite() && drive.power_w >= 0.,
                "invalid slip power"
            );
            if let Some(preparation) = &drive.preparation {
                ensure!(
                    preparation.mass.is_finite()
                        && preparation.mass > 0.
                        && preparation.work_j.is_finite()
                        && preparation.work_j >= 0.
                        && preparation.required_j.is_finite()
                        && preparation.required_j >= preparation.work_j,
                    "invalid slip preparation"
                );
            }
        }
        if let Some(bays) = &ship.bays {
            ensure!(
                bays.iter().all(|bay| bay.radius_m.is_finite()
                    && bay.radius_m > 0.
                    && bay.mass_capacity_kg.is_finite()
                    && bay.mass_capacity_kg > 0.
                    && bay
                        .centre_m
                        .iter()
                        .chain(&bay.rotation)
                        .all(|value| value.is_finite())),
                "invalid docking bay"
            );
        }
        let mut chain = BTreeSet::from([ship.id]);
        let mut current = ship;
        loop {
            let host = match current.presence {
                Presence::Docked { host, bay } => {
                    let station = ships.get(&host).context("docked host unavailable")?;
                    ensure!(
                        station
                            .bays
                            .as_ref()
                            .is_some_and(|bays| (bay as usize) < bays.len()),
                        "docked bay unavailable"
                    );
                    host
                }
                Presence::StoredInWreck(host) => host,
                _ => break,
            };
            ensure!(
                chain.insert(host) && chain.len() <= 9,
                "invalid containment cycle or depth"
            );
            current = ships.get(&host).context("containment host unavailable")?;
        }
    }
    for gate in &record.gates {
        ensure!(
            gates
                .get(&gate.gate.paired)
                .is_some_and(|paired| paired.gate.paired == gate.id),
            "gate pair unavailable or inconsistent"
        );
        ensure!(
            valid_pose(&gate.pose)
                && access_valid(&gate.owner, &gate.access)
                && control_valid(&gate.control)
                && iff_valid(&gate.iff),
            "invalid saved gate"
        );
    }
    let mut tracks = BTreeSet::new();
    for track in &record.tracks {
        ensure!(
            groups.contains(&track.group)
                && tracks.insert((track.group, track.physical))
                && valid_pose(&track.track.pose),
            "invalid saved sensor association"
        );
    }
    for projectile in &record.projectiles {
        ensure!(
            projectile
                .owner
                .is_none_or(|owner| ships.contains_key(&owner)),
            "projectile launch owner unavailable"
        );
        ensure!(
            valid_pose(&projectile.pose)
                && projectile.mass_kg.is_finite()
                && projectile.mass_kg > 0.
                && projectile.remaining_s.is_finite()
                && projectile.remaining_s > 0.
                && projectile.radius_m.is_finite()
                && projectile.radius_m > 0.,
            "invalid saved projectile"
        );
    }
    Ok(())
}

fn valid_pose(pose: &Pose) -> bool {
    pose.velocity
        .iter()
        .chain(&pose.angular_velocity)
        .chain(&pose.rotation)
        .all(|value| value.is_finite())
        && (pose.rotation.iter().map(|value| value * value).sum::<f64>() - 1.).abs() < 1e-5
}

pub fn restore(world: &mut World, bytes: &[u8]) -> Result<()> {
    let record: WorldRecord = postcard::from_bytes(bytes)?;
    ensure!(
        record.catalogue == world.resource::<registry::UniverseRegistry>().catalogue,
        "world catalogue differs from saved universe; explicitly start a new database for a different universe"
    );
    let resources = world
        .resource::<vessel::ShipCatalogue>()
        .0
        .resources
        .iter()
        .map(|resource| resource.id.clone())
        .collect::<Vec<_>>();
    ensure!(
        record.resource_ids == resources,
        "resource catalogue differs from snapshot"
    );
    validate(world, &record)?;
    let config = world.resource::<crate::sim::ScenarioConfig>().clone();
    let entities = world
        .query_filtered::<Entity, Without<bevy::ecs::resource::IsResource>>()
        .iter(world)
        .collect::<Vec<_>>();
    for entity in entities {
        if world.get_entity(entity).is_ok() {
            world.despawn(entity);
        }
    }
    world.resource_mut::<identity::IdentityIndex>().0.clear();
    world.resource_mut::<identity::GroupIndex>().0.clear();
    world
        .resource_mut::<intelligence::AssociationIndex>()
        .0
        .clear();
    *world.resource_mut::<orrery::activity::ActiveSystems>() = default();
    world
        .resource_mut::<crate::sim::session::Events>()
        .0
        .clear();
    world.resource_mut::<travel::TravelEvents>().0.clear();
    world.insert_resource(identity::WorldEpoch(record.epoch));
    world.insert_resource(ownership::Directory(record.directory));
    world.insert_resource(crate::sim::session::Clock {
        rate: record.rate,
        ..Default::default()
    });
    world.remove_resource::<physics::collision::CollisionReport>();
    world.insert_resource(crate::sim::services::PublishedWorld::default());
    world.insert_resource(crate::sim::combat::CombatHistory::default());
    world.insert_resource(spatial::SpatialIndex::default());
    world.insert_resource(identity::SensorSeed(record.sensor_seed));
    world.resource_mut::<simulation::SimulationCounters>().ticks = record.tick;
    let elapsed = Duration::from_nanos(record.elapsed_ns);
    let mut fixed = Time::<Fixed>::from_hz(10.0);
    fixed.advance_to(elapsed);
    world.insert_resource(fixed);
    let mut virtual_time = Time::<Virtual>::default();
    virtual_time.advance_to(elapsed);
    world.insert_resource(virtual_time);
    let mut time = Time::<()>::default();
    time.advance_to(elapsed);
    world.insert_resource(time);
    for group in record.groups {
        let entity = world
            .spawn((
                intelligence::Group {
                    id: group.id,
                    key: group.key,
                    snapshot: Arc::default(),
                },
                intelligence::Measurements::default(),
                intelligence::GroupTracks::default(),
            ))
            .id();
        identity::register(world, entity, group.id);
        if let Some(key) = group.key {
            world
                .resource_mut::<identity::GroupIndex>()
                .0
                .insert(key, entity);
        }
    }
    for account in record.accounts {
        let group = identity::lookup(world, account.group)?;
        let entity = world
            .spawn((
                identity::Account {
                    group,
                    debug: config.debug_account == Some(account.id),
                },
                identity::OwnedShips::default(),
            ))
            .id();
        identity::register(world, entity, account.id);
    }
    for &account in &config.accounts {
        identity::add_account(world, account, config.debug_account == Some(account));
    }
    let mut relationships = Vec::new();
    let mut drives = Vec::new();
    let mut thermal_parts = Vec::new();
    for mut ship in record.ships {
        let mut blueprint: toy_sim_ships::ShipBlueprint =
            toml::from_str(&ship.blueprint).context("decode saved ship blueprint")?;
        let checkpoint = ship
            .software
            .as_ref()
            .map(|software| -> Result<_> {
                let program = record
                    .programs
                    .get(&ship.program)
                    .context("saved program unavailable")?
                    .clone();
                ensure!(
                    *blake3::hash(&program).as_bytes() == ship.program,
                    "saved program checksum mismatch"
                );
                Ok(toy_sim_ship_wasm::ControllerCheckpoint {
                    program,
                    persistent_data: software.persistent_data.clone(),
                })
            })
            .transpose()?;
        blueprint.firmware =
            toy_sim_ships::Firmware::Custom(record.programs[&ship.program].clone());
        let design = Arc::new(blueprint.compile(&world.resource::<vessel::ShipCatalogue>().0)?);
        let mut controller = if let Some(checkpoint) = &checkpoint {
            world
                .resource_mut::<vessel::WasmRuntime>()
                .0
                .restore(checkpoint)?
        } else {
            world
                .resource_mut::<vessel::WasmRuntime>()
                .0
                .instantiate(design.blueprint.controller_bytes())?
        };
        controller.configure_hardware(&design, &world.resource::<vessel::ShipCatalogue>().0);
        let mut software = vessel::ShipSoftware::new(controller);
        if let Some(saved) = &ship.software {
            software.request_id = saved.request_id;
            software.hull_energy_j = saved.hull_energy_j;
            software.shield_energy_j = saved.shield_energy_j;
        }
        if ship.software.is_some() {
            ship.hardware.reset_commands(&design);
            ship.travel.estimated_arrival_tick = None;
            ship.travel.fuel_budget = None;
        }
        if let Some(bays) = &mut ship.bays {
            for bay in bays {
                bay.reservation = None;
            }
        }
        let (own_mass, own_inertia) = ship
            .hardware
            .mass_properties(&design, &world.resource::<vessel::ShipCatalogue>().0);
        let mass = own_mass + ship.stored_mass;
        let inertia = own_inertia * (mass / own_mass);
        let entity = world
            .spawn(vessel::ship_bundle(
                design.clone(),
                ship.hardware,
                software,
                precision::PreciseTransform {
                    translation_um: ship.pose.position,
                    rotation: DQuat::from_array(ship.pose.rotation),
                },
                DVec3::from_array(ship.pose.velocity),
                ship.name,
                physics::MassProps {
                    mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
            ))
            .id();
        if let Some(control) = ship.control {
            identity::attach_ship(world, entity, control.account)?;
            let temporary = id(world, entity)?;
            world
                .resource_mut::<identity::IdentityIndex>()
                .0
                .remove(&temporary);
            world.get_mut::<identity::Control>(entity).unwrap().revision = control.revision;
        }
        identity::register(world, entity, ship.id);
        if ship.software.is_none() {
            world.entity_mut(entity).remove::<vessel::ShipSoftware>();
        }
        world.entity_mut(entity).insert((
            physics::AngularVelocity(DVec3::from_array(ship.pose.angular_velocity)),
            travel::Travel(ship.travel),
            travel::StoredMass(ship.stored_mass),
        ));
        if let Some(iff) = ship.iff {
            world.entity_mut(entity).insert(identity::Transponder(iff));
        }
        if let Some(group) = ship.group {
            let group = identity::lookup(world, group)?;
            world.entity_mut(entity).insert(identity::Membership(group));
        }
        if ship.beacon {
            world.entity_mut(entity).insert(identity::BeaconEmitter);
        }
        if ship.fixed {
            world.entity_mut(entity).insert(identity::FixedBeacon);
        }
        if ship.controlled {
            world.entity_mut(entity).insert(vessel::ControlledVessel);
        }
        if let Some(bays) = ship.bays {
            world.entity_mut(entity).insert(travel::DockingBays(bays));
        }
        if let Some(transit) = ship.transit {
            world.entity_mut(entity).insert(transit);
        }
        if let Some(landmark) = ship.landmark {
            world.entity_mut(entity).insert(landmark);
        }
        if let Some((people, capacity, support_fraction)) = ship.crew {
            world.entity_mut(entity).insert(hardware::utilities::Crew {
                people,
                capacity,
                support_fraction,
            });
        }
        if let Some((range_m, occlusion)) = ship.sensor_override {
            world
                .entity_mut(entity)
                .insert(hardware::SensorOverride { range_m, occlusion });
        }
        world.entity_mut(entity).insert((
            ship.owner,
            ship.access,
            hardware::utilities::DockServiceRequest {
                cargo: ship.dock_services.0,
                power: ship.dock_services.1,
            },
        ));
        relationships.push((
            entity,
            ship.presence,
            ship.spatial_instance,
            ship.dormant_thermal_s,
        ));
        drives.push((entity, ship.drive));
        thermal_parts.push((entity, ship.parts));
    }
    let mut initialize = Schedule::default();
    initialize.add_systems(hardware::initialize);
    initialize.run(world);
    for (ship, parts) in thermal_parts {
        for part in parts {
            let entity = world.get::<hardware::PartDevices>(ship).unwrap().0[part.index];
            if let Some((core_energy_j, decay_energy_j, shutdown)) = part.reactor {
                let mut reactor = world
                    .get_mut::<hardware::reactors::Reactor>(entity)
                    .context("saved reactor component unavailable")?;
                reactor.core_energy_j = core_energy_j;
                reactor.decay_energy_j = decay_energy_j;
                reactor.shutdown = shutdown;
            }
            if let Some(decay_energy_j) = part.thermal_engine_decay_j {
                world
                    .get_mut::<hardware::devices::ThermalEngine>(entity)
                    .context("saved thermal engine component unavailable")?
                    .decay_energy_j = decay_energy_j;
            }
        }
    }
    for (entity, drive) in drives {
        if let Some(drive) = drive {
            world.entity_mut(entity).insert(drive);
        }
    }
    for (entity, presence, instance, dormant_thermal_s) in relationships {
        if let Presence::Docked { host, .. } | Presence::StoredInWreck(host) = presence {
            let host = identity::lookup(world, host)?;
            world.entity_mut(entity).insert(travel::DockedIn(host));
        }
        if presence != Presence::Space {
            travel::set_dormant(world, entity, presence);
        }
        world
            .entity_mut(entity)
            .insert(hardware::DormantThermalElapsed(dormant_thermal_s));
        if let Some(instance) = instance {
            world
                .entity_mut(entity)
                .insert(identity::SpatialInstance(instance));
        }
    }
    for gate in record.gates {
        let entity = world
            .spawn((
                precision::PreciseTransform {
                    translation_um: gate.pose.position,
                    rotation: DQuat::from_array(gate.pose.rotation),
                },
                physics::Velocity(DVec3::from_array(gate.pose.velocity)),
                identity::BeaconEmitter,
                identity::FixedBeacon,
                spatial::SpatialBody {
                    radius_m: gate.radius_m,
                    occludes: false,
                },
                gate.gate,
                gate.owner,
                gate.access,
            ))
            .id();
        identity::register(world, entity, gate.id);
        if let Some(control) = gate.control {
            world.entity_mut(entity).insert(identity::Control {
                account: control.account,
                revision: control.revision,
            });
        }
        if let Some(iff) = gate.iff {
            world.entity_mut(entity).insert(identity::Transponder(iff));
        }
        if let Some(landmark) = gate.landmark {
            world.entity_mut(entity).insert(landmark);
        }
        if let Some(orbit) = gate.orbit {
            world.entity_mut(entity).insert(orbit);
        }
    }
    for projectile in record.projectiles {
        let inertia = bevy::math::DMat3::from_cols_array(&projectile.inertia);
        let launch_owner = projectile
            .owner
            .map(|owner| identity::lookup(world, owner))
            .transpose()?;
        world.spawn((
            physics::collision::Projectile {
                launch_owner,
                remaining_s: projectile.remaining_s,
                radius_m: projectile.radius_m,
                hull_hp: projectile.hull_hp,
                thermal: projectile.thermal,
            },
            precision::PreciseTransform {
                translation_um: projectile.pose.position,
                rotation: DQuat::from_array(projectile.pose.rotation),
            },
            physics::Velocity(DVec3::from_array(projectile.pose.velocity)),
            physics::AngularVelocity(DVec3::from_array(projectile.pose.angular_velocity)),
            physics::MassProps {
                mass: projectile.mass_kg,
                inertia,
                inertia_inv: inertia.inverse(),
            },
        ));
    }
    for track in record.tracks {
        let group = identity::lookup(world, track.group)?;
        let entity = world
            .spawn((
                intelligence::TrackEstimate(track.track),
                intelligence::TrackGroup(group),
                intelligence::TrackAssociation(track.physical),
            ))
            .id();
        world
            .resource_mut::<intelligence::AssociationIndex>()
            .0
            .insert((group, track.physical), entity);
    }
    let mut activate = Schedule::default();
    activate.add_systems((orrery::activity::activate, identity::identify_celestials).chain());
    activate.run(world);
    let mut publish = Schedule::default();
    publish.add_systems(intelligence::publish);
    publish.run(world);
    travel::geometry::refresh(world);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::travel::{Order, QueuedOrder, Status};

    #[test]
    fn corrupt_references_are_rejected_before_replacing_the_world() {
        let account = Id::new();
        let mut app = crate::scenario(&[account], Some(account), None).unwrap();
        for _ in 0..3 {
            app.update();
        }
        let world = app.world_mut();
        let bytes = capture(world).unwrap();
        let epoch = world.resource::<identity::WorldEpoch>().0;
        let identities = world.resource::<identity::IdentityIndex>().0.clone();

        let mut missing_host: WorldRecord = postcard::from_bytes(&bytes).unwrap();
        missing_host.ships[0].presence = Presence::Docked {
            host: Id::new(),
            bay: 0,
        };
        let mut broken_pair: WorldRecord = postcard::from_bytes(&bytes).unwrap();
        broken_pair.gates[0].gate.paired = Id::new();
        let mut corrupt_program: WorldRecord = postcard::from_bytes(&bytes).unwrap();
        corrupt_program.programs.values_mut().next().unwrap()[0] ^= 1;

        for invalid in [missing_host, broken_pair, corrupt_program] {
            let invalid = postcard::to_stdvec(&invalid).unwrap();
            assert!(restore(world, &invalid).is_err());
            assert_eq!(world.resource::<identity::WorldEpoch>().0, epoch);
            assert_eq!(world.resource::<identity::IdentityIndex>().0, identities);
            assert!(
                identities
                    .values()
                    .all(|entity| world.get_entity(*entity).is_ok())
            );
        }
    }

    #[test]
    fn restored_slip_arrives_and_resumes_the_saved_order_queue() {
        let account = Id::new();
        let mut app = crate::scenario(&[account], Some(account), None).unwrap();
        for _ in 0..3 {
            app.update();
        }
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let ship_id = id(world, ship).unwrap();
        let departure = world
            .get::<precision::PreciseTransform>(ship)
            .unwrap()
            .translation_um;
        let destination = departure.offset_by(DVec3::X * 1e12);
        let tick = world.resource::<simulation::SimulationCounters>().ticks;
        let orders = vec![
            QueuedOrder::from(Order::Slip { destination }),
            QueuedOrder::from(Order::WaitUntil(tick + 1000)),
        ];
        world.entity_mut(ship).insert(travel::Travel(TravelState {
            orders: orders.clone(),
            autopilot_enabled: true,
            revision: 31,
            status: Status::Active,
            ..Default::default()
        }));
        travel::set_dormant(world, ship, Presence::SlipTransit(Id::new()));
        world.entity_mut(ship).insert(travel::Transit {
            origin: departure,
            destination,
            departed: tick,
            next_attempt: tick,
        });
        let bytes = capture(world).unwrap();
        restore(world, &bytes).unwrap();

        for _ in 0..3 {
            app.update();
        }
        let world = app.world_mut();
        let ship = identity::lookup(world, ship_id).unwrap();
        assert_eq!(
            world.get::<travel::PresenceState>(ship).unwrap().0,
            Presence::Space
        );
        assert!(world.get::<travel::Transit>(ship).is_none());
        assert!(world.get::<physics::Velocity>(ship).unwrap().0.is_finite());
        assert!(
            pose(world, ship)
                .unwrap()
                .position
                .relative_to(destination)
                .length()
                < 10000.
        );
        let travel = &world.get::<travel::Travel>(ship).unwrap().0;
        assert_eq!(travel.orders, orders);
        assert_eq!(travel.order, 1);
        assert_eq!(travel.revision, 31);
        assert!(travel.autopilot_enabled);
        assert!(world.resource::<simulation::SimulationCounters>().ticks > tick);
    }

    #[test]
    fn snapshot_restores_hardware_docking_transit_iff_and_programs_by_uuid() {
        let account = Id::new();
        let mut app = crate::scenario(&[account], Some(account), None).unwrap();
        for _ in 0..3 {
            app.update();
        }
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let ship_id = id(world, ship).unwrap();
        let station = world
            .query_filtered::<Entity, With<travel::DockingBays>>()
            .iter(world)
            .next()
            .unwrap();
        let station_id = id(world, station).unwrap();
        let transit_ship = world
            .query_filtered::<Entity, With<vessel::ShipSoftware>>()
            .iter(world)
            .find(|entity| *entity != ship && *entity != station)
            .unwrap();
        let transit_id = id(world, transit_ship).unwrap();
        world
            .get_mut::<hardware::ShipInventory>(ship)
            .unwrap()
            .0
            .energy_j = 12345;
        world.get_mut::<hardware::Hull>(ship).unwrap().0 = 123.0;
        world
            .get_mut::<hardware::ShipThermal>(ship)
            .unwrap()
            .0
            .hull_energy_j = 1234.0;
        world
            .get_mut::<identity::Transponder>(ship)
            .unwrap()
            .0
            .labels
            .insert("Preserved identity".into());
        let travel = TravelState {
            autopilot_enabled: true,
            orders: vec![QueuedOrder::from(Order::WaitUntil(1230))],
            status: Status::Planning,
            ..Default::default()
        };
        world
            .entity_mut(ship)
            .insert(travel::Travel(travel.clone()));
        travel::set_dormant(
            world,
            ship,
            Presence::Docked {
                host: station_id,
                bay: 0,
            },
        );
        world.entity_mut(ship).insert(travel::DockedIn(station));
        let departure = world
            .get::<precision::PreciseTransform>(transit_ship)
            .unwrap()
            .translation_um;
        travel::set_dormant(world, transit_ship, Presence::SlipTransit(Id::new()));
        world.entity_mut(transit_ship).insert(travel::Transit {
            origin: departure,
            destination: departure.offset_by(DVec3::X * 1e12),
            departed: 1,
            next_attempt: 100_000,
        });
        let expected_motion = pose(world, transit_ship).unwrap();
        let program = world
            .get::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller
            .checkpoint()
            .program;
        let checkpoint = toy_sim_ship_wasm::ControllerCheckpoint {
            program: program.clone(),
            persistent_data: b"durable guest data".to_vec(),
        };
        let restored = world
            .resource_mut::<vessel::WasmRuntime>()
            .0
            .restore(&checkpoint)
            .unwrap();
        world
            .get_mut::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller = restored;
        let before_tick = world.resource::<simulation::SimulationCounters>().ticks;
        let bytes = capture(world).unwrap();
        let decoded: WorldRecord = postcard::from_bytes(&bytes).unwrap();
        assert!(
            decoded.programs.len() < decoded.ships.len(),
            "identical firmware should be stored once"
        );

        restore(world, &bytes).unwrap();

        let ship = identity::lookup(world, ship_id).unwrap();
        let station = identity::lookup(world, station_id).unwrap();
        let transit_ship = identity::lookup(world, transit_id).unwrap();
        assert_eq!(
            world
                .get::<hardware::ShipInventory>(ship)
                .unwrap()
                .0
                .energy_j,
            12345
        );
        assert_eq!(world.get::<hardware::Hull>(ship).unwrap().0, 123.0);
        assert_eq!(
            world
                .get::<hardware::ShipThermal>(ship)
                .unwrap()
                .0
                .hull_energy_j,
            1234.0
        );
        assert!(
            world
                .get::<identity::Transponder>(ship)
                .unwrap()
                .0
                .labels
                .contains("Preserved identity")
        );
        assert_eq!(world.get::<travel::DockedIn>(ship).unwrap().0, station);
        let restored_travel = &world.get::<travel::Travel>(ship).unwrap().0;
        assert_eq!(restored_travel.orders, travel.orders);
        assert_eq!(restored_travel.order, travel.order);
        assert_eq!(restored_travel.autopilot_enabled, travel.autopilot_enabled);
        assert_eq!(restored_travel.revision, travel.revision);
        assert!(world.get::<physics::Velocity>(ship).is_none());
        assert_eq!(pose(world, transit_ship).unwrap(), expected_motion);
        assert!(world.get::<travel::Transit>(transit_ship).is_some());
        assert_eq!(
            world.resource::<simulation::SimulationCounters>().ticks,
            before_tick
        );
        let controller = &world.get::<vessel::ShipSoftware>(ship).unwrap().controller;
        assert_eq!(controller.checkpoint().program, program);
        assert_eq!(
            controller.checkpoint().persistent_data,
            b"durable guest data"
        );
        assert!(controller.is_booting());

        travel::destroy(world, station);
        let wreck = capture(world).unwrap();
        restore(world, &wreck).unwrap();
        let ship = identity::lookup(world, ship_id).unwrap();
        let station = identity::lookup(world, station_id).unwrap();
        assert_eq!(
            world.get::<travel::PresenceState>(ship).unwrap().0,
            Presence::StoredInWreck(station_id)
        );
        assert_eq!(world.get::<travel::DockedIn>(ship).unwrap().0, station);
        assert!(
            world
                .get::<travel::StoredShips>(station)
                .unwrap()
                .iter()
                .any(|child| child == ship)
        );
    }
}
