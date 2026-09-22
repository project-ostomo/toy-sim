use crate::precision::GalacticPosition;
use glam::DVec3;
use osg_spatial_bvh::{
    Aabb, BvhEntry, OPTICAL_LUMENS_PER_WATT, RecordKind, SpatialRecord, SpatialService,
};
use std::{f64::consts::PI, sync::Arc};

#[derive(Clone, Debug)]
pub struct Entry {
    pub position: GalacticPosition,
    pub luminosity: f64,
    pub influence: f64,
    pub radius: f64,
}

pub struct CatalogueIndex {
    pub entries: Vec<Entry>,
    pub spatial: Arc<SpatialService<SpatialRecord>>,
}

impl CatalogueIndex {
    pub fn new(entries: Vec<Entry>) -> Self {
        let spatial = Arc::new(SpatialService::new(entries.iter().enumerate().map(
            |(slot, entry)| BvhEntry {
                object: SpatialRecord {
                    kind: RecordKind::Source,
                    id: slot as u64,
                    position: entry.position.to_array(),
                    radius_m: entry.influence.max(entry.radius),
                },
                bounds: Aabb::sphere(entry.position.to_array(), entry.influence.max(entry.radius)),
                luminosity: entry.luminosity / OPTICAL_LUMENS_PER_WATT,
            },
        )));
        Self { entries, spatial }
    }

    pub fn brightest(&self, origin: GalacticPosition) -> Option<usize> {
        self.spatial
            .brightest(origin.to_array(), |r| {
                let luminosity = self.entries[r.id as usize].luminosity;
                (r.kind == RecordKind::Source && luminosity > 0.0).then(|| {
                    luminosity
                        / (4.0
                            * PI
                            * OPTICAL_LUMENS_PER_WATT
                            * r.distance(origin.to_array()).powi(2).max(1.0))
                })
            })
            .map(|r| r.id as usize)
    }

    pub fn visible(&self, origin: GalacticPosition, min_brightness: f64) -> Vec<usize> {
        let mut found: Vec<_> = self
            .spatial
            .visibility_candidates(
                origin.to_array(),
                0.0,
                min_brightness.max(0.0) / (4.0 * PI * OPTICAL_LUMENS_PER_WATT),
            )
            .into_iter()
            .filter(|r| r.kind == RecordKind::Source)
            .map(|r| r.id as usize)
            .filter(|&id| {
                let entry = &self.entries[id];
                entry.luminosity
                    >= min_brightness * entry.position.relative_to(origin).length_squared()
            })
            .collect();
        found.sort_unstable();
        found
    }

    pub fn containing_segment(&self, start: GalacticPosition, displacement: DVec3) -> Vec<usize> {
        let mut found: Vec<_> = self
            .spatial
            .segment_candidates(
                start.to_array(),
                start.offset_by(displacement).to_array(),
                0.0,
            )
            .into_iter()
            .filter(|r| r.kind == RecordKind::Source)
            .filter(|r| {
                let p = self.entries[r.id as usize].position.relative_to(start);
                let t = if displacement.length_squared() > 0.0 {
                    (p.dot(displacement) / displacement.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (p - t * displacement).length_squared()
                    <= r.radius_m.powi(2) * (1.0 + 64.0 * f64::EPSILON)
            })
            .map(|r| r.id as usize)
            .collect();
        found.sort_unstable();
        found
    }

    pub fn intersecting_sphere(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        self.spatial
            .sphere_candidates(centre.to_array(), radius)
            .into_iter()
            .filter(|r| {
                r.kind == RecordKind::Source
                    && r.distance(centre.to_array())
                        <= radius + self.entries[r.id as usize].influence
            })
            .map(|r| r.id as usize)
            .collect()
    }

    pub fn nearest(&self, origin: GalacticPosition) -> Option<usize> {
        self.nearest_many(origin, 1).first().copied()
    }

    pub fn illumination_sources(
        &self,
        origin: GalacticPosition,
        radius: f64,
        threshold: f64,
    ) -> Vec<usize> {
        self.spatial
            .visibility_candidates(
                origin.to_array(),
                radius,
                threshold / (4.0 * PI * OPTICAL_LUMENS_PER_WATT),
            )
            .into_iter()
            .filter(|r| r.kind == RecordKind::Source)
            .filter(|r| {
                let distance = (r.distance(origin.to_array()) - radius - r.radius_m).max(0.0);
                distance == 0.0
                    || self.entries[r.id as usize].luminosity >= threshold * distance.powi(2)
            })
            .map(|r| r.id as usize)
            .collect()
    }

    pub fn nearest_many(&self, origin: GalacticPosition, count: usize) -> Vec<usize> {
        self.nearest_filtered(origin, count, |_| true)
    }

    pub fn nearest_filtered(
        &self,
        origin: GalacticPosition,
        count: usize,
        accept: impl Fn(usize) -> bool,
    ) -> Vec<usize> {
        self.spatial
            .nearest(origin.to_array(), f64::INFINITY, count, |r| {
                (r.kind == RecordKind::Source && accept(r.id as usize))
                    .then(|| r.distance(origin.to_array()))
            })
            .into_iter()
            .map(|r| r.id as usize)
            .collect()
    }

    pub fn nearest_distance(&self, origin: GalacticPosition) -> f64 {
        self.nearest(origin).map_or(f64::INFINITY, |id| {
            self.entries[id].position.relative_to(origin).length()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogue_queries_match_brute_force() {
        let origin = GalacticPosition::new(1 << 90, -(1 << 90), 0);
        let entries = (0..100_000)
            .map(|i| Entry {
                position: origin.offset_by(DVec3::new(
                    (i * 7919 % 10001) as f64 - 5000.0,
                    (i * 3571 % 10001) as f64 - 5000.0,
                    (i * 101 % 10001) as f64 - 5000.0,
                )),
                luminosity: 10f64.powi(i % 8),
                influence: 100.0 + (i % 1000) as f64,
                radius: 1.0 + (i % 100) as f64,
            })
            .collect();
        let tree = CatalogueIndex::new(entries);
        for brightness in [0.0001, 0.1, 100.0] {
            let start = std::time::Instant::now();
            let mut got = tree.visible(origin, brightness);
            eprintln!(
                "100k catalogue brightness {brightness}: {} matches in {:?}",
                got.len(),
                start.elapsed()
            );
            got.sort_unstable();
            let expected: Vec<_> = tree
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.luminosity >= brightness * e.position.relative_to(origin).length_squared()
                })
                .map(|(i, _)| i)
                .collect();
            assert_eq!(got, expected);
        }
        let delta = DVec3::new(10000.0, 0.0, 0.0);
        let mut got = tree.containing_segment(origin, delta);
        got.sort_unstable();
        let expected: Vec<_> = tree
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let p = e.position.relative_to(origin);
                let t = (p.dot(delta) / delta.length_squared()).clamp(0.0, 1.0);
                (p - t * delta).length_squared() <= e.influence.powi(2)
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(got, expected);
        let brightest = tree.brightest(origin).unwrap();
        let flux = |id: usize| {
            tree.entries[id].luminosity
                / tree.entries[id]
                    .position
                    .relative_to(origin)
                    .length_squared()
                    .max(1.0)
        };
        assert!(
            tree.entries
                .iter()
                .enumerate()
                .all(|(i, _)| flux(i) <= flux(brightest))
        );
        let near = tree.nearest_distance(origin);
        let brute = tree
            .entries
            .iter()
            .enumerate()
            .map(|(_, e)| e.position.relative_to(origin).length())
            .fold(f64::INFINITY, f64::min);
        assert_eq!(near, brute);
    }

    #[test]
    fn zero_luminosity_and_stationary_boundary_queries_remain_complete() {
        let origin = GalacticPosition::new(1 << 90, -(1 << 90), 0);
        let index = CatalogueIndex::new(vec![
            Entry {
                position: origin,
                luminosity: 0.0,
                influence: 1.0,
                radius: 0.1,
            },
            Entry {
                position: origin.offset_by(DVec3::X * 2.0),
                luminosity: 2.0,
                influence: 1.0,
                radius: 0.1,
            },
        ]);
        assert_eq!(index.visible(origin, 0.0), vec![0, 1]);
        assert_eq!(index.brightest(origin), Some(1));
        let mut touching = index.containing_segment(origin.offset_by(DVec3::X), DVec3::ZERO);
        touching.sort_unstable();
        assert_eq!(touching, vec![0, 1]);
        assert_eq!(
            index.nearest_distance(origin.offset_by(DVec3::X * 0.25)),
            0.25
        );
        let empty = CatalogueIndex::new(Vec::new());
        assert!(empty.brightest(origin).is_none());
        assert!(empty.visible(origin, 0.0).is_empty());
        assert!(empty.nearest_distance(origin).is_infinite());
    }
}
