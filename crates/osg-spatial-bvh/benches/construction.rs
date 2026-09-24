use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use osg_spatial_bvh::{Aabb, BvhEntry, LuminosityBvh};

fn entries(count: usize, clustered: bool) -> Vec<BvhEntry<usize>> {
    let mut state = 42_u64;
    (0..count)
        .map(|object| {
            let center = std::array::from_fn(|axis| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let offset = (state % 1_000_000_000) as i128;
                if clustered {
                    // Dense local groups separated by astronomical distances.
                    (object % 32) as i128 * (axis + 1) as i128 * 1_000_000_000_000_000 + offset
                } else {
                    offset
                }
            });
            BvhEntry {
                object,
                bounds: Aabb {
                    min: center.map(|v| v - 1000),
                    max: center.map(|v| v + 1000),
                },
                luminosity: (object % 100) as f64,
            }
        })
        .collect()
}

fn construction(c: &mut Criterion) {
    // Initialize the original builder's Rayon pool outside measurement.
    rayon::join(|| (), || ());
    for clustered in [false, true] {
        let scene = if clustered { "clustered" } else { "uniform" };
        let mut group = c.benchmark_group(format!("construction/{scene}"));
        group.sample_size(20);
        group.warm_up_time(Duration::from_secs(1));
        group.measurement_time(Duration::from_secs(3));
        for count in [10_000, 46_908, 100_000, 1_000_000] {
            let input = entries(count, clustered);
            group.throughput(Throughput::Elements(count as u64));
            for morton in [false, true] {
                let name = if morton { "morton" } else { "median" };
                group.bench_function(BenchmarkId::new(name, count), |b| {
                    // Identical borrowed input; input cloning during collection
                    // is included, and output destruction is excluded.
                    b.iter_custom(|iterations| {
                        let mut elapsed = Duration::ZERO;
                        for _ in 0..iterations {
                            let start = Instant::now();
                            let entries = black_box(&input).iter().cloned();
                            let result = if morton {
                                LuminosityBvh::build_morton(entries)
                            } else {
                                LuminosityBvh::build_median(entries)
                            };
                            elapsed += start.elapsed();
                            drop(black_box(result));
                        }
                        elapsed
                    });
                });
            }
        }
        group.finish();
    }
}

criterion_group!(benches, construction);
criterion_main!(benches);
