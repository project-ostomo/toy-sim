use super::{Record, Records};
use imbl::OrdMap;
use osg_model::{Id, industry::CargoItem, ownership::Principal};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StockRecord {
    key: (Id, Principal),

    items: OrdMap<CargoItem, u64>,
}

impl Record for StockRecord {
    type Key = (Id, Principal);
    type Index = (Principal, Option<CargoItem>, Id);

    fn key(&self) -> Self::Key {
        self.key
    }

    fn indexes(&self) -> Vec<Self::Index> {
        std::iter::once((self.key.1, None, self.key.0))
            .chain(
                self.items
                    .keys()
                    .map(|item| (self.key.1, Some(item.clone()), self.key.0)),
            )
            .collect()
    }

    fn index_key(index: &Self::Index) -> Self::Key {
        (index.2, index.0)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StockStore {
    records: Records<StockRecord>,
}

impl StockStore {
    pub fn is_empty(&self) -> bool {
        self.records.len() == 0
    }

    pub fn get(&self, key: &(Id, Principal)) -> Option<&OrdMap<CargoItem, u64>> {
        self.records.get(key).map(|record| &record.items)
    }

    pub fn insert(&mut self, key: (Id, Principal), items: OrdMap<CargoItem, u64>) {
        if items.is_empty() {
            self.remove(&key);
        } else {
            self.records.insert(StockRecord { key, items });
        }
    }

    pub fn set_item(&mut self, key: (Id, Principal), item: CargoItem, quantity: u64) {
        let mut items = self.get(&key).cloned().unwrap_or_default();
        if quantity == 0 {
            items.remove(&item);
        } else {
            items.insert(item, quantity);
        }
        self.insert(key, items);
    }

    pub fn remove(&mut self, key: &(Id, Principal)) {
        self.records.remove(key);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(Id, Principal), &OrdMap<CargoItem, u64>)> {
        self.records
            .values()
            .map(|record| (&record.key, &record.items))
    }

    pub fn at_station(&self, station: Id) -> impl Iterator<Item = &OrdMap<CargoItem, u64>> {
        self.records
            .range(
                (station, Principal::Sovereignty(Id([0; 16])))
                    ..=(station, Principal::Player(Id([255; 16]))),
            )
            .map(|record| &record.items)
    }

    pub fn by_owner(
        &self,
        owner: Principal,
        item: Option<CargoItem>,
    ) -> impl Iterator<Item = (&(Id, Principal), &OrdMap<CargoItem, u64>)> {
        self.records
            .query((owner, item.clone(), Id([0; 16]))..=(owner, item, Id([255; 16])))
            .map(|record| (&record.key, &record.items))
    }
}

impl std::ops::Index<&(Id, Principal)> for StockStore {
    type Output = OrdMap<CargoItem, u64>;

    fn index(&self, key: &(Id, Principal)) -> &Self::Output {
        self.get(key).expect("stock unavailable")
    }
}
