pub mod query;

use glam::DVec3;
use osg_model::*;
use osg_spatial::{Entry, SpatialHash};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub tracks: BTreeMap<TrackId, Arc<Track>>,
    spatial: SpatialHash,
    slots: BTreeMap<TrackId, u32>,
    spatial_ids: Vec<Option<TrackId>>,
    free_slots: Vec<u32>,
    reply_sizes: BTreeMap<usize, usize>,
    speeds: BTreeMap<u64, usize>,
    tags: BTreeMap<Tag, BTreeSet<TrackId>>,
}

pub struct VolumeCandidates {
    pub tracks: Vec<Arc<Track>>,
    pub work: usize,
    pub complete: bool,
}

fn track_speed(track: &Track) -> f64 {
    DVec3::from_array(track.pose.velocity).length() + 3.0 * track.velocity_sigma_m_s
}

impl Snapshot {
    fn remove(&mut self, id: TrackId) {
        if let Some(track) = self.tracks.remove(&id) {
            let speed = track_speed(&track).to_bits();
            let count = self.speeds.get_mut(&speed).expect("indexed track speed");
            *count -= 1;
            if *count == 0 {
                self.speeds.remove(&speed);
            }
            let bytes = wasm_intel::track_arena_bytes(&track);
            let count = self
                .reply_sizes
                .get_mut(&bytes)
                .expect("indexed track size");
            *count -= 1;
            if *count == 0 {
                self.reply_sizes.remove(&bytes);
            }
            let slot = self.slots.remove(&id).unwrap();
            self.spatial.remove(slot);
            self.spatial_ids[slot as usize] = None;
            self.free_slots.push(slot);
            for tag in &track.tags {
                if let Some(ids) = self.tags.get_mut(tag) {
                    ids.remove(&id);
                    if ids.is_empty() {
                        self.tags.remove(tag);
                    }
                }
            }
        }
    }

    pub(crate) fn maximum_track_bytes(&self) -> usize {
        self.reply_sizes
            .last_key_value()
            .map_or(0, |(&bytes, _)| bytes)
    }

    pub fn nearby_volumes(
        &self,
        centre: GalacticPosition,
        radius_m: f64,
        after_seconds: f64,
        maximum_work: usize,
    ) -> VolumeCandidates {
        let maximum_speed = self
            .speeds
            .last_key_value()
            .map_or(0.0, |(&bits, _)| f64::from_bits(bits));
        let mut cursor =
            self.spatial
                .range_cursor(centre, radius_m + maximum_speed * after_seconds, true);
        // Leave room for the caller to extrapolate and classify every match.
        let batch = self
            .spatial
            .advance_range(&mut cursor, maximum_work / 5, maximum_work / 5);
        let work = batch.stats.work() + 4 * batch.ids.len();
        let tracks = batch
            .ids
            .into_iter()
            .map(|slot| {
                self.tracks[&self.spatial_ids[slot as usize].expect("indexed track")].clone()
            })
            .collect();
        VolumeCandidates {
            tracks,
            work,
            complete: batch.complete && !batch.invalidated,
        }
    }

    pub fn put(&mut self, track: Track) {
        self.remove(track.id);
        let slot = self.free_slots.pop().unwrap_or_else(|| {
            let slot = u32::try_from(self.spatial_ids.len()).expect("too many sensor tracks");
            self.spatial_ids.push(None);
            slot
        });
        self.spatial.insert(
            slot,
            Entry {
                position: track.pose.position,
                radius_m: track.radius_m.unwrap_or(1.0) + 3.0 * track.position_sigma_m,
                luminosity: 0.0,
            },
        );
        self.slots.insert(track.id, slot);
        self.spatial_ids[slot as usize] = Some(track.id);
        for tag in &track.tags {
            self.tags.entry(tag.clone()).or_default().insert(track.id);
        }
        let bytes = wasm_intel::track_arena_bytes(&track);
        *self.reply_sizes.entry(bytes).or_default() += 1;
        *self
            .speeds
            .entry(track_speed(&track).to_bits())
            .or_default() += 1;
        self.tracks.insert(track.id, Arc::new(track));
    }
}

#[derive(Clone)]
pub struct Measurement {
    pub physical: EntityId,
    pub platform: EntityId,
    pub pose: Pose,
    pub sigma_m: f64,
    pub sigma_velocity: f64,
    pub entity: Option<EntityId>,
    pub tags: BTreeSet<Tag>,
    pub provenance: Provenance,
    pub radius_m: Option<f64>,
    pub appearance: Option<[u8; 32]>,
}

impl Measurement {
    pub fn sensor(
        seed: &[u8; 32],
        platform: EntityId,
        physical: EntityId,
        origin: GalacticPosition,
        pose: &Pose,
        tick: u64,
    ) -> Self {
        let distance = pose.position.relative_to(origin).length();
        let sigma = (1. + (distance * 1e-5).powi(2)).sqrt();
        let mut measured = pose.clone();
        let mut offset = [0.; 3];
        for axis in 0..6 {
            let bias = noise(seed, platform, physical, axis, 0);
            let varying = noise(seed, platform, physical, axis, tick.saturating_add(1));
            let error = sigma * (0.9_f64.sqrt() * bias + 0.1_f64.sqrt() * varying);
            if axis < 3 {
                offset[axis as usize] = error;
            } else {
                measured.velocity[axis as usize - 3] += error;
            }
        }
        measured.position = measured.position.offset_by(DVec3::from_array(offset));
        measured.rotation = Pose::default().rotation;
        measured.angular_velocity = [0.; 3];
        Self {
            physical,
            platform,
            pose: measured,
            sigma_m: sigma,
            sigma_velocity: sigma,
            entity: None,
            tags: BTreeSet::from([Tag::Kind("ship".into())]),
            provenance: Provenance::Sensor,
            radius_m: None,
            appearance: None,
        }
    }

    pub fn authenticated(
        physical: EntityId,
        platform: EntityId,
        pose: Pose,
        iff: &IffIdentity,
        provenance: Provenance,
    ) -> Self {
        let mut tags = BTreeSet::from([Tag::Kind("ship".into()), Tag::IffOwner(iff.owner)]);
        if let Some(faction) = iff.faction {
            tags.insert(Tag::IffFaction(faction));
        }
        tags.extend(iff.labels.iter().cloned().map(Tag::Advertised));
        Self {
            physical,
            platform,
            pose,
            sigma_m: 0.,
            sigma_velocity: 0.,
            entity: Some(physical),
            tags,
            provenance,
            radius_m: None,
            appearance: None,
        }
    }

    fn valid(&self) -> bool {
        self.sigma_m.is_finite()
            && self.sigma_m >= 0.
            && self.sigma_velocity.is_finite()
            && self.sigma_velocity >= 0.
            && self
                .pose
                .velocity
                .iter()
                .chain(&self.pose.rotation)
                .chain(&self.pose.angular_velocity)
                .all(|x| x.is_finite())
            && self.tags.len() <= 32
            && self.tags.iter().all(Tag::valid)
    }
}

fn noise(seed: &[u8; 32], platform: Id, physical: Id, axis: u8, tick: u64) -> f64 {
    let mut hash = blake3::Hasher::new_keyed(seed);
    hash.update(&platform.0);
    hash.update(&physical.0);
    hash.update(&[axis]);
    hash.update(&tick.to_le_bytes());
    let bytes = hash.finalize();
    let a = u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap()) >> 11;
    let b = u64::from_le_bytes(bytes.as_bytes()[8..16].try_into().unwrap()) >> 11;
    let u = (a as f64 + 1.) / ((1_u64 << 53) as f64 + 1.);
    let v = b as f64 / (1_u64 << 53) as f64;
    (-2. * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

pub struct Group {
    pub id: GroupId,
    snapshot: Arc<Snapshot>,
    association: BTreeMap<EntityId, TrackId>,
    annotations: BTreeMap<EntityId, BTreeSet<String>>,
}

impl Group {
    pub fn new(id: GroupId) -> Self {
        Self {
            id,
            snapshot: Arc::default(),
            association: BTreeMap::new(),
            annotations: BTreeMap::new(),
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.clone()
    }

    pub fn annotate(&mut self, entity: EntityId, labels: BTreeSet<String>) -> anyhow::Result<()> {
        anyhow::ensure!(
            labels.len() <= 16 && labels.iter().all(|s| Tag::Annotation(s.clone()).valid()),
            "invalid annotations"
        );
        self.annotations.insert(entity, labels);
        Ok(())
    }

    pub fn update(&mut self, tick: u64, observations: impl IntoIterator<Item = Measurement>) {
        if tick <= self.snapshot.tick && tick != 0 {
            return;
        }
        let previous_tick = self.snapshot.tick;
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.tick = tick;
        let old: Vec<_> = snapshot.tracks.values().cloned().collect();
        for track in old {
            let age = tick.saturating_sub(track.observed_tick);
            if age > 600 {
                snapshot.remove(track.id);
            } else if tick > previous_tick {
                let mut predicted = (*track).clone();
                let dt = (tick - previous_tick) as f64 * 0.1;
                predicted.pose.position = predicted
                    .pose
                    .position
                    .offset_by(DVec3::from_array(predicted.pose.velocity) * dt);
                predicted.position_sigma_m = (predicted.position_sigma_m.powi(2)
                    + (dt * predicted.velocity_sigma_m_s).powi(2)
                    + (0.5
                        * ((age as f64 * 0.1).powi(2)
                            - (age.saturating_sub(tick - previous_tick) as f64 * 0.1).powi(2)))
                    .powi(2))
                .sqrt();
                predicted.estimate_tick = tick;
                predicted.provenance = Provenance::Extrapolated;
                snapshot.put(predicted);
            }
        }
        self.association
            .retain(|_, track| snapshot.tracks.contains_key(track));
        let mut grouped: BTreeMap<EntityId, BTreeMap<EntityId, Measurement>> = BTreeMap::new();
        for observation in observations.into_iter().filter(Measurement::valid) {
            let sources = grouped.entry(observation.physical).or_default();
            if let Some((platform, exact)) = sources.first_key_value() {
                if exact.sigma_m == 0. {
                    if observation.sigma_m != 0. || observation.platform >= *platform {
                        continue;
                    }
                    sources.clear();
                }
            }
            if observation.sigma_m == 0. {
                sources.clear();
            }
            if sources
                .get(&observation.platform)
                .is_none_or(|old| observation.sigma_m < old.sigma_m)
            {
                sources.insert(observation.platform, observation);
            }
        }
        for (entity, sources) in grouped {
            let best = sources
                .values()
                .min_by(|a, b| a.sigma_m.total_cmp(&b.sigma_m))
                .unwrap();
            let existing = self
                .association
                .get(&entity)
                .and_then(|id| snapshot.tracks.get(id));
            let continuous = existing.filter(|track| {
                best.entity.is_some() && track.entity == best.entity
                    || track.pose.position.relative_to(best.pose.position).length()
                        <= 3. * (track.position_sigma_m + best.sigma_m)
            });
            let id = continuous.map_or_else(Id::new, |track| track.id);
            let known = best
                .entity
                .or_else(|| continuous.and_then(|track| track.entity));
            let mut track = Track {
                spatial_instance: continuous.map_or_else(Id::new, |track| track.spatial_instance),
                id,
                entity: known,
                pose: best.pose.clone(),
                position_sigma_m: best.sigma_m,
                velocity_sigma_m_s: best.sigma_velocity,
                observed_tick: tick,
                estimate_tick: tick,
                tags: best.tags.clone(),
                provenance: best.provenance,
                radius_m: best.radius_m,
                appearance: best.appearance,
            };
            if best.sigma_m > 0. {
                let mut position = DVec3::ZERO;
                let mut velocity = DVec3::ZERO;
                let mut weight = 0.;
                for source in sources.values() {
                    let w = 1. / source.sigma_m.max(1e-6).powi(2);
                    weight += w;
                    position += source.pose.position.relative_to(best.pose.position) * w;
                    velocity += DVec3::from_array(source.pose.velocity) * w;
                }
                track.pose.position = best.pose.position.offset_by(position / weight);
                track.pose.velocity = (velocity / weight).to_array();
                track.position_sigma_m = weight.recip().sqrt().max(1.).max(best.sigma_m / 4.);
                track.velocity_sigma_m_s = track.position_sigma_m;
            }
            if let Some(known) = known {
                if let Some(labels) = self.annotations.get(&known) {
                    track
                        .tags
                        .extend(labels.iter().cloned().map(Tag::Annotation));
                }
                if best.entity.is_none()
                    && let Some(previous) = continuous
                {
                    track.tags.extend(
                        previous
                            .tags
                            .iter()
                            .filter(|tag| matches!(tag, Tag::IffOwner(_) | Tag::IffFaction(_)))
                            .cloned(),
                    );
                }
            }
            snapshot.put(track);
            self.association.insert(entity, id);
        }
    }
}

#[derive(Default)]
pub struct Intelligence {
    keys: BTreeMap<InfoGroupKey, GroupId>,
    pub groups: BTreeMap<GroupId, Group>,
}

impl Intelligence {
    pub fn join(&mut self, key: InfoGroupKey) -> GroupId {
        if let Some(id) = self.keys.get(&key) {
            return *id;
        }
        let id = Id::new();
        self.keys.insert(key, id);
        self.groups.insert(id, Group::new(id));
        id
    }

    pub fn retain_active(&mut self, active: &BTreeSet<GroupId>) {
        self.groups
            .retain(|id, group| active.contains(id) || !group.snapshot.tracks.is_empty());
        self.keys.retain(|_, id| self.groups.contains_key(id));
    }

    pub fn resolve(&self, key: &InfoGroupKey) -> Option<GroupId> {
        self.keys.get(key).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use query::Queries;

    #[test]
    fn measurement_is_shared_and_identical_queries_do_not_reroll_noise() {
        let mut group = Group::new(Id::new());
        let object = Id::new();
        let sensor = Id::new();
        let pose = Pose {
            position: GalacticPosition::new(1_000_000_000_000, 0, 0),
            ..Pose::default()
        };
        let sample =
            Measurement::sensor(&[7; 32], sensor, object, GalacticPosition::ZERO, &pose, 1);
        group.update(1, [sample.clone(), sample]);
        let snapshot = group.snapshot();
        assert_eq!(snapshot.tracks.len(), 1);
        assert_eq!(snapshot.tracks.values().next().unwrap().entity, None);
        let query = TrackQuery {
            limit: 256,
            work: 100_000,
            ..Default::default()
        };
        let mut queries = Queries::default();
        let a = queries
            .start(
                snapshot.clone(),
                query.clone(),
                1,
                osg_model::wasm_world::ReplyCapacity::UNLIMITED,
            )
            .unwrap();
        let b = queries
            .start(
                snapshot,
                query,
                1,
                osg_model::wasm_world::ReplyCapacity::UNLIMITED,
            )
            .unwrap();
        assert_eq!(a.tracks, b.tracks);
        assert_eq!(a.completion, Completion::Complete);
    }

    #[test]
    fn empty_expensive_search_is_metered_and_continuation_expires() {
        let mut group = Group::new(Id::new());
        group.update(
            1,
            (0..20).map(|_| {
                Measurement::sensor(
                    &[1; 32],
                    Id::new(),
                    Id::new(),
                    GalacticPosition::ZERO,
                    &Pose::default(),
                    1,
                )
            }),
        );
        let mut queries = Queries::default();
        let query = TrackQuery {
            limit: 256,
            work: 1200,
            exclude: BTreeSet::from([Tag::Kind("ship".into())]),
            ..Default::default()
        };
        let result = queries
            .start(
                group.snapshot(),
                query,
                1,
                osg_model::wasm_world::ReplyCapacity::UNLIMITED,
            )
            .unwrap();
        assert!(result.tracks.is_empty());
        assert_eq!(result.completion, Completion::WorkLimit);
        assert!(result.gas_used >= 1100 && result.gas_used <= 1200);
        assert!(
            queries
                .next(
                    result.continuation.unwrap(),
                    100_000,
                    12,
                    osg_model::wasm_world::ReplyCapacity::UNLIMITED
                )
                .is_err()
        );
    }

    #[test]
    fn expiry_breaks_unknown_identity_and_snapshot_is_immutable() {
        let mut group = Group::new(Id::new());
        let observation = Measurement::sensor(
            &[2; 32],
            Id::new(),
            Id::new(),
            GalacticPosition::ZERO,
            &Pose::default(),
            1,
        );
        group.update(1, [observation.clone()]);
        let old = group.snapshot();
        let old_id = *old.tracks.keys().next().unwrap();
        group.update(602, []);
        assert!(group.snapshot().tracks.is_empty());
        assert_eq!(old.tracks.len(), 1);
        group.update(603, [observation]);
        assert_ne!(*group.snapshot().tracks.keys().next().unwrap(), old_id);
    }
}
