use super::*;
use crate::Aabb;
use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

fn point(position: Position) -> Aabb {
    Aabb {
        min: position,
        max: position,
    }
}

fn entries(input: &[(Aabb, f64)]) -> impl Iterator<Item = BvhEntry<usize>> + '_ {
    input
        .iter()
        .enumerate()
        .map(|(object, &(bounds, luminosity))| BvhEntry {
            object,
            bounds,
            luminosity,
        })
}

fn visible(forest: &LuminosityForest<usize>, from: Position, threshold: f64) -> Vec<usize> {
    let mut objects: Vec<_> = forest.visible_from(from, threshold).copied().collect();
    objects.sort_unstable();
    assert!(objects.windows(2).all(|pair| pair[0] != pair[1]));
    objects
}

#[test]
fn empty_singleton_and_equal_luminosities() {
    let empty = LuminosityForest::<usize>::build([]);
    assert!(visible(&empty, [0; 3], 0.0).is_empty());
    for luminosity in [0.0, -0.0, f64::from_bits(1), 1.0, f64::MAX] {
        for count in [1, 17] {
            let forest =
                LuminosityForest::build(entries(&vec![(point([0; 3]), luminosity); count]));
            assert_eq!(
                visible(&forest, [0; 3], f64::MAX),
                (0..count).collect::<Vec<_>>()
            );
            assert_eq!(forest.buckets[0].visible_from([0; 3], 0.0).count(), count);
            assert!(
                forest.buckets[1..]
                    .iter()
                    .all(|tree| tree.visible_from([0; 3], 0.0).next().is_none())
            );
        }
    }
}

#[test]
fn power_of_two_buckets_follow_the_dimmest_input() {
    for offset in [-100, 0, 900] {
        let input: Vec<_> = (0..=64)
            .map(|exponent| (point([0; 3]), 2_f64.powi(exponent + offset)))
            .chain([(point([0; 3]), 0.0)])
            .collect();
        let forest = LuminosityForest::build(entries(&input));
        for (bucket, tree) in forest.buckets.iter().enumerate() {
            let mut objects: Vec<_> = tree.visible_from([0; 3], 0.0).copied().collect();
            objects.sort_unstable();
            let expected = match bucket {
                0 => vec![0, 65],
                63 => vec![63, 64],
                _ => vec![bucket],
            };
            assert_eq!(objects, expected);
        }
    }
}

#[test]
fn extreme_luminosities_and_adjacent_values_are_retained() {
    let input: Vec<_> = [0.0, f64::from_bits(1), f64::MIN_POSITIVE, 1.0, f64::MAX]
        .map(|luminosity| (point([0; 3]), luminosity))
        .into();
    let forest = LuminosityForest::build(entries(&input));
    assert_eq!(visible(&forest, [0; 3], 0.0), vec![0, 1, 2, 3, 4]);
    assert!(
        forest.buckets[0]
            .visible_from([0; 3], 0.0)
            .any(|id| *id == 1)
    );
    assert!(
        forest.buckets[63]
            .visible_from([0; 3], 0.0)
            .any(|id| *id == 4)
    );

    let input = [
        (point([0; 3]), f64::MAX.next_down()),
        (point([0; 3]), f64::MAX),
    ];
    let forest = LuminosityForest::build(entries(&input));
    assert_eq!(visible(&forest, [0; 3], 0.0), vec![0, 1]);
}

#[test]
fn visibility_matches_full_scan_across_buckets_and_observers() {
    let input: Vec<_> = (0..257)
        .map(|i| {
            let min = [
                i * 123_457 - 10_000_000,
                (i * 7919 % 101) * 1_000_000,
                -i * 31,
            ];
            let bounds = Aabb {
                min,
                max: min.map(|x| x + 250_000),
            };
            let luminosity = if i % 19 == 0 {
                0.0
            } else {
                10_f64.powi((i % 21) as i32 - 10)
            };
            (bounds, luminosity)
        })
        .collect();
    let forest = LuminosityForest::build(entries(&input));
    for from in [[0; 3], [1_000_000; 3], input[0].0.min, [1_i128 << 110; 3]] {
        for threshold in [0.0, f64::from_bits(1), 1e-30, 1e-8, 1.0, 1e10, f64::MAX] {
            let expected: Vec<_> = input
                .iter()
                .enumerate()
                .filter_map(|(object, (bounds, luminosity))| {
                    let mut distance_squared = 0.0;
                    for axis in 0..3 {
                        let distance = if from[axis] < bounds.min[axis] {
                            from[axis].abs_diff(bounds.min[axis])
                        } else if from[axis] > bounds.max[axis] {
                            from[axis].abs_diff(bounds.max[axis])
                        } else {
                            0
                        };
                        distance_squared += (distance as f64 / 1_000_000.0).powi(2);
                    }
                    (distance_squared == 0.0
                        || luminosity / (4.0 * PI * distance_squared)
                            >= threshold * (1.0 - 16.0 * f64::EPSILON))
                        .then_some(object)
                })
                .collect();
            assert_eq!(
                visible(&forest, from, threshold),
                expected,
                "from={from:?}, threshold={threshold}"
            );
        }
    }
}

#[test]
fn visibility_preserves_bvh_numeric_edge_cases() {
    let input = [
        (point([0; 3]), 0.0),
        (point([1, 0, 0]), f64::from_bits(1)),
        (point([1_000_000, 0, 0]), f64::from_bits(1)),
        (point([1_000_000, 0, 0]), 4.0 * PI),
        (point([i128::MAX; 3]), f64::MAX),
        (
            Aabb {
                min: [i128::MIN; 3],
                max: [i128::MAX; 3],
            },
            1.0,
        ),
    ];
    let forest = LuminosityForest::build(entries(&input));
    let (tree, _) = LuminosityBvh::build(entries(&input));
    for from in [[0; 3], [i128::MIN; 3], [i128::MAX; 3]] {
        for threshold in [
            0.0,
            f64::from_bits(1),
            1e-70,
            1.0,
            1.0_f64.next_up(),
            1.0 + 1e-12,
            f64::MAX,
        ] {
            let mut expected: Vec<_> = tree.visible_from(from, threshold).copied().collect();
            expected.sort_unstable();
            assert_eq!(visible(&forest, from, threshold), expected);
        }
    }
    assert!(visible(&forest, [0; 3], 1.0_f64.next_up()).contains(&3));
    assert!(!visible(&forest, [0; 3], 1.0 + 1e-12).contains(&3));
}

#[test]
fn invalid_entries_are_rejected() {
    for luminosity in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            std::panic::catch_unwind(|| LuminosityForest::build(entries(&[(
                point([0; 3]),
                luminosity
            )])))
            .is_err()
        );
    }
    for axis in 0..3 {
        let mut bounds = point([0; 3]);
        bounds.min[axis] = 1;
        assert!(
            std::panic::catch_unwind(|| LuminosityForest::build(entries(&[(bounds, 1.0)])))
                .is_err()
        );
    }
}

#[test]
fn invalid_thresholds_panic_before_iteration() {
    for input in [vec![], vec![(point([0; 3]), 1.0)]] {
        let forest = LuminosityForest::build(entries(&input));
        for threshold in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(std::panic::catch_unwind(|| forest.visible_from([0; 3], threshold)).is_err());
        }
    }
}

#[test]
fn iterator_borrows_non_clone_payloads_and_drops_them_once() {
    struct Payload {
        id: usize,
        drops: Rc<Cell<usize>>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let forest = LuminosityForest::build((0..10).map(|id| BvhEntry {
        object: Payload {
            id,
            drops: Rc::clone(&drops),
        },
        bounds: point([0; 3]),
        luminosity: 10_f64.powi(id as i32),
    }));
    let first = forest.visible_from([0; 3], 0.0).next().unwrap();
    assert!(std::ptr::eq(
        first,
        forest.visible_from([0; 3], 0.0).next().unwrap()
    ));
    assert_eq!(drops.get(), 0);
    let mut iter = forest.visible_from([0; 3], 0.0);
    let mut ids: Vec<_> = iter.by_ref().map(|object| object.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, (0..10).collect::<Vec<_>>());
    assert!(iter.next().is_none());
    assert!(iter.next().is_none());
    drop(iter);
    drop(forest);
    assert_eq!(drops.get(), 10);
}

#[test]
fn power_of_two_boundaries_and_overflow() {
    let input = [
        0.0,
        1.0,
        2.0_f64.next_down(),
        2.0,
        2.0_f64.next_up(),
        4.0,
        2_f64.powi(63),
        2_f64.powi(64),
        f64::MAX,
    ]
    .map(|luminosity| (point([0; 3]), luminosity));
    let forest = LuminosityForest::build(entries(&input));
    assert_eq!(forest.buckets.len(), 64);
    for (bucket, tree) in forest.buckets.iter().enumerate() {
        let mut actual: Vec<_> = tree.visible_from([0; 3], 0.0).copied().collect();
        actual.sort_unstable();
        let expected = match bucket {
            0 => vec![0, 1, 2],
            1 => vec![3, 4],
            2 => vec![5],
            63 => vec![6, 7, 8],
            _ => vec![],
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn power_of_two_exponents_cover_subnormals_and_normal_boundaries() {
    assert_eq!(binary_exponent(f64::from_bits(1)), -1074);
    assert_eq!(binary_exponent(f64::from_bits(2)), -1073);
    assert_eq!(binary_exponent(f64::MIN_POSITIVE.next_down()), -1023);
    assert_eq!(binary_exponent(f64::MIN_POSITIVE), -1022);
    assert_eq!(binary_exponent(f64::MAX), 1023);
    let input = [
        f64::from_bits(1),
        f64::from_bits(2),
        f64::MIN_POSITIVE,
        f64::MAX,
    ]
    .map(|luminosity| (point([0; 3]), luminosity));
    let forest = LuminosityForest::build(entries(&input));
    for (bucket, object) in [(0, 0), (1, 1), (52, 2), (63, 3)] {
        assert_eq!(
            forest.buckets[bucket]
                .visible_from([0; 3], 0.0)
                .copied()
                .collect::<Vec<_>>(),
            vec![object]
        );
    }
}

#[test]
fn visibility_matches_bvh_across_overflow_buckets() {
    let input: Vec<_> = (0..257)
        .map(|i| {
            let luminosity = if i % 17 == 0 {
                0.0
            } else {
                2_f64.powi(i - 128)
            };
            (point([i as i128 * 123_457, 0, 0]), luminosity)
        })
        .collect();
    let (tree, _) = LuminosityBvh::build(entries(&input));
    let forest = LuminosityForest::build(entries(&input));
    for from in [[0; 3], [1_000_000; 3], [i128::MAX; 3]] {
        for threshold in [0.0, f64::from_bits(1), 1e-30, 1.0, 1e30, f64::MAX] {
            let mut expected: Vec<_> = tree.visible_from(from, threshold).copied().collect();
            expected.sort_unstable();
            assert_eq!(visible(&forest, from, threshold), expected);
        }
    }
    for input in [
        vec![],
        vec![(point([0; 3]), 0.0); 3],
        vec![(point([0; 3]), 1.0); 3],
    ] {
        let forest = LuminosityForest::build(entries(&input));
        assert_eq!(
            visible(&forest, [0; 3], 0.0),
            (0..input.len()).collect::<Vec<_>>()
        );
    }
}
