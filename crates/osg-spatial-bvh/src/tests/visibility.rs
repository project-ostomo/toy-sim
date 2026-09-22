use super::{check_build, point};
use crate::{Aabb, BvhEntry, LuminosityBvh, NodeKind, Position};
use std::f64::consts::PI;

fn visible(tree: &LuminosityBvh<usize>, from: Position, threshold: f64) -> Vec<usize> {
    let mut objects: Vec<_> = tree.visible_from(from, threshold).copied().collect();
    objects.sort_unstable();
    assert!(objects.windows(2).all(|pair| pair[0] != pair[1]));
    objects
}

#[test]
fn flux_uses_watts_metres_and_four_pi() {
    let input = [
        (point([1_000_000, 0, 0]), 4.0 * PI),
        (point([2_000_000, 0, 0]), 4.0 * PI),
        (point([1_000_000, 0, 0]), 1.0),
        (point([500_000, 0, 0]), 4.0 * PI),
    ];
    let (tree, _) = check_build(&input);

    assert_eq!(visible(&tree, [0; 3], 1.0), vec![0, 3]);
    assert_eq!(visible(&tree, [0; 3], 0.25), vec![0, 1, 3]);
    assert_eq!(visible(&tree, [0; 3], 0.1), vec![0, 1, 3]);
    assert_eq!(visible(&tree, [0; 3], 1.001), vec![3]);
}

#[test]
fn uses_the_nearest_aabb_point_instead_of_its_center() {
    let bounds = Aabb {
        min: [1_000_000, 0, 0],
        max: [9_000_000, 0, 0],
    };
    let (tree, _) = check_build(&[(bounds, 4.0 * PI)]);

    assert_eq!(visible(&tree, [0; 3], 0.5), vec![0]);
    assert!(visible(&tree, [0, 2_000_000, 0], 0.5).is_empty());
    assert_eq!(visible(&tree, [5_000_000, 0, 0], f64::MAX), vec![0]);
    assert_eq!(visible(&tree, [1_000_000, 0, 0], f64::MAX), vec![0]);
}

#[test]
fn empty_zero_threshold_and_zero_distance_are_defined() {
    let (empty, _) = LuminosityBvh::<()>::build([]);
    assert!(empty.visible_from([0; 3], 0.0).next().is_none());

    let (tree, _) = check_build(&[
        (point([0; 3]), 0.0),
        (point([1_000_000; 3]), 0.0),
        (point([10_000_000; 3]), 1.0),
    ]);
    assert_eq!(visible(&tree, [0; 3], 0.0), vec![0, 1, 2]);
    assert_eq!(visible(&tree, [0; 3], f64::MAX), vec![0]);
}

#[test]
fn close_objects_remain_distinguishable_at_galactic_coordinates() {
    for origin in [[0; 3], [1_i128 << 120; 3], [-(1_i128 << 120); 3]] {
        let input = [
            (
                point([origin[0] + 1, origin[1], origin[2]]),
                4.0 * PI * 1e-12,
            ),
            (
                point([origin[0] + 2, origin[1], origin[2]]),
                4.0 * PI * 1e-12,
            ),
        ];
        let (tree, _) = check_build(&input);
        assert_eq!(visible(&tree, origin, 0.5), vec![0]);
    }
}

#[test]
fn opposite_coordinate_extremes_do_not_overflow() {
    let (tree, _) = check_build(&[(point([i128::MAX; 3]), 1.0)]);
    assert!(visible(&tree, [i128::MIN; 3], 1e-60).is_empty());
    assert_eq!(visible(&tree, [i128::MIN; 3], 1e-70), vec![0]);

    let bounds = Aabb {
        min: [i128::MIN; 3],
        max: [i128::MAX; 3],
    };
    let (tree, _) = check_build(&[(bounds, 1.0)]);
    assert_eq!(visible(&tree, [0; 3], f64::MAX), vec![0]);
}

#[test]
fn very_large_and_subnormal_fluxes_are_handled() {
    let smallest = f64::from_bits(1);
    let input = [
        (point([1, 0, 0]), f64::MAX),
        (point([1_000_000, 0, 0]), f64::MAX),
        (point([1, 0, 0]), smallest),
        (point([1_000_000, 0, 0]), smallest),
    ];
    let (tree, _) = check_build(&input);

    assert_eq!(visible(&tree, [0; 3], f64::MAX), vec![0]);
    assert_eq!(visible(&tree, [0; 3], smallest), vec![0, 1, 2]);
}

#[test]
fn near_threshold_rounding_remains_conservative() {
    let (tree, _) = check_build(&[(point([1_000_000, 0, 0]), 4.0 * PI)]);
    assert_eq!(visible(&tree, [0; 3], 1.0), vec![0]);
    assert_eq!(visible(&tree, [0; 3], 1.0_f64.next_up()), vec![0]);
    assert!(visible(&tree, [0; 3], 1.0 + 1e-12).is_empty());
}

#[test]
fn iterator_borrows_non_clone_payloads() {
    struct Object(String);

    let (tree, indices) = LuminosityBvh::build([BvhEntry {
        object: Object("star".into()),
        bounds: point([0; 3]),
        luminosity: 1.0,
    }]);
    let NodeKind::Leaf(original) = &tree.nodes[indices[0]].kind else {
        panic!("expected a leaf");
    };

    let mut candidates = tree.visible_from([0; 3], 1.0);
    let object = candidates.next().unwrap();
    assert_eq!(object.0, "star");
    assert!(std::ptr::eq(object, original));
    assert!(candidates.next().is_none());
    assert!(candidates.next().is_none());
}

#[test]
fn invalid_thresholds_are_rejected_even_for_an_empty_tree() {
    let (tree, _) = LuminosityBvh::<()>::build([]);
    for threshold in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(std::panic::catch_unwind(|| tree.visible_from([0; 3], threshold)).is_err());
    }
}
