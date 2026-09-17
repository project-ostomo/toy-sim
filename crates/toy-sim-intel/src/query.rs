use crate::{Cell, Snapshot, cell};
use anyhow::{Result, bail, ensure};
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use toy_sim_model::*;

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
    Spatial {
        low: Cell,
        high: Cell,
        current: Option<Cell>,
    },
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
}

impl Queries {
    pub fn expire(&mut self, tick: u64) {
        self.cursors.retain(|_, cursor| cursor.expires >= tick);
    }

    pub fn start(
        &mut self,
        snapshot: Arc<Snapshot>,
        query: TrackQuery,
        tick: u64,
    ) -> Result<QueryPage> {
        self.expire(tick);
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
            let extent = toy_sim_model::GalacticPosition::from_meters(glam::DVec3::splat(radius));
            Source::Spatial {
                low: cell(position.saturating_sub(extent)),
                high: cell(position.saturating_add(extent)),
                current: None,
            }
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
        self.expire(tick);
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
            self.cursors.remove(&id);
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
        loop {
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
                Source::Spatial { low, high, current } => {
                    if let Some(key) = current {
                        if let Some(id) = self.snapshot.cells[key]
                            .range((bound, Unbounded))
                            .next()
                            .copied()
                        {
                            self.pending = Some(id);
                            return true;
                        }
                    }
                    let next_cell = match current {
                        Some(key) => self
                            .snapshot
                            .cells
                            .range((Excluded(*key), Unbounded))
                            .next(),
                        None => self.snapshot.cells.range(*low..).next(),
                    };
                    let Some((key, _)) = next_cell else {
                        self.complete = true;
                        return false;
                    };
                    if key.0 > high.0 {
                        self.complete = true;
                        return false;
                    }
                    *current = Some(*key);
                    self.after = None;
                    if key.1 < low.1 || key.1 > high.1 || key.2 < low.2 || key.2 > high.2 {
                        self.after = Some(Id([255; 16]));
                    }
                    continue;
                }
            };
            self.pending = next;
            self.complete = next.is_none();
            return next.is_some();
        }
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
