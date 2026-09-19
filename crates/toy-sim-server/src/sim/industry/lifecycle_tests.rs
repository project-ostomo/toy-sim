use super::*;

#[test]
fn starter_stock_survives_initialization_and_real_ticks_finish_a_paid_factory_job() {
    let account = Id::new();
    let mut app = crate::sim::provision(&[account], None, None).unwrap();
    let world = app.world_mut();
    let facility = world
        .query_filtered::<Entity, With<IndustryFacility>>()
        .single(world)
        .unwrap();
    let id = world.get::<identity::Identity>(facility).unwrap().0;
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let metal = CargoItem::Resource("industrial_metals".into());
    let repair = CargoItem::Resource("repair_material".into());
    let initial = world
        .get::<hardware::ShipInventory>(facility)
        .unwrap()
        .0
        .clone();
    let metal_before = initial.cargo_quantity(&metal, &catalogue).unwrap();
    let repair_before = initial.cargo_quantity(&repair, &catalogue).unwrap();

    seed_demo(world, facility, account).unwrap();
    assert_eq!(
        postcard::to_stdvec(&world.get::<hardware::ShipInventory>(facility).unwrap().0).unwrap(),
        postcard::to_stdvec(&initial).unwrap()
    );
    execute(
        world,
        account,
        IndustryCommand::StartRecipe {
            facility: id,
            recipe: "repair_material".into(),
            batches: 1,
        },
        None,
    )
    .unwrap();
    let job = &world.get::<IndustryFacility>(facility).unwrap().jobs[0];
    let duration = job.view.duration_ticks;
    let energy = job.energy_j;
    let input = job
        .inputs
        .iter()
        .find(|stack| stack.item == metal)
        .unwrap()
        .quantity;
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(facility)
            .unwrap()
            .0
            .cargo_quantity(&metal, &catalogue)
            .unwrap(),
        metal_before
    );

    let mut paid_energy = 0.;
    for _ in 0..duration {
        app.update();
        let world = app.world();
        let design = &world.get::<vessel::ShipDesign>(facility).unwrap().0;
        for &device in &world.get::<hardware::PartDevices>(facility).unwrap().0 {
            let index = world.get::<hardware::InstalledPart>(device).unwrap().index;
            if matches!(
                design.parts[index].definition.equipment,
                Equipment::Utility {
                    utility: UtilityDef::Factory {
                        capability: IndustryCapability::Fabricator,
                        ..
                    }
                }
            ) {
                paid_energy += world
                    .get::<hardware::DevicePower>(device)
                    .unwrap()
                    .supplied_w
                    * 0.1;
            }
        }
    }

    let world = app.world();
    let inventory = &world.get::<hardware::ShipInventory>(facility).unwrap().0;
    assert!(
        world
            .get::<IndustryFacility>(facility)
            .unwrap()
            .jobs
            .is_empty()
    );
    assert_eq!(
        inventory.cargo_quantity(&metal, &catalogue).unwrap(),
        metal_before - input
    );
    assert_eq!(
        inventory.cargo_quantity(&repair, &catalogue).unwrap(),
        repair_before + 1
    );
    assert!(inventory.reservations.is_empty());
    assert_eq!(paid_energy, energy as f64);
}
