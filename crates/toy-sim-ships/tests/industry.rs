use toy_sim_model::industry::{CargoItem, IndustryCapability, ItemStack};
use toy_sim_ships::{Catalogue, Inventory, ShipState, aggregate_stacks, industry, missiles};

fn resource(id: &str, quantity: u64) -> ItemStack {
    ItemStack {
        item: CargoItem::Resource(id.into()),
        quantity,
    }
}

fn cargo_bytes(inventory: &Inventory) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(inventory, &mut bytes).unwrap();
    bytes
}

#[test]
fn catalogue_recipes_conserve_material_and_do_not_synthesize_fissiles_from_inert_stock() {
    let cat = Catalogue::builtin();
    let recipes = industry::recipes(&cat).unwrap();
    assert!(recipes.len() > cat.parts.len());

    for recipe in &recipes {
        industry::validate_recipe(recipe, &cat).unwrap();
        assert_eq!(
            industry::stack_mass_mg(&recipe.inputs, &cat).unwrap(),
            industry::stack_mass_mg(&recipe.outputs, &cat).unwrap(),
            "{}",
            recipe.id
        );
        if recipe
            .outputs
            .iter()
            .any(|stack| stack.item == resource("reactor_fuel", 1).item)
        {
            assert!(recipe.inputs.iter().any(|stack| {
                ["spent_fuel", "bred_fuel", "fissile_ore"]
                    .iter()
                    .any(|id| stack.item == resource(id, 1).item)
            }));
        }
    }

    let missile = recipes
        .iter()
        .find(|recipe| recipe.id == "interceptor_missile")
        .unwrap();
    assert_eq!(
        industry::stack_mass_mg(&missile.inputs, &cat).unwrap(),
        400_000_000
    );
    assert_eq!(missile.stored_energy_j, missiles::BATTERY_J);
    assert!(
        missile
            .inputs
            .contains(&resource(missiles::PROPELLANT, missiles::FUEL_KG))
    );
    assert_eq!(missile.outputs, vec![resource(missiles::AMMUNITION, 1)]);
}

#[test]
fn repeated_spent_fuel_recycling_has_bounded_energy_and_conserves_every_unit() {
    let cat = Catalogue::builtin();
    let recipes = industry::recipes(&cat).unwrap();
    let recipe = recipes
        .iter()
        .find(|recipe| recipe.id == "reprocess_spent_fuel")
        .unwrap();
    assert_eq!(recipe.inputs, vec![resource("spent_fuel", 10)]);
    assert_eq!(
        recipe.outputs,
        vec![
            resource("reactor_fuel", 2),
            resource("radioactive_waste", 8)
        ]
    );

    let initial = 1_000_000_u64;
    let mut fuel = initial;
    let mut residual = 0;
    let mut waste = 0;
    let mut burned = 0;
    while fuel > 0 {
        burned += fuel;
        residual += fuel;
        let batches = residual / 10;
        residual %= 10;
        fuel = batches * 2;
        waste += batches * 8;
        assert_eq!(fuel + residual + waste, initial);
    }
    assert_eq!(residual + waste, initial);
    let energy_tj = u128::from(burned) * 60;
    assert!(energy_tj <= u128::from(initial) * 75);
    assert!(energy_tj > u128::from(initial) * 74);
}

#[test]
fn reservations_preserve_mass_and_block_resource_transfer_refill_and_part_transfer() {
    let cat = Catalogue::builtin();
    let mut source = Inventory::empty(&cat);
    let mut target = Inventory::empty(&cat);
    let water = cat.resources.iter().position(|r| r.id == "water").unwrap();
    let kit = ItemStack {
        item: CargoItem::Part(missiles::BODY_PART.into()),
        quantity: 2,
    };
    source.tank_capacities_m3[water] = 1.;
    source.insert_cargo(water, 10, 100., &cat).unwrap();
    source.insert_item(&kit.item, 2, 100., &cat).unwrap();
    let mass = source.mass(&cat);
    let volume = source.cargo_volume(&cat);
    let reserved = vec![resource("water", 3), resource("water", 4), kit.clone()];
    source.reserve_cargo(&reserved, &cat).unwrap();

    assert_eq!(source.mass(&cat), mass);
    assert_eq!(source.cargo_volume(&cat), volume);
    assert!(
        source
            .transfer_cargo(&mut target, water, 4, 100., &cat)
            .is_err()
    );
    assert!(source.refill_cargo(water, 4, &cat).is_err());
    assert!(
        source
            .transfer_item(&mut target, &kit.item, 1, 100., &cat)
            .is_err()
    );
    source.refill_cargo(water, 3, &cat).unwrap();
    assert_eq!(source.quantities[water], 3);
    assert_eq!(
        source
            .cargo_available(&resource("water", 1).item, &cat)
            .unwrap(),
        0
    );

    source.release_cargo(&reserved, &cat).unwrap();
    source
        .transfer_cargo(&mut target, water, 7, 100., &cat)
        .unwrap();
    source
        .transfer_item(&mut target, &kit.item, 2, 100., &cat)
        .unwrap();
    assert!(source.packaged_parts.is_empty());
    assert!(source.reservations.is_empty());
    assert_eq!(source.mass(&cat) + target.mass(&cat), mass);
}

#[test]
fn overflow_and_partial_reservation_failures_are_atomic() {
    let cat = Catalogue::builtin();
    let mut inventory = Inventory::empty(&cat);
    let metal = resource(industry::METALS, 5);
    inventory.insert_item(&metal.item, 5, 1., &cat).unwrap();
    let before = cargo_bytes(&inventory);
    assert!(
        inventory
            .reserve_cargo(&[metal, resource("water", 1)], &cat)
            .is_err()
    );
    assert_eq!(cargo_bytes(&inventory), before);
    assert!(
        aggregate_stacks(
            &[
                resource(industry::METALS, u64::MAX),
                resource(industry::METALS, 1)
            ],
            &cat
        )
        .is_err()
    );
    assert!(
        inventory
            .reserve_cargo(&[resource("missing", 1)], &cat)
            .is_err()
    );
    assert_eq!(cargo_bytes(&inventory), before);
}

#[test]
fn completion_reclaims_inputs_before_capacity_check_and_is_atomic_when_blocked() {
    let cat = Catalogue::builtin();
    let mut inventory = Inventory::empty(&cat);
    let input = vec![resource("industrial_ore", 100)];
    let output = vec![
        resource(industry::METALS, 60_000_000),
        resource("tailings", 40),
    ];
    inventory
        .insert_item(&input[0].item, 100, 0.03, &cat)
        .unwrap();
    inventory.reserve_cargo(&input, &cat).unwrap();
    let before = cargo_bytes(&inventory);

    assert!(
        inventory
            .complete_cargo(&input, &output, 0.025, &cat)
            .is_err()
    );
    assert_eq!(cargo_bytes(&inventory), before);
    inventory
        .complete_cargo(&input, &output, 0.03, &cat)
        .unwrap();
    assert_eq!(inventory.mass(&cat), 100.);
    assert!(inventory.reservations.is_empty());
    let completed = cargo_bytes(&inventory);
    assert!(
        inventory
            .complete_cargo(&input, &output, 0.03, &cat)
            .is_err()
    );
    assert_eq!(cargo_bytes(&inventory), completed);
}

#[test]
fn cold_construction_bom_covers_fractional_tank_containment_without_initial_fuel() {
    let cat = Catalogue::builtin();
    let mut blueprint = industry::starter_ship();
    let tank = blueprint
        .parts
        .iter_mut()
        .flat_map(|part| &mut part.tanks)
        .find(|tank| tank.resource == "reactor_fuel")
        .unwrap();
    tank.volume_m3 = 0.0123456789;
    tank.initial_fill = 1.;
    let design = blueprint.compile(&cat).unwrap();
    let requirements = industry::construction_requirements(&design, &cat).unwrap();
    assert_eq!(
        industry::stack_mass_mg(&requirements.inputs, &cat).unwrap(),
        u128::from(industry::mass_mg(design.dry_mass).unwrap())
    );

    let warm = ShipState::new(&design, &cat);
    assert!(
        warm.inventory
            .quantities
            .iter()
            .any(|quantity| *quantity > 0)
    );
    let cold = ShipState::cold(&design, &cat);
    assert!(
        cold.inventory
            .quantities
            .iter()
            .all(|quantity| *quantity == 0)
    );
    assert_eq!(cold.inventory.energy_j, 0);
    assert_eq!(cold.thermal.shield_deployed_kg, 0.);
    assert_eq!(cold.thermal.shield_reserve_mg, 0);
    assert_eq!(cold.thermal.shield_energy_j, 0.);
    assert_eq!(
        cold.inventory.tank_capacities_m3,
        warm.inventory.tank_capacities_m3
    );
}

#[test]
fn starter_stock_funds_a_complete_launch_and_fits_the_station_warehouse() {
    let cat = Catalogue::builtin();
    let design = industry::starter_ship().compile(&cat).unwrap();
    let requirements = industry::construction_requirements(&design, &cat).unwrap();
    let mut inventory = Inventory::empty(&cat);
    for stack in industry::starter_stock(&cat).unwrap() {
        inventory
            .insert_item(&stack.item, stack.quantity, 40_000., &cat)
            .unwrap();
    }
    inventory.reserve_cargo(&requirements.inputs, &cat).unwrap();
    inventory.validate_cargo(&cat).unwrap();
    assert!(inventory.cargo_volume(&cat) < 40_000.);
}

#[test]
fn part_recipes_follow_equipment_family_for_new_catalogue_members() {
    let mut cat = Catalogue::builtin();
    let mut part = cat.part(missiles::BODY_PART).unwrap().clone();
    part.id = "new_interceptor_body".into();
    part.mass_kg = 123.456789;
    cat.parts.push(part);
    let recipes = industry::recipes(&cat).unwrap();
    let recipe = recipes
        .iter()
        .find(|recipe| recipe.id == "part:new_interceptor_body")
        .unwrap();
    assert_eq!(recipe.capability, IndustryCapability::Fabricator);
    assert_eq!(
        industry::stack_mass_mg(&recipe.inputs, &cat).unwrap(),
        123_456_789
    );
}
