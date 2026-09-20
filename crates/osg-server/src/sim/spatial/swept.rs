use crate::sim::precision::GalacticPosition;
use ahash::{AHashMap, AHashSet};
use bevy::math::DVec3;
use osg_spatial::{Entry, SpatialHash};
use parry3d_f64::math::Vector;
use rayon::prelude::*;
use std::collections::BTreeMap;

pub fn vector(v: DVec3) -> Vector {
    Vector::from_array(v.to_array())
}

pub fn dvec(v: Vector) -> DVec3 {
    DVec3::from_array(v.to_array())
}

#[derive(Clone, Copy, Debug)]
pub struct Proxy {
    pub id: u32,
    pub position: GalacticPosition,
    pub displacement: DVec3,
    pub radius: f64,
}

#[derive(Clone, Default)]
pub struct SweptIndex {
    buckets: BTreeMap<i16, SpatialHash>,
    proxies: AHashMap<u32, Proxy>,
    anchor: GalacticPosition,
    reference: Option<u32>,
}

impl Proxy {
    fn envelope(self) -> Entry {
        let length = self.displacement.length();
        assert!(length.is_finite() && self.radius.is_finite() && self.radius >= 0.0);
        let padding = length * (32.0 * f64::EPSILON) + 1e-6;
        Entry {
            position: self.position.offset_by(self.displacement * 0.5),
            radius_m: self.radius + length * 0.5 + padding,
            luminosity: 0.0,
        }
    }
}

impl SweptIndex {
    pub fn build(proxies: &[Proxy]) -> Self {
        let mut result = Self::default();
        result.refresh(proxies);
        result
    }

    pub fn refresh(&mut self, proxies: &[Proxy]) {
        // Keep coherent orbital motion from moving an entire fleet through the
        // hash's metre-scale leaves every tick. All translations stay integer.
        let reference = self
            .reference
            .and_then(|id| proxies.iter().find(|proxy| proxy.id == id))
            .or_else(|| proxies.first());
        let anchor = reference.and_then(|new| {
            if let Some(old) = self.proxies.get(&new.id) {
                Some(GalacticPosition::new(
                    self.anchor
                        .x
                        .checked_add(new.position.x.checked_sub(old.position.x)?)?,
                    self.anchor
                        .y
                        .checked_add(new.position.y.checked_sub(old.position.y)?)?,
                    self.anchor
                        .z
                        .checked_add(new.position.z.checked_sub(old.position.z)?)?,
                ))
            } else {
                Some(new.position)
            }
        });
        self.anchor = anchor
            .filter(|&anchor| {
                proxies
                    .iter()
                    .all(|proxy| relative_position(proxy.position, anchor).is_some())
            })
            .unwrap_or(GalacticPosition::ZERO);
        self.reference = reference.map(|proxy| proxy.id);

        let retained: AHashSet<_> = proxies.iter().map(|proxy| proxy.id).collect();
        let removed: Vec<_> = self
            .proxies
            .keys()
            .filter(|id| !retained.contains(id))
            .copied()
            .collect();
        for id in removed {
            self.remove(id);
        }
        for &proxy in proxies {
            self.update(proxy);
        }
    }

    pub fn update(&mut self, proxy: Proxy) {
        let mut indexed = proxy;
        indexed.position = relative_position(proxy.position, self.anchor)
            .expect("collision proxy must fit the current integer frame");
        let envelope = indexed.envelope();
        let bucket = radius_bucket(envelope.radius_m);
        if let Some(previous) = self.proxies.get(&proxy.id) {
            let previous_bucket = radius_bucket(previous.envelope().radius_m);
            if previous_bucket != bucket {
                self.remove_from_bucket(previous_bucket, proxy.id);
            }
        }
        self.buckets
            .entry(bucket)
            .or_default()
            .insert(proxy.id, envelope);
        self.proxies.insert(proxy.id, proxy);
    }

    pub fn remove(&mut self, id: u32) {
        if let Some(proxy) = self.proxies.remove(&id) {
            self.remove_from_bucket(radius_bucket(proxy.envelope().radius_m), id);
        }
    }

    fn remove_from_bucket(&mut self, bucket: i16, id: u32) {
        let hash = self.buckets.get_mut(&bucket).unwrap();
        hash.remove(id);
        if hash.is_empty() {
            self.buckets.remove(&bucket);
        }
    }

    pub fn neighbors(&self, proxy: Proxy) -> Vec<u32> {
        let padding = proxy.displacement.length() * (32.0 * f64::EPSILON) + 1e-6;
        let position = relative_position(proxy.position, self.anchor)
            .expect("collision query must fit the current integer frame");
        let mut neighbors: Vec<_> = self
            .buckets
            .values()
            .flat_map(|hash| {
                hash.segment_candidates(position, proxy.displacement, proxy.radius + padding)
                    .ids
            })
            .collect();
        neighbors.retain(|&id| id != proxy.id && sweeps_overlap(proxy, self.proxies[&id]));
        neighbors.sort_unstable();
        neighbors
    }

    pub fn pairs(&self) -> Vec<(u32, u32)> {
        let mut pairs: Vec<_> = self
            .proxies
            .par_iter()
            .flat_map_iter(|(&id, &proxy)| {
                let bucket = radius_bucket(proxy.envelope().radius_m);
                let position = relative_position(proxy.position, self.anchor).unwrap();
                let padding = proxy.displacement.length() * (32.0 * f64::EPSILON) + 1e-6;
                // A fast projectile queries the small bodies along its capsule.
                // Small bodies never query its enormous enclosing sphere.
                self.buckets
                    .range(..=bucket)
                    .flat_map(move |(&other_bucket, hash)| {
                        hash.segment_candidates(
                            position,
                            proxy.displacement,
                            proxy.radius + padding,
                        )
                        .ids
                        .into_iter()
                        .filter(move |&other| {
                            (other_bucket != bucket || other > id)
                                && sweeps_overlap(proxy, self.proxies[&other])
                        })
                        .map(move |other| (id.min(other), id.max(other)))
                    })
            })
            .collect();
        pairs.sort_unstable();
        pairs
    }
}

fn radius_bucket(radius: f64) -> i16 {
    ((radius.to_bits() >> 52) & 0x7ff) as i16 - 1023
}

fn relative_position(
    position: GalacticPosition,
    anchor: GalacticPosition,
) -> Option<GalacticPosition> {
    Some(GalacticPosition::new(
        position.x.checked_sub(anchor.x)?,
        position.y.checked_sub(anchor.y)?,
        position.z.checked_sub(anchor.z)?,
    ))
}

// Bodies can have different event times after an impact. Compare geometric
// capsules here; the solver narrows candidates to their overlapping time span.
fn sweeps_overlap(a: Proxy, b: Proxy) -> bool {
    let start = a.position.relative_to(b.position);
    let aa = a.displacement.length_squared();
    let bb = a.displacement.dot(b.displacement);
    let cc = b.displacement.length_squared();
    let lower_a = a.displacement.min(DVec3::ZERO) + start;
    let upper_a = a.displacement.max(DVec3::ZERO) + start;
    let lower_b = b.displacement.min(DVec3::ZERO);
    let upper_b = b.displacement.max(DVec3::ZERO);
    let padding = (start.abs().max_element() + a.displacement.length() + b.displacement.length())
        * 32.0
        * f64::EPSILON
        + 1e-6;
    let separation = DVec3::splat(a.radius + b.radius + padding);
    if (lower_a - separation).cmpgt(upper_b).any() || (lower_b - separation).cmpgt(upper_a).any() {
        return false;
    }
    // Closest-segment parameters become ill-conditioned for parallel motion.
    // Keep the AABB candidate in that case and let CCD make the final decision.
    if aa > 1e-30 && cc > 1e-30 && aa * cc - bb * bb <= aa * cc * 1e-12 {
        return true;
    }
    let dd = a.displacement.dot(start);
    let ee = b.displacement.dot(start);
    let (s, t) = if aa <= 1e-30 && cc <= 1e-30 {
        (0.0, 0.0)
    } else if aa <= 1e-30 {
        (0.0, (ee / cc).clamp(0.0, 1.0))
    } else if cc <= 1e-30 {
        ((-dd / aa).clamp(0.0, 1.0), 0.0)
    } else {
        let denominator = aa * cc - bb * bb;
        let mut s = if denominator > aa * cc * 1e-14 {
            ((bb * ee - cc * dd) / denominator).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut t = (bb * s + ee) / cc;
        if t < 0.0 {
            t = 0.0;
            s = (-dd / aa).clamp(0.0, 1.0);
        } else if t > 1.0 {
            t = 1.0;
            s = ((bb - dd) / aa).clamp(0.0, 1.0);
        }
        (s, t)
    };
    (start + a.displacement * s - b.displacement * t).length_squared()
        <= (a.radius + b.radius + padding).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_with_different_start_times_do_not_lose_intersections() {
        let mut index = SweptIndex::default();
        index.update(Proxy {
            id: 0,
            position: GalacticPosition::from_meters(DVec3::X * 5.0),
            displacement: DVec3::X * 5.0,
            radius: 0.01,
        });
        index.update(Proxy {
            id: 1,
            position: GalacticPosition::from_meters(DVec3::new(5.0, -5.0, 0.0)),
            displacement: DVec3::Y * 10.0,
            radius: 0.01,
        });
        assert_eq!(index.pairs(), vec![(0, 1)]);
    }

    #[test]
    fn near_parallel_long_segments_remain_conservative() {
        let origin = GalacticPosition::splat(1_i128 << 105);
        let a = Proxy {
            id: 0,
            position: origin,
            displacement: DVec3::X * 1e12,
            radius: 1.0,
        };
        let b = Proxy {
            id: 1,
            position: origin.offset_by(DVec3::Y * -5000.0),
            displacement: DVec3::new(1e12, 1e4, 0.0),
            radius: 1.0,
        };
        assert!(sweeps_overlap(a, b));
        assert_eq!(SweptIndex::build(&[a, b]).pairs(), vec![(0, 1)]);
    }

    #[test]
    fn independently_timed_crossings_survive_scale_and_origin_changes() {
        for origin in [
            GalacticPosition::ZERO,
            GalacticPosition::splat(1_i128 << 105),
        ] {
            for scale in [0.01, 1000.0, 1e9] {
                for i in 1..200 {
                    let a = Proxy {
                        id: 0,
                        position: origin,
                        displacement: DVec3::new(
                            (i * 71 % 137) as f64 - 68.0,
                            (i * 89 % 139) as f64 - 69.0,
                            (i * 97 % 149) as f64 - 74.0,
                        ) * scale,
                        radius: 0.001 * scale,
                    };
                    let displacement = DVec3::new(
                        (i * 101 % 151) as f64 - 75.0,
                        (i * 103 % 157) as f64 - 78.0,
                        (i * 107 % 163) as f64 - 81.0,
                    ) * scale;
                    let s = (i * 17 % 101) as f64 / 100.0;
                    let t = (i * 43 % 101) as f64 / 100.0;
                    let b = Proxy {
                        id: 1,
                        position: origin.offset_by(a.displacement * s - displacement * t),
                        displacement,
                        radius: a.radius,
                    };
                    assert!(sweeps_overlap(a, b), "scale={scale} crossing={i}");
                    assert_eq!(SweptIndex::build(&[a, b]).pairs(), vec![(0, 1)]);
                }
            }
        }
    }

    #[test]
    fn collision_refit_discovers_new_path_and_removes_old_path() {
        let anchor = GalacticPosition::splat(1_i128 << 100);
        let moving = Proxy {
            id: 100,
            position: anchor,
            displacement: DVec3::X * 100.0,
            radius: 1.0,
        };
        let old_path = Proxy {
            id: 200,
            position: anchor.offset_by(DVec3::X * 90.0),
            displacement: DVec3::ZERO,
            radius: 1.0,
        };
        let new_path = Proxy {
            id: 300,
            position: anchor.offset_by(DVec3::new(50.0, 40.0, 0.0)),
            ..old_path
        };
        let mut index = SweptIndex::build(&[moving, old_path, new_path]);
        assert_eq!(index.pairs(), vec![(100, 200)]);

        let redirected = Proxy {
            position: anchor.offset_by(DVec3::X * 50.0),
            displacement: DVec3::Y * 50.0,
            ..moving
        };
        index.update(redirected);
        assert_eq!(index.neighbors(redirected), vec![300]);
        assert_eq!(index.pairs(), vec![(100, 300)]);
        index.remove(300);
        assert!(index.pairs().is_empty());
    }

    #[test]
    fn capsule_filter_rejects_crossing_boxes_whose_segments_miss() {
        let a = Proxy {
            id: 0,
            position: GalacticPosition::ZERO,
            displacement: DVec3::new(100.0, 100.0, 0.0),
            radius: 0.1,
        };
        let b = Proxy {
            id: 1,
            position: GalacticPosition::from_meters(DVec3::Y * 50.0),
            displacement: DVec3::new(100.0, 100.0, 1.0),
            radius: 0.1,
        };
        assert!(!sweeps_overlap(a, b));
        assert!(SweptIndex::build(&[a, b]).pairs().is_empty());
    }

    #[test]
    fn moving_frame_and_reference_removal_preserve_collision_candidates() {
        let origin = GalacticPosition::splat(1_i128 << 100);
        let mut proxies = vec![
            Proxy {
                id: 10,
                position: origin,
                displacement: DVec3::X * 10.0,
                radius: 1.0,
            },
            Proxy {
                id: 20,
                position: origin.offset_by(DVec3::X * 8.0),
                displacement: DVec3::ZERO,
                radius: 1.0,
            },
            Proxy {
                id: 30,
                position: origin.offset_by(DVec3::X * 200.0),
                displacement: DVec3::ZERO,
                radius: 1.0,
            },
        ];
        let mut index = SweptIndex::default();
        for tick in 0..20 {
            for proxy in &mut proxies {
                proxy.position = proxy.position.offset_by(DVec3::new(1e6, 700.0, -500.0));
            }
            index.refresh(&proxies);
            assert_eq!(index.pairs(), vec![(10, 20)], "tick={tick}");
            assert_eq!(index.neighbors(proxies[0]), vec![20]);
        }
        proxies.remove(0);
        index.refresh(&proxies);
        assert!(index.pairs().is_empty());
        let projectile = Proxy {
            id: 40,
            position: proxies[0].position.offset_by(DVec3::X * -1000.0),
            displacement: DVec3::X * 1500.0,
            radius: 0.001,
        };
        index.update(projectile);
        assert_eq!(index.neighbors(projectile), vec![20, 30]);
        assert_eq!(index.pairs(), vec![(20, 40), (30, 40)]);
    }
}
