use crate::{Aabb, BvhEntry, LuminosityBvh, NodeKind};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

mod visibility;

#[test]
fn morton_build_preserves_bounds_payload_mapping_and_queries() {
    let mut state = 123_u64;
    let mut input: Vec<_> = (0..513)
        .map(|i| {
            let center = std::array::from_fn(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 100_000) as i128 - 50_000
            });
            (
                Aabb {
                    min: center,
                    max: center.map(|v| v + 500),
                },
                i as f64,
            )
        })
        .collect();
    // Exercise duplicate codes, zero spans, extreme centers and full-span boxes.
    let cases = vec![
        Vec::new(),
        vec![(point([0; 3]), 0.0)],
        vec![(point([42; 3]), 1.0); 129],
        input.clone(),
        {
            input.extend([
                (point([i128::MIN; 3]), 1.0),
                (point([i128::MAX; 3]), 2.0),
                (
                    Aabb {
                        min: [i128::MIN; 3],
                        max: [i128::MAX; 3],
                    },
                    3.0,
                ),
            ]);
            input
        },
    ];

    for input in cases {
        let (tree, mapping) = LuminosityBvh::build_morton(input.iter().enumerate().map(
            |(object, &(bounds, luminosity))| BvhEntry {
                object,
                bounds,
                luminosity,
            },
        ));
        assert_eq!(mapping.len(), input.len());
        if input.is_empty() {
            assert!(tree.root.is_none());
            assert!(tree.nodes.is_empty());
            continue;
        }

        let mut visited = Vec::new();
        let (mut objects, _) = audit_subtree(&tree, tree.root.unwrap(), &input, &mut visited);
        objects.sort_unstable();
        assert_eq!(objects, (0..input.len()).collect::<Vec<_>>());
        assert_eq!(visited, (0..2 * input.len() - 1).collect::<Vec<_>>());
        for (object, &index) in mapping.iter().enumerate() {
            assert!(matches!(tree.nodes[index].kind, NodeKind::Leaf(value) if value == object));
        }

        for origin in [[0; 3], [90_000; 3]] {
            for threshold in [0.0, 1.0, 1e6] {
                let mut actual: Vec<_> = tree.visible_from(origin, threshold).copied().collect();
                actual.sort_unstable();
                let expected: Vec<_> = input
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (bounds, power))| {
                        let distance = bounds.distance_squared(origin);
                        (distance == 0.0
                            || power / (4.0 * std::f64::consts::PI * distance) >= threshold)
                            .then_some(i)
                    })
                    .collect();
                assert_eq!(actual, expected);
            }
        }
    }
}

#[test]
fn morton_topology_survives_galactic_translation() {
    let build = |translation: i128| {
        LuminosityBvh::build_morton((0..257).map(|object| BvhEntry {
            object,
            bounds: point([
                translation + object * 17 % 257,
                translation + object * 73 % 257,
                translation,
            ]),
            luminosity: 0.0,
        }))
        .1
    };
    assert_eq!(build(0), build(1_i128 << 100));
}

fn point(position: [i128; 3]) -> Aabb {
    Aabb {
        min: position,
        max: position,
    }
}

fn audit_subtree(
    tree: &LuminosityBvh<usize>,
    index: usize,
    input: &[(Aabb, f64)],
    visited: &mut Vec<usize>,
) -> (Vec<usize>, usize) {
    assert!(!visited.contains(&index), "node visited more than once");
    visited.push(index);

    let node = &tree.nodes[index];

    let (objects, height) = match &node.kind {
        NodeKind::Leaf(object) => (vec![*object], 0),
        NodeKind::Branch { left, right } => {
            let (mut left_objects, left_height) = audit_subtree(tree, *left, input, visited);
            let (right_objects, right_height) = audit_subtree(tree, *right, input, visited);

            left_objects.extend(right_objects);
            (left_objects, 1 + left_height.max(right_height))
        }
    };

    // Compute aggregates directly from original input, independently of the
    // values already stored on child nodes.
    let mut min = [i128::MAX; 3];
    let mut max = [i128::MIN; 3];
    let mut luminosity: f64 = 0.0;
    for &object in &objects {
        let (bounds, power) = input[object];
        for axis in 0..3 {
            min[axis] = min[axis].min(bounds.min[axis]);
            max[axis] = max[axis].max(bounds.max[axis]);
        }
        luminosity = luminosity.max(power);
    }

    assert_eq!(node.bounds, Aabb { min, max });
    assert_eq!(node.max_luminosity, luminosity);
    (objects, height)
}

fn check_build(input: &[(Aabb, f64)]) -> (LuminosityBvh<usize>, Vec<usize>) {
    let entries = input
        .iter()
        .enumerate()
        .map(|(object, &(bounds, luminosity))| BvhEntry {
            object,
            bounds,
            luminosity,
        });
    let (tree, indices) = LuminosityBvh::build(entries);

    assert_eq!(indices.len(), input.len());
    assert_eq!(
        indices.iter().copied().collect::<BTreeSet<_>>().len(),
        input.len()
    );

    if input.is_empty() {
        assert_eq!(tree.root, None);
        assert!(tree.nodes.is_empty());
        return (tree, indices);
    }

    assert_eq!(tree.root, Some(0));
    assert_eq!(tree.nodes.len(), input.len() * 2 - 1);
    let mut visited = Vec::new();
    let (mut objects, height) = audit_subtree(&tree, 0, input, &mut visited);
    objects.sort_unstable();
    assert_eq!(objects, (0..input.len()).collect::<Vec<_>>());
    assert_eq!(visited, (0..tree.nodes.len()).collect::<Vec<_>>());
    assert_eq!(height, input.len().next_power_of_two().ilog2() as usize);

    let mut leaf_counts = vec![1; tree.nodes.len()];
    for (index, node) in tree.nodes.iter().enumerate().rev() {
        if let NodeKind::Branch { left, right } = node.kind {
            assert_eq!(left, index + 1, "left child immediately follows parent");
            let total = leaf_counts[left] + leaf_counts[right];
            assert_eq!(leaf_counts[left], total / 2);
            leaf_counts[index] = total;
        }
    }

    for (object, &index) in indices.iter().enumerate() {
        let node = &tree.nodes[index];
        assert!(matches!(&node.kind, NodeKind::Leaf(value) if *value == object));
    }

    (tree, indices)
}

fn leaf_order(tree: &LuminosityBvh<usize>) -> Vec<usize> {
    tree.nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Leaf(object) => Some(*object),
            NodeKind::Branch { .. } => None,
        })
        .collect()
}

#[test]
fn empty_and_singleton_builds() {
    check_build(&[]);
    let (_, indices) = check_build(&[(point([10, -20, 30]), 0.0)]);
    assert_eq!(indices, vec![0]);
}

#[test]
fn construction_preserves_objects_bounds_and_preorder_for_varied_scenes() {
    let mut random = 0x1234_5678_9abc_def0_u64;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };

    for count in [2, 3, 4, 5, 17, 64, 127, 256] {
        let input: Vec<_> = (0..count)
            .map(|_| {
                let min = std::array::from_fn(|_| next() as i64 as i128);
                let max = std::array::from_fn(|axis| min[axis] + (next() % 1_000) as i128);
                let luminosity = (next() % 100_000) as f64;
                (Aabb { min, max }, luminosity)
            })
            .collect();
        check_build(&input);
    }
}

#[test]
fn splits_on_the_widest_center_axis() {
    for axis in 0..3 {
        let input: Vec<_> = [20, -10, 10, 0]
            .into_iter()
            .enumerate()
            .map(|(index, coordinate)| {
                let mut position = [index as i128; 3];
                position[axis] = coordinate;
                (point(position), index as f64)
            })
            .collect();
        let (tree, _) = check_build(&input);
        assert_eq!(leaf_order(&tree), vec![1, 3, 2, 0]);
    }
}

#[test]
fn object_extent_does_not_choose_the_split_axis() {
    let input: Vec<_> = [20, -10, 10, 0]
        .into_iter()
        .map(|y| {
            (
                Aabb {
                    min: [-1_000_000, y, 0],
                    max: [1_000_000, y, 0],
                },
                1.0,
            )
        })
        .collect();
    let (tree, _) = check_build(&input);
    assert_eq!(leaf_order(&tree), vec![1, 3, 2, 0]);
}

#[test]
fn tied_centers_use_input_order_and_tied_axes_prefer_x() {
    let identical = vec![(point([4, -5, 6]), 1.0); 33];
    let (tree, _) = check_build(&identical);
    assert_eq!(leaf_order(&tree), (0..33).collect::<Vec<_>>());

    let input = [(point([1, -1, 0]), 1.0), (point([-1, 1, 0]), 2.0)];
    let (tree, _) = check_build(&input);
    assert_eq!(leaf_order(&tree), vec![1, 0]);

    let input = [(point([0, 1, -1]), 1.0), (point([0, -1, 1]), 2.0)];
    let (tree, _) = check_build(&input);
    assert_eq!(leaf_order(&tree), vec![1, 0]);
}

#[test]
fn rounding_centers_preserves_bounds_and_breaks_ties_by_input_order() {
    let input: Vec<_> = [(-1, 0), (-1, -1), (0, 1), (0, 0)]
        .into_iter()
        .map(|(min, max)| {
            (
                Aabb {
                    min: [min, 0, 0],
                    max: [max, 0, 0],
                },
                1.0,
            )
        })
        .collect();
    let (tree, _) = check_build(&input);
    // Centers round to [-1, -1, 0, 0]. Distinct source bounds remain intact.
    assert_eq!(leaf_order(&tree), vec![0, 1, 2, 3]);
}

#[test]
fn coordinate_extremes_and_large_luminosities_do_not_overflow() {
    let input = [
        (point([i128::MAX, 0, 0]), f64::MAX),
        (point([i128::MIN, 0, 0]), 0.0),
        (
            Aabb {
                min: [i128::MIN, 0, 0],
                max: [i128::MAX, 0, 0],
            },
            1.0,
        ),
        (point([i128::MIN / 2, 0, 0]), f64::MIN_POSITIVE),
        (point([i128::MAX / 2, 0, 0]), -0.0),
    ];
    let (tree, _) = check_build(&input);
    assert_eq!(leaf_order(&tree), vec![1, 3, 2, 4, 0]);

    let full_range = Aabb {
        min: [i128::MIN; 3],
        max: [i128::MAX; 3],
    };
    check_build(&[(full_range, f64::MAX); 7]);
}

#[test]
fn translating_a_scene_to_galactic_coordinates_preserves_partitioning() {
    let input: Vec<_> = (0..31)
        .map(|index| {
            let min = [(index * 13 % 31) - 15, (index * 7 % 31) - 15, index - 15];
            let max = min.map(|value| value + 1);
            (Aabb { min, max }, index as f64)
        })
        .collect();
    let (_, original_indices) = check_build(&input);

    for offset in [[1_i128 << 100; 3], [-(1_i128 << 100), 1_i128 << 110, 0]] {
        let translated: Vec<_> = input
            .iter()
            .map(|(bounds, luminosity)| {
                (
                    Aabb {
                        min: std::array::from_fn(|axis| bounds.min[axis] + offset[axis]),
                        max: std::array::from_fn(|axis| bounds.max[axis] + offset[axis]),
                    },
                    *luminosity,
                )
            })
            .collect();
        let (_, translated_indices) = check_build(&translated);
        assert_eq!(original_indices, translated_indices);
    }
}

#[test]
fn invalid_bounds_are_rejected_on_every_axis() {
    for axis in 0..3 {
        let mut bounds = point([0; 3]);
        bounds.min[axis] = 1;
        let result = std::panic::catch_unwind(|| {
            LuminosityBvh::build([BvhEntry {
                object: (),
                bounds,
                luminosity: 0.0,
            }])
        });
        assert!(result.is_err());
    }
}

#[test]
fn negative_and_nonfinite_luminosities_are_rejected() {
    for luminosity in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let result = std::panic::catch_unwind(|| {
            LuminosityBvh::build([BvhEntry {
                object: (),
                bounds: point([0; 3]),
                luminosity,
            }])
        });
        assert!(result.is_err());
    }
}

#[test]
fn payloads_are_moved_without_clone_or_ordering_requirements() {
    struct Payload {
        value: usize,
        drops: Rc<Cell<usize>>,
    }

    impl Drop for Payload {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    let drops = Rc::new(Cell::new(0));
    let (tree, indices) = LuminosityBvh::build((0..9).map(|value| BvhEntry {
        object: Payload {
            value,
            drops: Rc::clone(&drops),
        },
        bounds: point([-(value as i128), 0, 0]),
        luminosity: value as f64,
    }));

    assert_eq!(drops.get(), 0);
    for (expected, index) in indices.into_iter().enumerate() {
        match &tree.nodes[index].kind {
            NodeKind::Leaf(payload) => assert_eq!(payload.value, expected),
            NodeKind::Branch { .. } => panic!("returned index is not a leaf"),
        }
    }

    drop(tree);
    assert_eq!(drops.get(), 9);
}
