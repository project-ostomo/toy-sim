use glam::DVec3;
use osg_space::GalacticPosition;
use osg_spatial::{Entry, QueryResult, SpatialHash};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha20Rng;
use std::hint::black_box;
use std::time::Instant;

fn rss_kib() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn queries(label: &str, mut run: impl FnMut(u32) -> QueryResult) {
    let start = Instant::now();
    let mut cells = 0;
    let mut candidates = 0;
    let mut hits = 0;
    for i in 0..1000 {
        let result = run(i * 97 % 100_000);
        cells += result.stats.cells_visited;
        candidates += result.stats.candidates;
        hits += result.ids.len();
        black_box(result);
    }
    println!(
        "{label}: us/query={:.2} cells/query={:.1} candidates/query={:.1} hits/query={:.1}",
        start.elapsed().as_secs_f64() * 1000.0,
        cells as f64 / 1000.0,
        candidates as f64 / 1000.0,
        hits as f64 / 1000.0
    );
}

fn scenario(clustered: bool) {
    let mut rng = ChaCha20Rng::seed_from_u64(20260918);
    let origin = GalacticPosition::splat(1_i128 << 105);
    let mut index = SpatialHash::default();
    let initial_rss = rss_kib();
    let start = Instant::now();
    for id in 0..100_000 {
        let position = if clustered {
            let cluster = id / 1000;
            DVec3::new((cluster % 10) as f64, (cluster / 10) as f64, 0.0) * 1e17
                + DVec3::new(
                    rng.random_range(-1e8..1e8),
                    rng.random_range(-1e8..1e8),
                    rng.random_range(-1e8..1e8),
                )
        } else {
            DVec3::new(
                rng.random_range(-1e18..1e18),
                rng.random_range(-1e18..1e18),
                rng.random_range(-1e18..1e18),
            )
        };
        index.insert(
            id,
            Entry {
                position: origin.offset_by(position),
                radius_m: if id % 1000 == 0 { 1e9 } else { 10.0 },
                luminosity: if id % 1000 == 0 {
                    1e26
                } else {
                    10.0_f64.powf(rng.random_range(5.0..12.0))
                },
            },
        );
    }
    println!(
        "{}: entries={} build_ms={:.2} rss_delta_KiB={} geometry_cells={} brightness_buckets={}",
        if clustered { "clustered" } else { "sparse" },
        index.len(),
        start.elapsed().as_secs_f64() * 1000.0,
        rss_kib().saturating_sub(initial_rss),
        index.occupied_cells(),
        index.bucket_count()
    );
    queries("visibility", |id| {
        index.visible(index.get(id).unwrap().position, 1e-12)
    });
    queries("local-chat-500AU", |id| {
        index.within_radius(index.get(id).unwrap().position, 500.0 * 149_597_870_700.0)
    });
    queries("nearby-1000km", |id| {
        index.intersecting_sphere(index.get(id).unwrap().position, 1e6)
    });
    queries("sweep-100km", |id| {
        index.segment_candidates(index.get(id).unwrap().position, DVec3::splat(1e5), 10.0)
    });
    let start = Instant::now();
    for id in 0..10_000 {
        let mut entry = *index.get(id).unwrap();
        entry.position = entry.position.offset_by(DVec3::X * 3000.0);
        entry.luminosity *= 100.0;
        index.insert(id, entry);
    }
    println!(
        "10k position+luminosity updates_ms={:.2}",
        start.elapsed().as_secs_f64() * 1000.0
    );
}

fn main() {
    scenario(false);
    scenario(true);
}
