use super::*;
use osg_model::{industry::*, rpc::Page};
use std::collections::BTreeMap;

struct Read<Q, T> {
    load: Load<Q, T>,
    state: QueryState<T>,
}

impl<Q: Clone + PartialEq, T: Send + 'static> Read<Q, T> {
    fn update<F, Fut>(
        &mut self,
        context: Option<(SessionKey, Q)>,
        now: Duration,
        revision: u64,
        fetch: F,
    ) where
        F: FnOnce(Id, Q) -> Fut,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        self.load.refresh(revision);
        let (changed, result) = self.load.update(context, now, fetch);
        self.state.apply(changed, result);
    }

    fn ready(&self) -> bool {
        self.state.completed()
    }
}

impl<Q, T> Default for Read<Q, T> {
    fn default() -> Self {
        Self {
            load: Load::default(),
            state: QueryState::Loading,
        }
    }
}

#[derive(Resource, Default)]
pub struct IndustryState {
    pub snapshot: IndustryView,
    pub query: Option<IndustryQuery>,
    directory: Read<Option<Id>, Page<FacilitySummary, Id>>,
    hangar: Read<HangarQuery, HangarView>,
    catalogue: Read<(), IndustryCatalogue>,
    inventories: BTreeMap<Id, Read<Id, FacilityView>>,
}

impl IndustryState {
    pub fn set_query(&mut self, mut query: Option<IndustryQuery>) {
        if let Some(query) = &mut query {
            query.inventories.sort_unstable();
            query.inventories.dedup();
        }
        self.query = query;
    }

    pub fn ready(&self) -> bool {
        self.query.as_ref().is_some_and(|query| {
            (!query.directory || self.directory.ready())
                && (query.hangar.is_none() || self.hangar.ready())
                && (!query.catalogue || self.catalogue.ready())
                && query
                    .inventories
                    .iter()
                    .all(|id| self.inventories.get(id).is_some_and(Read::ready))
        })
    }

    pub fn loading(&self) -> bool {
        self.query.is_some() && !self.ready()
    }

    pub fn update(
        &mut self,
        net: &OsgNetClient,
        context: Option<SessionKey>,
        now: Duration,
        revision: u64,
    ) {
        let query = self
            .query
            .clone()
            .filter(|_| context.is_some())
            .unwrap_or_default();
        let directory = context
            .filter(|_| query.directory)
            .map(|key| (key, query.directory_after));
        let client = net.clone();
        self.directory
            .update(directory, now, revision, move |world, after| async move {
                call(client.list_facilities(world, after, 128)).await
            });

        let hangar = query
            .hangar
            .clone()
            .and_then(|query| context.map(|key| (key, query)));
        let client = net.clone();
        self.hangar
            .update(hangar, now, revision, move |world, query| async move {
                call(client.hangar(world, query.ship, query.after)).await
            });

        let client = net.clone();
        self.catalogue.update(
            context.filter(|_| query.catalogue).map(|key| (key, ())),
            now,
            revision,
            move |world, ()| async move { call(client.industry_catalogue(world)).await },
        );

        self.inventories
            .retain(|id, _| query.inventories.contains(id));
        for id in &query.inventories {
            let client = net.clone();
            self.inventories.entry(*id).or_default().update(
                context.map(|key| (key, *id)),
                now,
                revision,
                move |world, id| async move { call(client.facility(world, id)).await },
            );
        }

        self.snapshot = IndustryView {
            directory: self.directory.state.clone(),
            hangar: self.hangar.state.clone(),
            hangar_query: self.hangar.ready().then_some(query.hangar).flatten(),
            catalogue: self.catalogue.state.clone(),
            facilities: self
                .inventories
                .iter()
                .map(|(id, entry)| (*id, entry.state.clone()))
                .collect(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_inventory_refresh_failure_and_retry_remove_private_values() {
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let context = Some((
            SessionKey {
                world: Id([1; 16]),
                generation: 1,
            },
            7_u8,
        ));
        let mut denied = Read::<u8, Vec<u8>>::default();
        let ready = QueryState::Ready(vec![42]);
        denied.state = ready.clone();
        denied.load.context = context;
        let (send, receive) = oneshot::channel();
        denied.load.pending = Some(receive);
        denied.update(context, Duration::ZERO, 0, |_, _| async {
            panic!("refresh already in flight")
        });
        assert_eq!(denied.state, ready);

        send.send(Err("Permission revoked".into())).unwrap();
        denied.update(context, Duration::ZERO, 0, |_, _| async {
            panic!("refresh interval")
        });
        assert_eq!(
            denied.state,
            QueryState::Failed("Permission revoked".into())
        );
        assert!(denied.state.as_ref().is_none());
        // Another inventory has an independent result throughout this failure.
        let other = Read::<u8, Vec<u8>> {
            state: QueryState::Ready(vec![9]),
            ..Default::default()
        };
        assert_eq!(other.state.as_ref(), Some(&vec![9]));

        denied.update(context, Duration::ZERO, 1, |_, _| async { Ok(vec![84]) });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while denied.state.as_ref().is_none() {
            denied.update(context, Duration::ZERO, 1, |_, _| async {
                panic!("retry coalesced")
            });
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(denied.state.as_ref(), Some(&vec![84]));
        denied.update(
            Some((
                SessionKey {
                    world: Id([1; 16]),
                    generation: 2,
                },
                8,
            )),
            Duration::ZERO,
            1,
            |_, _| async { std::future::pending().await },
        );
        assert!(matches!(denied.state, QueryState::Loading));
    }
}
