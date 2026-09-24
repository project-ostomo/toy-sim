use super::*;
use osg_model::{Id, market::*};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Exchange {
    pub orders: BTreeMap<Id, Order>,
    pub history: Vec<Order>,
    pub trades: Vec<Trade>,
    pub sequence: u64,
}

pub(super) const ORDER_HISTORY_LIMIT: usize = 10_000;

impl Exchange {
    pub(super) fn append_archive(&mut self, mut order: Order, now: i64) {
        order.status = if order.filled_quantity == order.original_quantity {
            OrderStatus::Completed
        } else {
            OrderStatus::Cancelled
        };
        order.closed_ms = Some(now);
        order.remaining = 0;
        self.history.push(order);
    }

    fn archive(&mut self, order: Order, now: i64) {
        self.append_archive(order, now);
        if self.history.len() > ORDER_HISTORY_LIMIT {
            self.history.remove(0);
        }
    }
}

fn quote(instrument: &Instrument, quantity: u64, price: u64) -> Result<u64> {
    let product = quantity as u128 * price as u128;
    let value = product.div_ceil(instrument.quantity_scale() as u128);
    u64::try_from(value).context("order value overflow")
}

fn reserve_owner() -> Principal {
    Principal::Sovereignty(super::super::ownership::sovereignty_id("USE"))
}

impl Economy {
    pub fn reserved(&self, owner: Principal, currency: Currency) -> u64 {
        self.exchange
            .orders
            .values()
            .filter(|order| order.owner == owner)
            .filter_map(|order| match (order.side, currency) {
                (Side::Buy, currency) if currency == order.instrument.currency() => {
                    Some(quote(&order.instrument, order.remaining, order.price).unwrap_or(u64::MAX))
                }
                (Side::Sell, Currency::Lat) if order.instrument == Instrument::Fx => {
                    Some(order.remaining)
                }
                _ => None,
            })
            .fold(0, u64::saturating_add)
            .saturating_add(
                self.service_holds
                    .values()
                    .filter(|hold| hold.payer == owner && hold.currency == currency)
                    .map(|hold| hold.amount)
                    .fold(0, u64::saturating_add),
            )
    }

    pub(crate) fn available(&self, owner: Principal, currency: Currency) -> u64 {
        let total = self
            .balances
            .get(&owner)
            .map_or(0, |balance| match currency {
                Currency::Uec => balance.uec,
                Currency::Lat => balance.lat,
            });
        total.saturating_sub(self.reserved(owner, currency))
    }

    pub(super) fn release_unfunded_orders(&mut self) {
        let owners: Vec<_> = self.balances.keys().copied().collect();
        for owner in owners {
            while self.reserved(owner, Currency::Uec) > self.balances[&owner].uec {
                let newest = self
                    .exchange
                    .orders
                    .values()
                    .filter(|order| {
                        order.owner == owner
                            && order.side == Side::Buy
                            && order.instrument.currency() == Currency::Uec
                    })
                    .max_by_key(|order| order.sequence)
                    .map(|order| order.id);
                if let Some(id) = newest {
                    let order = self.exchange.orders.remove(&id).unwrap();
                    self.exchange.archive(order, self.last_day * DAY_MS);
                } else if let Some(id) = self
                    .service_holds
                    .iter()
                    .rev()
                    .find(|(_, hold)| hold.payer == owner && hold.currency == Currency::Uec)
                    .map(|(&id, _)| id)
                {
                    self.service_holds.remove(&id);
                } else {
                    break;
                }
            }
        }
    }

    pub(super) fn validate_exchange(
        &self,
        directory: &osg_model::ownership::OwnershipDirectory,
    ) -> Result<()> {
        ensure!(
            self.exchange.orders.len() <= 10_000,
            "too many market orders"
        );
        let mut reservations = BTreeMap::<(Principal, Currency), u64>::new();
        let mut sequences = BTreeSet::new();
        for (&id, order) in &self.exchange.orders {
            ensure!(
                id == order.id
                    && directory.contains(order.owner)
                    && order.remaining > 0
                    && order.status == OrderStatus::Open
                    && order.closed_ms.is_none()
                    && order.original_quantity > 0
                    && order.filled_quantity <= order.original_quantity
                    && order.remaining <= order.original_quantity - order.filled_quantity
                    && order.price > 0
                    && order.sequence > 0
                    && order.sequence <= self.exchange.sequence
                    && sequences.insert(order.sequence),
                "invalid market order"
            );
            let (currency, amount) = match order.side {
                Side::Buy => (
                    order.instrument.currency(),
                    quote(&order.instrument, order.remaining, order.price)?,
                ),
                Side::Sell if order.instrument == Instrument::Fx => {
                    (Currency::Lat, order.remaining)
                }
                Side::Sell => {
                    ensure!(
                        self.stock_available(order.owner, &order.instrument).is_ok(),
                        "unfunded commodity order"
                    );
                    continue;
                }
            };
            let reserved = reservations.entry((order.owner, currency)).or_default();
            *reserved = reserved
                .checked_add(amount)
                .context("reservation overflow")?;
        }
        for ((owner, currency), reserved) in reservations {
            let balance = self
                .balances
                .get(&owner)
                .context("missing reserved account")?;
            ensure!(
                reserved
                    <= match currency {
                        Currency::Uec => balance.uec,
                        Currency::Lat => balance.lat,
                    },
                "unfunded market order"
            );
        }
        ensure!(
            self.exchange.history.len() <= ORDER_HISTORY_LIMIT,
            "too much order history"
        );
        let mut historical_ids = BTreeSet::new();
        for order in &self.exchange.history {
            ensure!(
                directory.contains(order.owner)
                    && !self.exchange.orders.contains_key(&order.id)
                    && historical_ids.insert(order.id)
                    && order.remaining == 0
                    && order.closed_ms.is_some()
                    && order.original_quantity > 0
                    && order.filled_quantity <= order.original_quantity
                    && order.price > 0
                    && order.sequence > 0
                    && order.sequence <= self.exchange.sequence
                    && sequences.insert(order.sequence)
                    && if order.filled_quantity == order.original_quantity {
                        order.status == OrderStatus::Completed
                    } else {
                        order.status == OrderStatus::Cancelled
                    },
                "invalid market order history"
            );
        }
        for (index, trade) in self.exchange.trades.iter().enumerate() {
            ensure!(
                trade.sequence == index as u64 + 1
                    && trade.price > 0
                    && trade.quantity > 0
                    && directory.contains(trade.buyer)
                    && directory.contains(trade.seller)
                    && (!trade.backstop || trade.buyer == reserve_owner()),
                "invalid market trade"
            );
        }
        Ok(())
    }
}

impl Transaction<'_> {
    pub(super) fn move_money(
        &mut self,
        directory: Option<&osg_model::ownership::OwnershipDirectory>,
        from: Principal,
        to: Principal,
        currency: Currency,
        amount: u64,
        kind: EntryKind,
        now: i64,
    ) -> Result<u64> {
        if from == to {
            return Ok(amount);
        }
        ensure!(
            self.available(from, currency) >= amount,
            "insufficient available funds"
        );
        let target = self.balance_mut(to);
        let value = match currency {
            Currency::Uec => &mut target.uec,
            Currency::Lat => &mut target.lat,
        };
        let next = value
            .checked_add(amount)
            .context("recipient balance overflow")?;
        *value = next;
        let source = self.balance_mut(from);
        match currency {
            Currency::Uec => source.uec -= amount,
            Currency::Lat => source.lat -= amount,
        }
        self.record(from, currency, false, amount, kind, Some(to), now);
        self.record(to, currency, true, amount, kind, Some(from), now);
        self.levy_turnover(directory, Some(from), to, currency, amount, now)
    }

    fn fill(
        &mut self,
        instrument: &Instrument,
        directory: Option<&osg_model::ownership::OwnershipDirectory>,
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
                directory,
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
            let balance = &mut self.balance_mut(seller).uec;
            *balance = balance
                .checked_add(payment)
                .context("recipient balance overflow")?;
            self.record(
                seller,
                Currency::Uec,
                true,
                payment,
                EntryKind::Conversion,
                Some(buyer),
                now,
            );
            self.levy_turnover(directory, Some(buyer), seller, Currency::Uec, payment, now)?;
        } else {
            let restricted = instrument.currency() == Currency::Lat
                && directory.is_some_and(|directory| self.restricted(directory, seller));
            if restricted {
                self.transfer(directory, buyer, seller, Currency::Lat, payment, true, now)?;
            } else {
                self.move_money(
                    directory,
                    buyer,
                    seller,
                    instrument.currency(),
                    payment,
                    EntryKind::Market,
                    now,
                )?;
            }
        }
        self.record_trade(Trade {
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

    /// Every fill belongs to the surrounding transaction.
    pub(super) fn execute_order(
        &mut self,
        instrument: Instrument,
        directory: Option<&osg_model::ownership::OwnershipDirectory>,
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
            !self.exchange.orders.contains_key(&id)
                && !self.exchange.history.iter().any(|order| order.id == id),
            "order id already exists"
        );
        ensure!(
            !resting || self.exchange.orders.len() < 10_000,
            "market order capacity reached"
        );
        ensure!(
            !resting
                || self
                    .exchange
                    .orders
                    .values()
                    .filter(|order| order.owner == owner)
                    .count()
                    < 100,
            "account order capacity reached"
        );
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
        while remaining > 0 {
            let best = self
                .exchange
                .orders
                .values()
                .filter(|order| {
                    order.side != side
                        && order.instrument == instrument
                        && order.owner != owner
                        && if side == Side::Buy {
                            order.price <= price
                        } else {
                            order.price >= price
                        }
                })
                .min_by_key(|order| {
                    (
                        if side == Side::Buy {
                            order.price
                        } else {
                            u64::MAX - order.price
                        },
                        order.sequence,
                    )
                })
                .cloned();
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
                    let available = self.available(owner, instrument.currency());
                    let affordable = (available as u128 * instrument.quantity_scale() as u128
                        / maker.price as u128)
                        .min(u64::MAX as u128) as u64;
                    quantity = quantity.min(affordable);
                }
                if quantity == 0 {
                    break;
                }

                maker.remaining -= quantity;
                maker.filled_quantity += quantity;
                self.remove_order(maker.id);
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
                if maker.side == Side::Buy {
                    let available = self.available(maker.owner, instrument.currency());
                    let affordable = (available as u128 * instrument.quantity_scale() as u128
                        / maker.price as u128)
                        .min(u64::MAX as u128) as u64;
                    maker.remaining = maker.remaining.min(affordable);
                }
                if maker.remaining > 0 {
                    self.insert_order(maker);
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
                let available = self.available(owner, instrument.currency());
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
            self.insert_order(order);
        } else {
            self.archive(order, now);
        }
        Ok(())
    }
}

pub fn apply(world: &mut World, account: AccountId, id: Id, command: MarketCommand) -> Result<()> {
    if world
        .resource::<Economy>()
        .completed
        .contains(&(account, id))
    {
        return Ok(());
    }
    super::settle(world);
    if matches!(command, MarketCommand::MoveStorage { .. }) {
        return super::storage::apply(world, account, id, command);
    }
    let now = osg_model::calendar::now_unix_ms();
    let resting = matches!(command, MarketCommand::Limit { .. });
    world.resource_scope(|world, mut economy: Mut<Economy>| {
        let directory = &world.resource::<Directory>().0;
        economy.transaction(|staged| {
            match command {
                MarketCommand::Cancel { order } => {
                    let order = staged
                        .exchange
                        .orders
                        .get(&order)
                        .context("order unavailable")?;
                    ensure!(
                        directory.administers(account, order.owner),
                        "order administration required"
                    );
                    let id = order.id;
                    let order = staged.remove_order(id).unwrap();
                    staged.archive(order, now);
                }
                MarketCommand::Limit {
                    instrument,
                    owner,
                    side,
                    quantity,
                    price,
                }
                | MarketCommand::Immediate {
                    instrument,
                    owner,
                    side,
                    quantity,
                    price,
                } => {
                    ensure!(
                        directory.administers(account, owner),
                        "account administration required"
                    );
                    ensure!(
                        instrument != Instrument::Fx
                            && (instrument.currency() != Currency::Lat || side == Side::Sell)
                            || !staged.restricted(directory, owner),
                        "LAT licence required"
                    );
                    super::storage::validate_instrument(world, &instrument)?;
                    staged.execute_order(
                        instrument,
                        Some(directory),
                        id,
                        owner,
                        side,
                        quantity,
                        price,
                        resting,
                        now,
                    )?;
                }
                MarketCommand::MoveStorage { .. } => unreachable!(),
            }
            Ok(())
        })?;
        economy.completed.insert((account, id));
        Ok(())
    })
}

pub fn snapshot(world: &World, account: AccountId, subscription: &MarketQuery) -> MarketSnapshot {
    let directory = &world.resource::<Directory>().0;
    if !subscription.valid() || !directory.administers(account, subscription.owner) {
        return MarketSnapshot {
            error: Some("Market account access unavailable".into()),
            ..Default::default()
        };
    }
    if let Err(error) = super::storage::validate_instrument(world, &subscription.instrument) {
        return MarketSnapshot {
            error: Some(error.to_string()),
            ..Default::default()
        };
    }
    let economy = world.resource::<Economy>();
    let owner = subscription.owner;
    let mut stations: Vec<_> = world
        .resource::<crate::sim::identity::IdentityIndex>()
        .entries()
        .iter()
        .filter(|(id, _)| subscription.stations_after.is_none_or(|after| **id > after))
        .filter_map(|(id, entity)| {
            let landmark = world.get::<crate::sim::infrastructure::Landmark>(*entity)?;
            world.get::<crate::sim::hardware::ShipInventory>(*entity)?;
            crate::sim::ownership::can_access(
                world,
                account,
                *entity,
                osg_model::ownership::Permission::View,
            )
            .then(|| MarketStation {
                id: *id,
                name: landmark.name.clone(),
            })
        })
        .collect();
    stations.sort_by_key(|station| station.id);
    let more_stations = stations.len() > 128;
    stations.truncate(128);
    let mut bids: Vec<_> = economy
        .exchange
        .orders
        .values()
        .filter(|order| order.side == Side::Buy && order.instrument == subscription.instrument)
        .cloned()
        .collect();
    let mut asks: Vec<_> = economy
        .exchange
        .orders
        .values()
        .filter(|order| order.side == Side::Sell && order.instrument == subscription.instrument)
        .cloned()
        .collect();
    bids.sort_by_key(|order| (u64::MAX - order.price, order.sequence));
    asks.sort_by_key(|order| (order.price, order.sequence));
    bids.truncate(50);
    asks.truncate(50);
    let mut trades: Vec<_> = economy
        .exchange
        .trades
        .iter()
        .rev()
        .filter(|trade| trade.instrument == subscription.instrument)
        .filter(|trade| {
            subscription
                .before
                .is_none_or(|before| trade.sequence < before)
        })
        .take(subscription.limit as usize + 1)
        .cloned()
        .collect();
    let more = trades.len() > subscription.limit as usize;
    trades.truncate(subscription.limit as usize);
    MarketSnapshot {
        offers: Vec::new(),
        offers_next: None,
        stations_next: more_stations.then(|| stations.last().unwrap().id),
        stations,
        instrument: subscription.instrument.clone(),
        stock: economy.stock_snapshot(owner, &subscription.instrument),
        owner: Some(owner),
        available_uec: economy.available(owner, Currency::Uec),
        available_lat: economy.available(owner, Currency::Lat),
        reserved_uec: economy.reserved(owner, Currency::Uec),
        reserved_lat: economy.reserved(owner, Currency::Lat),
        lat_restricted: economy.restricted(directory, owner),
        last_price: economy
            .exchange
            .trades
            .iter()
            .rev()
            .find(|trade| trade.instrument == subscription.instrument)
            .map(|trade| trade.price),
        backstop_price: economy.official_uec_per_lat,
        bids,
        asks,
        orders: economy
            .exchange
            .orders
            .values()
            .filter(|order| order.owner == owner && order.instrument == subscription.instrument)
            .cloned()
            .collect(),
        orders_next: None,
        next_before: more.then(|| trades.last().unwrap().sequence),
        trades,
        error: None,
    }
}
