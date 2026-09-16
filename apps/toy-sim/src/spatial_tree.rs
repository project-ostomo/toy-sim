//! Library BVHs in integer-addressed regions. No absolute galactic floats enter
//! the narrow phase. Sweeps occupy their capsule's cells, not its enclosing box.
use crate::precision::GalacticPosition;
use ahash::AHashMap;
use bevy::math::DVec3;
use parry3d_f64::{
    bounding_volume::Aabb,
    math::Vector,
    partitioning::{Bvh, BvhBuildStrategy, BvhWorkspace},
};
use rayon::prelude::*;

pub const REGION_M: f64 = 100_000.0;
const REGION_UM: i128 = 100_000_000_000;
pub type Key = [i128; 3];

pub fn vector(v: DVec3) -> Vector {
    Vector::from_array(v.to_array())
}
pub fn dvec(v: Vector) -> DVec3 {
    DVec3::from_array(v.to_array())
}
pub fn key(p: GalacticPosition) -> Key {
    [
        p.x.div_euclid(REGION_UM),
        p.y.div_euclid(REGION_UM),
        p.z.div_euclid(REGION_UM),
    ]
}
pub fn origin(k: Key) -> GalacticPosition {
    GalacticPosition::new(k[0] * REGION_UM, k[1] * REGION_UM, k[2] * REGION_UM)
}

/// Expand for conversion roundoff at the scale of this particular index.
pub fn bounds(lo: DVec3, hi: DVec3) -> Aabb {
    let pad = lo.abs().max(hi.abs()) * (8.0 * f64::EPSILON) + DVec3::splat(1e-6);
    Aabb::new(vector(lo - pad), vector(hi + pad))
}

#[derive(Clone, Copy, Debug)]
pub struct Proxy {
    pub id: u32,
    pub position: GalacticPosition,
    pub displacement: DVec3,
    pub radius: f64,
}

#[derive(Clone, Default)]
pub struct Region {
    pub tree: Bvh,
    workspace: BvhWorkspace,
    ids: Vec<u32>,
    slots: AHashMap<u32, u32>,
    free: Vec<u32>,
}

impl Region {
    fn insert(&mut self, id: u32, aabb: Aabb) {
        let slot = if let Some(&slot) = self.slots.get(&id) {
            slot
        } else {
            let slot = self.free.pop().unwrap_or_else(|| {
                self.ids.push(u32::MAX);
                (self.ids.len() - 1) as u32
            });
            self.ids[slot as usize] = id;
            self.slots.insert(id, slot);
            slot
        };
        self.tree.insert(aabb, slot);
    }

    fn remove(&mut self, id: u32) {
        if let Some(slot) = self.slots.remove(&id) {
            self.tree.remove(slot);
            self.ids[slot as usize] = u32::MAX;
            self.free.push(slot);
        }
    }
}

#[derive(Clone, Default)]
pub struct RegionIndex {
    pub regions: AHashMap<Key, Region>,
    memberships: AHashMap<u32, Vec<Key>>,
    directory: Bvh,
    directory_keys: Vec<Key>,
    pub anchor: GalacticPosition,
}

fn memberships(p: Proxy) -> Vec<(Key, Aabb)> {
    let start_key = key(p.position);
    let start = p.position.relative_to(origin(start_key));
    let mut cell = [0_i128; 3];
    let mut t = 0.0;
    let mut result: AHashMap<Key, Aabb> = AHashMap::default();
    loop {
        let mut next: f64 = 1.0;
        let mut crossings = [f64::INFINITY; 3];
        for axis in 0..3 {
            let v = p.displacement[axis];
            if v != 0.0 {
                let edge = (cell[axis] + i128::from(v > 0.0)) as f64 * REGION_M;
                crossings[axis] = (edge - start[axis]) / v;
                next = next.min(crossings[axis].max(t));
            }
        }
        let a = start + p.displacement * t;
        let b = start + p.displacement * next;
        let low = ((a.min(b) - DVec3::splat(p.radius)) / REGION_M).floor();
        let high = ((a.max(b) + DVec3::splat(p.radius)) / REGION_M).floor();
        for x in low.x as i128..=high.x as i128 {
            for y in low.y as i128..=high.y as i128 {
                for z in low.z as i128..=high.z as i128 {
                    let relative = [x, y, z];
                    let shift =
                        DVec3::new(relative[0] as f64, relative[1] as f64, relative[2] as f64)
                            * REGION_M;
                    let lo = (a.min(b) - DVec3::splat(p.radius) - shift).max(DVec3::ZERO);
                    let hi =
                        (a.max(b) + DVec3::splat(p.radius) - shift).min(DVec3::splat(REGION_M));
                    if lo.cmple(hi).all() {
                        let k = [
                            start_key[0] + relative[0],
                            start_key[1] + relative[1],
                            start_key[2] + relative[2],
                        ];
                        let new = bounds(lo, hi);
                        result
                            .entry(k)
                            .and_modify(|old| {
                                old.mins = old.mins.min(new.mins);
                                old.maxs = old.maxs.max(new.maxs);
                            })
                            .or_insert(new);
                    }
                }
            }
        }
        if next >= 1.0 {
            break;
        }
        for axis in 0..3 {
            if crossings[axis] <= next {
                cell[axis] += if p.displacement[axis] > 0.0 { 1 } else { -1 };
            }
        }
        t = next;
    }
    result.into_iter().collect()
}

impl RegionIndex {
    /// Keep regional trees and their builder storage across ticks. Dense local
    /// leaf IDs remain independent of the caller's current body ordering.
    pub fn refresh(&mut self, proxies: &[Proxy]) {
        if self.regions.is_empty() {
            *self = Self::build(proxies);
            return;
        }

        let entries: Vec<_> = proxies
            .par_iter()
            .map(|&p| (p.id, memberships(p)))
            .collect();
        let mut grouped: AHashMap<Key, Vec<(u32, Aabb)>> = AHashMap::default();
        self.memberships.clear();
        for (id, entries) in entries {
            for &(key, aabb) in &entries {
                grouped.entry(key).or_default().push((id, aabb));
            }
            self.memberships
                .insert(id, entries.into_iter().map(|e| e.0).collect());
        }

        let work: Vec<_> = grouped
            .into_iter()
            .map(|(key, entries)| (key, entries, self.regions.remove(&key).unwrap_or_default()))
            .collect();
        self.regions = work
            .into_par_iter()
            .map(|(key, entries, mut region)| {
                let present: ahash::AHashSet<_> = entries.iter().map(|e| e.0).collect();
                let removed: Vec<_> = region
                    .slots
                    .keys()
                    .copied()
                    .filter(|id| !present.contains(id))
                    .collect();
                for id in removed {
                    region.remove(id);
                }
                for (id, aabb) in entries {
                    region.insert(id, aabb);
                }
                region
                    .tree
                    .rebuild(&mut region.workspace, BvhBuildStrategy::Binned);
                (key, region)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        self.rebuild_directory();
    }

    pub fn build(proxies: &[Proxy]) -> Self {
        let entries: Vec<_> = proxies
            .par_iter()
            .map(|&p| (p.id, memberships(p)))
            .collect();
        let mut index = Self::default();
        let mut grouped: AHashMap<Key, Vec<(u32, Aabb)>> = AHashMap::default();
        for (id, entries) in entries {
            for &(k, aabb) in &entries {
                grouped.entry(k).or_default().push((id, aabb));
            }
            index
                .memberships
                .insert(id, entries.into_iter().map(|e| e.0).collect());
        }
        index.regions = grouped
            .into_iter()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(k, entries)| {
                let ids: Vec<_> = entries.iter().map(|e| e.0).collect();
                let slots = ids
                    .iter()
                    .enumerate()
                    .map(|(slot, &id)| (id, slot as u32))
                    .collect();
                let boxes: Vec<_> = entries.iter().map(|e| e.1).collect();
                (
                    k,
                    Region {
                        tree: Bvh::from_leaves(BvhBuildStrategy::Binned, &boxes),
                        ids,
                        slots,
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        index.rebuild_directory();
        index
    }

    fn rebuild_directory(&mut self) {
        self.directory_keys = self
            .regions
            .iter()
            .filter(|(_, r)| r.tree.leaf_count() > 0)
            .map(|(&k, _)| k)
            .collect();
        self.directory_keys.sort_unstable();
        self.anchor = self
            .directory_keys
            .first()
            .copied()
            .map(origin)
            .unwrap_or(GalacticPosition::ZERO);
        let leaves: Vec<_> = self
            .directory_keys
            .iter()
            .map(|&k| {
                let lo = origin(k).relative_to(self.anchor);
                bounds(lo, lo + DVec3::splat(REGION_M))
            })
            .collect();
        self.directory = Bvh::from_leaves(BvhBuildStrategy::Binned, &leaves);
    }

    pub fn update(&mut self, proxy: Proxy) {
        if let Some(old) = self.memberships.remove(&proxy.id) {
            for k in old {
                self.regions.get_mut(&k).unwrap().remove(proxy.id);
            }
        }
        let entries = memberships(proxy);
        let mut new_region = false;
        for &(k, aabb) in &entries {
            new_region |= !self.regions.contains_key(&k) || self.regions[&k].tree.leaf_count() == 0;
            self.regions.entry(k).or_default().insert(proxy.id, aabb);
        }
        self.memberships
            .insert(proxy.id, entries.into_iter().map(|e| e.0).collect());
        if new_region {
            self.rebuild_directory();
        }
    }

    pub fn remove(&mut self, id: u32) {
        if let Some(old) = self.memberships.remove(&id) {
            for k in old {
                self.regions.get_mut(&k).unwrap().remove(id);
            }
        }
    }

    pub fn pairs(&self) -> Vec<(u32, u32)> {
        let mut result: Vec<_> = self
            .regions
            .par_iter()
            .flat_map_iter(|(_, r)| {
                r.tree
                    .leaf_pairs(&r.tree, |a, b| {
                        let a = a.aabb();
                        let b = b.aabb();
                        a.mins.cmple(b.maxs).all() && b.mins.cmple(a.maxs).all()
                    })
                    .filter(|&(a, b)| a < b)
                    .map(|(a, b)| {
                        let a = r.ids[a as usize];
                        let b = r.ids[b as usize];
                        (a.min(b), a.max(b))
                    })
            })
            .collect();
        result.sort_unstable();
        result.dedup();
        result
    }

    pub fn neighbors(&self, proxy: Proxy) -> Vec<u32> {
        let mut result = Vec::new();
        for (k, aabb) in memberships(proxy) {
            if let Some(r) = self.regions.get(&k) {
                result.extend(r.tree.intersect_aabb(&aabb).map(|id| r.ids[id as usize]));
            }
        }
        result.sort_unstable();
        result.dedup();
        result.retain(|&id| id != proxy.id);
        result
    }

    pub fn range(&self, position: GalacticPosition, radius: f64) -> Vec<u32> {
        let p = position.relative_to(self.anchor);
        let query = bounds(p - DVec3::splat(radius), p + DVec3::splat(radius));
        let mut result = Vec::new();
        for region_id in self.directory.intersect_aabb(&query) {
            let k = self.directory_keys[region_id as usize];
            let p = position.relative_to(origin(k));
            let region = &self.regions[&k];
            result.extend(
                region
                    .tree
                    .intersect_aabb(&bounds(p - DVec3::splat(radius), p + DVec3::splat(radius)))
                    .map(|id| region.ids[id as usize]),
            );
        }
        result.sort_unstable();
        result.dedup();
        result
    }

    /// Near-first tree traversal with a live k-th-distance bound. Leaf distances
    /// are supplied by the caller so sensing remains a center-distance query.
    pub fn nearest(
        &self,
        position: GalacticPosition,
        range: f64,
        n: usize,
        distance: impl Fn(u32) -> Option<(f64, u64)>,
    ) -> Vec<u32> {
        use ordered_float::OrderedFloat;
        use std::{cell::RefCell, collections::BinaryHeap};
        let best = RefCell::new(BinaryHeap::<(OrderedFloat<f64>, u64, u32)>::new());
        let limit = || {
            let heap = best.borrow();
            if heap.len() == n {
                heap.peek().unwrap().0.0.min(range * range)
            } else {
                range * range
            }
        };
        let cost = |box_: Aabb, p: DVec3| {
            let pad = DVec3::splat(1e-5);
            let lo = dvec(box_.mins) - pad;
            let hi = dvec(box_.maxs) + pad;
            (p - p.clamp(lo, hi)).length_squared()
        };
        self.directory.find_best::<f64>(
            f64::INFINITY,
            |node, _| {
                let d = cost(node.aabb(), position.relative_to(self.anchor));
                if d <= limit() { d } else { f64::INFINITY }
            },
            |region_id, _| {
                let k = self.directory_keys[region_id as usize];
                let p = position.relative_to(origin(k));
                self.regions[&k].tree.find_best::<f64>(
                    f64::INFINITY,
                    |node, _| {
                        let d = cost(node.aabb(), p);
                        if d <= limit() { d } else { f64::INFINITY }
                    },
                    |slot, _| {
                        let id = self.regions[&k].ids[slot as usize];
                        if let Some((d, tie)) = distance(id) {
                            if d <= limit() {
                                let mut heap = best.borrow_mut();
                                if !heap.iter().any(|entry| entry.2 == id) {
                                    heap.push((OrderedFloat(d), tie, id));
                                    if heap.len() > n {
                                        heap.pop();
                                    }
                                }
                            }
                        }
                        None
                    },
                );
                None
            },
        );
        best.into_inner()
            .into_sorted_vec()
            .into_iter()
            .map(|v| v.2)
            .collect()
    }
}
