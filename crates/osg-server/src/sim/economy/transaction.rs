use super::*;
use osg_model::{
    Id,
    industry::{CargoItem, ServicePayment},
    market::Order,
};

/// Exclusive transaction with an undo journal proportional to the records touched.
/// Dropping an uncommitted transaction also rolls back during unwinding.
pub(crate) struct Transaction<'a> {
    economy: &'a mut Economy,
    balances: BTreeMap<Principal, Option<Balance>>,
    stock: BTreeMap<(Id, Principal), Option<BTreeMap<CargoItem, u64>>>,
    orders: BTreeMap<Id, Option<Order>>,
    holds: BTreeMap<Id, Option<ServicePayment>>,
    ledger_len: usize,
    trades_len: usize,
    history_len: usize,
    sequence: u64,
    committed: bool,
}

impl std::ops::Deref for Transaction<'_> {
    type Target = Economy;

    fn deref(&self) -> &Economy {
        self.economy
    }
}

impl Economy {
    pub(crate) fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut transaction = Transaction {
            ledger_len: self.entries.len(),
            trades_len: self.exchange.trades.len(),
            history_len: self.exchange.history.len(),
            sequence: self.exchange.sequence,
            economy: self,
            balances: BTreeMap::new(),
            stock: BTreeMap::new(),
            orders: BTreeMap::new(),
            holds: BTreeMap::new(),
            committed: false,
        };
        let result = operation(&mut transaction)?;
        transaction.committed = true;
        let history = &mut transaction.economy.exchange.history;
        let excess = history.len().saturating_sub(exchange::ORDER_HISTORY_LIMIT);
        history.drain(..excess);
        Ok(result)
    }
}

impl Transaction<'_> {
    pub(super) fn balance_mut(&mut self, owner: Principal) -> &mut Balance {
        self.balances
            .entry(owner)
            .or_insert_with(|| self.economy.balances.get(&owner).cloned());
        self.economy.balances.entry(owner).or_default()
    }

    pub(super) fn stock_mut(&mut self, key: (Id, Principal)) -> &mut BTreeMap<CargoItem, u64> {
        self.stock
            .entry(key)
            .or_insert_with(|| self.economy.storage.get(&key).cloned());
        self.economy.storage.entry(key).or_default()
    }

    pub(super) fn remove_empty_stock(&mut self, key: (Id, Principal)) {
        if self
            .economy
            .storage
            .get(&key)
            .is_some_and(BTreeMap::is_empty)
        {
            self.economy.storage.remove(&key);
        }
    }

    pub(super) fn remove_order(&mut self, id: Id) -> Option<Order> {
        self.orders
            .entry(id)
            .or_insert_with(|| self.economy.exchange.orders.get(&id).cloned());
        self.economy.exchange.orders.remove(&id)
    }

    pub(super) fn insert_order(&mut self, order: Order) {
        self.orders
            .entry(order.id)
            .or_insert_with(|| self.economy.exchange.orders.get(&order.id).cloned());
        self.economy.exchange.orders.insert(order.id, order);
    }

    pub(crate) fn release_service_hold(&mut self, id: Id) -> Option<ServicePayment> {
        self.holds
            .entry(id)
            .or_insert_with(|| self.economy.service_holds.get(&id).cloned());
        self.economy.service_holds.remove(&id)
    }

    pub(crate) fn reference_payment(&mut self, start: usize, id: Id) {
        assert!(start >= self.ledger_len);
        for entry in &mut self.economy.entries[start..] {
            entry.reference = Some(id);
            if entry.kind == EntryKind::Transfer {
                entry.kind = EntryKind::Industry;
            }
        }
    }

    pub(super) fn next_order_sequence(&mut self) -> Result<u64> {
        let sequence = self
            .economy
            .exchange
            .sequence
            .checked_add(1)
            .context("order sequence overflow")?;
        self.economy.exchange.sequence = sequence;
        Ok(sequence)
    }

    pub(super) fn archive(&mut self, order: Order, now: i64) {
        self.economy.exchange.append_archive(order, now);
    }

    pub(super) fn record(
        &mut self,
        owner: Principal,
        currency: Currency,
        credit: bool,
        amount: u64,
        kind: EntryKind,
        counterparty: Option<Principal>,
        now: i64,
    ) {
        self.economy
            .record(owner, currency, credit, amount, kind, counterparty, now);
    }

    pub(super) fn record_trade(&mut self, trade: osg_model::market::Trade) {
        self.economy.exchange.trades.push(trade);
    }
}

fn restore<K: Ord, V>(target: &mut BTreeMap<K, V>, previous: BTreeMap<K, Option<V>>) {
    for (key, value) in previous {
        if let Some(value) = value {
            target.insert(key, value);
        } else {
            target.remove(&key);
        }
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        restore(
            &mut self.economy.balances,
            std::mem::take(&mut self.balances),
        );
        restore(&mut self.economy.storage, std::mem::take(&mut self.stock));
        restore(
            &mut self.economy.exchange.orders,
            std::mem::take(&mut self.orders),
        );
        restore(
            &mut self.economy.service_holds,
            std::mem::take(&mut self.holds),
        );
        self.economy.entries.truncate(self.ledger_len);
        self.economy.exchange.trades.truncate(self.trades_len);
        self.economy.exchange.history.truncate(self.history_len);
        self.economy.exchange.sequence = self.sequence;
    }
}
