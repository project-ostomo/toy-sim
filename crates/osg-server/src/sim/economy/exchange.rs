use super::*;
use osg_model::Id;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Exchange {
    pub orders: Orders,
    pub history: Orders,
    pub trades: Records<Trade>,
    pub sequence: u64,
}

impl Exchange {
    #[cfg(test)]
    fn archive(&mut self, mut order: Order, now: i64) {
        order.status = if order.filled_quantity == order.original_quantity {
            OrderStatus::Completed
        } else {
            OrderStatus::Cancelled
        };
        order.closed_ms = Some(now);
        order.remaining = 0;
        self.history.insert(order.id, order);
        self.history.trim(ORDER_HISTORY_LIMIT);
    }
}

pub const ORDER_HISTORY_LIMIT: usize = 10_000;

pub fn quote(instrument: &Instrument, quantity: u64, price: u64) -> Result<u64> {
    let product = quantity as u128 * price as u128;
    let value = product.div_ceil(instrument.quantity_scale() as u128);
    u64::try_from(value).context("order value overflow")
}

pub fn commitment(order: &Order) -> Result<Option<(Currency, u64)>> {
    Ok(match order.side {
        Side::Buy => Some((
            order.instrument.currency(),
            quote(&order.instrument, order.remaining, order.price)?,
        )),
        Side::Sell if order.instrument == Instrument::Fx => Some((Currency::Lat, order.remaining)),
        Side::Sell => None,
    })
}

pub fn reserve_owner() -> Principal {
    Principal::Sovereignty(super::super::ownership::sovereignty_id("USE"))
}

pub fn apply(
    economy: &mut Economy,
    directory: &crate::sim::society::OwnershipDirectory,
    account: AccountId,
    id: Id,
    command: MarketCommand,
) -> Result<()> {
    let now = osg_model::calendar::now_unix_ms();
    economy.settle(now);
    let resting = matches!(command, MarketCommand::Limit { .. });
    match command {
        MarketCommand::Cancel { order } => {
            let order = economy.order(order).context("order unavailable")?;
            ensure!(
                directory.administers(account, order.owner),
                "order administration required"
            );
            let id = order.id;
            let order = economy.remove_order(id, now)?.unwrap();
            economy.archive(order, now);
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
                    || !economy.restricted(directory, owner),
                "LAT licence required"
            );
            economy.execute_order(
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
}
