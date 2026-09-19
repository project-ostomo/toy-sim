use crate::Snapshot;
use anyhow::{Result, bail, ensure};
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use toy_sim_model::*;
use toy_sim_spatial::RangeCursor;

pub const CALL_GAS: u64 = 100;
pub const VISIT_GAS: u64 = 8;
pub const CANDIDATE_GAS: u64 = 1000;

#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub remaining: u64,
    pub spent: u64,
}

impl Budget {
    pub fn new(remaining: u64) -> Self {
        Self {
            remaining,
            spent: 0,
        }
    }

    pub fn charge(&mut self, amount: u64) -> bool {
        if self.remaining < amount {
            return false;
        }
        self.remaining -= amount;
        self.spent += amount;
        true
    }
}

#[derive(Clone)]
enum Source {
    All,
    Tag(Tag),
    UnionTags(Vec<Tag>),
    Spatial(RangeCursor),
    Direct(TrackId),
}

struct Cursor {
    snapshot: Arc<Snapshot>,
    query: TrackQuery,
    source: Source,
    after: Option<TrackId>,
    pending: Option<TrackId>,
    expires: u64,
    complete: bool,
}

#[derive(Default)]
pub struct Queries {
    cursors: BTreeMap<Id, Cursor>,
    // Keep a final reference until maintenance, so finishing a metered query
    // cannot synchronously destroy an entire old spatial index.
    retired: BTreeMap<usize, Arc<Snapshot>>,
}

impl Queries {
    /// Run once per tick outside script callbacks to release obsolete snapshots.
    pub fn expire(&mut self, tick: u64) {
        self.retired.clear();
        self.cursors.retain(|_, cursor| cursor.expires >= tick);
    }

    fn retire_expired(&mut self, tick: u64) {
        let retired = &mut self.retired;
        self.cursors.retain(|_, cursor| {
            if cursor.expires >= tick {
                return true;
            }
            retired
                .entry(Arc::as_ptr(&cursor.snapshot) as usize)
                .or_insert_with(|| cursor.snapshot.clone());
            false
        });
    }

    pub fn start(
        &mut self,
        snapshot: Arc<Snapshot>,
        query: TrackQuery,
        tick: u64,
    ) -> Result<QueryPage> {
        self.retire_expired(tick);
        ensure!(self.cursors.len() < 8, "query retention limit");
        ensure!(
            (1..=256).contains(&query.limit),
            "result limit must be 1..256"
        );
        ensure!(
            query.all.len() + query.any.len() + query.exclude.len() <= 32,
            "too many query tags"
        );
        ensure!(
            query
                .all
                .iter()
                .chain(&query.any)
                .chain(&query.exclude)
                .all(Tag::valid),
            "invalid tag"
        );
        if let Some((_, radius)) = query.sphere {
            ensure!(
                radius.is_finite() && radius >= 0. && radius <= 1e22,
                "invalid query radius"
            );
        }
        let smallest_tag = query
            .all
            .iter()
            .min_by_key(|tag| snapshot.tags.get(*tag).map_or(0, |ids| ids.len()));
        let source = if let Some(id) = query.track {
            Source::Direct(id)
        } else if let Some(tag) = smallest_tag.filter(|tag| {
            query.sphere.is_none() || snapshot.tags.get(*tag).map_or(0, |ids| ids.len()) <= 256
        }) {
            Source::Tag(tag.clone())
        } else if let Some((position, radius)) = query.sphere {
            Source::Spatial(snapshot.spatial.range_cursor(position, radius, false))
        } else if !query.any.is_empty() {
            Source::UnionTags(query.any.iter().cloned().collect())
        } else {
            Source::All
        };
        let id = Id::new();
        let work = query.work;
        self.cursors.insert(
            id,
            Cursor {
                snapshot,
                query,
                source,
                after: None,
                pending: None,
                expires: tick.saturating_add(10),
                complete: false,
            },
        );
        self.next(id, work, tick)
    }

    pub fn next(&mut self, id: Id, work: u64, tick: u64) -> Result<QueryPage> {
        self.retire_expired(tick);
        let Some(cursor) = self.cursors.get_mut(&id) else {
            bail!("query continuation expired");
        };
        let mut budget = Budget::new(work);
        let mut tracks = Vec::new();
        let mut completion = Completion::WorkLimit;
        if budget.charge(CALL_GAS) {
            while tracks.len() < usize::from(cursor.query.limit) {
                if !cursor.fill_pending(&mut budget) {
                    if cursor.complete {
                        completion = Completion::Complete;
                    }
                    break;
                }
                let id = cursor.pending.unwrap();
                let track = &cursor.snapshot.tracks[&id];
                if !budget.charge(CANDIDATE_GAS) {
                    break;
                }
                if matches_query(track, &cursor.query, cursor.snapshot.tick) {
                    let bytes = postcard::experimental::serialized_size(track.as_ref()).unwrap();
                    if !budget.charge((bytes as u64).div_ceil(8)) {
                        break;
                    }
                    tracks.push((**track).clone());
                }
                cursor.pending = None;
                cursor.after = Some(id);
            }
            if tracks.len() == usize::from(cursor.query.limit) {
                completion = Completion::ResultLimit;
            }
        }
        let revision = cursor.snapshot.tick;
        let continuation = if completion == Completion::Complete {
            let cursor = self.cursors.remove(&id).unwrap();
            self.retired
                .entry(Arc::as_ptr(&cursor.snapshot) as usize)
                .or_insert(cursor.snapshot);
            None
        } else {
            Some(id)
        };
        Ok(QueryPage {
            revision,
            tracks,
            completion,
            continuation,
            gas_used: budget.spent,
        })
    }
}

impl Cursor {
    fn fill_pending(&mut self, budget: &mut Budget) -> bool {
        if self.pending.is_some() {
            return true;
        }
        if let Source::Spatial(cursor) = &mut self.source {
            let work = usize::try_from(budget.remaining / VISIT_GAS).unwrap_or(usize::MAX);
            let batch = self.snapshot.spatial.advance_range(cursor, work, 1);
            assert!(!batch.invalidated, "sensor snapshot mutated during a query");
            let visits = batch.stats.work();
            assert!(budget.charge(visits as u64 * VISIT_GAS));
            self.pending = batch.ids.first().map(|&slot| {
                self.snapshot.spatial_ids[slot as usize].expect("missing sensor track slot")
            });
            self.complete = batch.complete && self.pending.is_none();
            return self.pending.is_some();
        }
        if !budget.charge(VISIT_GAS) {
            return false;
        }
        let bound = self.after.map_or(Unbounded, Excluded);
        let next = match &mut self.source {
            Source::All => self
                .snapshot
                .tracks
                .range((bound, Unbounded))
                .next()
                .map(|(id, _)| *id),
            Source::Tag(tag) => self
                .snapshot
                .tags
                .get(tag)
                .and_then(|ids| ids.range((bound, Unbounded)).next().copied()),
            Source::UnionTags(tags) => {
                if !budget.charge(VISIT_GAS * tags.len() as u64) {
                    return false;
                }
                tags.iter()
                    .filter_map(|tag| {
                        self.snapshot
                            .tags
                            .get(tag)?
                            .range((bound, Unbounded))
                            .next()
                            .copied()
                    })
                    .min()
            }
            Source::Direct(id) => {
                (self.after.is_none() && self.snapshot.tracks.contains_key(id)).then_some(*id)
            }
            Source::Spatial(_) => unreachable!(),
        };
        self.pending = next;
        self.complete = next.is_none();
        next.is_some()
    }
}

fn matches_query(track: &Track, query: &TrackQuery, tick: u64) -> bool {
    query.all.is_subset(&track.tags)
        && (query.any.is_empty() || !query.any.is_disjoint(&track.tags))
        && query.exclude.is_disjoint(&track.tags)
        && query
            .max_age_ticks
            .is_none_or(|age| tick.saturating_sub(track.observed_tick) <= age)
        && query.sphere.is_none_or(|(position, radius)| {
            track.pose.position.relative_to(position).length_squared() <= radius * radius
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use std::collections::BTreeSet;

    fn track(number: u128, position: GalacticPosition) -> Track {
        let id = Id(number.to_le_bytes());
        Track {
            spatial_instance: id,
            id,
            entity: Some(id),
            pose: Pose {
                position,
                ..Default::default()
            },
            position_sigma_m: 0.0,
            velocity_sigma_m_s: 0.0,
            observed_tick: 1,
            estimate_tick: 1,
            tags: BTreeSet::from([
                Tag::Kind("ship".into()),
                Tag::Annotation(if number % 2 == 0 { "even" } else { "odd" }.into()),
            ]),
            provenance: Provenance::Transponder,
            radius_m: None,
            appearance: None,
        }
    }

    #[test]
    fn metered_spatial_pages_match_brute_force_and_keep_their_snapshot() {
        let origin = GalacticPosition::new(1 << 90, -(1 << 90), 1 << 86);
        let mut snapshot = Snapshot {
            tick: 1,
            ..Default::default()
        };
        for number in 0..1000 {
            let offset = DVec3::new(
                (number % 10) as f64 * 100.0 - 450.0,
                ((number / 10) % 10) as f64 * 100.0 - 450.0,
                (number / 100) as f64 * 100.0 - 450.0,
            );
            snapshot.put(track(number, origin.offset_by(offset)));
        }
        let snapshot = Arc::new(snapshot);
        let query = TrackQuery {
            sphere: Some((origin, 300.0)),
            any: BTreeSet::from([Tag::Annotation("even".into())]),
            limit: 7,
            work: CALL_GAS + VISIT_GAS,
            ..Default::default()
        };
        let expected: BTreeSet<_> = snapshot
            .tracks
            .values()
            .filter(|track| matches_query(track, &query, 1))
            .map(|track| track.id)
            .collect();
        assert!(!expected.is_empty());
        let mut queries = Queries::default();
        let mut page = queries.start(snapshot.clone(), query, 1).unwrap();
        assert!(page.tracks.is_empty());
        assert!(page.gas_used <= CALL_GAS + VISIT_GAS);
        assert_eq!(page.completion, Completion::WorkLimit);

        let mut changed = snapshot.clone();
        for number in 0..1000 {
            Arc::make_mut(&mut changed).put(track(number, GalacticPosition::ZERO));
        }
        assert_eq!(changed.spatial_ids.len(), 1000);
        let mut received = BTreeSet::new();
        let mut pages = 0;
        loop {
            for track in page.tracks {
                assert!(received.insert(track.id), "duplicate result across pages");
                assert_ne!(track.pose.position, GalacticPosition::ZERO);
            }
            let Some(continuation) = page.continuation else {
                break;
            };
            let budget = if pages % 2 == 0 { 108 } else { 1400 };
            page = queries.next(continuation, budget, 1).unwrap();
            assert!(page.gas_used <= budget);
            pages += 1;
            assert!(pages < 10_000, "query continuation did not make progress");
        }
        assert_eq!(received, expected);
        assert!(pages > 1);
    }

    #[test]
    fn old_revisions_are_freed_by_tick_maintenance_instead_of_script_queries() {
        let mut queries = Queries::default();
        let snapshot = Arc::new(Snapshot::default());
        let weak = Arc::downgrade(&snapshot);
        let result = queries
            .start(
                snapshot,
                TrackQuery {
                    limit: 1,
                    work: 1000,
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        assert_eq!(result.completion, Completion::Complete);
        assert!(weak.upgrade().is_some());
        queries.expire(2);
        assert!(weak.upgrade().is_none());

        let snapshot = Arc::new(Snapshot::default());
        let weak = Arc::downgrade(&snapshot);
        let result = queries
            .start(
                snapshot,
                TrackQuery {
                    limit: 1,
                    work: 0,
                    ..Default::default()
                },
                2,
            )
            .unwrap();
        assert!(
            queries
                .next(result.continuation.unwrap(), 1000, 13)
                .is_err()
        );
        assert!(weak.upgrade().is_some());
        queries.expire(13);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn empty_spatial_search_charges_traversal_under_tiny_budgets() {
        let mut snapshot = Snapshot::default();
        for number in 1..1000 {
            snapshot.put(track(
                number,
                GalacticPosition::from_meters(DVec3::splat(number as f64)),
            ));
        }
        let mut queries = Queries::default();
        let budget = CALL_GAS + VISIT_GAS;
        let mut page = queries
            .start(
                Arc::new(snapshot),
                TrackQuery {
                    sphere: Some((GalacticPosition::ZERO, 0.1)),
                    limit: 1,
                    work: budget,
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let mut pages = 0;
        loop {
            assert!(page.tracks.is_empty());
            assert!(page.gas_used <= budget);
            let Some(continuation) = page.continuation else {
                break;
            };
            page = queries.next(continuation, budget, 1).unwrap();
            pages += 1;
            assert!(pages < 1000);
        }
        assert_eq!(page.completion, Completion::Complete);
        assert!(pages > 0);
    }
}
