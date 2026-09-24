use std::{
    f64::consts::PI,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use osg_spatial_bvh::{LuminosityBvh, Position, SpatialService};

#[path = "support/index.rs"]
mod index;
mod support;
use index::Index;
use support::{LUMENS_PER_OPTICAL_WATT, Star};

const STAR_COUNT: usize = 1_000_000;
const EARTH: Position = [0; 3];

fn magnitude_threshold(magnitude: f64) -> f64 {
    let parsec_m: f64 = 3.085_677_581_491_367e16;
    3.6e28 / (LUMENS_PER_OPTICAL_WATT * 4.0 * PI * (10.0 * parsec_m).powi(2))
        * 10_f64.powf((4.83 - magnitude) / 2.5)
}

fn load_stars() -> Vec<Star> {
    let path = std::env::var_os("GAIA_STARS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../osg-stars/data/gaia-dr3-earth-million.stars")
        });
    let bytes =
        std::fs::read(path).expect("read Gaia catalogue from osg-stars/data or set GAIA_STARS");
    assert!(
        bytes.starts_with(b"OSGSTAR\0"),
        "expected Gaia binary, not an LFS pointer"
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
    assert_eq!(u64_at(16), STAR_COUNT as u64);
    assert_eq!(u32_at(24), 1, "expected micrometre coordinates");
    assert_eq!(u32_at(28), 1, "expected ICRS J2016.0 frame");
    assert_eq!(u64_at(32), 1, "expected Gaia DR3 IDs");

    bytes[40..]
        .chunks_exact(84)
        .map(|record| {
            let coordinate =
                |offset| i128::from_le_bytes(record[offset..offset + 16].try_into().unwrap());
            let id = u64::from_le_bytes(record[..8].try_into().unwrap());
            let luminosity_w =
                f64::from_le_bytes(record[56..64].try_into().unwrap()) / LUMENS_PER_OPTICAL_WATT;
            assert!(luminosity_w.is_finite() && luminosity_w > 0.0);
            Star {
                id,
                position: [coordinate(8), coordinate(24), coordinate(40)],
                luminosity_w,
            }
        })
        .collect()
}

fn check_query<M: Index>(map: &M, stars: &[Star], from: Position, threshold: f64) {
    let mut expected: Vec<_> = stars
        .iter()
        .filter_map(|star| {
            let distance_squared: f64 = star
                .position
                .into_iter()
                .zip(from)
                .map(|(a, b)| (a.abs_diff(b) as f64 / 1e6).powi(2))
                .sum();
            (distance_squared == 0.0
                || star.luminosity_w / (4.0 * PI * distance_squared) >= threshold)
                .then_some(star.id)
        })
        .collect();
    let mut actual: Vec<_> = map.visible(from, threshold).collect();
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(
        actual,
        expected,
        "{} visibility differs from full scan",
        M::NAME
    );
    eprintln!(
        "{}: verified {} visible stars against full scan",
        M::NAME,
        actual.len()
    );
}

fn benchmark_index<M: Index>(c: &mut Criterion, stars: &[Star]) {
    let mut construction = c.benchmark_group(format!("{}/build", M::NAME));
    for count in [10_000, 100_000, STAR_COUNT] {
        construction.throughput(Throughput::Elements(count as u64));
        construction.bench_function(BenchmarkId::from_parameter(count), |b| {
            b.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    let start = Instant::now();
                    let built = M::build(black_box(&stars[..count]));
                    elapsed += start.elapsed();
                    // Destruction and catalogue loading are excluded.
                    black_box(&built);
                    drop(built);
                }
                elapsed
            });
        });
    }
    construction.finish();

    let mut index = None;
    let mut queries = c.benchmark_group(format!("{}/query", M::NAME));
    queries.throughput(Throughput::Elements(1));
    // Earth matches the reference suite. Additional observers sample different
    // parts of the catalogue, including a source at zero distance.
    for (location, from) in [
        ("earth", EARTH),
        ("star_104729", stars[104729].position),
        ("star_523645", stars[523645].position),
    ] {
        for magnitude in [0, 2, 4, 6] {
            let threshold = magnitude_threshold(f64::from(magnitude));
            let mut validated = false;
            queries.bench_function(format!("{location}_magnitude_{magnitude}"), |b| {
                let map = index.get_or_insert_with(|| M::build(stars));
                if !validated {
                    check_query(map, stars, from, threshold);
                    validated = true;
                }
                b.iter(|| {
                    for object in map.visible(black_box(from), black_box(threshold)) {
                        black_box(object);
                    }
                });
            });
        }
    }
    queries.finish();
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

fn report_memory<M: Index>(stars: &[Star]) {
    let before = resident_kib();
    let index = M::build(stars);
    black_box(&index);
    let after = resident_kib();
    eprintln!(
        "{}: RSS before={before} KiB, after={after} KiB, delta={} KiB",
        M::NAME,
        after as i64 - before as i64
    );
}

fn benchmark(c: &mut Criterion) {
    let stars = load_stars();
    if let Ok(layout) = std::env::var("GAIA_MEMORY") {
        match layout.as_str() {
            "bvh" => report_memory::<LuminosityBvh<u64>>(&stars),
            "service" => report_memory::<SpatialService<u64>>(&stars),
            _ => panic!("GAIA_MEMORY must be bvh or service"),
        }
        return;
    }
    benchmark_index::<LuminosityBvh<u64>>(c, &stars);
    benchmark_index::<SpatialService<u64>>(c, &stars);
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
