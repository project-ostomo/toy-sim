use std::{
    hint::black_box,
    path::Path,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use osg_spatial_hash::{LuminosityMap, NeighborMap, Position};

const STAR_COUNT: usize = 1_000_000;
const KM_PER_PARSEC: i64 = 30_856_775_814_914;
const LUMINOSITY_SCALE: f64 = (1_u128 << 64) as f64;
const EARTH: Position = Position { x: 0, y: 0, z: 0 };
const RADII: [(&str, i64); 6] = [
    ("1000_km", 1_000),
    ("500_au", 74_798_935_350),
    ("1_pc", KM_PER_PARSEC),
    ("10_pc", 10 * KM_PER_PARSEC),
    ("100_pc", 100 * KM_PER_PARSEC),
    ("1000_pc", 1_000 * KM_PER_PARSEC),
];

#[derive(Clone, Copy)]
struct Star {
    id: u64,
    position: Position,
    brightness: f64,
}

fn magnitude_threshold(magnitude: f64) -> f64 {
    // Same calibration as toy-sim: luminosity / distance², omitting 4π.
    let parsec_km: f64 = 3.085_677_581_491_367e13;
    3.6e28 / LUMINOSITY_SCALE / (10.0 * parsec_km).powi(2) * 10_f64.powf((4.83 - magnitude) / 2.5)
}

fn load_stars() -> Vec<Star> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../osg-stars/data/gaia-dr3-earth-million.stars");
    let bytes = std::fs::read(path).expect("read Gaia catalogue; run `git lfs pull` first");
    assert!(
        bytes.starts_with(b"OSGSTAR\0"),
        "expected the Gaia binary, not an LFS pointer; run `git lfs pull`"
    );
    assert_eq!(
        bytes.len(),
        40 + STAR_COUNT * 84,
        "unexpected catalogue size"
    );
    let u32_at = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let u64_at = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    assert_eq!(u32_at(8), 2, "expected OSGSTAR version 2");
    assert_eq!(u32_at(12), 84, "unexpected record size");
    assert_eq!(u64_at(16), STAR_COUNT as u64, "expected one million stars");
    assert_eq!(u32_at(24), 1, "expected micrometre coordinates");
    assert_eq!(u32_at(28), 1, "expected ICRS J2016.0 frame");
    assert_eq!(u64_at(32), 1, "expected Gaia DR3 IDs");

    bytes[40..]
        .chunks_exact(84)
        .map(|record| {
            let coordinate = |offset| {
                let micrometres =
                    i128::from_le_bytes(record[offset..offset + 16].try_into().unwrap());
                i64::try_from(micrometres / 1_000_000_000)
                    .expect("catalogue coordinate does not fit in i64 kilometres")
            };
            let id = u64::from_le_bytes(record[..8].try_into().unwrap());
            assert!(
                id < 1 << 63,
                "upper-half IDs are reserved for random inserts"
            );
            let brightness =
                f64::from_le_bytes(record[56..64].try_into().unwrap()) / LUMINOSITY_SCALE;
            assert!(brightness.is_finite() && brightness > 0.0);
            Star {
                id,
                position: Position {
                    x: coordinate(8),
                    y: coordinate(24),
                    z: coordinate(40),
                },
                brightness,
            }
        })
        .collect()
}

fn benchmark_neighbors(c: &mut Criterion, stars: &[Star], random_points: &[Star], name: &str) {
    let mut map = None;
    let build = || {
        eprintln!("Loading {STAR_COUNT} Gaia stars into {name} once...");
        let memory_before = std::env::var_os("GAIA_REPORT_MEMORY").map(|_| resident_kib());
        let mut map = NeighborMap::new();
        for star in stars {
            map.insert(star.id, star.position);
        }
        report_memory(name, memory_before);
        map
    };
    // Spread the query centres throughout the magnitude-ordered catalogue.
    let star_centres: Vec<_> = (0..64)
        .map(|index| stars[(index * 104_729) % STAR_COUNT].position)
        .collect();

    let mut queries = c.benchmark_group(format!("{name}/query"));
    queries.sample_size(30);
    queries.warm_up_time(Duration::from_secs(1));
    queries.measurement_time(Duration::from_secs(3));
    queries.throughput(Throughput::Elements(1));
    for (location, centres) in [("earth", &[EARTH][..]), ("stars", star_centres.as_slice())] {
        for (label, radius) in RADII {
            for finer_cells in [false, true] {
                let suffix = if finer_cells {
                    "_finer_precise"
                } else {
                    "_precise"
                };
                let label = format!("{label}{suffix}");
                let mut reported = false;
                let mut index = 0;
                queries.bench_with_input(
                    BenchmarkId::new(location, &label),
                    &radius,
                    |b, &radius| {
                        let map = map.get_or_insert_with(build);
                        if !reported {
                            // Check IDs and borrowed positions against an independent scan.
                            for &centre in centres {
                                let mut expected: Vec<_> = stars
                                    .iter()
                                    .filter(|star| {
                                        let dx = centre.x.abs_diff(star.position.x) as f64;
                                        let dy = centre.y.abs_diff(star.position.y) as f64;
                                        let dz = centre.z.abs_diff(star.position.z) as f64;
                                        dx * dx + dy * dy + dz * dz <= (radius as f64).powi(2)
                                    })
                                    .map(|star| (star.id, star.position))
                                    .collect();
                                let mut actual: Vec<_> = if finer_cells {
                                    map.nearest_finer(centre, radius)
                                        .map(|(&id, &p)| (id, p))
                                        .collect()
                                } else {
                                    map.nearest(centre, radius)
                                        .map(|(&id, &p)| (id, p))
                                        .collect()
                                };
                                expected.sort_unstable();
                                actual.sort_unstable();
                                assert_eq!(actual, expected);
                            }
                            let candidates: usize = centres
                                .iter()
                                .map(|&centre| {
                                    if finer_cells {
                                        map.nearest_finer(centre, radius).count()
                                    } else {
                                        map.nearest(centre, radius).count()
                                    }
                                })
                                .sum();
                            eprintln!(
                                "{location}/{label}: {:.1} returned stars/query over {} centres",
                                candidates as f64 / centres.len() as f64,
                                centres.len()
                            );
                            reported = true;
                        }
                        if finer_cells {
                            b.iter(|| {
                                let centre = centres[index];
                                index = (index + 1) % centres.len();
                                for result in
                                    map.nearest_finer(black_box(centre), black_box(radius))
                                {
                                    black_box(result);
                                }
                            });
                        } else {
                            b.iter(|| {
                                let centre = centres[index];
                                index = (index + 1) % centres.len();
                                // Consume every borrowed item and position within the radius.
                                for result in map.nearest(black_box(centre), black_box(radius)) {
                                    black_box(result);
                                }
                            });
                        }
                    },
                );
            }
        }
    }
    queries.finish();

    let mut insertion = c.benchmark_group(format!("{name}/insert"));
    insertion.throughput(Throughput::Elements(1));
    insertion.bench_function("random_into_gaia_1m", |b| {
        let map = map.get_or_insert_with(build);
        b.iter_custom(|iterations| {
            time_insertions(
                iterations,
                map,
                random_points,
                |map, star| map.insert(star.id, star.position),
                |map, id| map.remove(id),
            )
        });
    });
    insertion.finish();
}

fn random_points(stars: &[Star]) -> Vec<Star> {
    let mut random = oorandom::Rand64::new(20260921);
    (0..4096)
        .map(|index| {
            let star = stars[random.rand_range(0..stars.len() as u64) as usize];
            let mut jitter =
                || random.rand_range(0..(2 * KM_PER_PARSEC + 1) as u64) as i64 - KM_PER_PARSEC;
            Star {
                id: u64::MAX - index,
                position: Position {
                    x: star.position.x.saturating_add(jitter()),
                    y: star.position.y.saturating_add(jitter()),
                    z: star.position.z.saturating_add(jitter()),
                },
                brightness: star.brightness,
            }
        })
        .collect()
}

fn time_insertions<M>(
    iterations: u64,
    map: &mut M,
    points: &[Star],
    mut insert: impl FnMut(&mut M, &Star),
    mut remove: impl FnMut(&mut M, &u64),
) -> Duration {
    let mut elapsed = Duration::ZERO;
    let mut completed = 0;
    while completed < iterations {
        let start = (completed % points.len() as u64) as usize;
        let count = (iterations - completed).min(256) as usize;
        let batch = &points[start..start + count];
        let timer = Instant::now();
        for star in batch {
            insert(black_box(map), black_box(star));
        }
        elapsed += timer.elapsed();
        // Restore the million-star population without rebuilding it or timing removal.
        for star in batch {
            remove(map, &star.id);
        }
        completed += count as u64;
    }
    elapsed
}

fn benchmark_luminosity(c: &mut Criterion, stars: &[Star], random_points: &[Star], name: &str) {
    let mut map = None;
    let build = || {
        eprintln!("Loading {STAR_COUNT} Gaia stars into {name} once...");
        let memory_before = std::env::var_os("GAIA_REPORT_MEMORY").map(|_| resident_kib());
        let mut map = LuminosityMap::new();
        for star in stars {
            map.insert(star.id, star.position, star.brightness);
        }
        report_memory(name, memory_before);
        map
    };
    let mut queries = c.benchmark_group(format!("{name}/query"));
    queries.throughput(Throughput::Elements(1));
    for magnitude in [0, 2, 4, 6] {
        let threshold = magnitude_threshold(f64::from(magnitude));
        let mut validated = false;
        for (finer_cells, suffix) in [(false, "_precise"), (true, "_finer_precise")] {
            queries.bench_function(format!("earth_magnitude_{magnitude}{suffix}"), |b| {
                let map = map.get_or_insert_with(build);
                if !validated {
                    let mut expected: Vec<_> = stars
                        .iter()
                        .filter_map(|star| {
                            let x = star.position.x as f64;
                            let y = star.position.y as f64;
                            let z = star.position.z as f64;
                            (star.brightness / (x * x + y * y + z * z) >= threshold)
                                .then_some(star.id)
                        })
                        .collect();
                    let mut actual: Vec<_> = map.nearest_visible(EARTH, threshold).copied().collect();
                    let mut finer: Vec<_> = map
                        .nearest_visible_finer(EARTH, threshold)
                        .copied()
                        .collect();
                    expected.sort_unstable();
                    actual.sort_unstable();
                    finer.sort_unstable();
                    assert_eq!(actual, expected, "indexed visibility differs from full scan");
                    assert_eq!(finer, expected, "finer visibility differs from full scan");
                    eprintln!(
                        "Earth, magnitude <= {magnitude}: {} visible stars; verified against full scan",
                        actual.len()
                    );
                    validated = true;
                }
                if finer_cells {
                    b.iter(|| {
                        for star in map.nearest_visible_finer(black_box(EARTH), black_box(threshold)) {
                            black_box(star);
                        }
                    });
                } else {
                    b.iter(|| {
                        for star in map.nearest_visible(black_box(EARTH), black_box(threshold)) {
                            black_box(star);
                        }
                    });
                }
            });
        }
    }
    queries.finish();

    let mut insertion = c.benchmark_group(format!("{name}/insert"));
    insertion.throughput(Throughput::Elements(1));
    insertion.bench_function("random_into_gaia_1m", |b| {
        let map = map.get_or_insert_with(build);
        b.iter_custom(|iterations| {
            time_insertions(
                iterations,
                map,
                random_points,
                |map, star| {
                    map.insert(star.id, star.position, star.brightness);
                },
                |map, id| map.remove(id),
            )
        });
    });
    insertion.finish();
}

fn benchmark(c: &mut Criterion) {
    let stars = load_stars();
    // Run each memory measurement in a fresh process, without timing queries.
    if let Ok(layout) = std::env::var("GAIA_MEMORY") {
        let before = resident_kib();
        macro_rules! measure {
            ($map:ident, $($brightness:ident)?) => {{
                let mut map = $map::new();
                for star in &stars { map.insert(star.id, star.position $(, star.$brightness)?); }
                black_box(&map);
                let after = resident_kib();
                eprintln!("{layout}: RSS before={before} KiB, after={after} KiB, index delta={} KiB", after - before);
            }};
        }
        match layout.as_str() {
            "neighbor" => measure!(NeighborMap,),
            "luminosity" => measure!(LuminosityMap, brightness),
            _ => panic!("unknown GAIA_MEMORY layout"),
        }
        return;
    }
    let random_points = random_points(&stars);
    benchmark_neighbors(c, &stars, &random_points, "neighbor_map");
    benchmark_luminosity(c, &stars, &random_points, "luminosity_map");
}

fn report_memory(name: &str, before: Option<u64>) {
    if let Some(before) = before {
        let after = resident_kib();
        eprintln!(
            "{name}: RSS before={before} KiB, after={after} KiB, index delta={} KiB",
            after - before
        );
    }
}

fn resident_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("Linux RSS measurement");
    status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = benchmark
}
criterion_main!(benches);
