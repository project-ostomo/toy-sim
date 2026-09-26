use crate::sim::{
    hardware, identity, industry, infrastructure, orrery, ownership, physics, precision, sensors,
    simulation, spatial, travel, vessel,
};
use anyhow::{Context, Result};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use osg_model::travel::{AutopilotState, FirmwarePhase, FirmwareStatus, Presence};
use osg_model::{Id, IffIdentity, Pose};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Serialize, Deserialize)]
struct WorldRecord {
    society: crate::sim::society::SocietyState,
    epoch: Id,
    rate: f64,
    slip_history: crate::sim::slip_effects::SlipHistory,
    elapsed_ns: u64,
    tick: u64,
    accounts: Vec<AccountRecord>,
    ships: Vec<ShipRecord>,
    programs: BTreeMap<[u8; 32], Vec<u8>>,
    projectiles: Vec<ProjectileRecord>,
}

#[derive(Serialize, Deserialize)]
struct AccountRecord {
    id: Id,
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
    hardware: osg_ships::ShipState,
    parts: Vec<PartRecord>,
    software: Option<SoftwareRecord>,
    industry: Option<industry::IndustrialFacilityRecord>,
    control: Option<ControlRecord>,
    iff: Option<IffIdentity>,
    travel: AutopilotState,
    presence: Presence,
    physical_body: bool,
    stored_mass: f64,
    dormant_thermal_s: f64,
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
struct ProjectileRecord {
    owner: Option<Id>,
    pose: Pose,
    remaining_s: f64,
    radius_m: f64,
    hull_hp: f64,
    thermal: osg_ships::thermal::ThermalState,
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
    let mut pose = identity::pose(
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
        society: world
            .resource::<crate::sim::society::SocietyState>()
            .clone(),
        epoch: world.resource::<identity::WorldEpoch>().0,
        rate: world.resource::<crate::sim::session::Clock>().rate,
        slip_history: world
            .get_resource::<crate::sim::slip_effects::SlipHistory>()
            .cloned()
            .unwrap_or_default(),
        elapsed_ns: world
            .resource::<Time<Fixed>>()
            .elapsed()
            .as_nanos()
            .try_into()?,
        tick: world.resource::<simulation::SimulationCounters>().ticks,
        accounts: Vec::new(),
        ships: Vec::new(),
        programs: BTreeMap::new(),
        projectiles: Vec::new(),
    };
    let mut blueprints = std::collections::HashMap::<usize, String>::new();
    let mut design_programs = std::collections::HashMap::<usize, [u8; 32]>::new();
    let mut identities = world
        .resource::<identity::IdentityIndex>()
        .entries()
        .iter()
        .collect::<Vec<_>>();
    identities.sort_unstable_by_key(|(id, _)| **id);
    for (&stable_id, &entity) in identities {
        if world.get::<identity::Account>(entity).is_some() {
            record.accounts.push(AccountRecord { id: stable_id });
        }
        if let Some(design) = world.get::<vessel::ShipDesign>(entity) {
            // Compiled designs are immutable and shared by installations. Cache
            // their encoding for this capture, without changing the save format.
            let design_key = std::sync::Arc::as_ptr(&design.0) as usize;
            if let std::collections::hash_map::Entry::Vacant(entry) = blueprints.entry(design_key) {
                let mut blueprint = design.0.blueprint.clone();
                blueprint.firmware = osg_ships::Firmware::Standard;
                entry.insert(toml::to_string(&blueprint)?);
            }
            let controller = world
                .get::<vessel::ShipSoftware>(entity)
                .map(|software| software.controller.checkpoint());
            let program = if let Some(controller) = &controller {
                let program = *blake3::hash(&controller.program).as_bytes();
                record
                    .programs
                    .entry(program)
                    .or_insert_with(|| controller.program.clone());
                program
            } else {
                *design_programs.entry(design_key).or_insert_with(|| {
                    let bytes = design.0.blueprint.controller_bytes();
                    let program = *blake3::hash(bytes).as_bytes();
                    record
                        .programs
                        .entry(program)
                        .or_insert_with(|| bytes.to_vec());
                    program
                })
            };
            let software = world
                .get::<vessel::ShipSoftware>(entity)
                .map(|_| SoftwareRecord {
                    persistent_data: controller.as_ref().unwrap().persistent_data.clone(),
                    request_id: world.get::<vessel::ShipMailbox>(entity).unwrap().request_id,
                    hull_energy_j: world
                        .get::<vessel::PendingDamage>(entity)
                        .unwrap()
                        .hull_energy_j,
                    shield_energy_j: world
                        .get::<vessel::PendingDamage>(entity)
                        .unwrap()
                        .shield_energy_j,
                });
            let hardware = hardware::snapshot(world, entity)
                .context("ship hardware unavailable during checkpoint")?;
            let facility = world
                .get::<industry::IndustrialFacility>(entity)
                .map(|facility| facility.to_record());
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
                blueprint: blueprints[&design_key].clone(),
                program,
                pose: pose(world, entity)?,
                hardware,
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
                industry: facility,
                control: control(world, entity),
                iff: world
                    .get::<identity::Transponder>(entity)
                    .map(|iff| iff.0.clone()),
                travel: world
                    .get::<travel::Travel>(entity)
                    .map(|travel| travel.0.clone())
                    .unwrap_or_default(),
                presence: world
                    .get::<travel::PresenceState>(entity)
                    .map(|presence| presence.0.clone())
                    .unwrap_or(Presence::Space),
                physical_body: world.get::<spatial::SpatialBody>(entity).is_some()
                    || world
                        .get::<travel::DormantMotion>(entity)
                        .is_some_and(travel::DormantMotion::has_physical_body),
                stored_mass: world
                    .get::<travel::StoredMass>(entity)
                    .map_or(0.0, |mass| mass.0),
                dormant_thermal_s: world
                    .get::<hardware::DormantThermalElapsed>(entity)
                    .map_or(0.0, |elapsed| elapsed.0),
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
        }
    }
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

pub fn restore(world: &mut World, bytes: &[u8]) -> Result<()> {
    let record: WorldRecord = postcard::from_bytes(bytes)?;
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
    *world.resource_mut::<orrery::activity::ActiveSystems>() = default();
    world.remove_resource::<infrastructure::NavigationPublication>();
    world
        .resource_mut::<crate::sim::session::Events>()
        .0
        .clear();
    world.resource_mut::<travel::TravelEvents>().0.clear();
    world.insert_resource(record.slip_history);
    world.insert_resource(identity::WorldEpoch(record.epoch));
    world.insert_resource(record.society);
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .economy
        .settle(osg_model::calendar::now_unix_ms());
    world.insert_resource(crate::sim::society::AssetRecords::default());
    world.insert_resource(crate::sim::session::Clock {
        rate: record.rate,
        ..Default::default()
    });
    physics::collision::reset(world);
    world.insert_resource(crate::sim::services::PublishedWorld::default());
    world.insert_resource(crate::sim::combat::CombatHistory::default());
    world.insert_resource(spatial::SpatialIndex::default());
    world.insert_resource(crate::sim::sensors::SensorService::default());
    world.resource_mut::<simulation::SimulationCounters>().ticks = record.tick;
    let elapsed = Duration::from_nanos(record.elapsed_ns);
    let mut fixed = Time::<Fixed>::from_duration(osg_model::TICK_DURATION);
    fixed.advance_to(elapsed);
    world.insert_resource(fixed);
    let mut virtual_time = Time::<Virtual>::default();
    virtual_time.advance_to(elapsed);
    world.insert_resource(virtual_time);
    let mut time = Time::<()>::default();
    time.advance_to(elapsed);
    world.insert_resource(time);
    for account in record.accounts {
        let entity = world
            .spawn((
                identity::Account {
                    debug: config.debug_account == Some(account.id),
                },
                identity::OwnedShips::default(),
            ))
            .id();
        identity::register(world, entity, account.id)?;
    }
    for &account in &config.accounts {
        identity::add_account(world, account, config.debug_account == Some(account));
    }
    let mut relationships = Vec::new();
    let mut drives = Vec::new();
    let mut thermal_parts = Vec::new();
    let mut designs = BTreeMap::new();
    for mut ship in record.ships {
        let design = match designs.entry((ship.blueprint.clone(), ship.program)) {
            std::collections::btree_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let mut blueprint: osg_ships::ShipBlueprint =
                    toml::from_str(&ship.blueprint).context("decode saved ship blueprint")?;
                blueprint.firmware =
                    osg_ships::Firmware::Custom(record.programs[&ship.program].clone());
                let design =
                    Arc::new(blueprint.compile(&world.resource::<vessel::ShipCatalogue>().0)?);
                entry.insert(design.clone());
                design
            }
        };
        let checkpoint = ship
            .software
            .as_ref()
            .map(|software| -> Result<_> {
                let program = record
                    .programs
                    .get(&ship.program)
                    .context("saved program unavailable")?
                    .clone();
                Ok(osg_ship_wasm::ControllerCheckpoint {
                    program,
                    persistent_data: software.persistent_data.clone(),
                })
            })
            .transpose()?;
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
        let software = vessel::ShipSoftware::new(controller);
        if ship.software.is_some() {
            ship.hardware.reset_commands(&design);
        }
        if ship.travel.failure.is_some() {
            ship.travel.enabled = false;
        }
        ship.travel.status = FirmwareStatus {
            spent_loss_ppm: ship.travel.status.spent_loss_ppm,
            spent_exotic_fuel_kg: ship.travel.status.spent_exotic_fuel_kg,
            phase: if ship.travel.enabled {
                FirmwarePhase::Planning
            } else {
                FirmwarePhase::Idle
            },
            ..Default::default()
        };
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
            identity::attach_ship(world, entity, control.account, ship.id)?;
            world.get_mut::<identity::Control>(entity).unwrap().revision = control.revision;
        } else {
            identity::register(world, entity, ship.id)?;
        }
        if let Some(saved) = &ship.software {
            world
                .get_mut::<vessel::ShipMailbox>(entity)
                .unwrap()
                .request_id = saved.request_id;
            let mut damage = world.get_mut::<vessel::PendingDamage>(entity).unwrap();
            damage.hull_energy_j = saved.hull_energy_j;
            damage.shield_energy_j = saved.shield_energy_j;
        }
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

        if let Some(facility) = ship.industry {
            let design = &world.get::<vessel::ShipDesign>(entity).unwrap().0;
            let catalogue = &world.resource::<vessel::ShipCatalogue>().0;
            let facility = industry::IndustrialFacility::from_record(facility, design, catalogue)?;
            world.entity_mut(entity).insert(facility);
        }
        relationships.push((
            entity,
            ship.presence,
            ship.physical_body,
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
        if let Some(mut drive) = drive {
            if let Some(installed) = world.get::<travel::SlipDrive>(entity) {
                drive.power_w = installed.power_w;
            }
            world.entity_mut(entity).insert(drive);
        }
    }
    for (entity, presence, physical_body, instance, dormant_thermal_s) in relationships {
        if let Presence::Docked { host, .. } | Presence::StoredInWreck(host) = presence {
            let host = identity::lookup(world, host)?;
            world.entity_mut(entity).insert(travel::DockedIn(host));
        }
        if presence == Presence::Destroyed && !physical_body {
            world.entity_mut(entity).remove::<(
                spatial::SpatialBody,
                physics::RigidBody,
                physics::collision::CollisionBody,
            )>();
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
    let mut activate = Schedule::default();
    activate.add_systems(
        (
            orrery::activity::activate,
            identity::identify_celestials,
            spatial::rebuild,
            crate::sim::location::refresh,
        )
            .chain(),
    );
    activate.run(world);
    hardware::utilities::refresh_emitters(world);
    infrastructure::publish_navigation(world);
    let mut publish = Schedule::default();
    publish.add_systems((sensors::publish, crate::sim::services::publish_indexes).chain());
    publish.run(world);
    crate::sim::spatial::rebuild(world);
    Ok(())
}

#[cfg(test)]
#[path = "industry_tests.rs"]
mod industry_tests;

#[cfg(test)]
#[path = "route_tests.rs"]
mod route_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::gas;
    use osg_model::travel::{Directive, ItineraryEntry};

    fn dock_entry(station: Id) -> ItineraryEntry {
        ItineraryEntry {
            directive: Directive::DockAt(station),
            label: "Dock".into(),
        }
    }

    #[test]
    fn restore_rebuilds_sensor_handles() {
        let mut app =
            crate::sim::bootstrap::provision_combat_fixture(&[Id::new()], None, None).unwrap();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let ship_id = id(world, ship).unwrap();
        let handle = *sensors::observe(world, ship)
            .contacts
            .keys()
            .next()
            .unwrap();
        let saved = capture(world).unwrap();
        restore(world, &saved).unwrap();

        let restored = identity::lookup(world, ship_id).unwrap();
        let observations = sensors::observe(world, restored);
        assert!(!observations.contacts.contains_key(&handle));
        assert!(!observations.contacts.is_empty());
    }

    fn saved_transit(origin: osg_model::GalacticPosition, tick: u64) -> travel::Transit {
        travel::Transit {
            ignored_capture_body: None,
            origin,
            position: origin,
            destination: origin.offset_by(DVec3::X * 1e12),
            departed: tick,
            advanced_tick: tick,
            direction: DVec3::X.to_array(),
            speed_ly_s: osg_model::travel::slip::cruise_speed_ly_s(false),
            retained_velocity: [12.0, 34.0, 56.0],
            requested_delta_v: [1000.0, 2000.0, 0.0],
            departure_mass_kg: 100_000.0,
            distance_ly: 0.25,
            consumed_fuel_g: 190,
            navigation_beacon: Some(Id::new()),
            beacon_lost: true,
            nominal_direction: DVec3::X.to_array(),
            variance_m2: 1e12,
        }
    }

    #[test]
    fn destroyed_ship_restore_preserves_recoverable_physics_from_space_and_slip() {
        let account = Id::new();
        let mut app = crate::sim::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let ship_id = id(world, ship).unwrap();

        let velocity = DVec3::new(12.0, 34.0, 56.0);
        let angular_velocity = DVec3::new(0.1, 0.2, 0.3);
        for destroyed_in_slip in [false, true] {
            let ship = identity::lookup(world, ship_id).unwrap();
            world.entity_mut(ship).insert((
                physics::Velocity(velocity),
                physics::AngularVelocity(angular_velocity),
            ));
            if destroyed_in_slip {
                travel::set_dormant(world, ship, Presence::SlipTransit(Id::new()));
            }
            travel::set_dormant(world, ship, Presence::Destroyed);
            let bytes = capture(world).unwrap();
            restore(world, &bytes).unwrap();
            let ship = identity::lookup(world, ship_id).unwrap();

            assert_eq!(
                world.get::<travel::PresenceState>(ship).unwrap().0,
                Presence::Destroyed
            );
            assert!(
                world
                    .get::<travel::DormantMotion>(ship)
                    .unwrap()
                    .has_physical_body()
            );
            assert!(world.get::<physics::RigidBody>(ship).is_none());
            assert!(
                world
                    .get::<physics::collision::CollisionBody>(ship)
                    .is_none()
            );
            assert!(world.get::<spatial::SpatialBody>(ship).is_none());

            travel::debug_recover(world, ship).unwrap();
            assert!(world.get::<physics::RigidBody>(ship).is_some());
            assert!(
                world
                    .get::<physics::collision::CollisionBody>(ship)
                    .is_some()
            );
            assert!(world.get::<spatial::SpatialBody>(ship).is_some());
            assert_eq!(world.get::<physics::Velocity>(ship).unwrap().0, velocity);
            assert_eq!(
                world.get::<physics::AngularVelocity>(ship).unwrap().0,
                angular_velocity
            );
        }
    }

    #[test]
    fn gas_checkpoints_restore_spending_and_fairness_exactly() {
        use osg_model::ownership::Principal;

        let account = Id::new();
        let mut app = crate::scenario(&[account], Some(account), None).unwrap();
        for _ in 0..3 {
            app.update();
        }
        let world = app.world_mut();
        let owner = Principal::Player(account);
        let requests = [1, 2, 3].map(|id| gas::GasRequest {
            id: Id([id; 16]),
            maximum: 4,
            minimum: 1,
        });
        {
            let mut state = world.resource_mut::<crate::sim::society::SocietyState>();
            state.gas = gas::GasState::default();
            state.gas.ensure_account(owner, 5);
            let grants = state.gas.debit_fair(owner, &requests).unwrap();
            for (_, grant) in grants {
                state.gas.refund(owner, grant, 1);
            }
            // Other running computers still require their own payer accounts.
            for principal in state
                .directory
                .0
                .sovereignties
                .keys()
                .copied()
                .map(Principal::Sovereignty)
                .chain(
                    state
                        .directory
                        .0
                        .organizations
                        .keys()
                        .copied()
                        .map(Principal::Organization),
                )
                .collect::<Vec<_>>()
            {
                state.gas.ensure_account(principal, 0);
            }
        }
        let expected = world
            .resource::<crate::sim::society::SocietyState>()
            .gas
            .clone();
        assert_eq!(expected.accounts[&owner].available, 2);
        assert_eq!(expected.accounts[&owner].fairness, Some(Id([2; 16])));
        let bytes = capture(world).unwrap();
        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .gas
            .deposit(owner, 2)
            .unwrap();
        restore(world, &bytes).unwrap();
        assert_eq!(
            world.resource::<crate::sim::society::SocietyState>().gas,
            expected
        );
    }

    #[test]
    fn restore_rebuilds_optical_visibility_before_the_next_simulation_tick() {
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
        let tick = world.resource::<simulation::SimulationCounters>().ticks;
        let observations = world.query::<&sensors::Observations>().iter(world).count();
        let bytes = capture(world).unwrap();
        restore(world, &bytes).unwrap();

        let ship = identity::lookup(world, ship_id).unwrap();
        let index = world.resource::<spatial::SpatialIndex>();
        let ship_index = index.object_index(ship).unwrap();
        let origin = index.objects()[ship_index].position;
        assert!(
            index
                .visible(origin, 1e-9)
                .iter()
                .any(|&id| id != ship_index)
        );
        assert_eq!(world.resource::<crate::sim::session::Clock>().rate, 1.0);
        assert_eq!(
            world.resource::<simulation::SimulationCounters>().ticks,
            tick
        );
        assert_eq!(
            world.query::<&sensors::Observations>().iter(world).count(),
            observations
        );

        // A resumed tick must still publish collisions and apply destruction.
        let mut projectile = physics::collision::Projectile::new(0.1, 1.0);
        projectile.remaining_s = 0.01;
        let expired = world
            .spawn((
                projectile,
                precision::PreciseTransform {
                    translation_um: origin.offset_by(DVec3::Z * 2e6),
                    ..Default::default()
                },
            ))
            .id();
        app.update();
        assert!(app.world().get_entity(expired).is_err());
        assert!(
            app.world()
                .resource::<physics::collision::CollisionReport>()
                .report
                .destroyed
                .iter()
                .any(|destruction| destruction.entity == expired)
        );
    }

    #[test]
    fn restored_slip_preserves_realized_walk_fuel_velocity_and_itinerary() {
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
        let tick = world.resource::<simulation::SimulationCounters>().ticks;
        world
            .entity_mut(ship)
            .insert(travel::Travel(AutopilotState {
                itinerary: vec![dock_entry(Id::new())],
                enabled: true,
                directive_revision: 31,
                status: FirmwareStatus {
                    phase: FirmwarePhase::Planning,
                    ..Default::default()
                },
                ..Default::default()
            }));
        travel::set_dormant(world, ship, Presence::SlipTransit(Id::new()));
        let transit = saved_transit(departure, tick);
        let expected_transit = postcard::to_stdvec(&transit).unwrap();
        world.entity_mut(ship).insert(transit);
        let saved_travel = world.get::<travel::Travel>(ship).unwrap().0.clone();
        let bytes = capture(world).unwrap();
        restore(world, &bytes).unwrap();
        let restored_ship = identity::lookup(world, ship_id).unwrap();
        assert_eq!(
            world.get::<travel::Travel>(restored_ship).unwrap().0,
            saved_travel
        );
        assert!(matches!(
            world.get::<travel::PresenceState>(restored_ship).unwrap().0,
            Presence::SlipTransit(_)
        ));
        assert_eq!(
            world
                .get::<crate::sim::location::SpatialLocation>(restored_ship)
                .unwrap()
                .0
                .region,
            osg_model::location::LocationRegion::SlipTransit
        );
        assert_eq!(
            postcard::to_stdvec(world.get::<travel::Transit>(restored_ship).unwrap()).unwrap(),
            expected_transit
        );
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
            .query_filtered::<Entity, With<vessel::ShipDesign>>()
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
        let travel = AutopilotState {
            enabled: true,
            itinerary: vec![dock_entry(station_id)],
            status: FirmwareStatus {
                phase: FirmwarePhase::Planning,
                ..Default::default()
            },
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
        let tick = world.resource::<simulation::SimulationCounters>().ticks;
        world
            .entity_mut(transit_ship)
            .insert(saved_transit(departure, tick));
        let expected_motion = pose(world, transit_ship).unwrap();
        let program = world
            .get::<vessel::ShipSoftware>(ship)
            .unwrap()
            .controller
            .checkpoint()
            .program;
        let checkpoint = osg_ship_wasm::ControllerCheckpoint {
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
        world
            .get_mut::<vessel::ShipMailbox>(ship)
            .unwrap()
            .request_id = 42;
        world.entity_mut(ship).insert(vessel::PendingDamage {
            hull_energy_j: 17.0,
            shield_energy_j: 23.0,
        });
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
            world.get::<vessel::ShipMailbox>(ship).unwrap().request_id,
            42
        );
        let damage = world.get::<vessel::PendingDamage>(ship).unwrap();
        assert_eq!(damage.hull_energy_j, 17.0);
        assert_eq!(damage.shield_energy_j, 23.0);
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
        let ship_location = &world
            .get::<crate::sim::location::SpatialLocation>(ship)
            .unwrap()
            .0;
        let station_location = &world
            .get::<crate::sim::location::SpatialLocation>(station)
            .unwrap()
            .0;
        assert_eq!(ship_location, station_location);
        assert!(station_location.system.is_some());
        let restored_travel = &world.get::<travel::Travel>(ship).unwrap().0;
        assert_eq!(restored_travel.itinerary, travel.itinerary);
        assert_eq!(restored_travel.enabled, travel.enabled);
        assert_eq!(
            restored_travel.directive_revision,
            travel.directive_revision
        );
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
