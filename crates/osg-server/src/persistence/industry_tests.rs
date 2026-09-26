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
    crate::sim::hardware::synchronize_mass(world, &[entity]);
}

fn advance(world: &mut World) {
    world.resource_mut::<simulation::SimulationCounters>().ticks += 1;
    world
        .resource_mut::<Time<Fixed>>()
        .advance_by(osg_model::TICK_DURATION);
    industry::tick(world);
    let mut initialize = Schedule::default();
    initialize.add_systems(hardware::initialize);
    initialize.run(world);
}

#[test]
fn public_service_checkpoint_keeps_payment_and_customer_storage_together() {
    use crate::sim::industry::service_client;
    use osg_model::{
        economy::{Currency, MONEY_SCALE},
        industry::*,
    };

    let operator = Id::new();
    let customer = Id::new();
    let payer = Principal::Player(customer);
    let mut app = crate::scenario(&[operator, customer], Some(operator), None).unwrap();
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
    let state = industry::IndustrialFacility::from_design(
        &world.get::<vessel::ShipDesign>(facility).unwrap().0,
    );
    world
        .entity_mut(facility)
        .insert((ownership::AssetOwner(Principal::Player(operator)), state));
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let recipe = manufacturing::recipes(&catalogue)
        .unwrap()
        .into_iter()
        .find(|recipe| recipe.id == "railgun_pellets")
        .unwrap();
    stock(world, facility, &recipe.inputs);
    for input in &recipe.inputs {
        world
            .get_mut::<hardware::ShipInventory>(facility)
            .unwrap()
            .0
            .custody
            .insert(input.item.clone(), input.quantity);
        world
            .resource_mut::<crate::sim::society::SocietyState>()
            .map_unchanged(|state| &mut state.economy)
            .storage
            .set_item((facility_id, payer), input.item.clone(), input.quantity);
    }
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(
            payer,
            Currency::Uec,
            1000 * MONEY_SCALE,
            osg_model::calendar::now_unix_ms(),
        )
        .unwrap();
    service_client::publish(
        world,
        operator,
        facility_id,
        ServicePolicy {
            revision: 0,
            accepting: true,
            currency: Currency::Uec,
            rates: vec![ServiceRate {
                capability: recipe.capability,
                energy_per_mj: 1,
                time_per_hour: MONEY_SCALE,
                public_lanes: 1,
            }],
            tiers: vec![],
        },
    )
    .unwrap();
    let uploads = crate::blueprint_uploads::BlueprintUploads::default();
    let quote = service_client::quote(
        world,
        customer,
        facility_id,
        payer,
        ServiceWork::Recipe {
            recipe: recipe.id,
            batches: 1,
        },
        &uploads,
    )
    .unwrap();
    service_client::order(world, customer, quote.clone(), &uploads).unwrap();
    let job = service_client::jobs(world, customer, facility_id).unwrap()[0].id;
    let checkpoint = capture(world).unwrap();
    restore(world, &checkpoint).unwrap();
    assert_eq!(
        world
            .resource::<crate::sim::society::SocietyState>()
            .economy
            .reserved(payer, quote.currency),
        quote.total
    );
    service_client::cancel(world, customer, facility_id, job).unwrap();
    assert_eq!(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .available(payer, Currency::Uec),
        1000 * MONEY_SCALE
    );
    for input in &quote.inputs {
        assert_eq!(
            (&world
                .resource::<crate::sim::society::SocietyState>()
                .economy)
                .storage[&(facility_id, payer)][&input.item],
            input.quantity
        );
    }
    service_client::order(world, customer, quote.clone(), &uploads).unwrap();
    advance(world);
    let charged = (&world
        .resource::<crate::sim::society::SocietyState>()
        .economy)
        .balances[&payer]
        .uec;
    assert_eq!(charged, 1000 * MONEY_SCALE - quote.total);
    let checkpoint = capture(world).unwrap();
    restore(world, &checkpoint).unwrap();
    advance(world);
    assert_eq!(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .balances[&payer]
            .uec,
        charged
    );
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
    let mut state = industry::IndustrialFacility::from_design(
        &world.get::<vessel::ShipDesign>(facility).unwrap().0,
    );
    state.set_mine(industry::MineSource {
        output: CargoItem::Resource("industrial_ore".into()),
        units_per_second: 3,
        remainder: 7,
        last_recipient: Some(Id::new()),
    });
    world
        .entity_mut(facility)
        .insert((ownership::AssetOwner(Principal::Player(account)), state));
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
    industry::enqueue_command(
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
    industry::enqueue_command(
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
        .get::<industry::IndustrialFacility>(facility)
        .unwrap()
        .jobs();
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().any(|job| {
        matches!(&job.work.output, industry::WorkOutput::Ship(bytes) if bytes.as_ref() == blueprint_bytes)
    }));
    assert!(jobs.iter().all(|job| job.progress_ticks == 3));
    assert!(!inventory(world, facility).reservations.is_empty());
    let saved_jobs = postcard::to_stdvec(jobs).unwrap();
    let saved_inventory = postcard::to_stdvec(inventory(world, facility)).unwrap();
    let saved_mine = postcard::to_stdvec(
        &world
            .get::<industry::IndustrialFacility>(facility)
            .unwrap()
            .to_record()
            .mine,
    )
    .unwrap();
    let initial_ship_count = world.query::<&vessel::ShipDesign>().iter(world).count();
    let bytes = capture(world).unwrap();

    restore(world, &bytes).unwrap();
    let facility = identity::lookup(world, facility_id).unwrap();
    assert_eq!(
        postcard::to_stdvec(
            &world
                .get::<industry::IndustrialFacility>(facility)
                .unwrap()
                .jobs()
        )
        .unwrap(),
        saved_jobs
    );
    assert_eq!(
        postcard::to_stdvec(inventory(world, facility)).unwrap(),
        saved_inventory
    );
    world.run_schedule(FixedPreUpdate);
    assert_eq!(
        postcard::to_stdvec(inventory(world, facility)).unwrap(),
        saved_inventory
    );
    assert_eq!(
        postcard::to_stdvec(
            &world
                .get::<industry::IndustrialFacility>(facility)
                .unwrap()
                .to_record()
                .mine
        )
        .unwrap(),
        saved_mine
    );
    for _ in 3..construction.duration_ticks - 1 {
        advance(world);
    }
    let jobs = &world
        .get::<industry::IndustrialFacility>(facility)
        .unwrap()
        .jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].progress_ticks + 1, jobs[0].work.duration_ticks);
    let last_tick = capture(world).unwrap();
    restore(world, &last_tick).unwrap();
    advance(world);
    let facility = identity::lookup(world, facility_id).unwrap();
    assert!(
        world
            .get::<industry::IndustrialFacility>(facility)
            .unwrap()
            .jobs()
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
