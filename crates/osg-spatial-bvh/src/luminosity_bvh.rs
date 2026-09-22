//! An immutable BVH with contiguous nodes and aggregate luminosity bounds.

use crate::{Aabb, Position};
use std::f64::consts::PI;

/// An object and the spatial and luminosity bounds to index for it.
#[derive(Debug)]
pub struct BvhEntry<T> {
    pub object: T,
    pub bounds: Aabb,
    /// Finite, nonnegative total isotropic power in watts.
    pub luminosity: f64,
}

pub(super) fn validate(bounds: Aabb, luminosity: f64) {
    assert!(
        (0..3).all(|axis| bounds.min[axis] <= bounds.max[axis]),
        "invalid AABB bounds"
    );
    assert!(
        luminosity.is_finite() && luminosity >= 0.0,
        "luminosity must be finite and nonnegative"
    );
}

/// An immutable binary BVH backed by a contiguous vector of nodes.
///
/// Leaf indices refer only to the tree that returned them. The caller decides
/// when to build a replacement tree; indices do not transfer to it.
#[derive(Clone)]
pub struct LuminosityBvh<T> {
    pub(super) nodes: Vec<Node<T>>,
    pub(super) root: Option<usize>,
}

#[derive(Clone)]
pub(super) struct Node<T> {
    pub(super) bounds: Aabb,
    pub(super) max_luminosity: f64,
    pub(super) kind: NodeKind<T>,
}

#[derive(Clone)]
pub(super) enum NodeKind<T> {
    Leaf(T),
    Branch { left: usize, right: usize },
}

impl<T> LuminosityBvh<T> {
    /// Builds a tree by recursively partitioning AABB centers at the median
    /// along the axis with the widest center spread.
    /// Centers are rounded down to integer micrometres for split decisions;
    /// the indexed bounds retain their original coordinates.
    ///
    /// Returns leaf indices in input order. Empty input produces an empty tree.
    /// Nodes are initially stored in depth-first preorder: parent, left subtree,
    /// right subtree. Each leaf contains one object. Equal centers are ordered
    /// by input index; equal axis spreads prefer X, then Y, then Z.
    ///
    /// # Panics
    ///
    /// Panics if any bound has a minimum greater than its maximum, or if a
    /// luminosity is non-finite or negative.
    pub fn build(entries: impl IntoIterator<Item = BvhEntry<T>>) -> (Self, Vec<usize>) {
        build(entries)
    }
}

struct BuildEntry {
    input_index: usize,
    center: Position,
}

fn build<T>(entries: impl IntoIterator<Item = BvhEntry<T>>) -> (LuminosityBvh<T>, Vec<usize>) {
    let timer = std::time::Instant::now();
    let mut payloads: Vec<_> = entries.into_iter().map(Some).collect();
    let payload_ms = timer.elapsed().as_secs_f64() * 1000.;
    // Partition compact keys, moving each payload only once into its leaf.
    let mut entries: Vec<_> = payloads
        .iter()
        .enumerate()
        .map(|(input_index, entry)| {
            let entry = entry.as_ref().unwrap();
            validate(entry.bounds, entry.luminosity);

            let center = std::array::from_fn(|axis| {
                let min = entry.bounds.min[axis];
                let max = entry.bounds.max[axis];
                // The full span may exceed i128::MAX, but half always fits.
                // Round down; only partitioning uses this approximate center.
                min + (min.abs_diff(max) / 2) as i128
            });

            BuildEntry {
                input_index,
                center,
            }
        })
        .collect();

    let node_count = entries
        .len()
        .checked_add(entries.len().saturating_sub(1))
        .expect("too many entries for a BVH");
    let mut tree = LuminosityBvh {
        nodes: Vec::with_capacity(node_count),
        root: None,
    };
    let mut leaves = vec![0; entries.len()];
    let keys_ms = timer.elapsed().as_secs_f64() * 1000.;

    if !entries.is_empty() {
        partition(&mut entries);
        let partition_ms = timer.elapsed().as_secs_f64() * 1000.;
        tree.root = Some(build_subtree(
            &mut tree,
            &mut entries,
            &mut payloads,
            &mut leaves,
        ));
        let assembly_ms = timer.elapsed().as_secs_f64() * 1000.;
        let entry_count = entries.len();
        drop(payloads);
        drop(entries);
        if std::env::var_os("OSG_BVH_PROFILE").is_some() && entry_count > 1000 {
            eprintln!(
                "bvh_build entries={entry_count} payload_ms={payload_ms:.3} keys_ms={:.3} partition_ms={:.3} assembly_ms={:.3} cleanup_ms={:.3} node_bytes={} total_ms={:.3}",
                keys_ms - payload_ms,
                partition_ms - keys_ms,
                assembly_ms - partition_ms,
                timer.elapsed().as_secs_f64() * 1000. - assembly_ms,
                tree.nodes.capacity() * std::mem::size_of::<Node<T>>(),
                timer.elapsed().as_secs_f64() * 1000.
            );
        }
    }

    (tree, leaves)
}

fn build_subtree<T>(
    tree: &mut LuminosityBvh<T>,
    entries: &mut [BuildEntry],
    payloads: &mut [Option<BvhEntry<T>>],
    leaves: &mut [usize],
) -> usize {
    if entries.len() == 1 {
        let input = &mut entries[0];
        let entry = payloads[input.input_index]
            .take()
            .expect("each entry becomes one leaf");
        let index = tree.nodes.len();
        tree.nodes.push(Node {
            bounds: entry.bounds,
            max_luminosity: entry.luminosity,
            kind: NodeKind::Leaf(entry.object),
        });
        leaves[input.input_index] = index;
        return index;
    }

    // Insert a provisional branch before its descendants to preserve preorder.
    // Its bounds, luminosity, and child links are filled after both subtrees.
    let entry = payloads[entries[0].input_index]
        .as_ref()
        .expect("unbuilt entry");
    let index = tree.nodes.len();
    tree.nodes.push(Node {
        bounds: entry.bounds,
        max_luminosity: entry.luminosity,
        kind: NodeKind::Branch { left: 0, right: 0 },
    });
    let middle = entries.len() / 2;

    let (left_entries, right_entries) = entries.split_at_mut(middle);
    let left = build_subtree(tree, left_entries, payloads, leaves);
    let right = build_subtree(tree, right_entries, payloads, leaves);
    let left_node = &tree.nodes[left];
    let right_node = &tree.nodes[right];

    tree.nodes[index] = Node {
        bounds: left_node.bounds.union(right_node.bounds),
        max_luminosity: left_node.max_luminosity.max(right_node.max_luminosity),
        kind: NodeKind::Branch { left, right },
    };
    index
}

fn partition(entries: &mut [BuildEntry]) {
    if entries.len() < 2 {
        return;
    }
    let axis = widest_axis(entries);
    let middle = entries.len() / 2;
    entries.select_nth_unstable_by(middle, |a, b| {
        a.center[axis]
            .cmp(&b.center[axis])
            .then_with(|| a.input_index.cmp(&b.input_index))
    });
    let parallel = entries.len() >= 8192;
    let (left, right) = entries.split_at_mut(middle);
    if parallel {
        rayon::join(|| partition(left), || partition(right));
    } else {
        partition(left);
        partition(right);
    }
}

fn widest_axis(entries: &[BuildEntry]) -> usize {
    let mut min = entries[0].center;
    let mut max = min;

    for entry in &entries[1..] {
        for axis in 0..3 {
            min[axis] = min[axis].min(entry.center[axis]);
            max[axis] = max[axis].max(entry.center[axis]);
        }
    }

    let mut widest = 0;
    for axis in 1..3 {
        if max[axis].abs_diff(min[axis]) > max[widest].abs_diff(min[widest]) {
            widest = axis;
        }
    }

    widest
}

impl<T> LuminosityBvh<T> {
    /// Lazily yields candidate objects whose flux bound meets `threshold`.
    ///
    /// `from` is in integer micrometres and `threshold` is a finite, nonnegative
    /// flux in watts per square metre. Each node is tested using
    /// `max_luminosity / (4 * pi * d²)`, where `d` is the minimum distance in
    /// metres from `from` to its AABB. Equality is included, with a small
    /// rounding allowance to avoid pruning candidates at the threshold.
    ///
    /// Bounds containing the observer are retained, including at leaves. A
    /// zero threshold returns every object. Results are conservative candidates:
    /// the caller must use actual source positions and test occlusion if needed.
    /// No result ordering is guaranteed.
    ///
    /// Traversal uses a stack and stops when the iterator is dropped. Objects
    /// are borrowed from the tree; no `Clone` bound or result allocation is needed.
    ///
    /// # Panics
    ///
    /// Panics if `threshold` is negative or non-finite.
    pub fn visible_from(&self, from: Position, threshold: f64) -> impl Iterator<Item = &T> {
        assert!(
            threshold.is_finite() && threshold >= 0.0,
            "visibility threshold must be finite and nonnegative"
        );

        // Allow for rounding during integer conversion, distance calculation,
        // and division. This can retain a few candidates just below the cutoff.
        let cutoff = threshold * (1.0 - 16.0 * f64::EPSILON);
        let mut stack: Vec<_> = self.root.into_iter().collect();

        std::iter::from_fn(move || {
            while let Some(index) = stack.pop() {
                let node = &self.nodes[index];
                let distance_squared = node.bounds.distance_squared(from);

                if distance_squared > 0.0 {
                    let flux = node.max_luminosity / (4.0 * PI * distance_squared);
                    if flux < cutoff {
                        continue;
                    }
                }

                match &node.kind {
                    NodeKind::Leaf(object) => return Some(object),
                    NodeKind::Branch { left, right } => {
                        stack.push(*right);
                        stack.push(*left);
                    }
                }
            }

            None
        })
    }
}
