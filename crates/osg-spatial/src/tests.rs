use super::*;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha20Rng;

fn random_entry(rng: &mut ChaCha20Rng, origin: GalacticPosition) -> Entry {
    Entry {
        position: origin.offset_by(DVec3::new(
            rng.random_range(-1e10..1e10),
            rng.random_range(-1e10..1e10),
            rng.random_range(-1e10..1e10),
        )),
        radius_m: 10.0_f64.powf(rng.random_range(0.0..10.0)),
        luminosity: 10.0_f64.powf(rng.random_range(-4.0..30.0)),
    }
}

#[test]
fn parallel_updates_match_rebuilt_indexes_after_motion_and_removals() {
    let origin = GalacticPosition::new(1 << 100, -(1 << 100), 0);
    let mut index = SpatialHash::default();
    let mut expected = AHashMap::default();
    for id in 0..256 {
        let entry = Entry {
            position: origin.offset_by(DVec3::new(
                (id % 16) as f64 * 200_000.0 - 1_500_000.0,
                (id / 16) as f64 * 200_000.0 - 1_500_000.0,
                100.0,
            )),
            radius_m: 10.0,
            luminosity: 1e12,
        };
        index.insert(id, entry);
        expected.insert(id, entry);
    }

    for step in 0..8 {
        let mut cursor = index.range_cursor(origin, 1e7, true);
        expected.retain(|&id, entry| {
            if id % 31 == step {
                return false;
            }
            let displacement = if id % 3 == 0 { 800_000.0 } else { 1.0 };
            entry.position = entry.position.offset_by(DVec3::X * displacement);
            if id % 5 == 0 {
                entry.radius_m = if step % 2 == 0 { 1e6 } else { 1.0 };
                entry.luminosity = if step % 2 == 0 { 0.0 } else { 1e18 };
            }
            true
        });
        let removed = index.update_entries(|id, _| expected.get(&id).copied());
        assert!(removed.iter().all(|id| !expected.contains_key(id)));
        assert_eq!(index.len(), expected.len());
        assert!(index.advance_range(&mut cursor, 1, 1).invalidated);

        let mut rebuilt = SpatialHash::default();
        for (&id, &entry) in &expected {
            rebuilt.insert(id, entry);
            // Exact positions must advance even when tree membership is retained.
            assert_eq!(index.within_radius(entry.position, 0.0).ids, vec![id]);
        }
        for radius in [1.0, 300_000.0, 2e6, 1e7] {
            assert_eq!(
                index.intersecting_sphere(origin, radius).ids,
                rebuilt.intersecting_sphere(origin, radius).ids
            );
            assert_eq!(
                index.nearest(origin, radius, 20),
                rebuilt.nearest(origin, radius, 20)
            );
        }
        for threshold in [1e-8, 1.0, 1e6] {
            assert_eq!(
                index.visible(origin, threshold).ids,
                rebuilt.visible(origin, threshold).ids
            );
        }
    }
}

#[test]
fn extended_light_sources_include_off_centre_companions() {
    let observer = GalacticPosition::new(1_i128 << 100, -(1_i128 << 100), -1);
    let mut index = SpatialHash::default();
    index.insert(
        0,
        Entry {
            position: observer.offset_by(DVec3::X * 1e12),
            radius_m: 1e12 - 1e6,
            luminosity: 1e6,
        },
    );
    assert!(index.visible(observer, 1e-8).ids.is_empty());
    assert_eq!(index.extended_sources(observer, 0., 1e-8), vec![0]);
    assert!(
        index
            .extended_sources(observer.offset_by(-DVec3::X * 1e9), 0., 1e-8)
            .is_empty()
    );
    assert_eq!(
        index.extended_sources(observer.offset_by(-DVec3::X * 1e9), 1e9, 1e-8),
        vec![0]
    );
}

#[test]
fn range_visibility_and_nearest_match_brute_force_at_galactic_coordinates() {
    let origin = GalacticPosition::new(1_i128 << 105, -(1_i128 << 105), -1);
    let mut rng = ChaCha20Rng::seed_from_u64(1234);
    let mut index = SpatialHash::default();
    for id in 0..4000 {
        index.insert(id, random_entry(&mut rng, origin));
    }
    for displacement in [DVec3::ZERO, DVec3::splat(-1e10), DVec3::X * 1e10] {
        let observer = origin.offset_by(displacement);
        for radius in [0.0, 1e-6, 1e6, 1e8, 1e9, 1e10, 1e25] {
            for extents in [false, true] {
                let mut found = index.range(observer, radius, extents).ids;
                let mut expected: Vec<_> = index
                    .entries
                    .iter()
                    .filter_map(|(&id, entry)| {
                        let limit = radius + if extents { entry.radius_m } else { 0.0 };
                        (distance_squared(entry.position, observer) <= limit * limit).then_some(id)
                    })
                    .collect();
                found.sort_unstable();
                expected.sort_unstable();
                assert_eq!(found, expected);
            }
            let found = index.nearest(observer, radius, 10);
            let mut expected: Vec<_> = index
                .entries
                .iter()
                .filter_map(|(&id, entry)| {
                    (distance_squared(entry.position, observer) <= radius * radius).then_some(id)
                })
                .collect();
            expected.sort_unstable_by(|a, b| {
                distance_squared(index.entries[a].position, observer)
                    .total_cmp(&distance_squared(index.entries[b].position, observer))
                    .then_with(|| a.cmp(b))
            });
            expected.truncate(10);
            assert_eq!(found, expected);
        }
        for threshold in [0.0, 1e-20, 1e-8, 1.0, 1e10] {
            let mut found = index.visible(observer, threshold).ids;
            let mut expected: Vec<_> = index
                .entries
                .iter()
                .filter_map(|(&id, entry)| {
                    (entry.luminosity / distance_squared(entry.position, observer) >= threshold)
                        .then_some(id)
                })
                .collect();
            found.sort_unstable();
            expected.sort_unstable();
            assert_eq!(found, expected);
        }
        let expected = index
            .entries
            .iter()
            .max_by(|a, b| {
                (a.1.luminosity / distance_squared(a.1.position, observer).max(1.0)).total_cmp(
                    &(b.1.luminosity / distance_squared(b.1.position, observer).max(1.0)),
                )
            })
            .unwrap()
            .0;
        assert_eq!(index.brightest(observer), Some(*expected));
    }
}

#[test]
fn changing_brightness_position_radius_and_removing_objects_updates_all_queries() {
    let origin = GalacticPosition::splat(-(1_i128 << 105));
    let mut index = SpatialHash::default();
    for id in 0..1000 {
        index.insert(
            id,
            Entry {
                position: origin.offset_by(DVec3::new(id as f64, -1.0, 0.0)),
                radius_m: 1.0,
                luminosity: 0.0,
            },
        );
    }
    assert!(index.visible(origin, 1.0).ids.is_empty());
    index.set_luminosity(500, 1e30);
    assert_eq!(index.visible(origin, 1.0).ids, vec![500]);
    index.set_luminosity(500, 1e-30);
    assert!(index.visible(origin, 1.0).ids.is_empty());
    index.insert(
        500,
        Entry {
            position: origin,
            radius_m: 1e20,
            luminosity: 1e10,
        },
    );
    assert!(
        index
            .intersecting_sphere(origin.offset_by(DVec3::Y * 1e15), 1.0)
            .ids
            .contains(&500)
    );
    index.remove(500);
    assert!(
        index
            .intersecting_sphere(origin.offset_by(DVec3::Y * 1e15), 1.0)
            .ids
            .is_empty()
    );
    for id in 0..1000 {
        index.remove(id);
    }
    assert!(index.is_empty());
    assert_eq!(index.occupied_cells(), 0);
    assert_eq!(index.bucket_count(), 0);
}

#[test]
fn boundaries_and_coincident_objects_keep_micrometre_precision() {
    let origin = GalacticPosition::new(1_i128 << 90, -(1_i128 << 90), 0);
    let mut index = SpatialHash::default();
    for id in 0..100 {
        index.insert(
            id,
            Entry {
                position: origin,
                radius_m: 0.0,
                luminosity: 1.0,
            },
        );
    }
    index.insert(
        100,
        Entry {
            position: origin + GalacticPosition::X,
            radius_m: 0.0,
            luminosity: 1.0,
        },
    );
    assert_eq!(index.within_radius(origin, 0.0).ids.len(), 100);
    assert_eq!(index.within_radius(origin, 1e-6).ids.len(), 101);
    assert_eq!(
        index.nearest(origin + GalacticPosition::X, 1.0, 1),
        vec![100]
    );
}

#[test]
fn sparse_galactic_range_queries_prune_distant_population() {
    let mut index = SpatialHash::default();
    for id in 0..100_000 {
        index.insert(
            id,
            Entry {
                position: GalacticPosition::from_meters(DVec3::new(
                    (id % 100) as f64 * 1e17,
                    (id / 100 % 100) as f64 * 1e17,
                    (id / 10_000) as f64 * 1e17,
                )),
                radius_m: 100.0,
                luminosity: 1e9,
            },
        );
    }
    let result = index.within_radius(GalacticPosition::ZERO, 500.0 * 149_597_870_700.0);
    assert_eq!(result.ids, vec![0]);
    assert!(
        result.stats.candidates < 100,
        "{} candidates",
        result.stats.candidates
    );
    assert!(
        result.stats.cells_visited < 300,
        "{} cells",
        result.stats.cells_visited
    );
    let result = index.visible(GalacticPosition::ZERO, 1e-12);
    assert_eq!(result.ids, vec![0]);
    assert!(result.stats.candidates < 100);
}

#[test]
fn neighborhood_visibility_is_conservative_for_every_observer_in_sphere() {
    let mut rng = ChaCha20Rng::seed_from_u64(100);
    let origin = GalacticPosition::splat(1_i128 << 95);
    let mut index = SpatialHash::default();
    for id in 0..1000 {
        index.insert(id, random_entry(&mut rng, origin));
    }
    let radius = 1e9;
    for threshold in [1e-10, 1.0, 1e10] {
        let found: std::collections::BTreeSet<_> = index
            .visible_from_sphere(origin, radius, threshold)
            .ids
            .into_iter()
            .collect();
        for direction in [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::Z,
            DVec3::ONE.normalize(),
        ] {
            let observer = origin.offset_by(direction * radius);
            assert!(
                index
                    .visible(observer, threshold)
                    .ids
                    .into_iter()
                    .all(|id| found.contains(&id))
            );
        }
        let expected: std::collections::BTreeSet<_> = index
            .entries
            .iter()
            .filter_map(|(&id, entry)| {
                let distance = (entry.position.relative_to(origin).length() - radius).max(0.0);
                (entry.luminosity / distance.powi(2) >= threshold).then_some(id)
            })
            .collect();
        assert_eq!(found, expected);
    }
}

#[test]
fn very_small_and_large_luminosities_do_not_underflow_bucket_bounds() {
    let mut index = SpatialHash::default();
    let minimum = f64::from_bits(1);
    for (id, luminosity) in [(0, minimum), (1, f64::MAX)] {
        index.insert(
            id,
            Entry {
                position: GalacticPosition::from_meters(DVec3::X),
                radius_m: 0.0,
                luminosity,
            },
        );
    }
    let mut found = index.visible(GalacticPosition::ZERO, minimum).ids;
    found.sort_unstable();
    assert_eq!(found, vec![0, 1]);
    assert_eq!(index.visible(GalacticPosition::ZERO, f64::MAX).ids, vec![1]);
}

#[test]
fn moving_population_and_retained_snapshot_keep_independent_correct_indexes() {
    let mut rng = ChaCha20Rng::seed_from_u64(876);
    let origin = GalacticPosition::splat(-(1_i128 << 105));
    let mut index = SpatialHash::default();
    for id in 0..1000 {
        index.insert(id, random_entry(&mut rng, origin));
    }
    let previous = index.clone();
    let old_visible = previous.visible(origin, 1e-9).ids;
    for change in 0..2000 {
        let id = rng.random_range(0..1000);
        if change % 5 == 0 {
            index.remove(id);
        } else {
            index.insert(id, random_entry(&mut rng, origin));
        }
        if change % 100 == 0 {
            let mut actual = index.visible(origin, 1e-9).ids;
            let mut expected: Vec<_> = index
                .entries
                .iter()
                .filter_map(|(&id, entry)| {
                    (entry.luminosity / distance_squared(entry.position, origin) >= 1e-9)
                        .then_some(id)
                })
                .collect();
            actual.sort_unstable();
            expected.sort_unstable();
            assert_eq!(actual, expected);
            assert_eq!(previous.visible(origin, 1e-9).ids, old_visible);
        }
    }
}

#[test]
fn full_signed_coordinate_range_does_not_saturate_distances() {
    let low = GalacticPosition::splat(i128::MIN);
    let high = GalacticPosition::splat(i128::MAX);
    let span_m = u128::MAX as f64 / 1_000_000.0;
    assert!((distance_squared(low, high) / (3.0 * span_m * span_m) - 1.0).abs() < 1e-15);
    let mut index = SpatialHash::default();
    for (id, position) in [low, high, GalacticPosition::ZERO].into_iter().enumerate() {
        index.insert(
            id as u32,
            Entry {
                position,
                radius_m: 1.0,
                luminosity: 1e64,
            },
        );
    }
    assert_eq!(index.within_radius(low, span_m).ids, vec![0, 2]);
    assert_eq!(index.within_radius(low, span_m * 2.0).ids, vec![0, 1, 2]);
    assert_eq!(index.nearest(high, f64::INFINITY, 3), vec![1, 2, 0]);
    assert_eq!(index.visible(low, 0.1).ids, vec![0, 2]);
    let result = index.segment_candidates(low, DVec3::splat(span_m), 1.0);
    assert_eq!(result.ids, vec![0, 1, 2]);
}

#[test]
fn capsules_match_brute_force_after_mutations_and_at_boundary_tangencies() {
    let mut rng = ChaCha20Rng::seed_from_u64(151);
    let origin = GalacticPosition::splat(1_i128 << 110);
    let mut index = SpatialHash::default();
    for id in 0..3000 {
        index.insert(id, random_entry(&mut rng, origin));
    }
    for _ in 0..100 {
        let start = origin.offset_by(DVec3::new(
            rng.random_range(-1e10..1e10),
            rng.random_range(-1e10..1e10),
            rng.random_range(-1e10..1e10),
        ));
        let displacement = DVec3::new(
            rng.random_range(-1e10..1e10),
            rng.random_range(-1e10..1e10),
            rng.random_range(-1e10..1e10),
        );
        let radius = rng.random_range(0.0..1e8);
        let found = index.segment_candidates(start, displacement, radius).ids;
        let mut expected: Vec<_> = index
            .entries
            .iter()
            .filter_map(|(&id, entry)| {
                let offset = entry.position.relative_to(start);
                let t = (offset.dot(displacement) / displacement.length_squared()).clamp(0.0, 1.0);
                ((offset - t * displacement).length_squared() <= (radius + entry.radius_m).powi(2))
                    .then_some(id)
            })
            .collect();
        expected.sort_unstable();
        assert_eq!(found, expected);

        let (hit, _) = index.visit_segment_candidates(start, displacement, radius, |id| {
            if id % 17 == 0 {
                ControlFlow::Break(id)
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(hit.is_break(), expected.iter().any(|id| id % 17 == 0));
        if let ControlFlow::Break(id) = hit {
            assert!(expected.contains(&id));
        }
    }
    index.clear();
    for (id, x) in [-1.0, 5.0, 11.0].into_iter().enumerate() {
        index.insert(
            id as u32,
            Entry {
                position: origin.offset_by(DVec3::new(x, if id == 1 { 1.0 } else { 0.0 }, 0.0)),
                radius_m: 0.5,
                luminosity: 0.0,
            },
        );
    }
    assert_eq!(
        index.segment_candidates(origin, DVec3::X * 10.0, 0.5).ids,
        vec![0, 1, 2]
    );
    assert_eq!(
        index.segment_candidates(origin, DVec3::ZERO, 0.5).ids,
        vec![0]
    );
    assert!(
        index
            .segment_candidates(origin, DVec3::NAN, 0.5)
            .ids
            .is_empty()
    );
}

#[test]
fn segment_visitor_stops_at_confirmed_hit_inside_a_dense_leaf() {
    let origin = GalacticPosition::splat(-(1_i128 << 100));
    let mut index = SpatialHash::default();
    for id in 0..2048 {
        index.insert(
            id,
            Entry {
                position: origin.offset_by(DVec3::X),
                radius_m: 1.0,
                luminosity: 0.0,
            },
        );
    }

    let mut visited = Vec::new();
    let (hit, stats) = index.visit_segment_candidates(origin, DVec3::X * 10.0, 0.0, |id| {
        visited.push(id);
        if id == 1 {
            ControlFlow::Break(id)
        } else {
            ControlFlow::Continue(())
        }
    });
    assert_eq!(hit, ControlFlow::Break(1));
    assert_eq!(visited, vec![0, 1]);
    assert_eq!(stats.candidates, 2);
    assert_eq!(
        index
            .segment_candidates(origin, DVec3::X * 10.0, 0.0)
            .ids
            .len(),
        2048
    );

    for (displacement, radius) in [(DVec3::NAN, 0.0), (DVec3::X, -1.0)] {
        let (hit, stats) =
            index.visit_segment_candidates::<()>(origin, displacement, radius, |_| {
                panic!("invalid segment must not invoke the visitor")
            });
        assert!(hit.is_continue());
        assert_eq!(stats.cells_visited, 0);
        assert_eq!(stats.candidates, 0);
    }
}

#[test]
fn cursors_bound_coincident_leaf_work_and_preserve_every_result() {
    let mut index = SpatialHash::default();
    for id in 0..2000 {
        index.insert(
            id,
            Entry {
                position: GalacticPosition::ZERO,
                radius_m: 1.0,
                luminosity: 1.0,
            },
        );
    }
    for (work, results) in [(1, 1), (7, 2), (100, 3), (3, 100)] {
        let mut cursor = index.range_cursor(GalacticPosition::ZERO, 0.0, false);
        assert!(!index.advance_range(&mut cursor, 0, results).complete);
        assert!(!index.advance_range(&mut cursor, work, 0).complete);
        let mut actual = Vec::new();
        let mut total_work = 0;
        loop {
            let batch = index.advance_range(&mut cursor, work, results);
            assert!(!batch.invalidated);
            assert!(batch.stats.work() <= work);
            assert!(batch.ids.len() <= results);
            total_work += batch.stats.work();
            actual.extend(batch.ids);
            if batch.complete {
                break;
            }
        }
        actual.sort_unstable();
        assert_eq!(actual, (0..2000).collect::<Vec<_>>());
        assert!(total_work <= 2000 + index.occupied_cells());
    }
}

#[test]
fn paged_extent_queries_match_synchronous_queries_and_reject_mutated_indexes() {
    let mut rng = ChaCha20Rng::seed_from_u64(765);
    let origin = GalacticPosition::splat(-(1_i128 << 103));
    let mut index = SpatialHash::default();
    for id in 0..1000 {
        index.insert(id, random_entry(&mut rng, origin));
    }
    for extents in [false, true] {
        let mut cursor = index.range_cursor(origin, 1e9, extents);
        let mut actual = Vec::new();
        loop {
            let batch = index.advance_range(&mut cursor, 4, 2);
            assert!(batch.stats.work() <= 4);
            actual.extend(batch.ids);
            if batch.complete {
                break;
            }
        }
        actual.sort_unstable();
        assert_eq!(actual, index.range(origin, 1e9, extents).ids);
    }
    let cursor = index.range_cursor(origin, f64::INFINITY, false);
    let cloned = index.clone();
    assert!(cloned.advance_range(&mut cursor.clone(), 1, 1).invalidated);
    index.insert(0, *index.get(0).unwrap());
    assert!(!index.advance_range(&mut cursor.clone(), 1, 1).invalidated);
    index.set_luminosity(0, 0.0);
    assert!(index.advance_range(&mut cursor.clone(), 1, 1).invalidated);
}

#[test]
fn query_order_and_nearest_ties_do_not_depend_on_insertion_order() {
    let mut forward = SpatialHash::default();
    let mut reverse = SpatialHash::default();
    let entry = |id: u32| Entry {
        position: GalacticPosition::from_meters(DVec3::new(
            (id % 10) as f64,
            (id / 10) as f64,
            0.0,
        )),
        radius_m: 0.0,
        luminosity: if id % 2 == 0 { 1.0 } else { 1000.0 },
    };
    for id in 0..100 {
        forward.insert(id, entry(id));
    }
    for id in (0..100).rev() {
        reverse.insert(id, entry(id));
    }
    for radius in [0.0, 1.0, 2.0, 100.0] {
        assert_eq!(
            forward.within_radius(GalacticPosition::ZERO, radius).ids,
            reverse.within_radius(GalacticPosition::ZERO, radius).ids
        );
        assert_eq!(
            forward.nearest_filtered(GalacticPosition::ZERO, radius, 10, |_| Some(0)),
            reverse.nearest_filtered(GalacticPosition::ZERO, radius, 10, |_| Some(0))
        );
    }
    assert_eq!(
        forward.visible(GalacticPosition::ZERO, 0.01).ids,
        reverse.visible(GalacticPosition::ZERO, 0.01).ids
    );
}

#[test]
fn luminosity_bucket_bounds_enclose_every_finite_positive_exponent() {
    for bit in 0..63 {
        let value = f64::from_bits(1_u64 << bit);
        if value.is_finite() && value > 0.0 {
            assert!(luminosity_upper(luminosity_bucket(value).unwrap()) > value);
        }
    }
    for exponent in 1..2047_u64 {
        for mantissa in [0, 1, (1_u64 << 52) - 1] {
            let value = f64::from_bits((exponent << 52) | mantissa);
            let bucket = luminosity_bucket(value).unwrap();
            assert!(luminosity_upper(bucket) > value, "{value} bucket {bucket}");
        }
    }
}

#[test]
fn local_refits_preserve_extent_pruning_when_largest_radius_shrinks() {
    let origin = GalacticPosition::splat(1_i128 << 105);
    let mut index = SpatialHash::default();
    for id in 0..100 {
        index.insert(
            id,
            Entry {
                position: origin.offset_by(DVec3::X * id as f64),
                radius_m: 1000.0 + id as f64,
                luminosity: 1.0,
            },
        );
    }
    for id in (0..100).rev() {
        let mut entry = *index.get(id).unwrap();
        entry.position = entry.position.offset_by(DVec3::X * 0.000001);
        entry.radius_m = 0.0;
        index.insert(id, entry);
        let observer = origin.offset_by(DVec3::Y * 1050.0);
        let expected: Vec<_> = (0..100)
            .filter(|&candidate| {
                let entry = index.get(candidate).unwrap();
                distance_squared(entry.position, observer) <= entry.radius_m.powi(2)
            })
            .collect();
        assert_eq!(index.intersecting_sphere(observer, 0.0).ids, expected);
    }
}
