//! Host-owned observation frames and admitted sensor estimates. No world queries.
use crate::{Observation, SensorContact};
use osg_ship_api::abi as w;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub type Position = [i128; 3];

pub fn relative(a: Position, b: Position) -> Option<[f64; 3]> {
    let mut out = [0.; 3];
    for i in 0..3 {
        out[i] = a[i].checked_sub(b[i])? as f64 / 1e6;
    }

    Some(out)
}

pub fn offset(origin: Position, displacement: [f64; 3]) -> Option<Position> {
    let mut out = origin;
    for i in 0..3 {
        let v = displacement[i] * 1e6;
        if !v.is_finite() || v.abs() >= i128::MAX as f64 {
            return None;
        }
        out[i] = origin[i].checked_add(v.round() as i128)?;
    }

    Some(out)
}

pub fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    core::array::from_fn(|i| a[i] + b[i])
}

pub fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|v| v * s)
}

pub fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

pub fn rotation(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };

    let u = [q[0], q[1], q[2]];
    let t = scale(cross(u, v), 2.);
    add(v, add(scale(t, q[3]), cross(u, t)))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub id: u64,
    pub epoch: f64,
    pub origin: Position,
    pub velocity: [f64; 3],
    pub rotation: [f64; 4],
}

impl Snapshot {
    pub fn new(id: u64, origin: Position, o: &Observation) -> Self {
        Self {
            id,
            origin,
            epoch: o.time_s,
            velocity: o.velocity,
            rotation: o.rotation,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Estimate {
    pub position: Position,
    pub velocity: [f64; 3],
    pub epoch: f64,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub contact: SensorContact,
    pub previous: Option<Estimate>,
    pub latest: Estimate,
}

impl Track {
    pub fn at(&self, t: f64) -> Option<Estimate> {
        if !t.is_finite() || t >= self.latest.epoch + 2. {
            return None;
        }

        let latest = self.latest;
        if t < latest.epoch {
            let a = self.previous?;
            if t < a.epoch {
                return None;
            }

            let alpha = (t - a.epoch) / (latest.epoch - a.epoch);
            return Some(Estimate {
                position: offset(
                    a.position,
                    scale(relative(latest.position, a.position)?, alpha),
                )?,
                velocity: add(scale(a.velocity, 1. - alpha), scale(latest.velocity, alpha)),
                epoch: t,
            });
        }

        Some(Estimate {
            position: offset(latest.position, scale(latest.velocity, t - latest.epoch))?,
            epoch: t,
            ..latest
        })
    }
}

#[derive(Clone, Debug)]
pub struct Path {
    pub header: w::SpatialPath,
    pub snapshot: Option<Snapshot>,
    pub vertices: Arc<[w::SpatialVertex]>,
    pub revision: u64,
    pub published_at: f64,
}

impl Path {
    pub fn active(&self, t: f64) -> bool {
        t >= self.published_at && t < self.header.meta.valid_until_s
    }

    pub fn at(&self, t: f64) -> Option<Position> {
        if !t.is_finite() || self.header.kind != w::PATH_TIMED {
            return None;
        }

        let first = self.vertices.first()?;
        let last = self.vertices.last()?;
        if t < first.time_s || t > last.time_s {
            return None;
        }

        let i = self.vertices.partition_point(|p| p.time_s < t);
        let v = if i == 0 {
            first.position_m
        } else {
            let a = self.vertices[i - 1];
            let b = self.vertices[i.min(self.vertices.len() - 1)];
            let f = (t - a.time_s) / (b.time_s - a.time_s);
            add(scale(a.position_m, 1. - f), scale(b.position_m, f))
        };

        let s = self.snapshot?;
        offset(
            s.origin,
            add(scale(self.header.frame.origin_velocity_m_s, t - s.epoch), v),
        )
    }
}

#[derive(Clone, Debug)]
pub struct Marker {
    pub record: w::SpatialMarker,
    pub snapshot: Option<Snapshot>,
    pub path_revision: Option<u64>,
    pub published_at: f64,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub paths: BTreeMap<u64, Path>,
    pub markers: BTreeMap<u64, Marker>,
    pub tracks: BTreeMap<u64, Track>,
    pub revision: u64,
}

impl State {
    pub fn expire(&mut self, t: f64) {
        let count = self.paths.len() + self.markers.len();
        self.paths.retain(|_, p| p.header.meta.valid_until_s > t);
        self.markers.retain(|_, p| p.record.meta.valid_until_s > t);
        self.tracks.retain(|_, p| p.latest.epoch + 2. > t);
        if count != self.paths.len() + self.markers.len() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn admit(&mut self, contacts: &[SensorContact], s: Snapshot) {
        self.expire(s.epoch);
        for c in contacts {
            let Some(position) = offset(s.origin, c.position_m) else {
                continue;
            };
            let latest = Estimate {
                position,
                velocity: add(s.velocity, c.velocity_m_s),
                epoch: s.epoch,
            };
            let previous = self.tracks.get(&c.id).and_then(|p| {
                if p.latest.epoch < s.epoch {
                    Some(p.latest)
                } else {
                    p.previous
                }
            });
            self.tracks.insert(
                c.id,
                Track {
                    contact: c.clone(),
                    previous,
                    latest,
                },
            );
        }

        let excess = self.tracks.len().saturating_sub(w::MAX_TRACKS as usize);
        if excess == 0 {
            return;
        }

        let incoming: BTreeSet<_> = contacts.iter().map(|contact| contact.id).collect();
        let mut candidates: Vec<_> = self
            .tracks
            .iter()
            .filter(|(id, _)| !incoming.contains(id))
            .map(|(id, track)| (*id, track.latest.epoch))
            .collect();
        candidates.sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));

        for (id, _) in candidates.into_iter().take(excess) {
            self.tracks.remove(&id);
        }
    }

    fn frame(
        &self,
        f: w::SpatialFrame,
        s: Option<Snapshot>,
        t: f64,
        ship: Snapshot,
    ) -> Option<(Position, [f64; 4])> {
        let identity = [0., 0., 0., 1.];
        Some(match f.kind {
            w::FRAME_SNAPSHOT => {
                let s = s?;
                (
                    offset(s.origin, scale(f.origin_velocity_m_s, t - s.epoch))?,
                    identity,
                )
            }
            w::FRAME_SHIP => (ship.origin, identity),
            w::FRAME_SHIP_BODY => (ship.origin, ship.rotation),
            w::FRAME_CONTACT => (self.tracks.get(&f.reference)?.at(t)?.position, identity),
            w::FRAME_PATH => (self.paths.get(&f.reference)?.at(t)?, identity),
            _ => return None,
        })
    }

    pub fn marker_at(&self, id: u64, presentation: Snapshot) -> Option<Position> {
        let m = self.markers.get(&id)?;
        let r = m.record;
        let now = presentation.epoch;
        if now < m.published_at || now >= r.meta.valid_until_s {
            return None;
        }

        if let Some(revision) = m.path_revision {
            let p = self.paths.get(&r.frame.reference)?;
            if p.revision != revision || !p.active(now) {
                return None;
            }
        }

        let t = if r.time_mode == w::TIME_FIXED {
            r.time_s
        } else {
            now
        };
        let (origin, q) = self.frame(r.frame, m.snapshot, t, presentation)?;
        offset(origin, rotation(q, r.offset_m))
    }

    pub fn polyline_at(&self, id: u64, presentation: Snapshot) -> Option<Vec<Position>> {
        let p = self.paths.get(&id)?;
        if !p.active(presentation.epoch) {
            return None;
        }
        if p.header.kind == w::PATH_TIMED {
            return p.vertices.iter().map(|v| p.at(v.time_s)).collect();
        }
        let (origin, q) =
            self.frame(p.header.frame, p.snapshot, presentation.epoch, presentation)?;
        p.vertices
            .iter()
            .map(|v| offset(origin, rotation(q, v.position_m)))
            .collect()
    }

    pub fn remove(&mut self, id: u64) {
        let path = self.paths.remove(&id);
        let marker = self.markers.remove(&id);

        if path.is_some() || marker.is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.markers.clear();
        self.revision = self.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(time: f64) -> Snapshot {
        Snapshot {
            id: 1,
            epoch: time,
            origin: [1_i128 << 100; 3],
            velocity: [10_000., 0., 0.],
            rotation: [0., 0., 0., 1.],
        }
    }

    fn contact(x: f64) -> SensorContact {
        SensorContact {
            iff: None,
            measured: w::Contact {
                id: 7,
                position_m: [x, 0., 0.],
                velocity_m_s: [100., 0., 0.],
                ..Default::default()
            },
            name: "Observed target".into(),
        }
    }

    #[test]
    fn contact_frames_interpolate_and_extrapolate_only_admitted_measurements() {
        let first = snapshot(10.);
        let second = Snapshot {
            epoch: 10.1,
            origin: offset(first.origin, [1000., 0., 0.]).unwrap(),
            ..first
        };
        let mut state = State::default();
        state.admit(&[contact(100.)], first);
        state.admit(&[contact(110.)], second);
        let track = &state.tracks[&7];

        assert!(track.at(9.99).is_none());
        assert_eq!(
            relative(track.at(10.).unwrap().position, first.origin),
            Some([100., 0., 0.])
        );
        let middle = relative(track.at(10.05).unwrap().position, first.origin).unwrap();
        assert!((middle[0] - 605.).abs() < 1e-5);
        let later = relative(track.at(10.2).unwrap().position, first.origin).unwrap();
        assert!((later[0] - 2120.).abs() < 1e-5);
        assert!(track.at(12.1).is_none());
        assert!(track.at(f64::NAN).is_none());
    }

    #[test]
    fn ship_and_body_frames_follow_presentation_pose_at_huge_origins() {
        let mut state = State::default();
        let mut presented = snapshot(1.);
        // A half turn around Z sends local +X to world -X.
        presented.rotation = [0., 0., 1., 0.];

        for (id, kind, expected) in [(1, w::FRAME_SHIP, 1.), (2, w::FRAME_SHIP_BODY, -1.)] {
            state.markers.insert(
                id,
                Marker {
                    record: w::SpatialMarker {
                        meta: w::SpatialMeta {
                            id,
                            valid_until_s: 2.,
                            ..Default::default()
                        },
                        frame: w::SpatialFrame {
                            kind,
                            ..Default::default()
                        },
                        offset_m: [1., 0., 0.],
                        ..Default::default()
                    },
                    snapshot: None,
                    path_revision: None,
                    published_at: 0.,
                },
            );
            assert_eq!(
                relative(state.marker_at(id, presented).unwrap(), presented.origin),
                Some([expected, 0., 0.])
            );
        }
    }

    #[test]
    fn replacing_a_path_invalidates_markers_bound_to_the_previous_revision() {
        let source = snapshot(0.);
        let mut state = State::default();
        state.paths.insert(
            1,
            Path {
                header: w::SpatialPath {
                    meta: w::SpatialMeta {
                        id: 1,
                        valid_until_s: 10.,
                        ..Default::default()
                    },
                    kind: w::PATH_TIMED,
                    ..Default::default()
                },
                snapshot: Some(source),
                vertices: vec![
                    w::SpatialVertex {
                        time_s: 0.,
                        position_m: [0.; 3],
                    },
                    w::SpatialVertex {
                        time_s: 5.,
                        position_m: [5., 0., 0.],
                    },
                ]
                .into(),
                revision: 1,
                published_at: 0.,
            },
        );
        state.markers.insert(
            2,
            Marker {
                record: w::SpatialMarker {
                    meta: w::SpatialMeta {
                        id: 2,
                        valid_until_s: 10.,
                        ..Default::default()
                    },
                    frame: w::SpatialFrame {
                        kind: w::FRAME_PATH,
                        reference: 1,
                        ..Default::default()
                    },
                    time_mode: w::TIME_FIXED,
                    time_s: 5.,
                    ..Default::default()
                },
                snapshot: None,
                path_revision: Some(1),
                published_at: 0.,
            },
        );
        assert!(state.marker_at(2, source).is_some());
        state.paths.get_mut(&1).unwrap().revision = 2;
        assert!(state.marker_at(2, source).is_none());
        state.markers.get_mut(&2).unwrap().path_revision = Some(2);
        assert!(state.marker_at(2, source).is_some());
        state.remove(1);
        assert!(state.marker_at(2, source).is_none());
    }

    #[test]
    fn track_eviction_preserves_incoming_scan_and_breaks_age_ties_by_id() {
        let mut state = State::default();
        for batch in 0..2 {
            let contacts: Vec<_> = (0..256)
                .map(|index| {
                    let mut value = contact(0.);
                    value.measured.id = batch * 256 + index + 1;
                    value
                })
                .collect();
            state.admit(&contacts, snapshot(0.));
        }
        let mut fresh = contact(0.);
        fresh.measured.id = 999;
        state.admit(&[fresh], snapshot(0.1));
        assert_eq!(state.tracks.len(), 512);
        assert!(state.tracks.contains_key(&999));
        assert!(!state.tracks.contains_key(&1));
        assert!(state.tracks.contains_key(&2));
    }

    #[test]
    fn invalid_coordinates_and_times_cannot_overflow_or_extrapolate_paths() {
        assert!(offset([i128::MAX; 3], [1.; 3]).is_none());
        assert!(offset([0; 3], [f64::INFINITY; 3]).is_none());
        assert!(relative([i128::MAX; 3], [i128::MIN; 3]).is_none());
        let path = Path {
            header: w::SpatialPath {
                kind: w::PATH_TIMED,
                ..Default::default()
            },
            snapshot: Some(snapshot(0.)),
            vertices: vec![
                w::SpatialVertex::default(),
                w::SpatialVertex {
                    time_s: 1.,
                    position_m: [1.; 3],
                },
            ]
            .into(),
            revision: 1,
            published_at: 0.,
        };
        assert!(path.at(-0.01).is_none());
        assert!(path.at(1.01).is_none());
        assert!(path.at(f64::NAN).is_none());
    }
}
