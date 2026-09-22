use super::*;
use osg_model::{
    industry::{CargoItem, IndustryCommand, ItemStack},
    ownership::Principal,
};
use osg_ships::{Inventory, industry as manufacturing};

fn inventory(world: &World, entity: Entity) -> &Inventory {
    &world.get::<hardware::ShipInventory>(entity).unwrap().0
}

fn cargo_bytes(world: &World, entity: Entity) -> Vec<u8> {
    let stock = inventory(world, entity);
    postcard::to_stdvec(&(&stock.cargo, &stock.packaged_parts, &stock.reservations)).unwrap()
}

fn stock(world: &mut World, entity: Entity, items: &[ItemStack]) {
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let capacity = world
        .get::<vessel::ShipDesign>(entity)
        .unwrap()
        .0
        .capacity_m3;
    for item in items {
        world
            .get_mut::<hardware::ShipInventory>(entity)
            .unwrap()
            .0
            .insert_item(&item.item, item.quantity, capacity, &catalogue)
            .unwrap();
    }
    industry::synchronize_mass(world, &[entity]);
}

fn advance(world: &mut World) {
    world.resource_mut::<simulation::SimulationCounters>().ticks += 1;
    world
        .resource_mut::<Time<Fixed>>()
        .advance_by(osg_model::TICK_DURATION);
    industry::advance(world);
    let mut initialize = Schedule::default();
    initialize.add_systems(hardware::initialize);
    initialize.run(world);
}

#[tokio::test]
async fn industry_checkpoints_resume_reserved_work_and_complete_ship_construction_once() {
    let account = Id::new();
    let mut app = crate::scenario(&[account], Some(account), None).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    let facility = world
        .query_filtered::<Entity, With<travel::DockingBays>>()
        .iter(world)
        .next()
        .unwrap();
    let facility_id = id(world, facility).unwrap();
    world.entity_mut(facility).insert((
        ownership::AssetOwner(Principal::Player(account)),
        industry::IndustryFacility {
            seeded: true,
            ..Default::default()
        },
        industry::MineSource {
            output: CargoItem::Resource("industrial_ore".into()),
            units_per_second: 3,
            remainder: 7,
            last_recipient: Some(Id::new()),
        },
    ));
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let design = world.get::<vessel::ShipDesign>(facility).unwrap().0.clone();
    {
        let mut inventory = world.get_mut::<hardware::ShipInventory>(facility).unwrap();
        inventory.0.cargo.fill(0);
        inventory.0.packaged_parts.clear();
        inventory.0.reservations.clear();
        inventory.0.energy_j = design.battery_j;
    }
    let recipe = manufacturing::recipes(&catalogue)
        .unwrap()
        .into_iter()
        .find(|recipe| recipe.id == "railgun_pellets")
        .unwrap();
    let mut blueprint = manufacturing::starter_ship();
    blueprint.firmware = osg_ships::Firmware::Custom(osg_ships::EXAMPLE_CONTROLLER.to_vec());
    let blueprint_bytes = blueprint.to_bytes().unwrap();
    assert!(blueprint_bytes.len() > 48 * 1024);
    let uploads = crate::blueprint_uploads::BlueprintUploads::default();
    let blueprint_hash = *blake3::hash(&blueprint_bytes).as_bytes();
    let upload = [blueprint_hash.as_slice(), &blueprint_bytes].concat();
    assert_eq!(
        uploads.receive(&mut upload.as_slice()).await.unwrap(),
        blueprint_hash
    );
    let built_design = blueprint.compile(&catalogue).unwrap();
    let construction = manufacturing::construction_requirements(&built_design, &catalogue).unwrap();
    stock(world, facility, &recipe.inputs);
    stock(world, facility, &construction.inputs);
    let mut expected = inventory(world, facility).clone();
    for input in recipe.inputs.iter().chain(&construction.inputs) {
        expected
            .withdraw_cargo(&input.item, input.quantity, &catalogue)
            .unwrap();
    }
    for output in &recipe.outputs {
        expected
            .insert_item(
                &output.item,
                output.quantity,
                design.capacity_m3,
                &catalogue,
            )
            .unwrap();
    }
    industry::execute(
        world,
        account,
        IndustryCommand::StartRecipe {
            facility: facility_id,
            recipe: recipe.id.clone(),
            batches: 1,
        },
        None,
    )
    .unwrap();
    industry::execute(
        world,
        account,
        IndustryCommand::BuildShip {
            facility: facility_id,
            owner: Principal::Player(account),
            blueprint_hash,
        },
        Some(&uploads),
    )
    .unwrap();
    drop(uploads);
    for _ in 0..3 {
        advance(world);
    }
    let jobs = &world
        .get::<industry::IndustryFacility>(facility)
        .unwrap()
        .jobs;
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().any(|job| {
        matches!(&job.output, industry::JobOutput::Ship(bytes) if *bytes == blueprint_bytes)
    }));
    assert!(jobs.iter().all(|job| job.view.progress_ticks == 3));
    assert!(!inventory(world, facility).reservations.is_empty());
    let saved_jobs = postcard::to_stdvec(jobs).unwrap();
    let saved_inventory = postcard::to_stdvec(inventory(world, facility)).unwrap();
    let saved_mine =
        postcard::to_stdvec(world.get::<industry::MineSource>(facility).unwrap()).unwrap();
    let initial_ship_count = world.query::<&vessel::ShipDesign>().iter(world).count();
    let bytes = capture(world).unwrap();

    assert_corruption_is_rejected(world, &bytes, facility_id);
    restore(world, &bytes).unwrap();
    let facility = identity::lookup(world, facility_id).unwrap();
    assert_eq!(
        postcard::to_stdvec(
            &world
                .get::<industry::IndustryFacility>(facility)
                .unwrap()
                .jobs
        )
        .unwrap(),
        saved_jobs
    );
    assert_eq!(
        postcard::to_stdvec(inventory(world, facility)).unwrap(),
        saved_inventory
    );
    industry::seed_demo(world, facility, account).unwrap();
    assert_eq!(
        postcard::to_stdvec(inventory(world, facility)).unwrap(),
        saved_inventory
    );
    assert_eq!(
        postcard::to_stdvec(world.get::<industry::MineSource>(facility).unwrap()).unwrap(),
        saved_mine
    );
    for _ in 3..construction.duration_ticks - 1 {
        advance(world);
    }
    let jobs = &world
        .get::<industry::IndustryFacility>(facility)
        .unwrap()
        .jobs;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].view.progress_ticks + 1, jobs[0].view.duration_ticks);
    let last_tick = capture(world).unwrap();
    restore(world, &last_tick).unwrap();
    advance(world);
    let facility = identity::lookup(world, facility_id).unwrap();
    assert!(
        world
            .get::<industry::IndustryFacility>(facility)
            .unwrap()
            .jobs
            .is_empty()
    );
    assert_eq!(
        world.query::<&vessel::ShipDesign>().iter(world).count(),
        initial_ship_count + 1
    );
    assert_eq!(
        cargo_bytes(world, facility),
        postcard::to_stdvec(&(
            &expected.cargo,
            &expected.packaged_parts,
            &expected.reservations
        ))
        .unwrap()
    );
    let built = world
        .query::<(Entity, &vessel::Vessel, &travel::PresenceState)>()
        .iter(world)
        .find_map(|(entity, vessel, presence)| {
            (vessel.vessel_name.as_str() == blueprint.name
                && matches!(presence.0, Presence::Docked { host, .. } if host == facility_id))
            .then_some(entity)
        })
        .unwrap();
    let built_id = id(world, built).unwrap();
    assert_eq!(
        world
            .get::<vessel::ShipSoftware>(built)
            .unwrap()
            .controller
            .program(),
        blueprint.controller_bytes()
    );
    assert_eq!(inventory(world, built).energy_j, 0);
    assert!(
        inventory(world, built)
            .quantities
            .iter()
            .all(|quantity| *quantity == 0)
    );
    assert_eq!(
        world
            .get::<hardware::ShipThermal>(built)
            .unwrap()
            .0
            .shield_deployed_kg,
        0.
    );
    assert_eq!(
        world
            .get::<hardware::ShipThermal>(built)
            .unwrap()
            .0
            .shield_reserve_kg(),
        0.
    );
    assert!(world.get::<physics::Velocity>(built).is_none());
    assert!(
        world
            .get::<physics::collision::CollisionBody>(built)
            .is_none()
    );
    assert_eq!(
        world.get::<ownership::AssetOwner>(built).unwrap().0,
        Principal::Player(account)
    );
    assert!(
        (world.get::<physics::MassProps>(built).unwrap().mass - built_design.dry_mass).abs() < 1e-6
    );
    assert!(
        (world.get::<travel::StoredMass>(facility).unwrap().0 - built_design.dry_mass).abs() < 1e-6
    );

    let completed = capture(world).unwrap();
    restore(world, &completed).unwrap();
    for _ in 0..10 {
        advance(world);
    }
    let facility = identity::lookup(world, facility_id).unwrap();
    assert!(identity::lookup(world, built_id).is_ok());
    assert_eq!(
        world
            .get::<vessel::ShipSoftware>(identity::lookup(world, built_id).unwrap())
            .unwrap()
            .controller
            .program(),
        blueprint.controller_bytes()
    );
    assert_eq!(
        world.query::<&vessel::ShipDesign>().iter(world).count(),
        initial_ship_count + 1
    );
    assert_eq!(
        cargo_bytes(world, facility),
        postcard::to_stdvec(&(
            &expected.cargo,
            &expected.packaged_parts,
            &expected.reservations
        ))
        .unwrap()
    );
}

fn assert_corruption_is_rejected(world: &mut World, bytes: &[u8], facility_id: Id) {
    let identities = world.resource::<identity::IdentityIndex>().0.clone();
    let balances = world
        .resource::<gas::GasLedger>()
        .snapshot()
        .unwrap()
        .accounts;
    let facility = identity::lookup(world, facility_id).unwrap();
    let stock = cargo_bytes(world, facility);
    for case in 0..9 {
        let mut invalid: WorldRecord = postcard::from_bytes(bytes).unwrap();
        let saved = invalid
            .ships
            .iter_mut()
            .find(|ship| ship.id == facility_id)
            .unwrap();
        let jobs = &mut saved.industry.as_mut().unwrap().jobs;
        match case {
            0 => {
                *saved
                    .hardware
                    .inventory
                    .reservations
                    .values_mut()
                    .next()
                    .unwrap() -= 1
            }
            1 => jobs[1].view.id = jobs[0].view.id,
            2 => jobs[0].view.owner = Principal::Player(Id::new()),
            3 => jobs[0].view.progress_ticks = jobs[0].view.duration_ticks + 1,
            4 => match &mut jobs[0].output {
                industry::JobOutput::Cargo(outputs) => outputs[0].quantity += 1,
                _ => unreachable!(),
            },
            5 => match &mut jobs[1].output {
                industry::JobOutput::Ship(bytes) => bytes.clear(),
                _ => unreachable!(),
            },
            6 => saved.mine.as_mut().unwrap().remainder = 10,
            7 => jobs[0].view.module_part = Some(u64::MAX),
            8 => saved.industry = None,
            _ => unreachable!(),
        }
        assert!(
            restore(world, &postcard::to_stdvec(&invalid).unwrap()).is_err(),
            "case {case}"
        );
        assert_eq!(world.resource::<identity::IdentityIndex>().0, identities);
        assert_eq!(
            world
                .resource::<gas::GasLedger>()
                .snapshot()
                .unwrap()
                .accounts,
            balances
        );
        assert_eq!(cargo_bytes(world, facility), stock);
    }
}
