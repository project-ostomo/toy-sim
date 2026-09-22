use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::f64::consts::PI;

use super::{IndexedTree, SegmentHit, SpatialObject};
use crate::luminosity_bvh::NodeKind;
use crate::{Aabb, Position};

pub(super) struct Ranked<'a, T> {
    distance: f64,
    pub object: &'a T,
}

impl<T> PartialEq for Ranked<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T> Eq for Ranked<'_, T> {}

impl<T> PartialOrd for Ranked<'_, T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Ranked<'_, T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance.total_cmp(&other.distance)
    }
}

pub(super) fn nearest<'a, T: SpatialObject>(
    tree: &'a IndexedTree<T>,
    centre: Position,
    radius: f64,
    count: usize,
    distance: &impl Fn(&T) -> Option<f64>,
    best: &mut BinaryHeap<Ranked<'a, T>>,
) {
    let profile = std::env::var_os("OSG_SPATIAL_PROFILE_QUERIES").is_some();
    let started = std::time::Instant::now();
    let mut nodes_visited = 0_u64;
    let mut leaf_tests = 0_u64;
    let mut accepted = 0_u64;
    let mut stack: Vec<_> = tree.root.into_iter().collect();
    while let Some(index) = stack.pop() {
        nodes_visited += 1;
        let limit = if best.len() == count {
            radius.min(best.peek().unwrap().distance)
        } else {
            radius
        };
        let node = &tree.nodes[index];
        if !within_distance(node.bounds, centre, limit) {
            continue;
        }

        match &node.kind {
            NodeKind::Leaf(id) => {
                leaf_tests += 1;
                let record = &tree.records[id];
                let Some(value) = distance(record) else {
                    continue;
                };
                accepted += 1;
                assert!(value.is_finite() && value >= 0.0, "invalid exact distance");
                if value <= limit {
                    best.push(Ranked {
                        distance: value,
                        object: record,
                    });
                    if best.len() > count {
                        best.pop();
                    }
                }
            }
            NodeKind::Branch { left, right } => {
                let a = tree.nodes[*left].bounds.distance_squared(centre);
                let b = tree.nodes[*right].bounds.distance_squared(centre);
                let (near, far) = if a <= b {
                    (*left, *right)
                } else {
                    (*right, *left)
                };
                stack.push(far);
                stack.push(near);
            }
        }
    }
    if profile {
        eprintln!(
            "spatial_nearest nodes_visited={nodes_visited} leaf_tests={leaf_tests} accepted={accepted} elapsed_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.,
        );
    }
}

pub(super) fn first_hit<'a, T: SpatialObject>(
    tree: &'a IndexedTree<T>,
    start: Position,
    end: Position,
    intersect: &impl Fn(&T) -> Option<f64>,
    best: &mut Option<SegmentHit<'a, T>>,
) {
    let mut stack = Vec::new();
    if let Some(root) = tree.root {
        if let Some(entry) = segment_entry(tree.nodes[root].bounds, start, end, 0.0) {
            stack.push((root, entry));
        }
    }

    while let Some((index, entry)) = stack.pop() {
        if best
            .as_ref()
            .is_some_and(|hit| entry > hit.fraction + 64.0 * f64::EPSILON)
        {
            continue;
        }
        let node = &tree.nodes[index];
        match &node.kind {
            NodeKind::Leaf(id) => {
                let record = &tree.records[id];
                let Some(fraction) = intersect(record) else {
                    continue;
                };
                assert!(
                    (0.0..=1.0).contains(&fraction),
                    "invalid exact hit fraction"
                );
                if best.as_ref().is_none_or(|hit| fraction < hit.fraction) {
                    *best = Some(SegmentHit {
                        object: record,
                        fraction,
                    });
                }
            }
            NodeKind::Branch { left, right } => {
                let a = segment_entry(tree.nodes[*left].bounds, start, end, 0.0);
                let b = segment_entry(tree.nodes[*right].bounds, start, end, 0.0);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let (near, far) = if a <= b {
                            ((*left, a), (*right, b))
                        } else {
                            ((*right, b), (*left, a))
                        };
                        stack.push(far);
                        stack.push(near);
                    }
                    (Some(a), None) => stack.push((*left, a)),
                    (None, Some(b)) => stack.push((*right, b)),
                    (None, None) => {}
                }
            }
        }
    }
}

pub(super) fn pairs<'a, T: SpatialObject>(
    a: &'a IndexedTree<T>,
    b: &'a IndexedTree<T>,
    same_tree: bool,
    accept: &impl Fn(&T, &T) -> bool,
    result: &mut Vec<(&'a T, &'a T)>,
) {
    let (Some(root_a), Some(root_b)) = (a.root, b.root) else {
        return;
    };
    let profile = std::env::var_os("OSG_SPATIAL_PROFILE").is_some();
    let started = std::time::Instant::now();
    let mut node_pairs = 0_u64;
    let mut leaf_pairs = 0_u64;
    let initial_results = result.len();
    let mut stack = vec![(root_a, root_b)];

    while let Some((ia, ib)) = stack.pop() {
        node_pairs += 1;
        let left = &a.nodes[ia];
        let right = &b.nodes[ib];
        if !overlaps(left.bounds, right.bounds) {
            continue;
        }

        // Partition a self-query into two internal queries and one cross-query.
        // This visits each unordered pair once and never pairs a leaf with itself.
        if same_tree && ia == ib {
            if let NodeKind::Branch { left, right } = left.kind {
                stack.extend([(left, left), (left, right), (right, right)]);
            }
            continue;
        }

        match (&left.kind, &right.kind) {
            (NodeKind::Leaf(a_id), NodeKind::Leaf(b_id)) => {
                leaf_pairs += 1;
                let a = &a.records[a_id];
                let b = &b.records[b_id];
                if accept(a, b) {
                    result.push((a, b));
                }
            }
            (NodeKind::Branch { left: l, right: r }, NodeKind::Leaf(_)) => {
                stack.extend([(*l, ib), (*r, ib)]);
            }
            (NodeKind::Leaf(_), NodeKind::Branch { left: l, right: r }) => {
                stack.extend([(ia, *l), (ia, *r)]);
            }
            (
                NodeKind::Branch {
                    left: al,
                    right: ar,
                },
                NodeKind::Branch {
                    left: bl,
                    right: br,
                },
            ) => {
                if left.bounds.surface_area() >= right.bounds.surface_area() {
                    stack.extend([(*al, ib), (*ar, ib)]);
                } else {
                    stack.extend([(ia, *bl), (ia, *br)]);
                }
            }
        }
    }
    if profile {
        eprintln!(
            "spatial_pairs node_pairs={node_pairs} leaf_pairs={leaf_pairs} accepted={} elapsed_ms={:.3}",
            result.len() - initial_results,
            started.elapsed().as_secs_f64() * 1000.,
        );
    }
}

pub(super) fn validate_radius(radius: f64) {
    assert!(
        !radius.is_nan() && radius >= 0.0,
        "radius must be nonnegative"
    );
}

pub(super) fn brightest<'a, T: SpatialObject>(
    tree: &'a IndexedTree<T>,
    observer: Position,
    flux: &impl Fn(&T) -> Option<f64>,
    best: &mut Option<(f64, &'a T)>,
) {
    let mut stack: Vec<_> = tree.root.into_iter().collect();
    while let Some(index) = stack.pop() {
        let node = &tree.nodes[index];
        if !visible(
            node.bounds,
            node.max_luminosity,
            observer,
            0.0,
            best.map_or(0.0, |v| v.0),
        ) {
            continue;
        }
        match &node.kind {
            NodeKind::Leaf(id) => {
                let record = &tree.records[id];
                if let Some(value) = flux(record) {
                    assert!(value.is_finite() && value >= 0.0);
                    if best.as_ref().is_none_or(|(old, _)| value > *old) {
                        *best = Some((value, record));
                    }
                }
            }
            NodeKind::Branch { left, right } => {
                let bound = |id| {
                    let node: &crate::luminosity_bvh::Node<<T as SpatialObject>::Id> =
                        &tree.nodes[id];
                    node.max_luminosity / node.bounds.distance_squared(observer).max(1e-30)
                };
                if bound(*left) >= bound(*right) {
                    stack.extend([*right, *left]);
                } else {
                    stack.extend([*left, *right]);
                }
            }
        }
    }
}

pub(super) fn overlaps(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|axis| a.min[axis] <= b.max[axis] && b.min[axis] <= a.max[axis])
}

pub(super) fn within_distance(bounds: Aabb, centre: Position, radius: f64) -> bool {
    bounds.distance_squared(centre).sqrt() <= radius * (1.0 + 64.0 * f64::EPSILON) + 1e-6
}

pub(super) fn visible(
    bounds: Aabb,
    luminosity: f64,
    observer: Position,
    radius: f64,
    threshold: f64,
) -> bool {
    if threshold == 0.0 {
        return true;
    }
    let distance = bounds.distance_squared(observer).sqrt();
    // Round the minimum separation down before subtracting the observer radius.
    // This matters near the sphere boundary, where subtraction loses precision.
    let separation = (distance * (1.0 - 64.0 * f64::EPSILON) - radius).max(0.0);
    separation == 0.0
        || luminosity / (4.0 * PI * separation * separation)
            >= threshold * (1.0 - 64.0 * f64::EPSILON)
}

fn difference(a: i128, b: i128) -> f64 {
    let magnitude = a.abs_diff(b) as f64 / 1_000_000.0;
    if a >= b { magnitude } else { -magnitude }
}

/// Conservative slab intersection against a box expanded by a query radius.
/// Integer subtraction preserves local detail even at galactic coordinates.
pub(super) fn segment_entry(
    bounds: Aabb,
    start: Position,
    end: Position,
    radius: f64,
) -> Option<f64> {
    let mut enter = 0.0_f64;
    let mut exit = 1.0_f64;
    for axis in 0..3 {
        let direction = difference(end[axis], start[axis]);
        let near = difference(bounds.min[axis], start[axis]);
        let far = difference(bounds.max[axis], start[axis]);
        let scale = near.abs().max(far.abs()).max(direction.abs());
        let padding = radius + scale * 64.0 * f64::EPSILON + 1e-6;
        let lower = near - padding;
        let upper = far + padding;

        if direction == 0.0 {
            if lower > 0.0 || upper < 0.0 {
                return None;
            }
        } else {
            let a = lower / direction;
            let b = upper / direction;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return None;
            }
        }
    }
    Some(enter)
}
