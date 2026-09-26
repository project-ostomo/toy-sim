use super::super::economy::Economy;
use super::*;
use crate::sim::society::OwnershipDirectory;

pub fn tier_matches(
    directory: &OwnershipDirectory,
    customer: Principal,
    rule: &CustomerMatch,
) -> bool {
    let lineage = directory.lineage(customer);
    match rule {
        CustomerMatch::Principal(principal) => lineage.contains(principal),
        CustomerMatch::Bloc(id) => directory.diplomacy.blocs.get(id).is_some_and(|bloc| {
            lineage.iter().any(|principal| matches!(principal, Principal::Sovereignty(id) if bloc.members.contains(id)))
        }),
        CustomerMatch::Declaration { source, category } => lineage.iter().any(|target| {
            directory.diplomacy.declarations.get(&(*source, *category, *target)).is_some_and(|declaration| declaration.enabled)
        }),
        CustomerMatch::Everyone => true,
    }
}

pub fn storage_credit(
    economy: &Economy,
    inventory: &mut Inventory,
    station: Id,
    owner: Principal,
    stacks: &[ItemStack],
) -> Result<imbl::OrdMap<CargoItem, u64>> {
    let mut stock = economy
        .storage
        .get(&(station, owner))
        .cloned()
        .unwrap_or_default();
    let mut custody = inventory.custody.clone();
    for stack in stacks {
        let amount = stock.entry(stack.item.clone()).or_default();
        *amount = amount
            .checked_add(stack.quantity)
            .context("storage overflow")?;
        let amount = custody.entry(stack.item.clone()).or_default();
        *amount = amount
            .checked_add(stack.quantity)
            .context("custody overflow")?;
    }
    ensure!(stock.len() <= 1024, "storage item limit");
    inventory.custody = custody;
    Ok(stock)
}

pub fn write_stock(
    economy: &mut Economy,
    station: Id,
    owner: Principal,
    stock: imbl::OrdMap<CargoItem, u64>,
) {
    if stock.is_empty() {
        economy.storage.remove(&(station, owner));
    } else {
        economy.storage.insert((station, owner), stock);
    }
}

#[test]
fn storage_credit_preserves_all_items_when_later_custody_overflows() {
    let economy = Economy::default();
    let mut inventory = Inventory::empty(&Catalogue::builtin());
    let station = Id::new();
    let owner = Principal::Player(Id::new());
    let first = CargoItem::Resource("industrial_metals".into());
    let second = CargoItem::Resource("industrial_electronics".into());
    inventory.custody.insert(second.clone(), u64::MAX);
    let before = inventory.custody.clone();

    let result = storage_credit(
        &economy,
        &mut inventory,
        station,
        owner,
        &[
            ItemStack {
                item: first,
                quantity: 1,
            },
            ItemStack {
                item: second,
                quantity: 1,
            },
        ],
    );
    assert!(result.is_err());
    assert!(economy.storage.is_empty());
    assert_eq!(inventory.custody, before);
}
