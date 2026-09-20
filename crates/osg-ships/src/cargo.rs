use crate::{Catalogue, Inventory, industry};
use anyhow::{Context, Result, ensure};
use osg_model::industry::{CargoItem, CargoStack, ItemStack};
use std::collections::BTreeMap;

pub fn aggregate_stacks(stacks: &[ItemStack], cat: &Catalogue) -> Result<BTreeMap<CargoItem, u64>> {
    let mut totals = BTreeMap::<CargoItem, u64>::new();
    for stack in stacks {
        ensure!(stack.quantity > 0, "zero item quantity");
        industry::item_mass_kg(&stack.item, cat)?;

        let total = totals.entry(stack.item.clone()).or_default();
        *total = total
            .checked_add(stack.quantity)
            .context("quantity overflow")?;
    }
    Ok(totals)
}

impl Inventory {
    pub fn cargo_quantity(&self, item: &CargoItem, cat: &Catalogue) -> Result<u64> {
        match item {
            CargoItem::Resource(id) => {
                let index = cat
                    .resources
                    .iter()
                    .position(|resource| resource.id == *id)
                    .context("unknown resource")?;
                self.cargo
                    .get(index)
                    .copied()
                    .context("invalid cargo shape")
            }
            CargoItem::Part(id) => {
                ensure!(cat.part(id).is_some(), "unknown part kit");
                Ok(self.packaged_parts.get(id).copied().unwrap_or(0))
            }
        }
    }

    pub fn cargo_available(&self, item: &CargoItem, cat: &Catalogue) -> Result<u64> {
        let reserved = self.reservations.get(item).copied().unwrap_or(0);
        self.cargo_quantity(item, cat)?
            .checked_sub(reserved)
            .context("reserved cargo exceeds stock")
    }

    pub fn cargo_stacks(&self, cat: &Catalogue) -> Result<Vec<CargoStack>> {
        self.validate_cargo(cat)?;

        let resources = cat
            .resources
            .iter()
            .zip(&self.cargo)
            .filter(|(_, quantity)| **quantity > 0)
            .map(|(resource, &quantity)| (CargoItem::Resource(resource.id.clone()), quantity));
        let parts = self
            .packaged_parts
            .iter()
            .map(|(id, &quantity)| (CargoItem::Part(id.clone()), quantity));

        resources
            .chain(parts)
            .map(|(item, quantity)| {
                Ok(CargoStack {
                    reserved: self.reservations.get(&item).copied().unwrap_or(0),
                    name: industry::item_name(&item, cat)?.to_owned(),
                    unit_mass_kg: industry::item_mass_kg(&item, cat)?,
                    unit_volume_m3: industry::item_volume_m3(&item, cat)?,
                    item,
                    quantity,
                })
            })
            .collect()
    }

    pub fn product_stacks(&self, cat: &Catalogue) -> Result<Vec<CargoStack>> {
        self.validate_cargo(cat)?;

        Ok(cat
            .resources
            .iter()
            .zip(&self.quantities)
            .filter(|(resource, quantity)| resource.exportable_product && **quantity > 0)
            .map(|(resource, &quantity)| CargoStack {
                item: CargoItem::Resource(resource.id.clone()),
                quantity,
                reserved: 0,
                name: resource.title.clone(),
                unit_mass_kg: resource.mass_kg,
                unit_volume_m3: resource.volume_m3,
            })
            .collect())
    }

    pub fn validate_cargo(&self, cat: &Catalogue) -> Result<()> {
        ensure!(
            self.cargo.len() == cat.resources.len()
                && self.quantities.len() == cat.resources.len()
                && self.tank_capacities_m3.len() == cat.resources.len(),
            "invalid inventory shape"
        );
        for (id, quantity) in &self.packaged_parts {
            ensure!(*quantity > 0 && cat.part(id).is_some(), "invalid part kit");
        }
        for (item, quantity) in &self.reservations {
            ensure!(
                *quantity > 0 && *quantity <= self.cargo_quantity(item, cat)?,
                "invalid cargo reservation"
            );
        }
        Ok(())
    }

    fn set_cargo_quantity(&mut self, item: &CargoItem, quantity: u64, cat: &Catalogue) {
        match item {
            CargoItem::Resource(id) => {
                let index = cat
                    .resources
                    .iter()
                    .position(|resource| resource.id == *id)
                    .expect("validated resource");
                self.cargo[index] = quantity;
            }
            CargoItem::Part(id) => {
                if quantity == 0 {
                    self.packaged_parts.remove(id);
                } else {
                    self.packaged_parts.insert(id.clone(), quantity);
                }
            }
        }
    }

    pub fn insert_item(
        &mut self,
        item: &CargoItem,
        quantity: u64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(quantity > 0, "zero item quantity");
        ensure!(
            capacity.is_finite() && capacity >= 0.,
            "invalid cargo capacity"
        );

        let next = self
            .cargo_quantity(item, cat)?
            .checked_add(quantity)
            .context("quantity overflow")?;
        let volume =
            self.cargo_volume(cat) + industry::item_volume_m3(item, cat)? * quantity as f64;
        ensure!(
            volume.is_finite() && volume <= capacity + capacity.max(1.) * 1e-9,
            "cargo hold full"
        );

        self.set_cargo_quantity(item, next, cat);
        Ok(())
    }

    pub fn withdraw_cargo(
        &mut self,
        item: &CargoItem,
        quantity: u64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(
            quantity > 0 && quantity <= self.cargo_available(item, cat)?,
            "insufficient available cargo"
        );
        let next = self.cargo_quantity(item, cat)? - quantity;

        self.set_cargo_quantity(item, next, cat);
        Ok(())
    }

    pub fn transfer_item(
        &mut self,
        target: &mut Self,
        item: &CargoItem,
        quantity: u64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(
            quantity > 0 && quantity <= self.cargo_available(item, cat)?,
            "insufficient available cargo"
        );
        let remaining = self.cargo_quantity(item, cat)? - quantity;

        target.insert_item(item, quantity, capacity, cat)?;
        self.set_cargo_quantity(item, remaining, cat);
        Ok(())
    }

    pub fn refill_cargo(&mut self, resource: usize, quantity: u64, cat: &Catalogue) -> Result<()> {
        let id = cat
            .resources
            .get(resource)
            .context("unknown resource")?
            .id
            .clone();
        let item = CargoItem::Resource(id);
        ensure!(
            quantity > 0 && quantity <= self.cargo_available(&item, cat)?,
            "insufficient available cargo"
        );
        ensure!(
            quantity <= self.tank_room(resource, cat),
            "tank capacity exceeded"
        );

        let next = self.quantities[resource]
            .checked_add(quantity)
            .context("quantity overflow")?;
        let remaining = self.cargo_quantity(&item, cat)? - quantity;

        self.set_cargo_quantity(&item, remaining, cat);
        self.quantities[resource] = next;
        Ok(())
    }

    fn product_withdrawal(
        &self,
        resource: usize,
        quantity: u64,
        cat: &Catalogue,
    ) -> Result<(CargoItem, u64)> {
        let definition = cat.resources.get(resource).context("unknown resource")?;
        ensure!(
            definition.exportable_product,
            "operational consumables cannot be unloaded"
        );
        ensure!(quantity > 0, "zero product quantity");
        let remaining = self
            .quantities
            .get(resource)
            .context("invalid inventory shape")?
            .checked_sub(quantity)
            .context("insufficient product stock")?;

        Ok((CargoItem::Resource(definition.id.clone()), remaining))
    }

    pub fn package_product(
        &mut self,
        resource: usize,
        quantity: u64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        let (item, remaining) = self.product_withdrawal(resource, quantity, cat)?;
        self.insert_item(&item, quantity, capacity, cat)?;
        self.quantities[resource] = remaining;
        Ok(())
    }

    pub fn unload_product(
        &mut self,
        target: &mut Self,
        resource: usize,
        quantity: u64,
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        let (item, remaining) = self.product_withdrawal(resource, quantity, cat)?;
        target.insert_item(&item, quantity, capacity, cat)?;
        self.quantities[resource] = remaining;
        Ok(())
    }

    pub fn reserve_cargo(&mut self, inputs: &[ItemStack], cat: &Catalogue) -> Result<()> {
        let amounts = aggregate_stacks(inputs, cat)?;
        let mut reservations = self.reservations.clone();
        for (item, amount) in amounts {
            ensure!(
                amount <= self.cargo_available(&item, cat)?,
                "insufficient available cargo"
            );
            let reserved = reservations.entry(item).or_default();
            *reserved = reserved
                .checked_add(amount)
                .context("reservation overflow")?;
        }

        self.reservations = reservations;
        Ok(())
    }

    pub fn release_cargo(&mut self, inputs: &[ItemStack], cat: &Catalogue) -> Result<()> {
        let amounts = aggregate_stacks(inputs, cat)?;
        for (item, amount) in &amounts {
            ensure!(
                *amount <= self.reservations.get(item).copied().unwrap_or(0),
                "reservation missing"
            );
        }

        for (item, amount) in amounts {
            let reserved = self
                .reservations
                .get_mut(&item)
                .expect("validated reservation");
            *reserved -= amount;
            if *reserved == 0 {
                self.reservations.remove(&item);
            }
        }
        Ok(())
    }

    pub fn complete_cargo(
        &mut self,
        inputs: &[ItemStack],
        outputs: &[ItemStack],
        capacity: f64,
        cat: &Catalogue,
    ) -> Result<()> {
        ensure!(
            capacity.is_finite() && capacity >= 0.,
            "invalid cargo capacity"
        );
        let inputs = aggregate_stacks(inputs, cat)?;
        let outputs = aggregate_stacks(outputs, cat)?;
        let inputs: Vec<_> = inputs
            .into_iter()
            .map(|(item, quantity)| ItemStack { item, quantity })
            .collect();

        let mut next = self.clone();
        next.release_cargo(&inputs, cat)?;
        for stack in inputs {
            next.withdraw_cargo(&stack.item, stack.quantity, cat)?;
        }
        for (item, quantity) in outputs {
            next.insert_item(&item, quantity, capacity, cat)?;
        }
        ensure!(
            next.cargo_volume(cat) <= capacity + capacity.max(1.) * 1e-9,
            "cargo hold full"
        );

        *self = next;
        Ok(())
    }

    pub(crate) fn packaged_mass(&self, cat: &Catalogue) -> f64 {
        self.packaged_parts
            .iter()
            .map(|(id, &quantity)| {
                let part = cat.part(id).expect("validated part kit");
                part.mass_kg * quantity as f64
            })
            .sum()
    }

    pub(crate) fn packaged_volume(&self, cat: &Catalogue) -> f64 {
        self.packaged_parts
            .iter()
            .map(|(id, &quantity)| {
                let part = cat.part(id).expect("validated part kit");
                industry::packaged_part_volume(part) * quantity as f64
            })
            .sum()
    }
}
