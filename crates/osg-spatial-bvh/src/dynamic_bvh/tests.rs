use super::*;
use std::collections::HashSet;

fn validate<T>(tree: &DynamicLuminosityBvh<T>) {
    let mut nodes = HashSet::new();
    let mut leaves = HashSet::new();
    let mut stack = Vec::new();
    if tree.root != NONE {
        assert_eq!(tree.nodes[tree.root as usize].parent, NONE);
        stack.push(tree.root);
    }
    while let Some(index) = stack.pop() {
        assert!(nodes.insert(index), "node reached more than once");
        let node = tree.nodes[index as usize];
        if node.right == NONE {
            assert!(leaves.insert(node.left));
            let leaf = &tree.leaves[node.left as usize];
            assert_eq!(leaf.node, index);
            assert!(leaf.object.is_some());
            assert!(contains(node.bounds, leaf.bounds));
            assert_eq!(node.height, 0);
        } else {
            let left = tree.nodes[node.left as usize];
            let right = tree.nodes[node.right as usize];
            assert_eq!(left.parent, index);
            assert_eq!(right.parent, index);
            assert_eq!(node.bounds, left.bounds.union(right.bounds));
            assert_eq!(node.luminosity, left.luminosity.max(right.luminosity));
            assert_eq!(node.height, 1 + left.height.max(right.height));
            assert!(
                left.height.abs_diff(right.height) <= 1,
                "unbalanced node {index}"
            );
            stack.extend([node.left, node.right]);
        }
    }
    assert_eq!(leaves.len(), tree.len());
    assert_eq!(nodes.len(), tree.len().saturating_mul(2).saturating_sub(1));
    for &index in &tree.free_nodes {
        assert!(nodes.insert(index), "duplicate or live free node");
    }
    assert_eq!(nodes.len(), tree.nodes.len());
    for &slot in &tree.free_leaves {
        assert!(leaves.insert(slot), "duplicate or live free leaf");
        assert!(tree.leaves[slot as usize].object.is_none());
    }
    assert_eq!(leaves.len(), tree.leaves.len());
}

#[test]
fn contained_motion_keeps_topology_but_publishes_geometry_and_luminosity() {
    let (mut tree, handles) =
        DynamicLuminosityBvh::build([entry(0, 0, 1_000.0, 10.0), entry(1, 10_000, 1.0, 5.0)]);
    tree.take_stats();
    let moved = entry(0, 5, 1_000.0, 2.0);
    tree.update(handles[0], moved.clone()).unwrap();
    let stats = tree.take_stats();
    assert_eq!(stats.bounds_updates, 0);
    assert_eq!(stats.reinsertions, 0);
    assert_eq!(stats.rotations, 0);
    assert_eq!(tree.nodes[tree.root as usize].luminosity, 5.0);
    let mut seen = false;
    tree.try_visit([0; 3], |bounds, luminosity, object| {
        if object == Some(&0) {
            assert_eq!(bounds, moved.bounds);
            assert_eq!(luminosity, 2.0);
            seen = true;
        }
        Ok::<_, ()>(true)
    })
    .unwrap();
    assert!(seen);
    let moved_again = entry(0, 10, 1_000.0, 20.0);
    tree.update_batch(&mut vec![(handles[0], moved_again.clone())])
        .unwrap();
    let stats = tree.take_stats();
    assert_eq!(stats.bounds_updates, 0);
    assert_eq!(stats.reinsertions, 0);
    assert_eq!(stats.rotations, 0);
    assert_eq!(tree.nodes[tree.root as usize].luminosity, 20.0);
    assert_eq!(tree.leaf(handles[0]).unwrap().bounds, moved_again.bounds);
    validate(&tree);
}

#[test]
fn teleport_padding_shrinks_when_motion_stops_and_saturates_at_limits() {
    let mut tree = DynamicLuminosityBvh::new();
    let handle = tree.insert(entry(0, 0, 1.0, 0.0));
    let moved = entry(0, 1_000_000, 1.0, 0.0);
    tree.update(handle, moved.clone()).unwrap();
    let node = tree.leaf(handle).unwrap().node as usize;
    let large = tree.nodes[node].bounds;
    let largest_extent = (0..3)
        .map(|axis| moved.bounds.min[axis].abs_diff(moved.bounds.max[axis]))
        .max()
        .unwrap();
    for axis in 0..3 {
        assert!(large.min[axis].abs_diff(moved.bounds.min[axis]) <= 1 + 256 * largest_extent);
        assert!(large.max[axis].abs_diff(moved.bounds.max[axis]) <= 1 + 256 * largest_extent);
    }
    tree.update_batch(&mut vec![(handle, moved.clone())])
        .unwrap();
    assert!(contains(large, tree.nodes[node].bounds));
    assert_ne!(large, tree.nodes[node].bounds);
    assert!(contains(tree.nodes[node].bounds, moved.bounds));

    let extreme = Aabb {
        min: [i128::MIN; 3],
        max: [i128::MAX; 3],
    };
    assert_eq!(padded(extreme, moved.bounds, 4), extreme);
    let point = Aabb {
        min: [i128::MAX; 3],
        max: [i128::MAX; 3],
    };
    assert!(contains(padded(point, extreme, 4), point));
    validate(&tree);
}

fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state
}

fn entry(id: u64, position: i128, radius: f64, luminosity: f64) -> BvhEntry<u64> {
    BvhEntry {
        object: id,
        bounds: Aabb::sphere([position, position / 7, -position / 3], radius),
        luminosity,
    }
}

#[test]
fn randomized_mutations_match_fresh_tree_and_exhaustive_queries() {
    let mut tree = DynamicLuminosityBvh::new();
    let mut records: Vec<(LeafHandle, BvhEntry<u64>)> = Vec::new();
    let mut rng = 1983;
    for step in 0..4000 {
        let choice = next(&mut rng) % 5;
        if records.is_empty() || (choice == 0 && records.len() < 160) {
            let position = (next(&mut rng) % 200_000_000) as i128 - 100_000_000;
            let input = entry(step, position, (step % 7) as f64, (step % 31) as f64);
            let handle = tree.insert(entry(
                input.object,
                position,
                (step % 7) as f64,
                input.luminosity,
            ));
            records.push((handle, input));
        } else {
            let index = next(&mut rng) as usize % records.len();
            if choice == 1 {
                let (handle, removed) = records.swap_remove(index);
                assert_eq!(tree.remove(handle).unwrap(), removed.object);
                assert_eq!(tree.remove(handle), Err(InvalidLeafHandle));
                assert_eq!(tree.get(handle), Err(InvalidLeafHandle));
            } else {
                let (handle, record) = &mut records[index];
                if choice == 2 {
                    let position = (next(&mut rng) % 400_000_000) as i128 - 200_000_000;
                    record.bounds = entry(0, position, (step % 17) as f64, 0.0).bounds;
                }
                record.luminosity = (next(&mut rng) % 41) as f64;
                record.object = step;
                tree.update(
                    *handle,
                    BvhEntry {
                        object: record.object,
                        bounds: record.bounds,
                        luminosity: record.luminosity,
                    },
                )
                .unwrap();
            }
        }
        validate(&tree);
        if step % 23 != 0 {
            continue;
        }
        let (reference, _) = LuminosityBvh::build(records.iter().map(|(_, record)| BvhEntry {
            object: record.object,
            bounds: record.bounds,
            luminosity: record.luminosity,
        }));
        let origin = [23_000_000, -11_000_000, 0];
        for flux in [0.0, 0.001, 1.0] {
            let accepts = |bounds: Aabb, luminosity: f64| {
                let distance = bounds.distance_squared(origin);
                distance <= 10000.0 && luminosity >= flux * distance
            };
            let mut actual = Vec::new();
            tree.try_visit(origin, |bounds, luminosity, object| {
                let keep = accepts(bounds, luminosity);
                if keep && let Some(&object) = object {
                    actual.push(object);
                }
                Ok::<_, ()>(keep)
            })
            .unwrap();
            let mut rebuilt = Vec::new();
            reference
                .try_visit(origin, |bounds, luminosity, object| {
                    let keep = accepts(bounds, luminosity);
                    if keep && let Some(&object) = object {
                        rebuilt.push(object);
                    }
                    Ok::<_, ()>(keep)
                })
                .unwrap();
            let mut expected: Vec<_> = records
                .iter()
                .filter(|(_, record)| accepts(record.bounds, record.luminosity))
                .map(|(_, record)| record.object)
                .collect();
            actual.sort_unstable();
            rebuilt.sort_unstable();
            expected.sort_unstable();
            assert_eq!(actual, expected);
            assert_eq!(actual, rebuilt);
        }
    }
}

#[test]
fn bulk_build_extreme_bounds_reuse_and_clone_independence() {
    let input = [i128::MIN, -(1_i128 << 110), 0, 0, 1_i128 << 110, i128::MAX];
    let (mut tree, handles) =
        DynamicLuminosityBvh::build(input.into_iter().enumerate().map(|(id, x)| BvhEntry {
            object: id,
            bounds: Aabb::sphere([x, 0, 0], 1.0),
            luminosity: id as f64,
        }));
    validate(&tree);
    let saved = tree.clone();
    let node_slots = tree.nodes.len();
    let leaf_slots = tree.leaves.len();
    for (id, handle) in handles.iter().copied().enumerate() {
        assert_eq!(*tree.get(handle).unwrap(), id);
        tree.update(
            handle,
            BvhEntry {
                object: id + 10,
                bounds: Aabb::sphere([1_i128 << 110, id as i128, 0], 0.0),
                luminosity: 0.0,
            },
        )
        .unwrap();
        validate(&tree);
    }
    assert_eq!(tree.nodes[tree.root as usize].luminosity, 0.0);
    for handle in &handles {
        tree.remove(*handle).unwrap();
        validate(&tree);
    }
    assert!(tree.is_empty());
    for id in 0..input.len() {
        tree.insert(BvhEntry {
            object: id,
            bounds: Aabb::sphere([0; 3], 0.0),
            luminosity: 1.0,
        });
        validate(&tree);
    }
    assert_eq!(tree.nodes.len(), node_slots);
    assert_eq!(tree.leaves.len(), leaf_slots);
    for (id, handle) in handles.into_iter().enumerate() {
        assert_eq!(tree.get(handle), Err(InvalidLeafHandle));
        assert_eq!(*saved.get(handle).unwrap(), id);
    }
    let empty = DynamicLuminosityBvh::<usize>::build([]).0;
    validate(&empty);
    assert_eq!(
        empty.try_visit([0; 3], |_, _, _| Err::<bool, _>("visited")),
        Ok(())
    );
    let mut visits = 0;
    assert_eq!(
        tree.try_visit([0; 3], |_, _, _| {
            visits += 1;
            Err::<bool, _>("stop")
        }),
        Err("stop")
    );
    assert_eq!(visits, 1);
}

#[test]
fn ordered_insertions_and_teleports_stay_balanced() {
    let mut tree = DynamicLuminosityBvh::new();
    let handles: Vec<_> = (0..1024)
        .map(|id| tree.insert(entry(id, id as i128 * 1_000_000, 0.1, 1.0)))
        .collect();
    validate(&tree);
    for pass in 0..4 {
        for (id, &handle) in handles.iter().enumerate() {
            let position = ((1023 - id) as i128 - 1024 * pass) * 1_000_000;
            tree.update(handle, entry(id as u64, position, 0.1, pass as f64))
                .unwrap();
        }
        validate(&tree);
        assert!(tree.height() <= 15);
    }
}

#[test]
fn invalid_update_leaves_the_existing_record_intact() {
    let mut tree = DynamicLuminosityBvh::new();
    let handle = tree.insert(entry(1, 0, 1.0, 2.0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tree.update(handle, entry(99, 0, 1.0, f64::NAN)).unwrap();
    }));
    assert!(result.is_err());
    assert_eq!(*tree.get(handle).unwrap(), 1);
    validate(&tree);
}

#[test]
fn batches_refit_exact_bounds_and_validate_before_mutating() {
    let (mut tree, handles) = DynamicLuminosityBvh::build(
        (0..256).map(|id| entry(id, id as i128 * 1_000_000, 10.0, 1.0)),
    );
    for frame in 0..100 {
        let mut changes: Vec<_> = handles
            .iter()
            .enumerate()
            .map(|(id, &handle)| {
                let x = id as i128 * 1_000_000 + (frame % 17) * 12345;
                (handle, entry(id as u64, x, 10.0, (frame % 5) as f64))
            })
            .collect();
        tree.update_batch(&mut changes).unwrap();
        assert!(changes.is_empty());
        validate(&tree);
        for &handle in &handles {
            let leaf = tree.leaf(handle).unwrap();
            let id = *leaf.object.as_ref().unwrap();
            let expected = entry(
                id,
                id as i128 * 1_000_000 + (frame % 17) * 12345,
                10.0,
                (frame % 5) as f64,
            );
            assert_eq!(leaf.bounds, expected.bounds);
            assert!(contains(
                tree.nodes[leaf.node as usize].bounds,
                expected.bounds
            ));
            assert_eq!(
                tree.nodes[leaf.node as usize].luminosity,
                expected.luminosity
            );
        }
    }
    let stale = handles[0];
    tree.remove(stale).unwrap();
    let mut changes = vec![
        (handles[1], entry(999, 0, 0.0, 0.0)),
        (stale, entry(999, 0, 0.0, 0.0)),
    ];
    assert_eq!(tree.update_batch(&mut changes), Err(InvalidLeafHandle));
    assert_eq!(*tree.get(handles[1]).unwrap(), 1);
    validate(&tree);
}

#[test]
fn identical_dark_leaves_still_propagate_height_changes() {
    let mut tree = DynamicLuminosityBvh::new();
    for id in 0..128 {
        tree.insert(entry(id, 0, 0.0, 0.0));
        validate(&tree);
    }
}
