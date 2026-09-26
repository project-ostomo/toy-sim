use super::{Record, Records};
use osg_model::{Id, market::*, ownership::Principal};
use serde::{Deserialize, Serialize};
use std::ops::Bound::{Excluded, Included};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrderIndex {
    Owner(Principal, Option<Instrument>, Option<OrderStatus>, Id),
    Book(Instrument, Side, u64, u64, Id),
    Sequence(u64, Id),
    RestrictedCurrency(Id),
    Commodity(
        osg_model::industry::CargoItem,
        Id,
        osg_model::economy::Currency,
        Id,
    ),
}

impl Record for Order {
    type Key = Id;
    type Index = OrderIndex;

    fn key(&self) -> Id {
        self.id
    }

    fn indexes(&self) -> Vec<OrderIndex> {
        let mut indexes = Vec::with_capacity(6);
        for instrument in [None, Some(self.instrument.clone())] {
            for status in [None, Some(self.status)] {
                indexes.push(OrderIndex::Owner(
                    self.owner,
                    instrument.clone(),
                    status,
                    self.id,
                ));
            }
        }
        if self.status == OrderStatus::Open {
            if self.instrument == Instrument::Fx
                || self.side == Side::Buy
                    && self.instrument.currency() == osg_model::economy::Currency::Lat
            {
                indexes.push(OrderIndex::RestrictedCurrency(self.id));
            }
            if let Instrument::Commodity {
                station,
                item,
                currency,
            } = &self.instrument
            {
                indexes.push(OrderIndex::Commodity(
                    item.clone(),
                    *station,
                    *currency,
                    self.id,
                ));
            }
            let price = match self.side {
                Side::Buy => u64::MAX - self.price,
                Side::Sell => self.price,
            };
            indexes.push(OrderIndex::Book(
                self.instrument.clone(),
                self.side,
                price,
                self.sequence,
                self.id,
            ));
        }
        indexes.push(OrderIndex::Sequence(self.sequence, self.id));
        indexes
    }

    fn index_key(index: &OrderIndex) -> Id {
        match index {
            OrderIndex::Owner(_, _, _, id)
            | OrderIndex::Book(_, _, _, _, id)
            | OrderIndex::Sequence(_, id)
            | OrderIndex::Commodity(_, _, _, id)
            | OrderIndex::RestrictedCurrency(id) => *id,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Orders {
    records: Records<Order>,
}

impl Orders {
    pub fn insert(&mut self, id: Id, order: Order) {
        assert_eq!(id, order.id);
        self.records.insert(order);
    }

    pub fn remove(&mut self, id: &Id) -> Option<Order> {
        self.records.remove(id)
    }

    pub fn get(&self, id: &Id) -> Option<&Order> {
        self.records.get(id)
    }

    pub fn contains_key(&self, id: &Id) -> bool {
        self.records.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &Order> {
        self.records.values()
    }

    pub fn by_owner(
        &self,
        owner: Principal,
        instrument: Option<Instrument>,
        status: Option<OrderStatus>,
        after: Option<Id>,
    ) -> impl Iterator<Item = &Order> {
        let first = OrderIndex::Owner(
            owner,
            instrument.clone(),
            status,
            after.unwrap_or(Id([0; 16])),
        );
        let start = if after.is_some() {
            Excluded(first)
        } else {
            Included(first)
        };
        let end = OrderIndex::Owner(owner, instrument, status, Id([255; 16]));
        self.records.query((start, Included(end)))
    }

    pub fn book(&self, instrument: &Instrument, side: Side) -> impl Iterator<Item = &Order> {
        self.records.query(
            OrderIndex::Book(instrument.clone(), side, 0, 0, Id([0; 16]))
                ..=OrderIndex::Book(instrument.clone(), side, u64::MAX, u64::MAX, Id([255; 16])),
        )
    }

    pub fn trim(&mut self, limit: usize) {
        let excess = self.len().saturating_sub(limit);
        let ids: Vec<_> = self
            .records
            .query(
                OrderIndex::Sequence(0, Id([0; 16]))
                    ..=OrderIndex::Sequence(u64::MAX, Id([255; 16])),
            )
            .take(excess)
            .map(|order| order.id)
            .collect();
        for id in ids {
            self.remove(&id);
        }
    }

    pub fn restricted_currency(&self) -> impl Iterator<Item = &Order> {
        self.records.query(
            OrderIndex::RestrictedCurrency(Id([0; 16]))
                ..=OrderIndex::RestrictedCurrency(Id([255; 16])),
        )
    }

    pub fn commodity(
        &self,
        item: &osg_model::industry::CargoItem,
        after: Option<CommodityOfferCursor>,
    ) -> impl Iterator<Item = &Order> {
        use osg_model::economy::Currency;
        let start = after.map_or_else(
            || {
                Included(OrderIndex::Commodity(
                    item.clone(),
                    Id([0; 16]),
                    Currency::Uec,
                    Id([0; 16]),
                ))
            },
            |(station, currency)| {
                Excluded(OrderIndex::Commodity(
                    item.clone(),
                    station,
                    currency,
                    Id([255; 16]),
                ))
            },
        );
        self.records.query((
            start,
            Included(OrderIndex::Commodity(
                item.clone(),
                Id([255; 16]),
                Currency::Lat,
                Id([255; 16]),
            )),
        ))
    }
}

impl std::ops::Index<&Id> for Orders {
    type Output = Order;

    fn index(&self, id: &Id) -> &Order {
        self.get(id).expect("order unavailable")
    }
}
