use osg_spatial_hash::{LuminosityMap, Position};

#[test]
fn incremental_queries_match_scan_including_dark_records() {
    for shift in [0, 9, 63] {
        check_incremental_queries(shift);
    }
}

fn check_incremental_queries(shift: u32) {
    let mut map = LuminosityMap::with_minimum_cell_shift(shift);
    let mut records = Vec::new();
    for i in 0..200_i64 {
        let p = Position {
            x: i * 13 - 1000,
            y: i % 17 - 8,
            z: i % 23 - 11,
        };
        let brightness = if i % 4 == 0 {
            0.0
        } else {
            2_f64.powi((i % 80) as i32 - 10)
        };
        map.insert(i, p, brightness);
        records.push((i, p, brightness));
    }
    for tick in 0..10 {
        for (id, p, brightness) in &mut records {
            p.x += tick - 5;
            if *id % 7 == 0 {
                *brightness *= 4.0;
            }
            map.insert(*id, *p, *brightness);
        }
        for radius in [-1, 0, 20, 100, 2000] {
            let centre = Position {
                x: -500,
                y: 0,
                z: 0,
            };
            let mut got: Vec<_> = map
                .within_radius(centre, radius)
                .map(|(&id, _)| id)
                .collect();
            let mut expected: Vec<_> = records
                .iter()
                .filter(|(_, p, _)| {
                    radius >= 0
                        && ((p.x - centre.x).pow(2) + p.y.pow(2) + p.z.pow(2)) as f64
                            <= (radius as f64).powi(2)
                })
                .map(|(id, _, _)| *id)
                .collect();
            got.sort();
            expected.sort();
            assert_eq!(got, expected);
        }
        let centre = Position {
            x: -500,
            y: 0,
            z: 0,
        };
        for threshold in [0.001, 1.0, 1000.0] {
            let mut expected: Vec<_> = records
                .iter()
                .filter(|(_, p, brightness)| {
                    let distance = ((p.x - centre.x).pow(2) + p.y.pow(2) + p.z.pow(2)) as f64;
                    *brightness > 0.0 && *brightness / distance >= threshold
                })
                .map(|(id, _, _)| *id)
                .collect();
            expected.sort();
            for mut actual in [
                map.nearest_visible(centre, threshold)
                    .copied()
                    .collect::<Vec<_>>(),
                map.nearest_visible_finer(centre, threshold)
                    .copied()
                    .collect::<Vec<_>>(),
            ] {
                actual.sort();
                assert_eq!(actual, expected);
            }
        }
    }
    for (id, _, _) in &records {
        map.remove(id);
    }
    assert_eq!(
        map.within_radius(Position { x: 0, y: 0, z: 0 }, i64::MAX)
            .count(),
        0
    );
}

#[test]
fn visibility_padding_retains_quantized_boundary_sources() {
    let mut map = LuminosityMap::new();
    map.insert(1, Position { x: 11, y: 0, z: 0 }, 100.0);
    map.insert(2, Position { x: 0, y: 0, z: 0 }, 0.0);
    let centre = Position { x: 0, y: 0, z: 0 };
    assert_eq!(map.nearest_visible(centre, 1.0).count(), 0);
    assert_eq!(
        map.visible_candidates(centre, 1.0, 2.0)
            .copied()
            .collect::<Vec<_>>(),
        vec![1]
    );
    map.insert(1, centre, 0.0);
    assert_eq!(map.visible_candidates(centre, 1.0, 2.0).count(), 0);
}
