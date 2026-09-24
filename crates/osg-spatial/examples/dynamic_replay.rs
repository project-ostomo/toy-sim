//! Deterministic update/query comparison. See README for capture commands.
use glam::DVec3;
use osg_space::GalacticPosition;
use osg_spatial::{GalacticIndex, QueryBudget, SpatialRecord};
use std::{io::Read, time::Instant};

struct Frame {
    records: Vec<(u64, SpatialRecord)>,
    queries: Vec<Query>,
}

enum Query {
    Radius(GalacticPosition, f64),
    Segment(GalacticPosition, DVec3),
}

fn read_bytes<const N: usize>(reader: &mut impl Read) -> [u8; N] {
    let mut bytes = [0; N];
    reader
        .read_exact(&mut bytes)
        .expect("complete spatial capture");
    bytes
}

fn load(path: &str) -> Vec<Frame> {
    let mut file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    assert_eq!(&read_bytes::<8>(&mut file), b"OSGRPLY1");
    let count = u64::from_le_bytes(read_bytes(&mut file)) as usize;
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let count = u64::from_le_bytes(read_bytes(&mut file)) as usize;
        let mut records = Vec::with_capacity(count);
        let mut bodies = Vec::new();
        for _ in 0..count {
            let id = u64::from_le_bytes(read_bytes(&mut file));
            let position = GalacticPosition::new(
                i128::from_le_bytes(read_bytes(&mut file)),
                i128::from_le_bytes(read_bytes(&mut file)),
                i128::from_le_bytes(read_bytes(&mut file)),
            );
            let radius_m = f64::from_le_bytes(read_bytes(&mut file));
            let luminosity = f64::from_le_bytes(read_bytes(&mut file));
            let collision_radius = f64::from_le_bytes(read_bytes(&mut file));
            records.push((
                id,
                SpatialRecord {
                    position,
                    radius_m,
                    luminosity,
                },
            ));
            if collision_radius >= 0.0 {
                bodies.push((position, collision_radius));
            }
        }
        let maximum = bodies.iter().map(|(_, radius)| *radius).fold(0.0, f64::max);
        let mut queries: Vec<_> = bodies
            .iter()
            .map(|&(position, radius)| {
                // Production collision-discovery radius for a 0.1-second tick.
                Query::Radius(position, 100_000.0 + radius + maximum)
            })
            .collect();
        if !bodies.is_empty() {
            for i in 0..64 {
                let origin = bodies[i % bodies.len()].0;
                let target = bodies[(i * 7919 + 1) % bodies.len()].0;
                queries.push(Query::Segment(origin, target.relative_to(origin)));
            }
        }
        frames.push(Frame { records, queries });
    }
    assert_eq!(file.read(&mut [0]).unwrap(), 0, "trailing capture data");
    frames
}

fn synthetic() -> Vec<Frame> {
    (0..240)
        .map(|frame| {
            let records: Vec<_> = (0..4096_u64)
                .map(|slot| {
                    // Every 32nd object is replaced every tick; others keep identity.
                    let id = if slot % 32 == 0 {
                        slot + (frame as u64 + 1) * 4096
                    } else {
                        slot
                    };
                    let drift = if slot % 4 == 0 {
                        0.0
                    } else {
                        frame as f64 * 3.0
                    };
                    let teleport = if slot % 11 == 0 {
                        (frame / 60) as f64 * 50_000.0
                    } else {
                        0.0
                    };
                    let position = GalacticPosition::splat(1_i128 << 100).offset_by(DVec3::new(
                        (slot % 64) as f64 * 100.0 + drift + teleport,
                        (slot / 64) as f64 * 100.0 - drift,
                        (slot % 7) as f64 * 50.0,
                    ));
                    (
                        id,
                        SpatialRecord {
                            position,
                            radius_m: 2.0 + (slot % 8) as f64,
                            luminosity: ((slot + frame as u64) % 17) as f64,
                        },
                    )
                })
                .collect();
            let mut queries = Vec::new();
            for i in 0..128 {
                let origin = records[i * 31 % records.len()].1.position;
                queries.push(Query::Radius(origin, 250.0));
                let target = records[(i * 7919 + 1) % records.len()].1.position;
                queries.push(Query::Segment(origin, target.relative_to(origin)));
            }
            Frame { records, queries }
        })
        .collect()
}

#[derive(Default)]
struct Measurements {
    updates: Vec<f64>,
    queries: Vec<f64>,
    nodes: usize,
    leaves: usize,
    unchanged: usize,
    bounds_updates: usize,
    reinsertions: usize,
    luminosity: usize,
    insertions: usize,
    removals: usize,
    rotations: usize,
    bytes: usize,
    height: u32,
}

fn run(
    index: &mut GalacticIndex<u64>,
    frame: &Frame,
    stats: &mut Measurements,
    measured: bool,
) -> Vec<Vec<u64>> {
    let start = Instant::now();
    index.replace(frame.records.iter().copied()).unwrap();
    let update_ms = start.elapsed().as_secs_f64() * 1000.0;
    let counters = index.take_dynamic_stats();
    let mut nodes = 0;
    let mut leaves = 0;
    let start = Instant::now();
    let mut results: Vec<_> = frame
        .queries
        .iter()
        .map(|query| {
            let mut budget = QueryBudget::new(usize::MAX);
            let found = match *query {
                Query::Radius(origin, radius) => {
                    index.within_radius_budgeted(origin, radius, false, &mut budget)
                }
                Query::Segment(origin, displacement) => {
                    index.segment_candidates(origin, displacement, 0.0, &mut budget)
                }
            }
            .unwrap();
            nodes += budget.probes();
            leaves += budget.used() - budget.probes();
            found
        })
        .collect();
    let query_ms = start.elapsed().as_secs_f64() * 1000.0;
    // Normalize results for exact equality outside timing.
    for found in &mut results {
        found.sort_unstable();
    }
    if measured {
        stats.bytes = index.bvh_storage_bytes().unwrap();
        stats.updates.push(update_ms);
        stats.queries.push(query_ms);
        stats.nodes += nodes;
        stats.leaves += leaves;
        if let Some((work, unchanged, bytes, height)) = counters {
            stats.unchanged += unchanged;
            stats.bounds_updates += work.bounds_updates;
            stats.reinsertions += work.reinsertions;
            stats.luminosity += work.luminosity_updates;
            stats.insertions += work.insertions;
            stats.removals += work.removals;
            stats.rotations += work.rotations;
            stats.bytes = bytes;
            stats.height = stats.height.max(height);
        }
    }
    results
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn report(name: &str, repeat: usize, stats: Measurements) {
    let frames = stats.updates.len() as f64;
    let window = stats.queries.len().min(20);
    println!(
        "backend={name} repeat={repeat} frames={frames} update_ms={:.4} query_ms={:.4} total_ms={:.4} early_query_ms={:.4} late_query_ms={:.4} nodes={:.1} leaves={:.1}",
        mean(&stats.updates),
        mean(&stats.queries),
        mean(&stats.updates) + mean(&stats.queries),
        mean(&stats.queries[..window]),
        mean(&stats.queries[stats.queries.len() - window..]),
        stats.nodes as f64 / frames,
        stats.leaves as f64 / frames,
    );
    println!("backend={name} tree_storage_bytes={}", stats.bytes);
    if name == "dynamic" {
        println!(
            "work unchanged={:.1} bounds_updates={:.1} reinserted={:.1} luminosity={:.1} inserted={:.1} removed={:.1} rotations={:.1} max_height={} tree_storage_bytes={}",
            stats.unchanged as f64 / frames,
            stats.bounds_updates as f64 / frames,
            stats.reinsertions as f64 / frames,
            stats.luminosity as f64 / frames,
            stats.insertions as f64 / frames,
            stats.removals as f64 / frames,
            stats.rotations as f64 / frames,
            stats.height,
            stats.bytes,
        );
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let frames = match args.as_slice() {
        [] => synthetic(),
        [flag, path] if flag == "--capture" => load(path),
        _ => panic!("usage: dynamic_replay [--capture FILE]"),
    };
    assert!(frames.len() >= 2);
    println!(
        "frames={} first_records={} first_queries={} initial_frame_excluded=true",
        frames.len(),
        frames[0].records.len(),
        frames[0].queries.len()
    );
    for repeat in 0..3 {
        let mut static_index = GalacticIndex::bvh();
        let mut dynamic_index = GalacticIndex::dynamic_bvh();
        let mut static_stats = Measurements::default();
        let mut dynamic_stats = Measurements::default();
        for (number, frame) in frames.iter().enumerate() {
            let measured = number != 0;
            // Alternate execution order to reduce systematic cache-order bias.
            let (a, b) = if (number + repeat) % 2 == 0 {
                let a = run(&mut static_index, frame, &mut static_stats, measured);
                let b = run(&mut dynamic_index, frame, &mut dynamic_stats, measured);
                (a, b)
            } else {
                let b = run(&mut dynamic_index, frame, &mut dynamic_stats, measured);
                let a = run(&mut static_index, frame, &mut static_stats, measured);
                (a, b)
            };
            assert_eq!(a, b, "query mismatch in frame {number}");
        }
        report("static", repeat, static_stats);
        report("dynamic", repeat, dynamic_stats);
    }
}
