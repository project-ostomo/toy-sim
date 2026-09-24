use anyhow::{Context, Result, ensure};
use bevy::prelude::*;
use osg_model::{AccountId, economy::*, ownership::Principal};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::ownership::Directory;

pub mod exchange;
pub mod storage;
mod tax;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Balance {
    pub uec: u64,
    pub lat: u64,
}

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct Economy {
    pub service_holds: BTreeMap<osg_model::Id, osg_model::industry::ServicePayment>,
    pub turnover_taxes: BTreeMap<osg_model::Id, u16>,
    pub storage:
        BTreeMap<(osg_model::Id, Principal), BTreeMap<osg_model::industry::CargoItem, u64>>,
    pub exchange: exchange::Exchange,
    pub completed: BTreeSet<(AccountId, osg_model::Id)>,
    pub balances: BTreeMap<Principal, Balance>,
    pub licences: BTreeSet<Principal>,
    pub official_uec_per_lat: u64,
    pub last_day: i64,
    pub entries: Vec<LedgerEntry>,
}

impl Default for Economy {
    fn default() -> Self {
        Self::at(osg_model::calendar::now_unix_ms())
    }
}

impl Economy {
    pub fn at(now: i64) -> Self {
        Self {
            service_holds: BTreeMap::new(),
            turnover_taxes: BTreeMap::new(),
            storage: BTreeMap::new(),
            exchange: Default::default(),
            completed: BTreeSet::new(),
            balances: BTreeMap::new(),
            licences: BTreeSet::new(),
            official_uec_per_lat: 3_200_000,
            last_day: now.div_euclid(DAY_MS),
            entries: Vec::new(),
        }
    }

    pub fn validate(&self, directory: &osg_model::ownership::OwnershipDirectory) -> Result<()> {
        ensure!(
            self.service_holds.values().all(|hold| {
                directory.contains(hold.payer) && directory.contains(hold.operator) && !hold.charged
            }),
            "invalid industry payment reservation"
        );
        ensure!(
            self.turnover_taxes
                .iter()
                .all(|(id, rate)| directory.sovereignties.contains_key(id) && *rate <= 10_000),
            "invalid turnover tax"
        );
        ensure!(
            self.official_uec_per_lat > 0,
            "invalid official conversion rate"
        );
        ensure!(
            self.last_day >= 0
                && self.last_day <= osg_model::calendar::now_unix_ms().div_euclid(DAY_MS),
            "invalid settlement day"
        );
        ensure!(
            self.completed
                .iter()
                .all(|(account, _)| directory.players.contains_key(account)),
            "invalid wallet receipt"
        );
        ensure!(
            self.balances.keys().all(|owner| directory.contains(*owner)),
            "invalid money account"
        );
        ensure!(
            self.licences.iter().all(|owner| directory.contains(*owner)),
            "invalid LAT licence"
        );
        ensure!(
            self.entries.iter().enumerate().all(|(index, entry)| {
                entry.sequence == index as u64 + 1
                    && directory.contains(entry.owner)
                    && entry
                        .counterparty
                        .is_none_or(|owner| directory.contains(owner))
            }),
            "invalid money ledger"
        );
        let mut replay = BTreeMap::<(Principal, Currency), u64>::new();
        for entry in &self.entries {
            let previous = replay
                .get(&(entry.owner, entry.currency))
                .copied()
                .unwrap_or(0);
            let balance = if entry.credit {
                previous.checked_add(entry.amount)
            } else {
                previous.checked_sub(entry.amount)
            }
            .context("invalid ledger arithmetic")?;
            ensure!(balance == entry.balance, "ledger entry balance mismatch");
            replay.insert((entry.owner, entry.currency), balance);
        }
        for (&owner, balance) in &self.balances {
            ensure!(
                replay.remove(&(owner, Currency::Uec)).unwrap_or(0) == balance.uec
                    && replay.remove(&(owner, Currency::Lat)).unwrap_or(0) == balance.lat,
                "account balance differs from ledger"
            );
        }
        ensure!(replay.is_empty(), "ledger account unavailable");
        self.validate_exchange(directory)?;
        for (&owner, balance) in &self.balances {
            ensure!(
                self.reserved(owner, Currency::Uec) <= balance.uec
                    && self.reserved(owner, Currency::Lat) <= balance.lat,
                "unfunded money reservation"
            );
        }
        Ok(())
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
        self.entries.push(LedgerEntry {
            sequence: self.entries.len() as u64 + 1,
            time_ms: now,
            owner,
            currency,
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
        let account = self.balances.entry(owner).or_default();
        let balance = match currency {
            Currency::Uec => &mut account.uec,
            Currency::Lat => &mut account.lat,
        };
        *balance = balance
            .checked_add(amount)
            .context("money balance overflow")?;
        self.record(owner, currency, true, amount, EntryKind::Issue, None, now);
        Ok(())
    }

    /// Charge each completed UTC day once, including days elapsed offline.
    pub fn settle(&mut self, now: i64) {
        let today = now.div_euclid(DAY_MS);
        while self.last_day < today {
            let rate = daily_rate(self.last_day);
            let mut charges = Vec::new();
            for (&owner, balance) in &mut self.balances {
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
            // Demurrage applies to total balances, including reserved funds.
            // Cancel newest bids until the remaining reservations are funded.
            self.release_unfunded_orders();
        }
    }

    pub fn restricted(
        &self,
        directory: &osg_model::ownership::OwnershipDirectory,
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

    pub(crate) fn transfer(
        &mut self,
        directory: Option<&osg_model::ownership::OwnershipDirectory>,
        from: Principal,
        to: Principal,
        currency: Currency,
        amount: u64,
        restricted: bool,
        now: i64,
    ) -> Result<()> {
        let mut staged = self.clone();
        staged.transfer_inner(directory, from, to, currency, amount, restricted, now)?;
        *self = staged;
        Ok(())
    }

    fn transfer_inner(
        &mut self,
        directory: Option<&osg_model::ownership::OwnershipDirectory>,
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
        let source = self.balances.get(&from).cloned().unwrap_or_default();
        let available = match currency {
            Currency::Uec => source.uec,
            Currency::Lat => source.lat,
        };
        ensure!(
            available.saturating_sub(self.reserved(from, currency)) >= amount,
            "insufficient available funds"
        );

        if currency == Currency::Lat && restricted {
            let net = self.move_money(
                directory,
                from,
                to,
                Currency::Lat,
                amount,
                EntryKind::Transfer,
                now,
            )?;
            if net == 0 {
                return Ok(());
            }
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
            return Ok(());
        }

        self.move_money(
            directory,
            from,
            to,
            currency,
            amount,
            EntryKind::Transfer,
            now,
        )
        .map(|_| ())
    }
}

pub fn settle(world: &mut World) {
    let now = osg_model::calendar::now_unix_ms();
    if world.resource::<Economy>().last_day < now.div_euclid(DAY_MS) {
        world.resource_mut::<Economy>().settle(now);
    }
    let economy = world.resource::<Economy>();
    let directory = &world.resource::<Directory>().0;
    let invalid: Vec<_> = economy
        .exchange
        .orders
        .values()
        .filter(|order| {
            (order.instrument == osg_model::market::Instrument::Fx
                || order.side == osg_model::market::Side::Buy
                    && order.instrument.currency() == Currency::Lat)
                && economy.restricted(directory, order.owner)
        })
        .map(|order| order.id)
        .collect();
    if !invalid.is_empty() {
        let mut economy = world.resource_mut::<Economy>();
        for id in invalid {
            economy.exchange.orders.remove(&id);
        }
    }
}

pub fn apply(
    world: &mut World,
    account: AccountId,
    id: osg_model::Id,
    command: WalletCommand,
) -> Result<()> {
    if world
        .resource::<Economy>()
        .completed
        .contains(&(account, id))
    {
        return Ok(());
    }
    settle(world);
    if let WalletCommand::SetTurnoverTax {
        sovereignty,
        basis_points,
    } = &command
    {
        ensure!(*basis_points <= 10_000, "turnover tax cannot exceed 100%");
        ensure!(
            world
                .resource::<Directory>()
                .0
                .administers(account, Principal::Sovereignty(*sovereignty)),
            "sovereignty officer required"
        );
        let mut economy = world.resource_mut::<Economy>();
        economy.turnover_taxes.insert(*sovereignty, *basis_points);
        economy.completed.insert((account, id));
        return Ok(());
    }
    let directory = &world.resource::<Directory>().0;
    let (from, to) = match &command {
        WalletCommand::SetTurnoverTax { .. } => unreachable!(),
        WalletCommand::Transfer { from, to, .. } | WalletCommand::TransferGas { from, to, .. } => {
            (*from, *to)
        }
    };
    ensure!(
        directory.administers(account, from),
        "account administration required"
    );
    ensure!(directory.contains(to), "recipient unavailable");
    match command {
        WalletCommand::SetTurnoverTax { .. } => unreachable!(),
        WalletCommand::Transfer {
            currency, amount, ..
        } => {
            let economy = world.resource::<Economy>();
            ensure!(
                currency != Currency::Lat || !economy.restricted(directory, from),
                "LAT licence required"
            );
            let restricted = economy.restricted(directory, to);
            world.resource_scope(|world, mut economy: Mut<Economy>| {
                economy.transfer(
                    Some(&world.resource::<Directory>().0),
                    from,
                    to,
                    currency,
                    amount,
                    restricted,
                    osg_model::calendar::now_unix_ms(),
                )
            })
        }
        WalletCommand::TransferGas { amount, .. } => {
            ensure!(
                directory.administers(account, to),
                "gas can only move between administered accounts"
            );
            world
                .resource::<super::gas::GasLedger>()
                .transfer(from, to, amount)
        }
    }?;
    world
        .resource_mut::<Economy>()
        .completed
        .insert((account, id));
    Ok(())
}

pub fn snapshot(world: &World, account: AccountId, subscription: &WalletQuery) -> WalletSnapshot {
    let directory = &world.resource::<Directory>().0;
    if !subscription.valid() || !directory.administers(account, subscription.owner) {
        return WalletSnapshot {
            error: Some("Wallet access unavailable".into()),
            ..Default::default()
        };
    }
    let economy = world.resource::<Economy>();
    let owners = std::iter::once(Principal::Player(account))
        .chain(
            directory
                .organizations
                .keys()
                .copied()
                .map(Principal::Organization),
        )
        .chain(
            directory
                .sovereignties
                .keys()
                .copied()
                .map(Principal::Sovereignty),
        )
        .filter(|owner| directory.administers(account, *owner));
    let owners = std::iter::once(subscription.owner)
        .chain(owners.filter(|owner| *owner != subscription.owner));
    let balances = owners
        .map(|owner| {
            let balance = economy.balances.get(&owner).cloned().unwrap_or_default();
            WalletBalance {
                turnover_tax_bps: economy
                    .tax_rate(directory, owner)
                    .map_or(0, |(_, rate)| rate),
                owner,
                uec: balance.uec,
                lat: balance.lat,
                reserved_uec: economy.reserved(owner, Currency::Uec),
                reserved_lat: economy.reserved(owner, Currency::Lat),
                next_demurrage: (balance.uec.saturating_sub(DEMURRAGE_EXEMPTION) as u128
                    * daily_rate(economy.last_day))
                .div_ceil(RATE_SCALE) as u64,
                lat_restricted: economy.restricted(directory, owner),
            }
        })
        .take(256)
        .collect();
    let mut entries: Vec<_> = economy
        .entries
        .iter()
        .rev()
        .filter(|entry| {
            entry.owner == subscription.owner
                && subscription
                    .before
                    .is_none_or(|before| entry.sequence < before)
        })
        .take(subscription.limit as usize + 1)
        .cloned()
        .collect();
    let more = entries.len() > subscription.limit as usize;
    entries.truncate(subscription.limit as usize);
    let next_before = more.then(|| entries.last().unwrap().sequence);
    WalletSnapshot {
        fx_trades: economy
            .exchange
            .trades
            .iter()
            .rev()
            .filter(|trade| trade.instrument == osg_model::market::Instrument::Fx)
            .take(60)
            .cloned()
            .collect(),
        owner: Some(subscription.owner),
        balances,
        entries,
        next_before,
        next_charge_ms: (economy.last_day + 1) * DAY_MS,
        market_uec_per_lat: economy
            .exchange
            .trades
            .iter()
            .rev()
            .find(|trade| trade.instrument == osg_model::market::Instrument::Fx)
            .map(|trade| trade.price),
        error: None,
    }
}
