use crate::{Catalogue, CompiledShipDesign, Equipment, PartDef, ShipBlueprint};
use anyhow::{Context, Result, ensure};
use osg_model::industry::{CargoItem, IndustryCapability, ItemStack, Recipe};
use std::collections::BTreeMap;

pub const MILLIGRAM_KG: f64 = 0.000001;
pub const METALS: &str = "industrial_metals";
pub const ELECTRONICS: &str = "industrial_electronics";
pub const REACTOR_MATERIAL: &str = "reactor_material";
pub const CHEMICAL_FEEDSTOCK: &str = "chemical_feedstock";

pub fn mass_mg(mass_kg: f64) -> Result<u64> {
    let value = mass_kg / MILLIGRAM_KG;
    ensure!(
        value.is_finite() && value >= 0. && value < u64::MAX as f64,
        "material mass outside milligram range"
    );

    Ok(value.round() as u64)
}

pub fn tank_containment_mass_kg(volume_m3: f64, containment_kg_m3: f64) -> Result<f64> {
    Ok(mass_mg(volume_m3 * containment_kg_m3)? as f64 * MILLIGRAM_KG)
}

pub fn item_name<'a>(item: &CargoItem, cat: &'a Catalogue) -> Result<&'a str> {
    match item {
        CargoItem::Resource(id) => Ok(&cat
            .resources
            .iter()
            .find(|r| r.id == *id)
            .context("unknown resource")?
            .title),
        CargoItem::Part(id) => Ok(&cat.part(id).context("unknown part kit")?.title),
    }
}

pub fn item_mass_kg(item: &CargoItem, cat: &Catalogue) -> Result<f64> {
    match item {
        CargoItem::Resource(id) => Ok(cat
            .resources
            .iter()
            .find(|r| r.id == *id)
            .context("unknown resource")?
            .mass_kg),
        CargoItem::Part(id) => Ok(cat.part(id).context("unknown part kit")?.mass_kg),
    }
}

pub fn packaged_part_volume(part: &PartDef) -> f64 {
    // Kits contain disassembled components packed at two tonnes per cubic metre.
    // The installed bounding box includes empty space and does not set kit volume.
    part.mass_kg / 2000.
}

pub fn item_volume_m3(item: &CargoItem, cat: &Catalogue) -> Result<f64> {
    match item {
        CargoItem::Resource(id) => Ok(cat
            .resources
            .iter()
            .find(|r| r.id == *id)
            .context("unknown resource")?
            .volume_m3),
        CargoItem::Part(id) => Ok(packaged_part_volume(
            cat.part(id).context("unknown part kit")?,
        )),
    }
}

pub fn stack_mass_mg(stacks: &[ItemStack], cat: &Catalogue) -> Result<u128> {
    stacks.iter().try_fold(0_u128, |sum, stack| {
        ensure!(stack.quantity > 0, "zero recipe quantity");
        let mass =
            u128::from(mass_mg(item_mass_kg(&stack.item, cat)?)?) * u128::from(stack.quantity);
        sum.checked_add(mass).context("recipe mass overflow")
    })
}

pub fn validate_recipe(recipe: &Recipe, cat: &Catalogue) -> Result<()> {
    ensure!(
        !recipe.id.is_empty() && recipe.id.len() <= 96 && !recipe.name.is_empty(),
        "invalid recipe identity"
    );
    ensure!(
        recipe.capability != IndustryCapability::Shipyard,
        "shipyard recipes require a blueprint"
    );
    ensure!(
        !recipe.inputs.is_empty()
            && !recipe.outputs.is_empty()
            && recipe.inputs.len() <= 32
            && recipe.outputs.len() <= 32,
        "invalid recipe item count"
    );
    ensure!(
        recipe.duration_ticks > 0
            && recipe.energy_j > 0
            && recipe.stored_energy_j <= recipe.energy_j,
        "invalid recipe energy or duration"
    );
    ensure!(
        stack_mass_mg(&recipe.inputs, cat)? == stack_mass_mg(&recipe.outputs, cat)?,
        "recipe {} does not conserve material mass",
        recipe.id
    );

    Ok(())
}

pub struct ConstructionRequirements {
    pub inputs: Vec<ItemStack>,
    pub duration_ticks: u64,
    pub energy_j: u64,
}

fn resource(id: &str, quantity: u64) -> ItemStack {
    ItemStack {
        item: CargoItem::Resource(id.into()),
        quantity,
    }
}

fn material_inputs(mass: u64, electronics_percent: u64, reactor_percent: u64) -> Vec<ItemStack> {
    let electronics = (u128::from(mass) * u128::from(electronics_percent) / 100) as u64;
    let reactor = (u128::from(mass) * u128::from(reactor_percent) / 100) as u64;
    [
        (METALS, mass - electronics - reactor),
        (ELECTRONICS, electronics),
        (REACTOR_MATERIAL, reactor),
    ]
    .into_iter()
    .filter(|(_, quantity)| *quantity > 0)
    .map(|(id, quantity)| resource(id, quantity))
    .collect()
}

pub fn construction_requirements(
    design: &CompiledShipDesign,
    cat: &Catalogue,
) -> Result<ConstructionRequirements> {
    let mut kits = BTreeMap::<String, u64>::new();
    let mut containment = 0_u64;

    for part in &design.parts {
        *kits.entry(part.definition.id.clone()).or_default() += 1;
        for tank in &part.placed.tanks {
            let material = cat
                .resources
                .iter()
                .find(|r| r.id == tank.resource)
                .context("unknown tank resource")?;
            containment = containment
                .checked_add(mass_mg(tank_containment_mass_kg(
                    tank.volume_m3,
                    material.storage.containment_kg_m3,
                )?)?)
                .context("containment mass overflow")?;
        }
    }

    let mut inputs: Vec<_> = kits
        .into_iter()
        .map(|(id, quantity)| ItemStack {
            item: CargoItem::Part(id),
            quantity,
        })
        .collect();

    // Distributed avionics are physical equipment even though the design compiler
    // represents them independently of visible parts.
    inputs.extend(material_inputs(mass_mg(crate::AVIONICS_MASS_KG)?, 40, 0));
    if containment > 0 {
        inputs.push(resource(METALS, containment));
    }

    let mut totals = BTreeMap::<CargoItem, u64>::new();
    for stack in inputs {
        let total = totals.entry(stack.item).or_default();
        *total = total
            .checked_add(stack.quantity)
            .context("construction quantity overflow")?;
    }

    let inputs: Vec<_> = totals
        .into_iter()
        .map(|(item, quantity)| ItemStack { item, quantity })
        .collect();
    ensure!(
        stack_mass_mg(&inputs, cat)? == u128::from(mass_mg(design.dry_mass)?),
        "construction mass does not match dry hull"
    );

    let duration_ticks = (design.parts.len() as u64 * 100).max(100);
    let energy_j = duration_ticks
        .checked_mul(100_000)
        .context("construction energy overflow")?;

    Ok(ConstructionRequirements {
        inputs,
        duration_ticks,
        energy_j,
    })
}

fn part_recipe(part: &PartDef) -> Result<Recipe> {
    let (electronics, reactor) = match part.equipment {
        Equipment::Structure | Equipment::Storage { .. } | Equipment::CoolantTank { .. } => (0, 0),
        Equipment::Reactor { .. }
        | Equipment::ThermalEngine { .. }
        | Equipment::FuelProcessor { .. } => (10, 40),
        Equipment::Engine { .. } | Equipment::MicropulseEngine { .. } | Equipment::Rcs { .. } => {
            (15, 10)
        }
        Equipment::Weapon { .. } | Equipment::Torquer { .. } | Equipment::Shield { .. } => (25, 10),
        Equipment::Battery { .. } | Equipment::Generator { .. } => (25, 0),
        Equipment::Utility { .. } => (30, 10),
        Equipment::Radiator { .. }
        | Equipment::EmergencyCooling { .. }
        | Equipment::HeatSink { .. } => (5, 0),
    };

    let mass = mass_mg(part.mass_kg)?;
    let energy_j = mass
        .checked_mul(2)
        .context("part processing energy overflow")?
        .max(1_000_000);

    Ok(Recipe {
        id: format!("part:{}", part.id),
        name: format!("Manufacture {} kit", part.title),
        capability: IndustryCapability::Fabricator,
        inputs: material_inputs(mass, electronics, reactor),
        outputs: vec![ItemStack {
            item: CargoItem::Part(part.id.clone()),
            quantity: 1,
        }],
        duration_ticks: energy_j.div_ceil(500_000).max(10),
        energy_j,
        stored_energy_j: 0,
    })
}

pub fn recipes(cat: &Catalogue) -> Result<Vec<Recipe>> {
    use IndustryCapability::*;

    let mut recipes = Vec::new();
    let mut add = |id: &str,
                   name: &str,
                   capability,
                   inputs: Vec<ItemStack>,
                   outputs: Vec<ItemStack>,
                   energy_j: u64,
                   stored_energy_j: u64| {
        recipes.push(Recipe {
            id: id.into(),
            name: name.into(),
            capability,
            inputs,
            outputs,
            duration_ticks: energy_j.div_ceil(500_000).max(10),
            energy_j,
            stored_energy_j,
        });
    };

    let mg = |kg: u64| kg * 1_000_000;

    add(
        "refine_metals",
        "Refine industrial ore",
        Refinery,
        vec![resource("industrial_ore", 100)],
        vec![resource(METALS, mg(60)), resource("tailings", 40)],
        600_000_000,
        0,
    );
    add(
        "electronics",
        "Electronic components",
        Fabricator,
        vec![resource(METALS, mg(8)), resource(CHEMICAL_FEEDSTOCK, mg(2))],
        vec![resource(ELECTRONICS, mg(10))],
        200_000_000,
        0,
    );
    add(
        "reactor_material",
        "Reactor alloys and ceramics",
        Refinery,
        vec![resource(METALS, mg(9)), resource(CHEMICAL_FEEDSTOCK, mg(1))],
        vec![resource(REACTOR_MATERIAL, mg(10))],
        100_000_000,
        0,
    );
    add(
        "reprocess_spent_fuel",
        "Recover residual reactor fuel",
        Refinery,
        vec![resource("spent_fuel", 10)],
        vec![
            resource("reactor_fuel", 2),
            resource("radioactive_waste", 8),
        ],
        100_000_000,
        0,
    );
    add(
        "refine_bred_fuel",
        "Refine breeder fuel",
        Refinery,
        vec![resource("bred_fuel", 100)],
        vec![
            resource("reactor_fuel", 95),
            resource("radioactive_waste", 5),
        ],
        2_000_000_000,
        0,
    );
    add(
        "refine_fissiles",
        "Separate mined fissile feedstock",
        Refinery,
        vec![resource("fissile_ore", 100)],
        vec![
            resource("reactor_fuel", 1),
            resource("fertile_feedstock", 9),
            resource("tailings", 90),
        ],
        1_000_000_000,
        0,
    );
    add(
        "pulse_charges",
        "Micropulse fuel charges",
        FuelPlant,
        vec![resource("reactor_fuel", 1), resource("repair_material", 99)],
        vec![resource("micropulse_charge", 100)],
        2_000_000_000,
        0,
    );
    add(
        "rocket_propellant",
        "Storable gel rocket propellant",
        FuelPlant,
        vec![resource(CHEMICAL_FEEDSTOCK, mg(10))],
        vec![resource("rocket_propellant", 10)],
        80_000_000,
        45_000_000,
    );
    add(
        "generator_fuel",
        "Generator fuel",
        FuelPlant,
        vec![resource(CHEMICAL_FEEDSTOCK, mg(10))],
        vec![resource("fuel", 10)],
        400_000_000,
        250_000_000,
    );
    add(
        "liquid_hydrogen",
        "Electrolyse and liquefy hydrogen",
        FuelPlant,
        vec![resource("water", 9)],
        vec![resource("hydrogen", 1), resource("oxygen", 8)],
        250_000_000,
        143_000_000,
    );
    add(
        "railgun_pellets",
        "Railgun darts",
        Fabricator,
        vec![resource(METALS, mg(1))],
        vec![resource("railgun_dart", 1000)],
        5_000_000,
        0,
    );
    add(
        "bearings",
        "Ball bearings",
        Fabricator,
        vec![resource(METALS, mg(1))],
        vec![resource("bearing", 100)],
        5_000_000,
        0,
    );
    add(
        "coil_slugs",
        "Heavy coilgun rounds",
        Fabricator,
        vec![resource(METALS, mg(10))],
        vec![resource("coil_slug", 1)],
        20_000_000,
        0,
    );
    add(
        "autocannon_rounds",
        "Autocannon ammunition",
        Fabricator,
        vec![resource(METALS, mg(60)), resource("rocket_propellant", 40)],
        vec![resource("autocannon_round", 1000)],
        50_000_000,
        0,
    );
    add(
        "repair_material",
        "Repair material",
        Fabricator,
        vec![resource(METALS, mg(1))],
        vec![resource("repair_material", 1)],
        1_000_000,
        0,
    );

    add(
        "shield_coolant",
        "Radiator shield working fluid",
        FuelPlant,
        vec![resource(METALS, mg(10))],
        vec![resource("shield_coolant", 10)],
        20_000_000,
        0,
    );

    let missile = crate::missiles::blueprint().compile(cat)?;
    let mut missile_inputs = construction_requirements(&missile, cat)?.inputs;
    missile_inputs.push(resource(
        crate::missiles::PROPELLANT,
        crate::missiles::FUEL_KG,
    ));
    add(
        "interceptor_missile",
        "Assemble and charge Kite interceptor",
        Fabricator,
        missile_inputs,
        vec![resource(crate::missiles::AMMUNITION, 1)],
        100_000_000,
        crate::missiles::BATTERY_J,
    );

    recipes.extend(
        cat.parts
            .iter()
            .map(part_recipe)
            .collect::<Result<Vec<_>>>()?,
    );

    for recipe in &recipes {
        validate_recipe(recipe, cat)?;
    }

    Ok(recipes)
}

pub fn starter_ship() -> ShipBlueprint {
    let mut ship = crate::ntr_patrol();
    ship.name = "Kestrel service launch".into();
    ship
}

pub fn starter_stock(cat: &Catalogue) -> Result<Vec<ItemStack>> {
    let design = starter_ship().compile(cat)?;
    let mut stock = construction_requirements(&design, cat)?.inputs;
    stock.extend([
        resource(METALS, 100_000_000_000),
        resource(ELECTRONICS, 10_000_000_000),
        resource(REACTOR_MATERIAL, 10_000_000_000),
        resource(CHEMICAL_FEEDSTOCK, 10_000_000_000),
        resource("industrial_ore", 10_000),
        resource("reactor_fuel", 1000),
        resource("spent_fuel", 100),
        resource("bred_fuel", 100),
        resource("repair_material", 1000),
        resource("water", 20_000),
        resource("rocket_propellant", 1000),
        resource("shield_coolant", 10_000),
    ]);

    Ok(crate::aggregate_stacks(&stock, cat)?
        .into_iter()
        .map(|(item, quantity)| ItemStack { item, quantity })
        .collect())
}
