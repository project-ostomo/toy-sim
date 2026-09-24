//! Incrementally maintained BVH with fat bounds and aggregate luminosity.
use crate::{Aabb, BvhEntry, LuminosityBvh, Position, luminosity_bvh::NodeKind};

const NONE: u32 = u32::MAX;

fn contains(outer: Aabb, inner: Aabb) -> bool {
    (0..3).all(|axis| outer.min[axis] <= inner.min[axis] && outer.max[axis] >= inner.max[axis])
}

// One percent of the extent plus four updates of observed motion. Integer
// arithmetic preserves small motion at astronomical coordinates. Saturation
// makes padding conservative even at the coordinate limits.
fn padded(bounds: Aabb, previous: Aabb, scale: u128) -> Aabb {
    let mut result = bounds;
    // A teleport must not leave an arbitrarily large retained box, especially
    // when the caller skips subsequent updates for stationary objects. Allow
    // ordinary fast motion across many object diameters between publications.
    let motion_limit = (0..3)
        .map(|axis| bounds.min[axis].abs_diff(bounds.max[axis]))
        .max()
        .unwrap()
        .max(1)
        .saturating_mul(64);
    for axis in 0..3 {
        let motion = bounds.min[axis]
            .abs_diff(previous.min[axis])
            .max(bounds.max[axis].abs_diff(previous.max[axis]))
            .min(motion_limit);
        let margin = (bounds.min[axis].abs_diff(bounds.max[axis]) / 100)
            .max(1)
            .saturating_add(motion.saturating_mul(4))
            .saturating_mul(scale);
        result.min[axis] = bounds.min[axis].saturating_sub_unsigned(margin);
        result.max[axis] = bounds.max[axis].saturating_add_unsigned(margin);
    }
    result
}

/// A leaf identity scoped to its originating tree (or an independent clone).
/// Removing a leaf invalidates its handle, even when its storage is reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LeafHandle {
    slot: u32,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidLeafHandle;

impl std::fmt::Display for InvalidLeafHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("removed or invalid dynamic BVH leaf handle")
    }
}

impl std::error::Error for InvalidLeafHandle {}

/// Cumulative mutation work since construction or the last `take_stats`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DynamicTreeStats {
    pub insertions: usize,
    pub removals: usize,
    /// Changes to retained bounds; contained movement does not increment this.
    pub bounds_updates: usize,
    pub reinsertions: usize,
    pub luminosity_updates: usize,
    pub payload_updates: usize,
    pub rotations: usize,
}

// Payloads do not inflate branches. The geometry and links fit in 128 bytes.
#[derive(Clone, Copy)]
struct Node {
    bounds: Aabb,
    luminosity: f64,
    parent: u32,
    // For leaves, left is a payload slot and right is NONE.
    left: u32,
    right: u32,
    height: u32,
}

#[derive(Clone)]
struct Leaf<T> {
    object: Option<T>,
    bounds: Aabb,
    node: u32,
    generation: u64,
}

/// A mutable binary BVH. Every branch stores the exact union of its children
/// and their maximum luminosity. Leaves retain padded bounds for maintenance,
/// while traversal callbacks receive their current exact bounds.
///
/// Updates are immediately visible; callers control publication by borrowing
/// mutably at their chosen boundary. Traversals borrow immutably and never
/// rebuild. Invalid geometric inputs panic before modifying the tree.
#[derive(Clone)]
pub struct DynamicLuminosityBvh<T> {
    nodes: Vec<Node>,
    leaves: Vec<Leaf<T>>,
    free_nodes: Vec<u32>,
    free_leaves: Vec<u32>,
    dirty: Vec<bool>,
    reinsert: Vec<u32>,
    root: u32,
    len: usize,
    stats: DynamicTreeStats,
}

impl<T> Default for DynamicLuminosityBvh<T> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            leaves: Vec::new(),
            free_nodes: Vec::new(),
            free_leaves: Vec::new(),
            dirty: Vec::new(),
            reinsert: Vec::new(),
            root: NONE,
            len: 0,
            stats: DynamicTreeStats::default(),
        }
    }
}

impl<T> DynamicLuminosityBvh<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bulk-build with the immutable builder's balanced median topology.
    /// Returned handles correspond to input order. No payload cloning.
    pub fn build(entries: impl IntoIterator<Item = BvhEntry<T>>) -> (Self, Vec<LeafHandle>) {
        let (source, order) = LuminosityBvh::build_median(entries);
        assert!(source.nodes.len() < NONE as usize, "too many BVH nodes");
        let mut tree = Self::new();
        tree.nodes.reserve(source.nodes.len());
        tree.leaves.reserve(order.len());
        tree.len = order.len();
        tree.root = source.root.map_or(NONE, |root| root as u32);
        for (index, source) in source.nodes.into_iter().enumerate() {
            let (left, right) = match source.kind {
                NodeKind::Leaf(object) => {
                    let slot = tree.leaves.len() as u32;
                    tree.leaves.push(Leaf {
                        object: Some(object),
                        bounds: source.bounds,
                        node: index as u32,
                        generation: 0,
                    });
                    (slot, NONE)
                }
                NodeKind::Branch { left, right } => (left as u32, right as u32),
            };
            tree.nodes.push(Node {
                bounds: if right == NONE {
                    padded(source.bounds, source.bounds, 1)
                } else {
                    source.bounds
                },
                luminosity: source.max_luminosity,
                parent: NONE,
                left,
                right,
                height: 0,
            });
        }

        // Preorder guarantees children follow their parents.
        for index in (0..tree.nodes.len()).rev() {
            let node = tree.nodes[index];
            if node.right != NONE {
                tree.nodes[node.left as usize].parent = index as u32;
                tree.nodes[node.right as usize].parent = index as u32;
                tree.refit(index as u32);
            }
        }
        let handles = order
            .into_iter()
            .map(|index| LeafHandle {
                slot: tree.nodes[index].left,
                generation: 0,
            })
            .collect();
        (tree, handles)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn height(&self) -> u32 {
        if self.root == NONE {
            0
        } else {
            self.nodes[self.root as usize].height
        }
    }

    /// Allocated vector storage, excluding allocations owned by T.
    pub fn allocated_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.leaves.capacity() * std::mem::size_of::<Leaf<T>>()
            + (self.free_nodes.capacity() + self.free_leaves.capacity())
                * std::mem::size_of::<u32>()
            + self.dirty.capacity() * std::mem::size_of::<bool>()
            + self.reinsert.capacity() * std::mem::size_of::<u32>()
    }

    pub fn take_stats(&mut self) -> DynamicTreeStats {
        std::mem::take(&mut self.stats)
    }

    fn leaf(&self, handle: LeafHandle) -> Result<&Leaf<T>, InvalidLeafHandle> {
        self.leaves
            .get(handle.slot as usize)
            .filter(|leaf| leaf.generation == handle.generation && leaf.object.is_some())
            .ok_or(InvalidLeafHandle)
    }

    pub fn get(&self, handle: LeafHandle) -> Result<&T, InvalidLeafHandle> {
        Ok(self.leaf(handle)?.object.as_ref().unwrap())
    }

    pub fn insert(&mut self, entry: BvhEntry<T>) -> LeafHandle {
        crate::luminosity_bvh::validate(entry.bounds, entry.luminosity);
        let slot = self.free_leaves.pop().unwrap_or_else(|| {
            assert!(self.leaves.len() < NONE as usize, "too many BVH leaves");
            let slot = self.leaves.len() as u32;
            self.leaves.push(Leaf {
                object: None,
                bounds: entry.bounds,
                node: NONE,
                generation: 0,
            });
            slot
        });
        let node = self.allocate(Node {
            bounds: padded(entry.bounds, entry.bounds, 1),
            luminosity: entry.luminosity,
            parent: NONE,
            left: slot,
            right: NONE,
            height: 0,
        });
        let leaf = &mut self.leaves[slot as usize];
        leaf.node = node;
        leaf.bounds = entry.bounds;
        leaf.object = Some(entry.object);
        let handle = LeafHandle {
            slot,
            generation: leaf.generation,
        };
        self.attach(node);
        self.len += 1;
        self.stats.insertions += 1;
        handle
    }

    pub fn update(
        &mut self,
        handle: LeafHandle,
        entry: BvhEntry<T>,
    ) -> Result<(), InvalidLeafHandle> {
        let node = self.leaf(handle)?.node;
        crate::luminosity_bvh::validate(entry.bounds, entry.luminosity);
        let old = self.nodes[node as usize];
        let bounds = self.retained_bounds(handle.slot, entry.bounds);
        if old.bounds != bounds {
            let refit = self.can_refit(old, bounds);
            if refit {
                self.nodes[node as usize].bounds = bounds;
                self.nodes[node as usize].luminosity = entry.luminosity;
                self.refit_up(old.parent);
            } else {
                self.detach(node);
                self.nodes[node as usize].bounds = bounds;
                self.nodes[node as usize].luminosity = entry.luminosity;
                self.attach(node);
                self.stats.reinsertions += 1;
            }
            self.stats.bounds_updates += 1;
        } else if old.luminosity != entry.luminosity {
            self.nodes[node as usize].luminosity = entry.luminosity;
            let mut ancestor = old.parent;
            while ancestor != NONE {
                let previous = self.nodes[ancestor as usize].luminosity;
                self.refit(ancestor);
                let current = self.nodes[ancestor as usize];
                if current.luminosity == previous {
                    break;
                }
                ancestor = current.parent;
            }
        }
        if old.luminosity != entry.luminosity {
            self.stats.luminosity_updates += 1;
        }
        self.leaves[handle.slot as usize].object = Some(entry.object);
        self.leaves[handle.slot as usize].bounds = entry.bounds;
        self.stats.payload_updates += 1;
        Ok(())
    }

    /// Apply a batch and refit each affected ancestor once before reinsertion.
    /// Drains the caller's scratch vector on success, retaining its capacity.
    /// All handles and geometry are validated before any mutation. Repeated
    /// handles are applied in order; the final payload wins.
    pub fn update_batch(
        &mut self,
        updates: &mut Vec<(LeafHandle, BvhEntry<T>)>,
    ) -> Result<(), InvalidLeafHandle> {
        for (handle, entry) in updates.iter() {
            self.leaf(*handle)?;
            crate::luminosity_bvh::validate(entry.bounds, entry.luminosity);
        }
        self.dirty.resize(self.nodes.len(), false);
        for (handle, entry) in updates.drain(..) {
            let index = self.leaves[handle.slot as usize].node;
            let old = self.nodes[index as usize];
            let bounds = self.retained_bounds(handle.slot, entry.bounds);
            if old.bounds != bounds {
                if !self.can_refit(old, bounds) {
                    self.reinsert.push(index);
                }
                self.stats.bounds_updates += 1;
                self.nodes[index as usize].bounds = bounds;
            }
            if old.luminosity != entry.luminosity {
                self.stats.luminosity_updates += 1;
                self.nodes[index as usize].luminosity = entry.luminosity;
            }
            if old.bounds != bounds || old.luminosity != entry.luminosity {
                let mut parent = old.parent;
                while parent != NONE && !self.dirty[parent as usize] {
                    self.dirty[parent as usize] = true;
                    parent = self.nodes[parent as usize].parent;
                }
            }
            self.leaves[handle.slot as usize].object = Some(entry.object);
            self.leaves[handle.slot as usize].bounds = entry.bounds;
            self.stats.payload_updates += 1;
        }
        if self.root != NONE {
            self.refit_dirty(self.root);
        }
        while let Some(index) = self.reinsert.pop() {
            self.detach(index);
            self.attach(index);
            self.stats.reinsertions += 1;
        }
        Ok(())
    }

    fn retained_bounds(&self, slot: u32, bounds: Aabb) -> Aabb {
        let leaf = &self.leaves[slot as usize];
        let old = self.nodes[leaf.node as usize].bounds;
        if contains(old, bounds) && contains(padded(bounds, leaf.bounds, 4), old) {
            old
        } else {
            padded(bounds, leaf.bounds, 1)
        }
    }

    fn can_refit(&self, old: Node, bounds: Aabb) -> bool {
        if old.parent == NONE {
            return true;
        }
        let parent = self.nodes[old.parent as usize];
        let center: Position = std::array::from_fn(|axis| {
            let min = bounds.min[axis];
            min + (min.abs_diff(bounds.max[axis]) / 2) as i128
        });
        (0..3).all(|axis| {
            center[axis] >= parent.bounds.min[axis] && center[axis] <= parent.bounds.max[axis]
        }) && parent.bounds.union(bounds).surface_area() <= 2.0 * parent.bounds.surface_area()
    }

    fn refit_dirty(&mut self, index: u32) {
        if !self.dirty[index as usize] {
            return;
        }
        let node = self.nodes[index as usize];
        self.refit_dirty(node.left);
        self.refit_dirty(node.right);
        self.refit(index);
        // Contained motion often leaves distant ancestor boxes unchanged.
        // Avoid surface-area rotation searches on those ancestors.
        if self.nodes[index as usize].bounds != node.bounds {
            self.improve(index);
        }
        self.dirty[index as usize] = false;
    }

    pub fn remove(&mut self, handle: LeafHandle) -> Result<T, InvalidLeafHandle> {
        let node = self.leaf(handle)?.node;
        self.detach(node);
        self.free_nodes.push(node);
        let leaf = &mut self.leaves[handle.slot as usize];
        let object = leaf.object.take().unwrap();
        leaf.node = NONE;
        // Retire a slot on generation exhaustion instead of resurrecting a handle.
        if let Some(generation) = leaf.generation.checked_add(1) {
            leaf.generation = generation;
            self.free_leaves.push(handle.slot);
        }
        self.len -= 1;
        self.stats.removals += 1;
        Ok(object)
    }

    /// Same pruning and early-error contract as `LuminosityBvh::try_visit`.
    pub fn try_visit<E>(
        &self,
        origin: Position,
        mut visit: impl FnMut(Aabb, f64, Option<&T>) -> Result<bool, E>,
    ) -> Result<(), E> {
        let mut stack = Vec::new();
        if self.root != NONE {
            stack.push(self.root);
        }
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            let object = if node.right == NONE {
                self.leaves[node.left as usize].object.as_ref()
            } else {
                None
            };
            let bounds = if node.right == NONE {
                self.leaves[node.left as usize].bounds
            } else {
                node.bounds
            };
            if !visit(bounds, node.luminosity, object)? {
                continue;
            }
            if node.right != NONE {
                let a = self.nodes[node.left as usize]
                    .bounds
                    .distance_squared(origin);
                let b = self.nodes[node.right as usize]
                    .bounds
                    .distance_squared(origin);
                let (near, far) = if a <= b {
                    (node.left, node.right)
                } else {
                    (node.right, node.left)
                };
                stack.push(far);
                stack.push(near);
            }
        }
        Ok(())
    }

    fn allocate(&mut self, node: Node) -> u32 {
        if let Some(index) = self.free_nodes.pop() {
            self.nodes[index as usize] = node;
            index
        } else {
            assert!(self.nodes.len() < NONE as usize, "too many BVH nodes");
            let index = self.nodes.len() as u32;
            self.nodes.push(node);
            index
        }
    }

    fn refit(&mut self, index: u32) {
        let node = self.nodes[index as usize];
        let left = self.nodes[node.left as usize];
        let right = self.nodes[node.right as usize];
        let current = &mut self.nodes[index as usize];
        current.bounds = left.bounds.union(right.bounds);
        current.luminosity = left.luminosity.max(right.luminosity);
        current.height = 1 + left.height.max(right.height);
    }

    fn replace_child(&mut self, parent: u32, old: u32, new: u32) {
        if parent == NONE {
            self.root = new;
        } else if self.nodes[parent as usize].left == old {
            self.nodes[parent as usize].left = new;
        } else {
            debug_assert_eq!(self.nodes[parent as usize].right, old);
            self.nodes[parent as usize].right = new;
        }
        if new != NONE {
            self.nodes[new as usize].parent = parent;
        }
    }

    fn attach(&mut self, leaf: u32) {
        if self.root == NONE {
            self.root = leaf;
            self.nodes[leaf as usize].parent = NONE;
            return;
        }
        let bounds = self.nodes[leaf as usize].bounds;
        let mut sibling = self.root;
        // Descend to a leaf so insertion increases any subtree height by at
        // most one. This keeps the local height-balancing invariant tractable.
        while self.nodes[sibling as usize].right != NONE {
            let current = self.nodes[sibling as usize];
            let cost = |index: u32| {
                let child = self.nodes[index as usize];
                let union = child.bounds.union(bounds).surface_area();
                if child.right == NONE {
                    union
                } else {
                    union - child.bounds.surface_area()
                }
            };
            // The inherited enlargement cost is identical for both choices.
            let left = cost(current.left);
            let right = cost(current.right);
            sibling = if left < right
                || (left == right
                    && self.nodes[current.left as usize].height
                        <= self.nodes[current.right as usize].height)
            {
                current.left
            } else {
                current.right
            };
        }
        let parent = self.nodes[sibling as usize].parent;
        let branch = self.allocate(Node {
            bounds,
            luminosity: 0.0,
            parent,
            left: sibling,
            right: leaf,
            height: 1,
        });
        self.replace_child(parent, sibling, branch);
        self.nodes[sibling as usize].parent = branch;
        self.nodes[leaf as usize].parent = branch;
        self.refit(branch);
        self.refit_up(parent);
    }

    fn detach(&mut self, leaf: u32) {
        let parent = self.nodes[leaf as usize].parent;
        if parent == NONE {
            self.root = NONE;
        } else {
            let branch = self.nodes[parent as usize];
            let sibling = if branch.left == leaf {
                branch.right
            } else {
                branch.left
            };
            self.replace_child(branch.parent, parent, sibling);
            self.free_nodes.push(parent);
            self.refit_up(branch.parent);
        }
        self.nodes[leaf as usize].parent = NONE;
    }

    fn refit_up(&mut self, mut index: u32) {
        while index != NONE {
            let previous = self.nodes[index as usize];
            self.refit(index);
            let root = self.balance(index);
            self.improve(root);
            let current = self.nodes[root as usize];
            if current.bounds == previous.bounds
                && current.luminosity == previous.luminosity
                && current.height == previous.height
            {
                break;
            }
            index = current.parent;
        }
    }

    fn balance(&mut self, index: u32) -> u32 {
        let node = self.nodes[index as usize];
        let left = self.nodes[node.left as usize];
        let right = self.nodes[node.right as usize];
        if left.height.abs_diff(right.height) <= 1 {
            return index;
        }
        let (heavy, light) = if left.height > right.height {
            (node.left, node.right)
        } else {
            (node.right, node.left)
        };
        let branch = self.nodes[heavy as usize];
        let a = self.nodes[branch.left as usize];
        let b = self.nodes[branch.right as usize];
        let light_bounds = self.nodes[light as usize].bounds;
        // Move the shorter grandchild down. At equal heights choose the
        // smaller new lower box; the promoted root's overall box is unchanged.
        let move_a = a.height < b.height
            || (a.height == b.height
                && a.bounds.union(light_bounds).surface_area()
                    <= b.bounds.union(light_bounds).surface_area());
        let (moved, retained) = if move_a {
            (branch.left, branch.right)
        } else {
            (branch.right, branch.left)
        };
        self.replace_child(node.parent, index, heavy);
        self.nodes[index as usize].left = light;
        self.nodes[index as usize].right = moved;
        self.nodes[index as usize].parent = heavy;
        self.nodes[moved as usize].parent = index;
        self.nodes[heavy as usize].left = index;
        self.nodes[heavy as usize].right = retained;
        self.refit(index);
        self.refit(heavy);
        self.stats.rotations += 1;
        heavy
    }

    // Exchange a child with a grandchild when this strictly reduces total
    // internal surface area without changing this subtree's height or balance.
    // Bounds and luminosity at the subtree root are unchanged by the exchange.
    fn improve(&mut self, index: u32) {
        let root = self.nodes[index as usize];
        let mut best = None;
        let mut best_gain = 0.0;
        for (branch_index, other_index) in [(root.left, root.right), (root.right, root.left)] {
            let branch = self.nodes[branch_index as usize];
            if branch.right == NONE {
                continue;
            }
            let other = self.nodes[other_index as usize];
            for (moved_index, retained_index) in
                [(branch.left, branch.right), (branch.right, branch.left)]
            {
                let moved = self.nodes[moved_index as usize];
                let retained = self.nodes[retained_index as usize];
                let branch_height = 1 + other.height.max(retained.height);
                if other.height.abs_diff(retained.height) > 1
                    || branch_height.abs_diff(moved.height) > 1
                    || 1 + branch_height.max(moved.height) != root.height
                {
                    continue;
                }
                let gain = branch.bounds.surface_area()
                    - other.bounds.union(retained.bounds).surface_area();
                if gain > best_gain {
                    best_gain = gain;
                    best = Some((branch_index, other_index, moved_index, retained_index));
                }
            }
        }
        if let Some((branch, other, moved, retained)) = best {
            self.nodes[index as usize].left = branch;
            self.nodes[index as usize].right = moved;
            self.nodes[moved as usize].parent = index;
            self.nodes[branch as usize].left = other;
            self.nodes[branch as usize].right = retained;
            self.nodes[other as usize].parent = branch;
            self.refit(branch);
            self.refit(index);
            self.stats.rotations += 1;
        }
    }
}

#[cfg(test)]
mod tests;
