use super::*;
use glam::DVec3;

const MAX_SAMPLES: usize = 8192;
const MAX_VISITS: usize = 65536;

#[derive(Default)]
pub(super) struct Tree {
    nodes: Vec<Node>,
    indices: Vec<usize>,
}

struct Node {
    lo: DVec3,
    hi: DVec3,
    range: std::ops::Range<usize>,
    children: Option<[usize; 2]>,
}

impl Tree {
    pub fn new(positions: &[DVec3]) -> Self {
        Self::from_indices(positions, (0..positions.len()).collect())
    }

    pub fn from_indices(positions: &[DVec3], indices: Vec<usize>) -> Self {
        let count = indices.len();
        let mut tree = Self {
            indices,
            ..Default::default()
        };
        if count != 0 {
            tree.build(positions, 0..count);
        }
        tree
    }

    fn build(&mut self, positions: &[DVec3], range: std::ops::Range<usize>) -> usize {
        let (lo, hi) = self.indices[range.clone()].iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &index| (lo.min(positions[index]), hi.max(positions[index])),
        );
        let index = self.nodes.len();
        self.nodes.push(Node {
            lo,
            hi,
            range: range.clone(),
            children: None,
        });
        if range.len() > 16 {
            let extent = hi - lo;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let middle = range.len() / 2;
            self.indices[range.clone()].select_nth_unstable_by(middle, |&a, &b| {
                positions[a][axis]
                    .total_cmp(&positions[b][axis])
                    .then(a.cmp(&b))
            });
            let split = range.start + middle;
            let left = self.build(positions, range.start..split);
            let right = self.build(positions, split..range.end);
            self.nodes[index].children = Some([left, right]);
        }
        index
    }

    pub fn visible(
        &self,
        positions: &[DVec3],
        camera: &camera::Camera,
        rect: egui::Rect,
    ) -> Vec<usize> {
        if self.nodes.is_empty() {
            return Vec::new();
        }
        let mut queue = std::collections::VecDeque::from([0]);
        let mut found = Vec::new();
        let mut visits = 0;
        while let Some(index) = queue.pop_front() {
            visits += 1;
            let node = &self.nodes[index];
            let center = (node.lo + node.hi) * 0.5;
            let radius = ((node.hi - node.lo).length() * 0.5 * camera.scale) as f32;
            let projected = camera.project(center, rect).0;
            if !rect.expand(radius + 10.).contains(projected) {
                continue;
            }
            if let Some(children) = node.children {
                if radius > 2. && visits < MAX_VISITS && queue.len() + found.len() < MAX_SAMPLES {
                    queue.extend(children);
                    continue;
                }
                let sample = self.indices[node.range.start];
                if rect
                    .expand(10.)
                    .contains(camera.project(positions[sample], rect).0)
                {
                    found.push(sample);
                }
            } else {
                for &sample in &self.indices[node.range.clone()] {
                    if rect
                        .expand(10.)
                        .contains(camera.project(positions[sample], rect).0)
                    {
                        found.push(sample);
                        if found.len() >= MAX_SAMPLES {
                            return found;
                        }
                    }
                }
            }
            if found.len() >= MAX_SAMPLES {
                break;
            }
        }
        found
    }

    pub fn nearest(&self, positions: &[DVec3], point: DVec3) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut stack = vec![0];
        let mut closest = None;
        let mut distance = f64::INFINITY;
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            if point.clamp(node.lo, node.hi).distance_squared(point) > distance {
                continue;
            }
            if let Some([a, b]) = node.children {
                let center_a = (self.nodes[a].lo + self.nodes[a].hi) * 0.5;
                let center_b = (self.nodes[b].lo + self.nodes[b].hi) * 0.5;
                if center_a.distance_squared(point) < center_b.distance_squared(point) {
                    stack.extend([b, a]);
                } else {
                    stack.extend([a, b]);
                }
            } else {
                for &sample in &self.indices[node.range.clone()] {
                    let candidate = positions[sample].distance_squared(point);
                    if candidate < distance {
                        distance = candidate;
                        closest = Some(sample);
                    }
                }
            }
        }
        closest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_catalogue_queries_bound_rendering_and_preserve_nearest_selection() {
        let positions: Vec<_> = (0..100_000)
            .map(|i| {
                DVec3::new(
                    (i % 100) as f64,
                    ((i / 100) % 100) as f64,
                    (i / 10_000) as f64,
                )
            })
            .collect();
        let tree = Tree::new(&positions);
        for index in [0, 91, 50123, 99999] {
            assert_eq!(
                tree.nearest(&positions, positions[index] + DVec3::splat(0.01)),
                Some(index)
            );
        }
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 600.));
        let mut camera = camera::Camera::default();
        camera.fit(positions.iter().copied(), rect);
        let visible = tree.visible(&positions, &camera, rect);
        assert!(!visible.is_empty());
        assert!(visible.len() <= MAX_SAMPLES);
        assert!(visible.iter().all(|&index| {
            rect.expand(10.)
                .contains(camera.project(positions[index], rect).0)
        }));
    }
}
