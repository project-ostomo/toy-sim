use imbl::{OrdMap, OrdSet};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::ops::RangeBounds;
use std::sync::Arc;

/// A record defines its indexes once. All writes, including restoration and
/// record replacement, pass through the same index maintenance operations.
pub trait Record: Clone {
    type Key: Clone + Ord + std::fmt::Debug;
    type Index: Clone + Ord + std::fmt::Debug;

    fn key(&self) -> Self::Key;
    fn indexes(&self) -> Vec<Self::Index>;
    fn index_key(index: &Self::Index) -> Self::Key;
}

impl<R: Record> Record for Arc<R> {
    type Key = R::Key;
    type Index = R::Index;

    fn key(&self) -> Self::Key {
        self.as_ref().key()
    }

    fn indexes(&self) -> Vec<Self::Index> {
        self.as_ref().indexes()
    }

    fn index_key(index: &Self::Index) -> Self::Key {
        R::index_key(index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Records<R: Record> {
    rows: OrdMap<R::Key, R>,
    indexes: OrdSet<R::Index>,
}

impl<R: Record> Records<R> {
    pub fn insert(&mut self, row: R) -> Option<R> {
        let key = row.key();
        let previous = self.remove(&key);
        self.indexes.extend(row.indexes());
        self.rows.insert(key, row);
        previous
    }

    pub fn remove(&mut self, key: &R::Key) -> Option<R> {
        let row = self.rows.remove(key)?;
        for index in row.indexes() {
            self.indexes.remove(&index);
        }
        Some(row)
    }

    pub fn get(&self, key: &R::Key) -> Option<&R> {
        self.rows.get(key)
    }

    pub fn contains_key(&self, key: &R::Key) -> bool {
        self.rows.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &R> {
        self.rows.values()
    }

    pub fn range(&self, range: impl RangeBounds<R::Key>) -> impl DoubleEndedIterator<Item = &R> {
        self.rows.range(range).map(|(_, row)| row)
    }

    pub fn query(&self, range: impl RangeBounds<R::Index>) -> impl DoubleEndedIterator<Item = &R> {
        self.indexes
            .range(range)
            .map(|index| &self.rows[&R::index_key(index)])
    }

    pub fn last_key(&self) -> Option<&R::Key> {
        self.rows.get_max().map(|(key, _)| key)
    }
}

/// Map interface for keyed domain records. Replace a record to update it and
/// its indexes together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedMap<R: Record> {
    records: Records<Arc<R>>,
}

impl<R: Record> IndexedMap<R> {
    pub fn insert(&mut self, key: R::Key, row: R) {
        assert_eq!(key, row.key());
        self.records.insert(Arc::new(row));
    }

    pub fn get(&self, key: &R::Key) -> Option<&R> {
        self.records.get(key).map(AsRef::as_ref)
    }

    pub fn remove(&mut self, key: &R::Key) {
        self.records.remove(key);
    }

    pub fn contains_key(&self, key: &R::Key) -> bool {
        self.records.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.records = Records::default();
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &R> {
        self.records.values().map(AsRef::as_ref)
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &R::Key> {
        self.records.rows.keys()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&R::Key, &R)> {
        self.records
            .rows
            .iter()
            .map(|(key, row)| (key, row.as_ref()))
    }

    pub fn query(&self, range: impl RangeBounds<R::Index>) -> impl DoubleEndedIterator<Item = &R> {
        self.records.query(range).map(AsRef::as_ref)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&R::Key, &R) -> bool) {
        let removed: Vec<_> = self
            .iter()
            .filter_map(|(key, row)| (!keep(key, row)).then(|| key.clone()))
            .collect();
        for key in removed {
            self.remove(&key);
        }
    }

    pub fn to_map(&self) -> BTreeMap<R::Key, R> {
        self.iter()
            .map(|(key, row)| (key.clone(), row.clone()))
            .collect()
    }
}

impl<R: Record> Default for IndexedMap<R> {
    fn default() -> Self {
        Self {
            records: Records::default(),
        }
    }
}

impl<R: Record> std::ops::Index<&R::Key> for IndexedMap<R> {
    type Output = R;

    fn index(&self, key: &R::Key) -> &R {
        &self.records.rows[key]
    }
}

impl<R: Record> Default for Records<R> {
    fn default() -> Self {
        Self {
            rows: OrdMap::new(),
            indexes: OrdSet::new(),
        }
    }
}

impl<R: Record + Serialize> Serialize for Records<R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.rows.values())
    }
}

impl<'de, R: Record + Deserialize<'de>> Deserialize<'de> for Records<R> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rows = Vec::<R>::deserialize(deserializer)?;
        let mut records = Self::default();
        for row in rows {
            records.insert(row);
        }
        Ok(records)
    }
}

impl Record for osg_model::economy::LedgerEntry {
    type Key = u64;
    type Index = (osg_model::ownership::Principal, u64);

    fn key(&self) -> u64 {
        self.sequence
    }
    fn indexes(&self) -> Vec<Self::Index> {
        vec![(self.owner, self.sequence)]
    }
    fn index_key(index: &Self::Index) -> u64 {
        index.1
    }
}

impl Record for osg_model::market::Trade {
    type Key = u64;
    type Index = (osg_model::market::Instrument, u64);

    fn key(&self) -> u64 {
        self.sequence
    }
    fn indexes(&self) -> Vec<Self::Index> {
        vec![(self.instrument.clone(), self.sequence)]
    }
    fn index_key(index: &Self::Index) -> u64 {
        index.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::society::{FIRST_ID, LAST_ID, SocialIndex};
    use osg_model::{Id, ownership::PlayerAffiliation};

    #[test]
    fn edits_removals_and_restore_keep_secondary_indexes_consistent() {
        let account = Id([1; 16]);
        let organization = Id([2; 16]);
        let mut players = IndexedMap::default();
        players.insert(
            account,
            PlayerAffiliation {
                account,
                name: "Original pilot".into(),
                organization: None,
            },
        );
        let unaffiliated = SocialIndex::Parent(None, FIRST_ID)..=SocialIndex::Parent(None, LAST_ID);
        let affiliated = SocialIndex::Parent(Some(organization), FIRST_ID)
            ..=SocialIndex::Parent(Some(organization), LAST_ID);
        assert_eq!(players.query(unaffiliated.clone()).count(), 1);

        let snapshot = players.clone();
        let snapshot_bytes = postcard::to_stdvec(&snapshot).unwrap();

        {
            let mut player = players.get(&account).unwrap().clone();
            player.organization = Some(organization);
            player.name = "Renamed pilot".into();
            players.insert(account, player);
        }
        assert_eq!(players.query(unaffiliated).count(), 0);
        assert_eq!(
            players.query(affiliated.clone()).next().unwrap().account,
            account
        );
        assert_eq!(
            players
                .query(
                    SocialIndex::Gram("ori".into(), FIRST_ID)
                        ..=SocialIndex::Gram("ori".into(), LAST_ID)
                )
                .count(),
            0
        );

        let encoded = postcard::to_stdvec(&players).unwrap();
        let mut restored: IndexedMap<PlayerAffiliation> = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(restored, players);
        assert_eq!(restored.query(affiliated.clone()).count(), 1);
        restored.remove(&account);
        assert_eq!(restored.query(affiliated).count(), 0);
        assert_eq!(postcard::to_stdvec(&snapshot).unwrap(), snapshot_bytes);
        assert_eq!(snapshot.get(&account).unwrap().name, "Original pilot");
    }

    #[test]
    fn cloning_records_does_not_visit_rows() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Counted(u64, Arc<AtomicUsize>);

        impl Clone for Counted {
            fn clone(&self) -> Self {
                self.1.fetch_add(1, Ordering::Relaxed);
                Self(self.0, self.1.clone())
            }
        }

        impl Record for Counted {
            type Key = u64;
            type Index = u64;

            fn key(&self) -> u64 {
                self.0
            }

            fn indexes(&self) -> Vec<u64> {
                vec![self.0]
            }

            fn index_key(index: &u64) -> u64 {
                *index
            }
        }

        let clones = Arc::new(AtomicUsize::new(0));
        let mut records = Records::default();
        for key in 0..10_000 {
            records.insert(Counted(key, clones.clone()));
        }
        clones.store(0, Ordering::Relaxed);
        let snapshot = records.clone();
        assert_eq!(clones.load(Ordering::Relaxed), 0);
        records.remove(&42);
        assert!(records.query(42..=42).next().is_none());
        assert_eq!(snapshot.query(42..=42).next().unwrap().0, 42);
    }
}
