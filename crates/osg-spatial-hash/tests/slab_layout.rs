use osg_spatial_hash::{LuminosityMap, NeighborMap, Position};
#[test]
fn neighbor_updates_and_reused_slots_match_scan() {
    let mut slab = NeighborMap::new();
    let origin = Position { x: 0, y: 0, z: 0 };
    for (id, position) in [
        (1, origin),
        (2, origin),
        (1, Position { x: 1, ..origin }),
        (
            1,
            Position {
                x: i64::MAX,
                y: -7,
                z: 9,
            },
        ),
    ] {
        slab.insert(id, position);
    }

    slab.remove(&2);

    slab.insert(3, Position { x: 3, y: 4, z: 0 });
    for centre in [
        origin,
        Position {
            x: i64::MAX,
            y: -7,
            z: 9,
        },
    ] {
        for radius in [-1, 0, 4, 5, 1 << 40, i64::MAX] {
            let records = [
                (
                    1,
                    Position {
                        x: i64::MAX,
                        y: -7,
                        z: 9,
                    },
                ),
                (3, Position { x: 3, y: 4, z: 0 }),
            ];
            let mut expected: Vec<_> = records
                .iter()
                .filter(|(_, p)| {
                    radius >= 0 && distance_squared(centre, *p) <= (radius as f64).powi(2)
                })
                .map(|(id, p)| (id, p))
                .collect();
            let mut actual: Vec<_> = slab.nearest(centre, radius).collect();
            let mut finer: Vec<_> = slab.nearest_finer(centre, radius).collect();
            expected.sort_unstable();
            actual.sort_unstable();
            finer.sort_unstable();
            assert_eq!(actual, expected);
            assert_eq!(finer, expected);
        }
    }
}

#[test]
fn luminosity_migration_removal_and_extremes_match_scan() {
    let mut slab = LuminosityMap::new();
    let origin = Position { x: 0, y: 0, z: 0 };
    for (id, p, b) in [
        (1, origin, 0.0),
        (2, origin, 0.01),
        (3, Position { x: 3, y: 4, z: 0 }, 25.0),
        (
            4,
            Position {
                x: i64::MAX,
                ..origin
            },
            f64::MAX,
        ),
        (3, origin, 0.5),
        (3, Position { x: 2, ..origin }, 0.75),
    ] {
        slab.insert(id, p, b);
    }

    slab.remove(&2);
    slab.insert(
        5,
        Position {
            x: i64::MIN,
            ..origin
        },
        f64::MAX,
    );
    for p in [
        origin,
        Position {
            x: i64::MIN,
            ..origin
        },
    ] {
        for threshold in [0.01, 0.25, 1.0, f64::MAX] {
            let records = [
                (1, origin, 0.0),
                (3, Position { x: 2, ..origin }, 0.75),
                (
                    4,
                    Position {
                        x: i64::MAX,
                        ..origin
                    },
                    f64::MAX,
                ),
                (
                    5,
                    Position {
                        x: i64::MIN,
                        ..origin
                    },
                    f64::MAX,
                ),
            ];
            let mut expected: Vec<_> = records
                .iter()
                .filter(|(_, position, brightness)| {
                    *brightness > 0.0 && brightness / distance_squared(p, *position) >= threshold
                })
                .map(|(id, _, _)| id)
                .collect();
            let mut actual: Vec<_> = slab.nearest_visible(p, threshold).collect();
            let mut finer: Vec<_> = slab.nearest_visible_finer(p, threshold).collect();
            expected.sort_unstable();
            actual.sort_unstable();
            finer.sort_unstable();
            assert_eq!(actual, expected);
            assert_eq!(finer, expected);
        }
    }
}

fn distance_squared(a: Position, b: Position) -> f64 {
    let dx = a.x.abs_diff(b.x) as f64;
    let dy = a.y.abs_diff(b.y) as f64;
    let dz = a.z.abs_diff(b.z) as f64;
    dx * dx + dy * dy + dz * dz
}
