//! Immutable median-split tree. All bounds stay in integer galactic coordinates.
use crate::precision::GalacticPosition;
use glam::DVec3;

#[derive(Clone, Debug)]
pub struct Entry {
    pub position: GalacticPosition,
    pub luminosity: f64,
    pub influence: f64,
    pub radius: f64,
}
#[derive(Debug)]
struct Node {
    lo: GalacticPosition,
    hi: GalacticPosition,
    luminosity: f64,
    influence: f64,
    children: Option<(usize, usize)>,
    entries: Vec<usize>,
}
#[derive(Debug)]
pub struct CatalogueTree {
    pub entries: Vec<Entry>,
    nodes: Vec<Node>,
}
fn coordinates(p: GalacticPosition) -> [i128; 3] {
    [p.x, p.y, p.z]
}
impl CatalogueTree {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut tree = Self {
            entries,
            nodes: Vec::new(),
        };
        if !tree.entries.is_empty() {
            tree.build((0..tree.entries.len()).collect());
        }
        tree
    }
    fn build(&mut self, mut ids: Vec<usize>) -> usize {
        let mut lo = [i128::MAX; 3];
        let mut hi = [i128::MIN; 3];
        let mut luminosity: f64 = 0.0;
        let mut influence: f64 = 0.0;
        for &id in &ids {
            let e = &self.entries[id];
            let p = coordinates(e.position);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            luminosity = luminosity.max(e.luminosity);
            influence = influence.max(e.influence);
        }
        let node = self.nodes.len();
        self.nodes.push(Node {
            lo: GalacticPosition::new(lo[0], lo[1], lo[2]),
            hi: GalacticPosition::new(hi[0], hi[1], hi[2]),
            luminosity,
            influence,
            children: None,
            entries: vec![],
        });
        if ids.len() <= 8 {
            self.nodes[node].entries = ids;
        } else {
            let axis = (0..3).max_by_key(|&k| hi[k].saturating_sub(lo[k])).unwrap();
            let mid = ids.len() / 2;
            ids.select_nth_unstable_by_key(mid, |&id| coordinates(self.entries[id].position)[axis]);
            let right = ids.split_off(mid);
            let a = self.build(ids);
            let b = self.build(right);
            self.nodes[node].children = Some((a, b));
        }
        node
    }
    fn walk(&self, reject: impl Fn(&Node) -> bool, accept: impl Fn(&Entry) -> bool) -> Vec<usize> {
        let mut result = Vec::new();
        if self.nodes.is_empty() {
            return result;
        }
        let mut stack = vec![0];
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id];
            if reject(n) {
                continue;
            }
            if let Some((a, b)) = n.children {
                stack.extend([a, b]);
            } else {
                result.extend(
                    n.entries
                        .iter()
                        .copied()
                        .filter(|&id| accept(&self.entries[id])),
                );
            }
        }
        result
    }
    pub fn brightest(&self, origin: GalacticPosition) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = 0.0;
        let mut result = None;
        let mut stack = vec![0];
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id];
            if n.luminosity < best * box_distance_squared(n, origin) {
                continue;
            }
            if let Some((a, b)) = n.children {
                let upper = |i: usize| {
                    self.nodes[i].luminosity / box_distance_squared(&self.nodes[i], origin).max(1.0)
                };
                if upper(a) > upper(b) {
                    stack.extend([b, a]);
                } else {
                    stack.extend([a, b]);
                }
            } else {
                for &i in &n.entries {
                    let e = &self.entries[i];
                    let flux =
                        e.luminosity / e.position.relative_to(origin).length_squared().max(1.0);
                    if flux > best {
                        best = flux;
                        result = Some(i);
                    }
                }
            }
        }
        result
    }
    pub fn visible(&self, origin: GalacticPosition, min_brightness: f64) -> Vec<usize> {
        self.walk(
            |n| n.luminosity < min_brightness * box_distance_squared(n, origin),
            |e| e.luminosity >= min_brightness * e.position.relative_to(origin).length_squared(),
        )
    }
    pub fn containing_segment(&self, start: GalacticPosition, displacement: DVec3) -> Vec<usize> {
        // A sphere around the segment is a conservative broad phase; leaves test exactly.
        let middle = start.offset_by(displacement * 0.5);
        let padding = displacement.length() * 0.5;
        self.walk(
            |n| box_distance_squared(n, middle) > (n.influence + padding).powi(2),
            |e| {
                let p = e.position.relative_to(start);
                let t = if displacement.length_squared() > 0.0 {
                    (p.dot(displacement) / displacement.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (p - displacement * t).length_squared() <= e.influence.powi(2)
            },
        )
    }
    /// Includes faint stars, so moving towards a previously invisible star invalidates a bake.
    pub fn nearest_distance(&self, origin: GalacticPosition) -> f64 {
        if self.nodes.is_empty() {
            return f64::INFINITY;
        }
        let mut best = f64::INFINITY;
        let mut stack = vec![0];
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id];
            if box_distance_squared(n, origin) >= best {
                continue;
            }
            if let Some((a, b)) = n.children {
                if box_distance_squared(&self.nodes[a], origin)
                    < box_distance_squared(&self.nodes[b], origin)
                {
                    stack.extend([b, a]);
                } else {
                    stack.extend([a, b]);
                }
            } else {
                for &i in &n.entries {
                    best = best.min(
                        self.entries[i]
                            .position
                            .relative_to(origin)
                            .length_squared(),
                    );
                }
            }
        }
        best.sqrt()
    }
}
fn box_distance_squared(n: &Node, p: GalacticPosition) -> f64 {
    let p0 = coordinates(p);
    let lo = coordinates(n.lo);
    let hi = coordinates(n.hi);
    let closest = GalacticPosition::new(
        p0[0].clamp(lo[0], hi[0]),
        p0[1].clamp(lo[1], hi[1]),
        p0[2].clamp(lo[2], hi[2]),
    );
    closest.relative_to(p).length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree_queries_match_brute_force() {
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
        let tree = CatalogueTree::new(entries);
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
}
