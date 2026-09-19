use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use bevy::{ecs::system::RunSystemOnce, math::DVec3, prelude::*};
use toy_sim_model::{
    Id,
    firmware::ChatterProfile,
    industry::CargoItem,
    ownership::{AccessGrant, Permission, Principal},
};
use toy_sim_ships::{Catalogue, CompiledShipDesign, ShipState};
use toy_sim_universe::organizations::{self, OrganizationProfile, OrganizationRole};

use super::{
    logistics::{HaulDuty, HaulStage},
    state::{NpcAsset, NpcOrganization, NpcRole},
};
use crate::sim::{
    defense::DefenseDuty,
    hardware, identity, industry, infrastructure, ownership,
    physics::{MassProps, Velocity},
    precision::PreciseTransform,
    registry,
    simulation::SimulationCounters,
    travel, vessel,
};

mod designs;
#[cfg(test)]
mod tests;

const COOPERATIVE: &str = "Helion Flight Cooperative";
const FUEL_MUTUAL: &str = "Helion Fuel Mutual";

pub(crate) fn population_id(organization: Id, role: &str) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("toy-sim NPC population identity v1");
    hash.update(&organization.0);
    hash.update(role.as_bytes());
    Id(hash.finalize().as_bytes()[..16].try_into().unwrap())
}

/// Populate a newly provisioned world. Existing organization records are never
/// recreated: losses, ownership changes, and inventories survive restoration.
pub fn populate(world: &mut World) -> Result<()> {
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let designs = designs::Designs::new(&catalogue)?;
    let tick = world.resource::<SimulationCounters>().ticks;
    let anchors = anchors(world);
    let neris = find_neris(world);
    let neris_frame = neris.map(|entity| frame(world, entity)).transpose()?;
    let mut home_counts = BTreeMap::<Id, usize>::new();
    let mut guarded = BTreeSet::new();
    let mut pending_docks = Vec::new();
    let mut pending_stock = Vec::new();
    let mut new_records = Vec::new();

    for (index, profile) in organizations::catalogue().iter().enumerate() {
        let organization = Id(profile.id());
        let home = registry::system_identity(&profile.home_system);
        let slot = home_counts.entry(home).or_default();
        let local_slot = *slot;
        *slot += 1;

        let roles = vessel_roles(profile);
        let guard_active = profile
            .roles
            .iter()
            .any(|role| matches!(role, OrganizationRole::Defense | OrganizationRole::Patrol))
            && guarded.insert(home);
        if identity::lookup(world, organization).is_ok() {
            continue;
        }

        let officer = population_id(organization, "officer");
        let account_entity = register_officer(world, officer, organization, profile);
        let origin = anchors
            .get(&home)
            .context("NPC home system has no physical gate")?;
        let offset = DVec3::new(
            40_000.0 + (local_slot % 6) as f64 * 30_000.0,
            40_000.0 + (local_slot / 6) as f64 * 30_000.0,
            15_000.0,
        );
        let mut facility_frame = offset_frame(*origin, offset);
        if profile.name == FUEL_MUTUAL
            && let Some(origin) = neris_frame
        {
            facility_frame = offset_frame(origin, DVec3::X * 100_000.0);
        }

        let facility = if profile.name == COOPERATIVE
            && let Some(existing) = neris
        {
            take_station_control(world, existing, account_entity, officer, profile, index)?;
            existing
        } else {
            let design = station_design(&designs, profile);
            let entity = spawn(
                world,
                design,
                facility_frame,
                officer,
                organization,
                population_id(organization, "facility"),
                format!("{} Terminal", profile.name),
                profile,
                index,
                &catalogue,
            )?;
            world.entity_mut(entity).insert((
                identity::BeaconEmitter,
                infrastructure::Landmark {
                    system: home,
                    name: format!("{} Terminal", profile.name),
                },
                industry::IndustryFacility {
                    seeded: true,
                    ..Default::default()
                },
                defense(
                    officer,
                    organization,
                    Some(population_id(organization, "facility")),
                ),
            ));

            if profile.roles.contains(&OrganizationRole::Mining) {
                world.entity_mut(entity).insert(industry::MineSource {
                    output: CargoItem::Resource("industrial_ore".into()),
                    units_per_second: 10 + (index as u64 % 4) * 5,
                    remainder: 0,
                    last_recipient: None,
                });
            }
            pending_stock.push((entity, profile));
            entity
        };

        let facility_id = world.get::<identity::Identity>(facility).unwrap().0;
        let facility_frame = frame(world, facility)?;
        let mut assets = vec![NpcAsset {
            id: facility_id,
            role: NpcRole::Station,
        }];

        for (number, role) in roles.into_iter().enumerate() {
            let id = population_id(organization, &format!("vessel-{number}"));
            let design = match role {
                NpcRole::Patrol => designs.patrol.clone(),
                NpcRole::Research | NpcRole::Broadcast => designs.survey.clone(),
                _ => designs.freighter.clone(),
            };
            let near_neris = profile.name == "Kisaragi Memory House" && role == NpcRole::Broadcast;
            let origin = if near_neris {
                neris_frame.unwrap_or(facility_frame)
            } else {
                facility_frame
            };
            let pose = offset_frame(
                origin,
                DVec3::new(2_000.0, (number + 1) as f64 * 2_000.0, 1_000.0),
            );
            let entity = spawn(
                world,
                design,
                pose,
                officer,
                organization,
                id,
                format!("{} {} {}", profile.name, role_name(role), number + 1),
                profile,
                index * 3 + number + 1,
                &catalogue,
            )?;

            assets.push(NpcAsset { id, role });
            let active = (guard_active && number == 1 && role == NpcRole::Patrol) || near_neris;
            if active && role == NpcRole::Patrol {
                world
                    .entity_mut(entity)
                    .insert(defense(officer, organization, Some(facility_id)));
            }
            if !active {
                pending_docks.push((entity, facility));
            }
        }

        new_records.push(NpcOrganization::new(
            organization,
            officer,
            home,
            facility_id,
            assets,
            tick.saturating_add(600 + index as u64 * 17),
        ));
    }

    world
        .run_system_once(hardware::initialize)
        .map_err(|error| anyhow::anyhow!("initialize NPC hardware: {error:?}"))?;
    for (entity, profile) in pending_stock {
        stock_facility(world, entity, profile, &catalogue)?;
    }

    for record in &new_records {
        let facility = identity::lookup(world, record.facility)?;
        if let Some(mut bays) = world.get_mut::<travel::DockingBays>(facility) {
            for bay in &mut bays.0 {
                bay.public = true;
            }
        }
    }

    for (ship, host) in pending_docks {
        let owner = world.get::<ownership::AssetOwner>(ship).unwrap().0;
        let radius = world.get::<vessel::ShipDesign>(ship).unwrap().0.radius;
        let mass = world.get::<MassProps>(ship).unwrap().mass;
        let bay = travel::construction_bay(world, host, owner, radius, mass)?;
        travel::store_constructed(world, ship, host, bay)?;
    }

    let seeded_cooperative = new_records
        .iter()
        .any(|record| record.organization == ownership::organization_id(COOPERATIVE));
    for record in new_records {
        let id = record.organization;
        let entity = world.spawn(record).id();
        identity::register(world, entity, id);
    }

    if seeded_cooperative {
        seed_haul(world)?;
    }
    industry::refresh_publication(world);
    infrastructure::publish_navigation(world);
    travel::geometry::refresh(world);
    Ok(())
}

fn register_officer(
    world: &mut World,
    officer: Id,
    organization: Id,
    profile: &OrganizationProfile,
) -> Entity {
    let entity = identity::add_account(world, officer, false);
    ownership::affiliate(world, officer, Some(organization))
        .expect("validated organization catalogue");

    let mut directory = world.resource_mut::<ownership::Directory>();
    directory.0.players.get_mut(&officer).unwrap().name = format!("{} Duty Office", profile.name);
    directory
        .0
        .organizations
        .get_mut(&organization)
        .unwrap()
        .officers
        .insert(officer);
    entity
}

fn chatter(profile: &OrganizationProfile, name: String, stagger: usize) -> ChatterProfile {
    ChatterProfile {
        name,
        personality: format!("{}\n\n{}", profile.culture, profile.doctrine),
        context: format!(
            "Public organization: {}. Home: {}. Sovereignty: {}.\n\n{}\n\nCurrent public objectives:\n{}\n\nAvailable institutional resources:\n{}",
            profile.name,
            profile.home_system,
            profile.sovereignty,
            profile.history,
            profile.goals.join("\n"),
            profile.resources.join("\n")
        ),
        interval_seconds: 300 + (stagger as u32 * 37 % 300),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    world: &mut World,
    design: Arc<CompiledShipDesign>,
    frame: (PreciseTransform, DVec3),
    officer: Id,
    organization: Id,
    id: Id,
    name: String,
    profile: &OrganizationProfile,
    stagger: usize,
    catalogue: &Catalogue,
) -> Result<Entity> {
    let profile = chatter(profile, name.clone(), stagger);
    ensure!(
        profile.valid(),
        "NPC chatter profile exceeds the firmware limits"
    );

    let mut controller = world
        .resource_mut::<vessel::WasmRuntime>()
        .0
        .instantiate_with_chatter(design.blueprint.controller_bytes(), profile)?;
    controller.configure_hardware(&design, catalogue);

    let mut state = ShipState::new(&design, catalogue);
    state.inventory.energy_j = design.battery_j;
    let (mass, inertia) = state.mass_properties(&design, catalogue);

    let entity = world
        .spawn(vessel::ship_bundle(
            design,
            state,
            vessel::ShipSoftware::new(controller),
            frame.0,
            frame.1,
            name,
            MassProps {
                mass,
                inertia,
                inertia_inv: inertia.inverse(),
            },
        ))
        .id();

    identity::attach_ship(world, entity, officer)?;
    let temporary_id = world.get::<identity::Identity>(entity).unwrap().0;
    world
        .resource_mut::<identity::IdentityIndex>()
        .0
        .remove(&temporary_id);
    identity::register(world, entity, id);
    world
        .entity_mut(entity)
        .insert(ownership::AssetOwner(Principal::Organization(organization)));
    Ok(entity)
}

fn take_station_control(
    world: &mut World,
    station: Entity,
    account: Entity,
    officer: Id,
    profile: &OrganizationProfile,
    stagger: usize,
) -> Result<()> {
    let group = world.get::<identity::Account>(account).unwrap().group;
    let revision = world
        .get::<identity::Control>(station)
        .map_or(1, |control| control.revision + 1);

    let old_design = world.get::<vessel::ShipDesign>(station).unwrap().0.clone();
    let mut design = (*old_design).clone();
    design.blueprint.firmware =
        toy_sim_ships::Firmware::Custom(toy_sim_ships::CHATTER_CONTROLLER.to_vec());
    let design = Arc::new(design);

    let name = world
        .get::<vessel::Vessel>(station)
        .unwrap()
        .vessel_name
        .to_string();
    let mut controller = world
        .resource_mut::<vessel::WasmRuntime>()
        .0
        .instantiate_with_chatter(
            toy_sim_ships::CHATTER_CONTROLLER,
            chatter(profile, name, stagger),
        )?;
    controller.configure_hardware(&design, &world.resource::<vessel::ShipCatalogue>().0);

    world.entity_mut(station).insert((
        identity::Control {
            account: officer,
            revision,
        },
        identity::ControlledBy(account),
        identity::Membership(group),
        vessel::ShipDesign(design),
        vessel::ShipSoftware::new(controller),
    ));

    if let Some(mut iff) = world.get_mut::<identity::Transponder>(station) {
        iff.0.owner = officer;
        iff.0.faction = Some(Id(profile.id()));
    }

    if let Some(mut environment) =
        world.get_mut::<crate::sim::displays::DisplayEnvironment>(station)
    {
        environment.firmware = Arc::from(toy_sim_ships::CHATTER_CONTROLLER);
    }
    Ok(())
}

fn station_design(
    designs: &designs::Designs,
    profile: &OrganizationProfile,
) -> Arc<CompiledShipDesign> {
    if profile.roles.contains(&OrganizationRole::Mining) {
        designs.mining_station.clone()
    } else if profile.roles.contains(&OrganizationRole::Industry) {
        designs.industrial_station.clone()
    } else if profile.roles.contains(&OrganizationRole::Defense) {
        designs.defense_station.clone()
    } else {
        designs.service_station.clone()
    }
}

fn vessel_roles(profile: &OrganizationProfile) -> [NpcRole; 2] {
    let primary = if profile.roles.contains(&OrganizationRole::Broadcast) {
        NpcRole::Broadcast
    } else if profile.roles.contains(&OrganizationRole::Mining) {
        NpcRole::Miner
    } else if profile.roles.contains(&OrganizationRole::Trade)
        || profile.roles.contains(&OrganizationRole::Relief)
        || profile.roles.contains(&OrganizationRole::Industry)
    {
        NpcRole::Freighter
    } else if profile.roles.contains(&OrganizationRole::Research) {
        NpcRole::Research
    } else {
        NpcRole::Patrol
    };

    let secondary = if profile.roles.contains(&OrganizationRole::Defense)
        || profile.roles.contains(&OrganizationRole::Patrol)
    {
        NpcRole::Patrol
    } else {
        NpcRole::Freighter
    };
    [primary, secondary]
}

fn role_name(role: NpcRole) -> &'static str {
    match role {
        NpcRole::Station => "Terminal",
        NpcRole::Freighter => "Tender",
        NpcRole::Patrol => "Guard",
        NpcRole::Miner => "Ore Carrier",
        NpcRole::Research => "Survey",
        NpcRole::Broadcast => "Dispatch",
    }
}

fn defense(account: Id, organization: Id, installation: Option<Id>) -> DefenseDuty {
    DefenseDuty {
        account,
        organization,
        installation,
        engagement_range_m: 1_000_000.0,
        hostile_iff: true,
    }
}

fn stock_facility(
    world: &mut World,
    entity: Entity,
    profile: &OrganizationProfile,
    catalogue: &Catalogue,
) -> Result<()> {
    let capacity = world
        .get::<vessel::ShipDesign>(entity)
        .unwrap()
        .0
        .capacity_m3;

    let stacks = if profile.roles.contains(&OrganizationRole::Industry) {
        toy_sim_ships::industry::starter_stock(catalogue)?
    } else {
        [
            ("water", 20_000),
            ("reactor_fuel", 1_000),
            ("shield_coolant", 5_000),
            ("repair_material", 1_000),
        ]
        .into_iter()
        .map(|(resource, quantity)| toy_sim_model::industry::ItemStack {
            item: CargoItem::Resource(resource.into()),
            quantity,
        })
        .collect()
    };

    let mut inventory = world.get_mut::<hardware::ShipInventory>(entity).unwrap();
    for stack in stacks {
        inventory
            .0
            .insert_item(&stack.item, stack.quantity, capacity, catalogue)?;
    }

    industry::synchronize_mass(world, &[entity]);
    Ok(())
}

fn anchors(world: &mut World) -> BTreeMap<Id, (PreciseTransform, DVec3)> {
    let mut gates = world
        .query_filtered::<(
            &identity::Identity,
            &infrastructure::Landmark,
            &PreciseTransform,
            &Velocity,
        ), With<travel::Gate>>()
        .iter(world)
        .map(|(id, landmark, pose, velocity)| (id.0, landmark.system, *pose, velocity.0))
        .collect::<Vec<_>>();
    gates.sort_unstable_by_key(|gate| gate.0);

    let mut result = BTreeMap::new();
    for (_, system, pose, velocity) in gates {
        result.entry(system).or_insert((pose, velocity));
    }
    result
}

fn find_neris(world: &mut World) -> Option<Entity> {
    world
        .query::<(Entity, &infrastructure::Landmark, &ownership::AssetOwner)>()
        .iter(world)
        .find(|(_, landmark, owner)| {
            landmark.name == "Neris Anchorage"
                && owner.0 == Principal::Organization(ownership::organization_id(COOPERATIVE))
        })
        .map(|(entity, _, _)| entity)
}

fn frame(world: &World, entity: Entity) -> Result<(PreciseTransform, DVec3)> {
    Ok((
        *world
            .get::<PreciseTransform>(entity)
            .context("NPC anchor pose unavailable")?,
        world
            .get::<Velocity>(entity)
            .context("NPC anchor velocity unavailable")?
            .0,
    ))
}

fn offset_frame(mut frame: (PreciseTransform, DVec3), offset: DVec3) -> (PreciseTransform, DVec3) {
    frame.0.translation_um = frame.0.translation_um.offset_by(offset);
    frame
}

fn seed_haul(world: &mut World) -> Result<()> {
    let cooperative = ownership::organization_id(COOPERATIVE);
    let fuel_mutual = ownership::organization_id(FUEL_MUTUAL);
    let owner = identity::lookup(world, cooperative)?;
    let destination = identity::lookup(world, fuel_mutual)?;
    let origin = world.get::<NpcOrganization>(owner).unwrap().clone();
    let destination = world.get::<NpcOrganization>(destination).unwrap().facility;
    let ship_id = population_id(cooperative, "vessel-0");
    let ship = identity::lookup(world, ship_id)?;
    if world.get::<HaulDuty>(ship).is_some() {
        return Ok(());
    }

    let facility = identity::lookup(world, destination)?;
    let mut access = world.get_mut::<ownership::AssetAccess>(facility).unwrap();
    let subject = Principal::Player(origin.officer);
    if let Some(grant) = access
        .0
        .grants
        .iter_mut()
        .find(|grant| grant.principal == subject)
    {
        grant
            .permissions
            .extend([Permission::View, Permission::TransferCargo]);
    } else {
        access.0.grants.push(AccessGrant {
            principal: subject,
            permissions: [Permission::View, Permission::TransferCargo]
                .into_iter()
                .collect(),
        });
    }

    let tick = world.resource::<SimulationCounters>().ticks;
    world.entity_mut(ship).insert(HaulDuty {
        account: origin.officer,
        source: origin.facility,
        destination,
        item: CargoItem::Resource("industrial_ore".into()),
        quantity: 100,
        stage: HaulStage::Loading,
        next_check_tick: tick + 650,
        problem: None,
    });
    Ok(())
}
