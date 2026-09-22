use super::*;

#[test]
fn entity_proxy_roles_and_explicit_clones_stay_distinct() {
    use crate::{RecordKind, SpatialRecord};

    let body = SpatialRecord {
        kind: RecordKind::Body,
        id: 42,
        position: [0; 3],
        radius_m: 1.,
    };
    let collision = SpatialRecord {
        kind: RecordKind::Collision,
        radius_m: 2.,
        ..body
    };
    let mut first = SpatialService::new([]);
    first.rebuild_dynamic([
        body.dynamic(10., [100., 0., 0.]),
        collision.dynamic(0., [100., 0., 0.]),
    ]);
    let moved = SpatialRecord {
        position: [100_000_000, 0, 0],
        ..body
    };
    let mut second = first.clone();
    second.rebuild_dynamic([moved.dynamic(10., [0.; 3])]);

    let original = first.sphere_candidates([0; 3], 0.);
    assert_eq!(original.len(), 2);
    assert!(
        original
            .iter()
            .any(|r| r.kind == RecordKind::Body && r.radius_m == 1.)
    );
    assert!(
        original
            .iter()
            .any(|r| r.kind == RecordKind::Collision && r.radius_m == 2.)
    );
    assert!(first.sphere_candidates(moved.position, 0.).is_empty());
    assert_eq!(
        first
            .motion_candidates(Aabb::sphere(moved.position, 0.))
            .len(),
        2
    );
    assert!(second.sphere_candidates([0; 3], 0.).is_empty());
    assert_eq!(second.sphere_candidates(moved.position, 0.)[0].id, body.id);
}

#[test]
#[should_panic(expected = "duplicate spatial ID")]
fn duplicate_ids_in_one_tree_are_rejected() {
    SpatialService::new([0, 1].map(|x| BvhEntry {
        object: 42_u64,
        bounds: Aabb::sphere([x, 0, 0], 0.),
        luminosity: 0.,
    }));
}

#[test]
fn dynamic_scope_excludes_static_work_and_brightest_matches_exact_flux() {
    let mut scene = SpatialService::new((0..1000).map(|i| BvhEntry {
        object: i,
        bounds: Aabb::sphere([i * 1_000_000 + 10_000_000, 0, 0], 0.),
        luminosity: (i + 1) as f64,
    }));
    scene.rebuild_dynamic([DynamicEntry {
        object: 1000,
        bounds: Aabb::sphere([1_000_000, 0, 0], 0.),
        swept_bounds: Aabb::sphere([1_000_000, 0, 0], 1.),
        luminosity: 100.,
    }]);
    let mut cursor = scene.query_dynamic(SpatialQuery::Sphere {
        centre: [0; 3],
        radius_m: f64::INFINITY,
    });
    let batch = cursor.advance(QueryBudget {
        max_work: 2,
        max_results: 1,
    });
    assert_eq!(batch.objects, vec![&1000]);
    assert!(batch.complete);
    assert_eq!(cursor.stats().work(), 2);
    let flux = |id: &i128| {
        Some(if *id == 1000 {
            100. / (4. * std::f64::consts::PI)
        } else {
            (*id + 1) as f64 / (4. * std::f64::consts::PI * (*id + 10).pow(2) as f64)
        })
    };
    assert_eq!(scene.brightest([0; 3], flux), Some(&1000));
    assert!(scene.dynamic_collision_candidates(|_, _| true).is_empty());
    assert_eq!(
        scene.nearest_dynamic([0; 3], 2., 1, |_| Some(1.)),
        vec![&1000]
    );
}
use std::cell::Cell;
use std::collections::BTreeSet;
use std::f64::consts::PI;

fn point(position: Position) -> Aabb {
    Aabb {
        min: position,
        max: position,
    }
}

fn offset(origin: Position, metres: [i128; 3]) -> Position {
    std::array::from_fn(|axis| origin[axis] + metres[axis] * 1_000_000)
}

fn sorted(objects: Vec<&usize>) -> Vec<usize> {
    let mut ids: Vec<_> = objects.into_iter().copied().collect();
    ids.sort_unstable();
    ids
}

#[derive(Clone, Copy)]
struct Fixture {
    centre: Position,
    bounds: Aabb,
    swept: Aabb,
    luminosity: f64,
    dynamic: bool,
}

fn scene(origin: Position) -> (SpatialService<usize>, Vec<Fixture>) {
    let objects: Vec<_> = (0..128)
        .map(|i| {
            let centre = offset(
                origin,
                [i * 31 % 101 - 50, i * 43 % 83 - 41, i * 17 % 71 - 35],
            );
            let extent = 1 + i % 7;
            let bounds = Aabb {
                min: offset(centre, [-extent; 3]),
                max: offset(centre, [extent; 3]),
            };
            let swept = Aabb {
                min: offset(bounds.min, [-i % 19, -i % 5, -i % 11]),
                max: offset(bounds.max, [i % 23, i % 13, i % 3]),
            };
            Fixture {
                centre,
                bounds,
                swept,
                luminosity: 10_f64.powi((i % 9) as i32),
                dynamic: i % 3 != 0,
            }
        })
        .collect();
    let statics = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.dynamic)
        .map(|(id, o)| BvhEntry {
            object: id,
            bounds: o.bounds,
            luminosity: o.luminosity,
        });
    let dynamics = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.dynamic)
        .map(|(id, o)| DynamicEntry {
            object: id,
            bounds: o.bounds,
            swept_bounds: o.swept,
            luminosity: o.luminosity,
        });
    let mut service = SpatialService::new(statics);
    service.rebuild_dynamic(dynamics);
    (service, objects)
}

// Independent full-scan predicates; they do not use service traversal helpers.
fn box_distance(bounds: Aabb, from: Position) -> f64 {
    (0..3)
        .map(|axis| {
            let gap = if from[axis] < bounds.min[axis] {
                bounds.min[axis].abs_diff(from[axis])
            } else if from[axis] > bounds.max[axis] {
                from[axis].abs_diff(bounds.max[axis])
            } else {
                0
            };
            (gap as f64 / 1e6).powi(2)
        })
        .sum::<f64>()
        .sqrt()
}

fn overlap(a: Aabb, b: Aabb) -> bool {
    !(0..3).any(|axis| a.max[axis] < b.min[axis] || b.max[axis] < a.min[axis])
}

fn centre_distance(a: Position, b: Position) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a.abs_diff(b) as f64 / 1e6).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[test]
fn geometry_visibility_and_nearest_match_full_scan() {
    for origin in [[0; 3], [1 << 100, -(1 << 100), 1 << 105]] {
        let (service, objects) = scene(origin);
        for step in 0..12 {
            let from = offset(origin, [step * 7 - 40, step * 3 - 20, step - 10]);
            let radius = (step * 4 + 1) as f64;
            let bounds = Aabb {
                min: offset(from, [-7; 3]),
                max: offset(from, [11; 3]),
            };
            let expected = |predicate: &dyn Fn(&Fixture) -> bool| {
                objects
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| predicate(o))
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                sorted(service.sphere_candidates(from, radius)),
                expected(&|o| box_distance(o.bounds, from) <= radius)
            );
            assert_eq!(
                sorted(service.aabb_candidates(bounds)),
                expected(&|o| overlap(o.bounds, bounds))
            );
            assert_eq!(
                sorted(service.motion_candidates(bounds)),
                expected(&|o| overlap(if o.dynamic { o.swept } else { o.bounds }, bounds))
            );

            for observer_radius in [0.0, 5.0, 25.0] {
                for threshold in [0.0, 0.123, 7.321, 999.0] {
                    let reference = expected(&|o| {
                        let separation = (box_distance(o.bounds, from) - observer_radius).max(0.0);
                        separation == 0.0
                            || o.luminosity / (4.0 * PI * separation.powi(2)) >= threshold
                    });
                    assert_eq!(
                        sorted(service.visibility_candidates(from, observer_radius, threshold)),
                        reference
                    );
                }
            }

            for count in [0, 1, 7, 200] {
                let found = service.nearest(from, radius, count, |&id| {
                    (id % 4 != 0).then(|| centre_distance(objects[id].centre, from))
                });
                let actual: Vec<_> = found
                    .iter()
                    .map(|&&id| centre_distance(objects[id].centre, from))
                    .collect();
                let mut expected: Vec<_> = objects
                    .iter()
                    .enumerate()
                    .filter(|(id, _)| id % 4 != 0)
                    .map(|(_, o)| centre_distance(o.centre, from))
                    .filter(|&distance| distance <= radius)
                    .collect();
                expected.sort_by(f64::total_cmp);
                expected.truncate(count);
                assert_eq!(actual, expected);
            }
        }
    }
}

#[test]
fn swept_proximity_does_not_leak_into_instantaneous_queries() {
    let origin = [1 << 100; 3];
    let instant = point(offset(origin, [100, 0, 0]));
    let mut service = SpatialService::new([]);
    service.rebuild_dynamic([DynamicEntry {
        object: 7,
        bounds: instant,
        swept_bounds: Aabb {
            min: origin,
            max: instant.max,
        },
        luminosity: 4.0 * PI,
    }]);
    assert!(service.sphere_candidates(origin, 1.0).is_empty());
    assert!(service.aabb_candidates(point(origin)).is_empty());
    assert!(service.visibility_candidates(origin, 0.0, 0.01).is_empty());
    assert!(
        service
            .segment_candidates(origin, offset(origin, [1, 0, 0]), 0.0)
            .is_empty()
    );
    assert_eq!(service.motion_candidates(point(origin)), vec![&7]);
    assert_eq!(service.visibility_candidates(origin, 99.0, 1.0), vec![&7]);
}

#[test]
fn collision_pairs_match_full_scan_without_duplicates() {
    let (service, objects) = scene([1 << 100; 3]);
    let accept = |a: &usize, b: &usize| (a + b) % 3 != 0;
    let mut expected = BTreeSet::new();
    for (a, left) in objects.iter().enumerate() {
        for (b, right) in objects.iter().enumerate().skip(a + 1) {
            let left_bounds = if left.dynamic {
                left.swept
            } else {
                left.bounds
            };
            let right_bounds = if right.dynamic {
                right.swept
            } else {
                right.bounds
            };
            if (left.dynamic || right.dynamic)
                && overlap(left_bounds, right_bounds)
                && accept(&a, &b)
            {
                expected.insert((a, b));
            }
        }
    }
    let pairs = service.collision_candidates(accept);
    let actual: BTreeSet<_> = pairs.iter().map(|&(a, b)| (*a.min(b), *a.max(b))).collect();
    assert!(!actual.is_empty());
    assert_eq!(pairs.len(), actual.len());
    assert_eq!(actual, expected);
}

#[test]
fn segment_queries_match_axis_aligned_reference_in_both_directions() {
    let origin = [1 << 103, -(1 << 101), 0];
    let (service, objects) = scene(origin);
    for y in [-30, 0, 30] {
        for reverse in [false, true] {
            let start = offset(origin, [if reverse { 100 } else { -100 }, y, 0]);
            let end = offset(origin, [if reverse { -100 } else { 100 }, y, 0]);
            for radius in [0, 3, 9] {
                let reference = Aabb {
                    min: offset(origin, [-100 - radius, y - radius, -radius]),
                    max: offset(origin, [100 + radius, y + radius, radius]),
                };
                let expected: Vec<_> = objects
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| overlap(o.bounds, reference))
                    .map(|(id, _)| id)
                    .collect();
                assert_eq!(
                    sorted(service.segment_candidates(start, end, radius as f64)),
                    expected
                );
            }

            let exact = |id: &usize| {
                let bounds = objects[*id].bounds;
                if *id % 3 == 0
                    || !(bounds.min[1]..=bounds.max[1]).contains(&start[1])
                    || !(bounds.min[2]..=bounds.max[2]).contains(&start[2])
                {
                    return None;
                }
                let face = if reverse {
                    bounds.max[0]
                } else {
                    bounds.min[0]
                };
                Some(start[0].abs_diff(face) as f64 / start[0].abs_diff(end[0]) as f64)
            };
            let expected = (0..objects.len())
                .filter_map(|id| exact(&id))
                .min_by(f64::total_cmp);
            assert_eq!(
                service.first_hit(start, end, exact).map(|hit| hit.fraction),
                expected
            );
        }
    }
}

#[test]
fn rebuilding_preserves_static_non_clone_records_and_replaces_dynamic_records() {
    struct Payload(String);
    impl SpatialObject for Payload {
        type Id = u64;
        fn spatial_id(&self) -> u64 {
            match self.0.as_str() {
                "star" => 0,
                "ship" => 1,
                _ => unreachable!(),
            }
        }
    }
    let mut service = SpatialService::new([BvhEntry {
        object: Payload("star".into()),
        bounds: point([0; 3]),
        luminosity: 1.0,
    }]);
    let static_tree = Arc::clone(&service.static_tree);
    service.rebuild_dynamic([DynamicEntry {
        object: Payload("ship".into()),
        bounds: point([0; 3]),
        swept_bounds: point([0; 3]),
        luminosity: 1.0,
    }]);
    let objects: Vec<_> = service
        .sphere_candidates([0; 3], 0.0)
        .iter()
        .map(|o| o.0.as_str())
        .collect();
    assert_eq!(objects, ["star", "ship"]);
    service.rebuild_dynamic([]);
    assert!(Arc::ptr_eq(&static_tree, &service.static_tree));
    assert_eq!(service.sphere_candidates([0; 3], 0.0).len(), 1);
}

#[test]
fn paged_queries_obey_work_and_result_budgets_and_match_complete_queries() {
    let (service, _) = scene([0; 3]);
    for query in [
        SpatialQuery::Sphere {
            centre: [0; 3],
            radius_m: 30.0,
        },
        SpatialQuery::Aabb(Aabb {
            min: [-10_000_000; 3],
            max: [10_000_000; 3],
        }),
        SpatialQuery::Visibility {
            observer: [0; 3],
            observer_radius_m: 7.0,
            min_flux_w_m2: 1.0,
        },
        SpatialQuery::Segment {
            start: [-30_000_000; 3],
            end: [30_000_000; 3],
            radius_m: 5.0,
        },
        SpatialQuery::Motion(Aabb {
            min: [-10_000_000; 3],
            max: [10_000_000; 3],
        }),
    ] {
        let mut complete = service.query(query);
        let full = complete.advance(QueryBudget {
            max_work: usize::MAX,
            max_results: usize::MAX,
        });
        assert!(full.complete);
        for max_work in [1, 3, 19] {
            for max_results in [1, 7] {
                let mut cursor = service.query(query);
                let mut actual: Vec<&usize> = Vec::new();
                let mut work = 0;
                while !cursor.is_complete() {
                    let batch = cursor.advance(QueryBudget {
                        max_work,
                        max_results,
                    });
                    assert!(batch.stats.work() > 0 && batch.stats.work() <= max_work);
                    assert!(batch.objects.len() <= max_results);
                    work += batch.stats.work();
                    actual.extend(batch.objects);
                    assert!(work <= full.stats.work());
                }
                assert_eq!(actual, full.objects);
                assert_eq!(cursor.stats(), full.stats);
            }
        }
    }
}

#[test]
fn cursor_charges_nodes_and_objects_and_zero_budgets_pause() {
    let mut service = SpatialService::new([]);
    service.rebuild_dynamic([DynamicEntry {
        object: 1,
        bounds: point([100_000_000; 3]),
        swept_bounds: Aabb {
            min: [0; 3],
            max: [100_000_000; 3],
        },
        luminosity: 0.0,
    }]);
    let mut cursor = service.query(SpatialQuery::Sphere {
        centre: [0; 3],
        radius_m: 1.0,
    });
    for budget in [
        QueryBudget {
            max_work: 0,
            max_results: 1,
        },
        QueryBudget {
            max_work: 1,
            max_results: 0,
        },
    ] {
        let batch = cursor.advance(budget);
        assert_eq!(batch.stats.work(), 0);
        assert!(!batch.complete);
    }
    let node = cursor.advance(QueryBudget {
        max_work: 1,
        max_results: 1,
    });
    assert_eq!(node.stats.nodes_visited, 1);
    assert!(node.complete);
    assert!(node.objects.is_empty());
    assert_eq!(node.stats.objects_tested, 0);

    let mut cursor = service.query(SpatialQuery::Sphere {
        centre: [100_000_000; 3],
        radius_m: 1.0,
    });
    let node = cursor.advance(QueryBudget {
        max_work: 1,
        max_results: 1,
    });
    assert!(!node.complete);
    let object = cursor.advance(QueryBudget {
        max_work: 1,
        max_results: 1,
    });
    assert_eq!(object.stats.objects_tested, 1);
    assert_eq!(object.objects, vec![&1]);
    assert!(object.complete);

    let mut far = service.query(SpatialQuery::Sphere {
        centre: [-100_000_000; 3],
        radius_m: 1.0,
    });
    let rejected = far.advance(QueryBudget {
        max_work: 1,
        max_results: 1,
    });
    assert!(rejected.complete && rejected.objects.is_empty());
    assert_eq!(
        rejected.stats,
        QueryStats {
            nodes_visited: 1,
            objects_tested: 0
        }
    );
}

#[test]
fn coincident_objects_can_be_paged_without_duplicates() {
    let service = SpatialService::new((0..256).map(|id| BvhEntry {
        object: id,
        bounds: point([0; 3]),
        luminosity: 0.0,
    }));
    let mut cursor = service.query(SpatialQuery::Sphere {
        centre: [0; 3],
        radius_m: 0.0,
    });
    let mut found = Vec::new();
    while !cursor.is_complete() {
        let batch = cursor.advance(QueryBudget {
            max_work: 5,
            max_results: 2,
        });
        assert!(batch.stats.work() <= 5 && batch.objects.len() <= 2);
        found.extend(batch.objects);
    }
    assert_eq!(sorted(found), (0..256).collect::<Vec<_>>());
    assert_eq!(cursor.stats().objects_tested, 256);
    assert_eq!(cursor.stats().nodes_visited, 511);
}

#[test]
fn empty_queries_and_zero_length_segments_are_defined() {
    let empty = SpatialService::<usize>::new([]);
    assert!(empty.sphere_candidates([0; 3], f64::INFINITY).is_empty());
    assert!(
        empty
            .collision_candidates(|_, _| panic!("empty"))
            .is_empty()
    );
    assert!(
        empty
            .nearest([0; 3], 1.0, 1, |_| panic!("empty"))
            .is_empty()
    );
    assert!(
        empty
            .first_hit([0; 3], [0; 3], |_| panic!("empty"))
            .is_none()
    );
    let mut cursor = empty.query(SpatialQuery::Aabb(point([0; 3])));
    assert!(
        cursor
            .advance(QueryBudget {
                max_work: 0,
                max_results: 0
            })
            .complete
    );

    let singleton = SpatialService::new([BvhEntry {
        object: 1,
        bounds: point([0; 3]),
        luminosity: 0.0,
    }]);
    assert_eq!(singleton.segment_candidates([0; 3], [0; 3], 0.0), [&1]);
    assert_eq!(singleton.visibility_candidates([0; 3], 0.0, 100.0), [&1]);
    assert_eq!(
        singleton
            .first_hit([0; 3], [0; 3], |_| Some(0.0))
            .unwrap()
            .fraction,
        0.0
    );
}

#[test]
fn full_coordinate_span_and_boundary_contacts_remain_conservative() {
    let far = point([i128::MAX; 3]);
    let service = SpatialService::new([BvhEntry {
        object: 1,
        bounds: far,
        luminosity: f64::MAX,
    }]);
    assert_eq!(
        service.segment_candidates([i128::MIN; 3], [i128::MAX; 3], 0.0),
        [&1]
    );
    assert_eq!(service.aabb_candidates(far), [&1]);
    assert_eq!(
        service.visibility_candidates([i128::MIN; 3], 0.0, 1.0),
        [&1]
    );

    let origin = [1 << 110; 3];
    let bound = point(offset(origin, [3, 4, 0]));
    let tangent = SpatialService::new([BvhEntry {
        object: 2,
        bounds: bound,
        luminosity: 100.0 * PI,
    }]);
    assert_eq!(tangent.sphere_candidates(origin, 5.0), [&2]);
    assert_eq!(tangent.visibility_candidates(origin, 0.0, 1.0), [&2]);
    assert_eq!(
        tangent.segment_candidates(origin, offset(origin, [6, 0, 0]), 4.0),
        [&2]
    );
}

#[test]
fn nearest_and_ray_queries_prune_distant_records() {
    let service = SpatialService::new((0..1024).map(|id| BvhEntry {
        object: id,
        bounds: point([id * 1_000_000, 0, 0]),
        luminosity: 1.0,
    }));
    let count = Cell::new(0);
    assert_eq!(
        service.nearest([0; 3], f64::INFINITY, 1, |&id| {
            count.set(count.get() + 1);
            Some(id as f64)
        }),
        [&0]
    );
    assert!(count.get() < 10);

    count.set(0);
    let hit = service
        .first_hit([-1_000_000, 0, 0], [2_000_000_000, 0, 0], |&id| {
            count.set(count.get() + 1);
            Some((id as f64 + 1.0) / 2001.0)
        })
        .unwrap();
    assert_eq!(*hit.object, 0);
    assert!(count.get() < 10);
}

#[test]
fn invalid_inputs_and_callback_values_are_rejected() {
    let service = SpatialService::new([BvhEntry {
        object: 0,
        bounds: point([0; 3]),
        luminosity: 1.0,
    }]);
    for value in [-1.0, f64::NAN, f64::NEG_INFINITY] {
        assert!(std::panic::catch_unwind(|| service.sphere_candidates([0; 3], value)).is_err());
    }
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            std::panic::catch_unwind(|| service.visibility_candidates([0; 3], 0.0, value)).is_err()
        );
        assert!(
            std::panic::catch_unwind(|| service.nearest([0; 3], 1.0, 1, |_| Some(value))).is_err()
        );
        assert!(
            std::panic::catch_unwind(|| service.first_hit([0; 3], [0; 3], |_| Some(value)))
                .is_err()
        );
    }
    assert!(
        std::panic::catch_unwind(|| SpatialService::new([]).rebuild_dynamic([DynamicEntry {
            object: 1,
            bounds: point([1; 3]),
            swept_bounds: point([0; 3]),
            luminosity: 1.0,
        }]))
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| service.aabb_candidates(Aabb {
            min: [1; 3],
            max: [0; 3]
        }))
        .is_err()
    );
}
