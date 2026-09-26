use crate::sim::society::OwnershipDirectory;
use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use exchange::{ORDER_HISTORY_LIMIT, commitment, quote, reserve_owner};
use imbl::{OrdMap, OrdSet};
use osg_model::{AccountId, economy::*, ownership::Principal};
use osg_model::{Id, industry::CargoItem, market::*};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::BTreeMap;

pub const DEMURRAGE_EXEMPTION: u64 = 50_000 * MONEY_SCALE;
pub const RATE_SCALE: u128 = 1_000_000_000_000_000_000;

/// Rate for the UTC day being charged, rounded down at 18 decimal places.
/// Compounding over the calendar year retains 80% of the taxable balance,
/// within the fixed point rounding precision.
pub fn daily_rate(day: i64) -> u128 {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month = (5 * doy + 2) / 153;
    let year = yoe + era * 400 + i64::from(month >= 10);
    if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
        609_496_015_987_604
    } else {
        611_165_357_704_478
    }
}

pub mod exchange;
mod orders;
mod records;
mod stock;
pub mod storage;
mod tax;
pub use orders::Orders;
pub use records::{IndexedMap, Record, Records};
pub use stock::StockStore;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Balance {
    pub uec: u64,
    pub lat: u64,
    pub reserved_uec: u64,
    pub reserved_lat: u64,
}

impl Balance {
    pub fn credit(&mut self, currency: Currency, amount: u64) -> Result<()> {
        let (available, reserved) = match currency {
            Currency::Uec => (&mut self.uec, self.reserved_uec),
            Currency::Lat => (&mut self.lat, self.reserved_lat),
        };
        available
            .checked_add(reserved)
            .and_then(|total| total.checked_add(amount))
            .context("money balance overflow")?;
        *available += amount;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Economy {
    pub turnover_taxes: OrdMap<osg_model::Id, u16>,
    pub storage: StockStore,
    pub exchange: exchange::Exchange,

    pub balances: OrdMap<Principal, Balance>,
    pub licences: OrdSet<Principal>,
    pub official_uec_per_lat: u64,
    pub last_day: i64,
    pub entries: Records<LedgerEntry>,
}

impl Economy {
    pub fn charge_service(
        &mut self,
        job: osg_model::Id,
        payment: &osg_model::industry::ServicePayment,
        directory: &crate::sim::society::OwnershipDirectory,
        revenue: &Balance,
        now: i64,
    ) -> Result<Balance> {
        if payment.charged {
            return Ok(revenue.clone());
        }
        self.release(payment.payer, payment.currency, payment.amount, job, now)?;
        let mut updated = revenue.clone();
        if payment.amount > 0 && payment.payer != payment.operator {
            let before = self.balance(payment.operator);
            let restricted =
                payment.currency == Currency::Lat && self.restricted(directory, payment.operator);
            let start = self.entry_count();
            self.transfer(
                Some(directory),
                payment.payer,
                payment.operator,
                payment.currency,
                payment.amount,
                restricted,
                now,
            )?;
            self.reference_payment(start, job);
            let after = self.balance(payment.operator);
            let uec = after
                .uec
                .checked_sub(before.uec)
                .context("service receipt underflow")?;
            let lat = after
                .lat
                .checked_sub(before.lat)
                .context("service receipt underflow")?;
            updated.uec = updated
                .uec
                .checked_add(uec)
                .context("service revenue overflow")?;
            updated.lat = updated
                .lat
                .checked_add(lat)
                .context("service revenue overflow")?;
        }
        Ok(updated)
    }

    pub fn at(now: i64) -> Self {
        Self {
            turnover_taxes: OrdMap::new(),
            storage: StockStore::default(),
            exchange: Default::default(),
            balances: OrdMap::new(),
            licences: OrdSet::new(),
            official_uec_per_lat: 3_200_000,
            last_day: now.div_euclid(DAY_MS),
            entries: Records::default(),
        }
    }

    fn record(
        &mut self,
        owner: Principal,
        currency: Currency,
        credit: bool,
        amount: u64,
        kind: EntryKind,
        counterparty: Option<Principal>,
        now: i64,
    ) {
        let account = &self.balances[&owner];
        self.entries.insert(LedgerEntry {
            sequence: self.entries.last_key().copied().unwrap_or(0) + 1,
            time_ms: now,
            owner,
            currency,
            bucket: BalanceBucket::Available,
            kind,
            credit,
            amount,
            balance: match currency {
                Currency::Uec => account.uec,
                Currency::Lat => account.lat,
            },
            counterparty,
            reference: None,
        });
    }

    /// Explicit issuance for scenario provisioning and monetary policy.
    /// No player command exposes this operation.
    pub fn issue(
        &mut self,
        owner: Principal,
        currency: Currency,
        amount: u64,
        now: i64,
    ) -> Result<()> {
        self.settle(now);
        let mut balance = self.balances.get(&owner).cloned().unwrap_or_default();
        balance.credit(currency, amount)?;
        self.balances.insert(owner, balance);
        self.record(owner, currency, true, amount, EntryKind::Issue, None, now);
        Ok(())
    }

    /// Charge each completed UTC day once, including days elapsed offline.
    pub fn settle(&mut self, now: i64) {
        let today = now.div_euclid(DAY_MS);
        while self.last_day < today {
            let rate = daily_rate(self.last_day);
            let mut charges = Vec::new();
            for owner in self.balances.keys().copied().collect::<Vec<_>>() {
                let balance = self.balances.get_mut(&owner).unwrap();
                let taxable = balance.uec.saturating_sub(DEMURRAGE_EXEMPTION);
                let charge = (taxable as u128 * rate).div_ceil(RATE_SCALE) as u64;
                balance.uec -= charge;
                if charge > 0 {
                    charges.push((owner, charge));
                }
            }
            self.last_day += 1;
            for (owner, charge) in charges {
                self.record(
                    owner,
                    Currency::Uec,
                    false,
                    charge,
                    EntryKind::Demurrage,
                    None,
                    self.last_day * DAY_MS,
                );
            }
        }
    }

    pub fn restricted(
        &self,
        directory: &crate::sim::society::OwnershipDirectory,
        owner: Principal,
    ) -> bool {
        let use_owner = Principal::Sovereignty(super::ownership::sovereignty_id("USE"));
        let licensed = directory
            .diplomacy
            .declarations
            .get(&(
                use_owner,
                osg_model::diplomacy::DeclarationCategory::Licence,
                owner,
            ))
            .map_or_else(
                || self.licences.contains(&owner),
                |declaration| declaration.enabled,
            );
        owner != use_owner && directory.lineage(owner).contains(&use_owner) && !licensed
    }

    pub fn transfer(
        &mut self,
        directory: Option<&crate::sim::society::OwnershipDirectory>,
        from: Principal,
        to: Principal,
        currency: Currency,
        amount: u64,
        restricted: bool,
        now: i64,
    ) -> Result<()> {
        ensure!(
            from != to && amount > 0,
            "transfer needs distinct accounts and a positive amount"
        );
        self.move_money(from, to, currency, amount, EntryKind::Transfer, now)?;
        let net = self.levy_turnover(directory, Some(from), to, currency, amount, now)?;
        if currency == Currency::Lat && restricted && net > 0 {
            self.execute_order(
                osg_model::market::Instrument::Fx,
                directory,
                osg_model::Id::new(),
                to,
                osg_model::market::Side::Sell,
                net,
                self.official_uec_per_lat,
                false,
                now,
            )?;
        }
        Ok(())
    }

    pub fn reserved(&self, owner: Principal, currency: Currency) -> u64 {
        self.balances
            .get(&owner)
            .map_or(0, |balance| match currency {
                Currency::Uec => balance.reserved_uec,
                Currency::Lat => balance.reserved_lat,
            })
    }

    pub fn available(&self, owner: Principal, currency: Currency) -> u64 {
        self.balances
            .get(&owner)
            .map_or(0, |balance| match currency {
                Currency::Uec => balance.uec,
                Currency::Lat => balance.lat,
            })
    }

    pub fn stock_reserved(&self, owner: Principal, station: Id, item: &CargoItem) -> u64 {
        [Currency::Uec, Currency::Lat]
            .into_iter()
            .flat_map(|currency| {
                self.exchange.orders.by_owner(
                    owner,
                    Some(Instrument::Commodity {
                        station,
                        item: item.clone(),
                        currency,
                    }),
                    None,
                    None,
                )
            })
            .filter(|order| order.side == Side::Sell)
            .fold(0_u64, |total, order| total.saturating_add(order.remaining))
    }

    pub fn stock_available(&self, owner: Principal, instrument: &Instrument) -> Result<u64> {
        let Instrument::Commodity { station, item, .. } = instrument else {
            anyhow::bail!("commodity instrument required");
        };
        self.storage
            .get(&(*station, owner))
            .and_then(|stock| stock.get(item))
            .copied()
            .unwrap_or(0)
            .checked_sub(self.stock_reserved(owner, *station, item))
            .context("stock reservations exceed custody")
    }

    #[cfg(test)]
    pub fn custody_totals(&self, station: Id) -> Result<BTreeMap<CargoItem, u64>> {
        let mut totals = BTreeMap::<CargoItem, u64>::new();
        for stock in self.storage.at_station(station) {
            ensure!(
                stock.len() <= 1024 && stock.values().all(|quantity| *quantity > 0),
                "invalid stored stock"
            );
            for (item, quantity) in stock {
                let total = totals.entry(item.clone()).or_default();
                *total = total
                    .checked_add(*quantity)
                    .context("stock custody overflow")?;
            }
        }
        Ok(totals)
    }

    pub fn tax_rate(
        &self,
        directory: &OwnershipDirectory,
        recipient: Principal,
    ) -> Option<(Principal, u16)> {
        directory
            .lineage(recipient)
            .into_iter()
            .find_map(|principal| {
                let Principal::Sovereignty(id) = principal else {
                    return None;
                };
                self.turnover_taxes.get(&id).map(|rate| (principal, *rate))
            })
    }

    pub fn balance(&self, owner: Principal) -> Balance {
        self.balances.get(&owner).cloned().unwrap_or_default()
    }

    pub fn order(&self, id: Id) -> Option<&Order> {
        self.exchange.orders.get(&id)
    }

    fn order_count(&self, owner: Option<Principal>) -> usize {
        owner.map_or_else(
            || self.exchange.orders.len(),
            |owner| {
                self.exchange
                    .orders
                    .by_owner(owner, None, None, None)
                    .count()
            },
        )
    }

    fn best_order(
        &self,
        instrument: &Instrument,
        side: Side,
        owner: Principal,
        limit: u64,
    ) -> Option<&Order> {
        let opposite = if side == Side::Buy {
            Side::Sell
        } else {
            Side::Buy
        };
        self.exchange
            .orders
            .book(instrument, opposite)
            .take_while(|order| {
                if side == Side::Buy {
                    order.price <= limit
                } else {
                    order.price >= limit
                }
            })
            .find(|order| order.owner != owner)
    }

    fn move_money(
        &mut self,
        from: Principal,
        to: Principal,
        currency: Currency,
        amount: u64,
        kind: EntryKind,
        now: i64,
    ) -> Result<()> {
        if from == to {
            return Ok(());
        }
        ensure!(
            self.available(from, currency) >= amount,
            "insufficient available funds"
        );
        self.balance_mut(to).credit(currency, amount)?;
        let source = self.balance_mut(from);
        match currency {
            Currency::Uec => source.uec -= amount,
            Currency::Lat => source.lat -= amount,
        }
        self.record(from, currency, false, amount, kind, Some(to), now);
        self.record(to, currency, true, amount, kind, Some(from), now);
        Ok(())
    }

    fn fill(
        &mut self,
        instrument: &Instrument,
        directory: Option<&crate::sim::society::OwnershipDirectory>,
        buyer: Principal,
        seller: Principal,
        quantity: u64,
        price: u64,
        backstop: bool,
        now: i64,
    ) -> Result<()> {
        let payment = quote(instrument, quantity, price)?;
        ensure!(payment > 0, "trade is below monetary precision");
        if *instrument == Instrument::Fx {
            self.move_money(
                seller,
                buyer,
                Currency::Lat,
                quantity,
                EntryKind::Market,
                now,
            )?;
        } else {
            self.move_stock(seller, buyer, instrument, quantity)?;
        }
        if backstop {
            self.balance_mut(seller).credit(Currency::Uec, payment)?;
            self.record(
                seller,
                Currency::Uec,
                true,
                payment,
                EntryKind::Conversion,
                Some(buyer),
                now,
            );
        } else {
            self.move_money(
                buyer,
                seller,
                instrument.currency(),
                payment,
                EntryKind::Market,
                now,
            )?;
            let restricted = instrument.currency() == Currency::Lat
                && directory.is_some_and(|directory| self.restricted(directory, seller));
            if restricted {
                self.execute_order(
                    Instrument::Fx,
                    directory,
                    Id::new(),
                    seller,
                    Side::Sell,
                    payment,
                    self.official_uec_per_lat,
                    false,
                    now,
                )?;
            }
        }
        self.exchange.trades.insert(Trade {
            instrument: instrument.clone(),
            sequence: self.exchange.trades.len() as u64 + 1,
            time_ms: now,
            price,
            quantity,
            buyer,
            seller,
            backstop,
        });
        Ok(())
    }

    pub fn execute_order(
        &mut self,
        instrument: Instrument,
        directory: Option<&crate::sim::society::OwnershipDirectory>,
        id: Id,
        owner: Principal,
        side: Side,
        quantity: u64,
        price: u64,
        resting: bool,
        now: i64,
    ) -> Result<()> {
        ensure!(
            quantity > 0 && price > 0,
            "positive quantity and limit price required"
        );
        ensure!(
            self.order(id).is_none() && !self.exchange.history.contains_key(&id),
            "order id already exists"
        );
        ensure!(
            !resting || self.order_count(None) < 10_000,
            "market order capacity reached"
        );
        ensure!(
            !resting || self.order_count(Some(owner)) < 100,
            "account order capacity reached"
        );
        // Buyers pay once on the submitted value before calculating fills or
        // reserving the remainder. Replacing a partially filled maker skips this.
        if side == Side::Buy {
            let value = quote(&instrument, quantity, price)?;
            let entry_start = self.entry_count();
            self.levy_turnover(directory, None, owner, instrument.currency(), value, now)?;
            self.reference_payment(entry_start, id);
        }

        let available = if side == Side::Buy {
            self.available(owner, instrument.currency())
        } else if instrument == Instrument::Fx {
            self.available(owner, Currency::Lat)
        } else {
            self.stock_available(owner, &instrument)?
        };
        let required = if side == Side::Buy {
            quote(&instrument, quantity, price)?
        } else {
            quantity
        };
        ensure!(available >= required, "insufficient available funds");
        let sequence = self.next_order_sequence()?;
        let mut remaining = quantity;
        let mut budget = required;
        while remaining > 0 {
            let best = self.best_order(&instrument, side, owner, price).cloned();
            let backstop = instrument == Instrument::Fx
                && side == Side::Sell
                && owner != reserve_owner()
                && price <= self.official_uec_per_lat
                && best
                    .as_ref()
                    .is_none_or(|order| order.price < self.official_uec_per_lat);
            if backstop {
                self.fill(
                    &instrument,
                    directory,
                    reserve_owner(),
                    owner,
                    remaining,
                    self.official_uec_per_lat,
                    true,
                    now,
                )?;
                remaining = 0;
            } else if let Some(mut maker) = best {
                let mut quantity = remaining.min(maker.remaining);
                if side == Side::Buy {
                    let available = budget;
                    let affordable = (available as u128 * instrument.quantity_scale() as u128
                        / maker.price as u128)
                        .min(u64::MAX as u128) as u64;
                    quantity = quantity.min(affordable);
                }
                if quantity == 0 {
                    break;
                }

                let maker_budget = commitment(&maker)?.map(|(_, amount)| amount);
                maker.remaining -= quantity;
                maker.filled_quantity += quantity;
                self.remove_order(maker.id, now)?;
                let (buyer, seller) = if side == Side::Buy {
                    (owner, maker.owner)
                } else {
                    (maker.owner, owner)
                };
                self.fill(
                    &instrument,
                    directory,
                    buyer,
                    seller,
                    quantity,
                    maker.price,
                    false,
                    now,
                )?;
                if side == Side::Buy {
                    budget -= quote(&instrument, quantity, maker.price)?;
                }
                if maker.side == Side::Buy {
                    let available =
                        maker_budget.unwrap() - quote(&instrument, quantity, maker.price)?;
                    let affordable = (available as u128 * instrument.quantity_scale() as u128
                        / maker.price as u128)
                        .min(u64::MAX as u128) as u64;
                    maker.remaining = maker.remaining.min(affordable);
                }
                if maker.remaining > 0 {
                    self.insert_order(maker, now)?;
                } else {
                    self.archive(maker, now);
                }
                remaining -= quantity;
            } else {
                break;
            }
        }
        let filled_quantity = quantity - remaining;
        if resting && remaining > 0 {
            if side == Side::Buy {
                let available = budget;
                let affordable = (available as u128 * instrument.quantity_scale() as u128
                    / price as u128)
                    .min(u64::MAX as u128) as u64;
                remaining = remaining.min(affordable);
            }
        }
        let order = Order {
            status: OrderStatus::Open,
            closed_ms: None,
            original_quantity: quantity,
            filled_quantity,
            instrument,
            id,
            owner,
            side,
            price,
            remaining,
            sequence,
            time_ms: now,
        };
        if resting && remaining > 0 {
            self.insert_order(order, now)?;
        } else {
            self.archive(order, now);
        }
        Ok(())
    }

    pub fn move_stock(
        &mut self,
        from: Principal,
        to: Principal,
        instrument: &Instrument,
        quantity: u64,
    ) -> Result<()> {
        let Instrument::Commodity { station, item, .. } = instrument else {
            anyhow::bail!("commodity instrument required");
        };
        ensure!(
            from != to && quantity > 0 && self.stock_available(from, instrument)? >= quantity,
            "insufficient available stock"
        );
        let mut recipient = self
            .storage
            .get(&(*station, to))
            .cloned()
            .unwrap_or_default();
        ensure!(
            recipient.contains_key(item) || recipient.len() < 1024,
            "storage item limit"
        );
        let amount = recipient.entry(item.clone()).or_default();
        *amount = amount.checked_add(quantity).context("stock overflow")?;
        let mut source = self
            .storage
            .get(&(*station, from))
            .cloned()
            .unwrap_or_default();
        *source.get_mut(item).unwrap() -= quantity;
        if source[item] == 0 {
            source.remove(item);
        }
        self.write_stock((*station, from), source);
        self.write_stock((*station, to), recipient);
        Ok(())
    }

    pub fn levy_turnover(
        &mut self,
        directory: Option<&OwnershipDirectory>,
        source: Option<Principal>,
        recipient: Principal,
        currency: Currency,
        gross: u64,
        now: i64,
    ) -> Result<u64> {
        if self.turnover_taxes.is_empty() && (source.is_none() || directory.is_none()) {
            return Ok(gross);
        }

        let directory = directory.context("tax jurisdiction unavailable")?;
        let mut remaining = gross;
        if let Some((collector, rate)) = self.tax_rate(directory, recipient) {
            if recipient != collector && rate > 0 {
                remaining -= self.levy(recipient, collector, currency, gross, rate, now)?;
            }
        }
        if let Some(source) = source {
            let tariff = directory
                .lineage(source)
                .into_iter()
                .flat_map(|collector| {
                    directory
                        .lineage(recipient)
                        .into_iter()
                        .flat_map(move |partner| {
                            directory
                                .diplomacy
                                .active_terms(collector, partner)
                                .filter_map(move |term| {
                                    if let osg_model::diplomacy::AgreementTerm::Tariff {
                                        basis_points,
                                    } = term
                                    {
                                        Some((collector, *basis_points))
                                    } else {
                                        None
                                    }
                                })
                        })
                })
                .max_by_key(|(_, rate)| *rate);
            if let Some((collector, rate)) = tariff {
                if recipient != collector && rate > 0 {
                    remaining -= self.levy(recipient, collector, currency, remaining, rate, now)?;
                }
            }
        }
        Ok(remaining)
    }

    fn levy(
        &mut self,
        recipient: Principal,
        collector: Principal,
        currency: Currency,
        gross: u64,
        rate: u16,
        now: i64,
    ) -> Result<u64> {
        let tax = (gross as u128 * rate as u128).div_ceil(10_000) as u64;
        self.balance_mut(collector).credit(currency, tax)?;

        let payer = self.balance_mut(recipient);
        let balance = match currency {
            Currency::Uec => &mut payer.uec,
            Currency::Lat => &mut payer.lat,
        };
        *balance = balance
            .checked_sub(tax)
            .context("insufficient available funds for tax")?;

        self.record(
            recipient,
            currency,
            false,
            tax,
            EntryKind::Tax,
            Some(collector),
            now,
        );
        self.record(
            collector,
            currency,
            true,
            tax,
            EntryKind::Tax,
            Some(recipient),
            now,
        );
        Ok(tax)
    }

    fn balance_mut(&mut self, owner: Principal) -> &mut Balance {
        self.balances.entry(owner).or_default()
    }

    fn write_stock(&mut self, key: (Id, Principal), items: OrdMap<CargoItem, u64>) {
        self.storage.insert(key, items);
    }

    pub fn remove_order(&mut self, id: Id, now: i64) -> Result<Option<Order>> {
        let order = self.order(id).cloned();
        if let Some(order) = &order {
            if let Some((currency, amount)) = commitment(order)? {
                self.release(order.owner, currency, amount, id, now)?;
            }
            self.exchange.orders.remove(&id);
        }
        Ok(order)
    }

    fn insert_order(&mut self, order: Order, now: i64) -> Result<()> {
        if let Some((currency, amount)) = commitment(&order)? {
            self.reserve(order.owner, currency, amount, order.id, now)?;
        }
        self.exchange.orders.insert(order.id, order);
        Ok(())
    }

    pub fn reserve(
        &mut self,
        owner: Principal,
        currency: Currency,
        amount: u64,
        reference: Id,
        now: i64,
    ) -> Result<()> {
        self.move_bucket(owner, currency, amount, reference, now, true)
    }

    pub fn release(
        &mut self,
        owner: Principal,
        currency: Currency,
        amount: u64,
        reference: Id,
        now: i64,
    ) -> Result<()> {
        self.move_bucket(owner, currency, amount, reference, now, false)
    }

    fn move_bucket(
        &mut self,
        owner: Principal,
        currency: Currency,
        amount: u64,
        reference: Id,
        now: i64,
        reserve: bool,
    ) -> Result<()> {
        let balance = self.balance_mut(owner);
        let (available, reserved) = match currency {
            Currency::Uec => (&mut balance.uec, &mut balance.reserved_uec),
            Currency::Lat => (&mut balance.lat, &mut balance.reserved_lat),
        };
        let (source, target) = if reserve {
            (available, reserved)
        } else {
            (reserved, available)
        };
        let next_source = source
            .checked_sub(amount)
            .context("insufficient funds in balance bucket")?;
        let next_target = target
            .checked_add(amount)
            .context("balance bucket overflow")?;
        *source = next_source;
        *target = next_target;

        let kind = if reserve {
            EntryKind::Reserve
        } else {
            EntryKind::Release
        };
        self.record(owner, currency, !reserve, amount, kind, None, now);
        let mut entry = self
            .entries
            .get(self.entries.last_key().unwrap())
            .unwrap()
            .clone();
        entry.reference = Some(reference);
        let sequence = entry.sequence + 1;
        self.entries.insert(entry);
        self.entries.insert(LedgerEntry {
            sequence,
            time_ms: now,
            owner,
            currency,
            bucket: BalanceBucket::Reserved,
            kind,
            credit: reserve,
            amount,
            balance: self.reserved(owner, currency),
            counterparty: None,
            reference: Some(reference),
        });
        Ok(())
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn reference_payment(&mut self, start: usize, id: Id) {
        let entries: Vec<_> = self.entries.range((start as u64 + 1)..).cloned().collect();
        for mut entry in entries {
            entry.reference = Some(id);
            if entry.kind == EntryKind::Transfer {
                entry.kind = EntryKind::Industry;
            }
            self.entries.insert(entry);
        }
    }

    fn next_order_sequence(&mut self) -> Result<u64> {
        let next = self
            .exchange
            .sequence
            .checked_add(1)
            .context("order sequence overflow")?;
        self.exchange.sequence = next;
        Ok(next)
    }

    pub fn archive(&mut self, mut order: Order, now: i64) {
        order.status = if order.filled_quantity == order.original_quantity {
            OrderStatus::Completed
        } else {
            OrderStatus::Cancelled
        };
        order.closed_ms = Some(now);
        order.remaining = 0;
        self.exchange.history.insert(order.id, order);
        self.exchange.history.trim(ORDER_HISTORY_LIMIT);
    }
}

impl Default for Economy {
    fn default() -> Self {
        Self::at(osg_model::calendar::now_unix_ms())
    }
}

pub fn settle(mut society: ResMut<super::society::SocietyState>, mut revision: Local<Option<u64>>) {
    let now = osg_model::calendar::now_unix_ms();
    let state = &mut *society;
    state.economy.settle(now);
    if *revision == Some(state.social_revision) {
        return;
    }
    *revision = Some(state.social_revision);
    cancel_restricted(&mut state.economy, &state.directory.0, now);
}

pub fn cancel_restricted(economy: &mut Economy, directory: &OwnershipDirectory, now: i64) {
    let invalid: Vec<_> = economy
        .exchange
        .orders
        .restricted_currency()
        .filter(|order| economy.restricted(directory, order.owner))
        .map(|order| order.id)
        .collect();
    for id in invalid {
        if let Some(order) = economy
            .remove_order(id, now)
            .expect("open order commitment must be funded")
        {
            economy.archive(order, now);
        }
    }
}

pub fn apply(
    economy: &mut Economy,
    directory: &crate::sim::society::OwnershipDirectory,
    gas: &mut super::gas::GasState,
    account: AccountId,
    command: WalletCommand,
) -> Result<()> {
    let now = osg_model::calendar::now_unix_ms();
    economy.settle(now);
    if let WalletCommand::SetTurnoverTax {
        sovereignty,
        basis_points,
    } = command
    {
        ensure!(basis_points <= 10_000, "turnover tax cannot exceed 100%");
        ensure!(
            directory.administers(account, Principal::Sovereignty(sovereignty)),
            "sovereignty officer required"
        );
        economy.turnover_taxes.insert(sovereignty, basis_points);
        return Ok(());
    }
    let (from, to, amount) = match &command {
        WalletCommand::Transfer {
            from, to, amount, ..
        }
        | WalletCommand::TransferGas { from, to, amount } => (*from, *to, *amount),
        WalletCommand::SetTurnoverTax { .. } => unreachable!(),
    };
    ensure!(from != to && amount > 0, "invalid transfer");
    ensure!(
        directory.administers(account, from),
        "account administration required"
    );
    ensure!(directory.contains(to), "recipient unavailable");
    match command {
        WalletCommand::Transfer { currency, .. } => {
            ensure!(
                currency != Currency::Lat || !economy.restricted(directory, from),
                "LAT licence required"
            );
            let restricted = economy.restricted(directory, to);
            economy.transfer(Some(directory), from, to, currency, amount, restricted, now)
        }
        WalletCommand::TransferGas { .. } => {
            ensure!(
                directory.administers(account, to),
                "gas can only move between administered accounts"
            );
            gas.transfer(from, to, amount)
        }
        WalletCommand::SetTurnoverTax { .. } => unreachable!(),
    }
}
