use std::{hint::black_box, time::Duration};

#[cfg(not(feature = "bench-allocations"))]
use criterion::{BenchmarkId, Throughput};
use criterion::{Criterion, criterion_group, criterion_main};
use osg_spatial_hash::{LuminosityMap, NeighborMap, Position};

#[cfg(feature = "bench-allocations")]
#[path = "support/allocations.rs"]
mod allocations;

const SIDE: usize = 32;
const OBJECTS: usize = SIDE * SIDE * SIDE;
const SPACING: i64 = 4096;
const PERIOD: usize = 4096;
const CASES: [(&str, usize); 3] = [("unchanged", 0), ("slow", 1), ("fast", 64)];

struct Body {
    anchor: Position,
    phase: usize,
}

struct Motion {
    bodies: Vec<Body>,
    offsets: Vec<Position>,
}

impl Motion {
    fn new() -> Self {
        let mut random = oorandom::Rand64::new(20260921);
        let bodies = (0..OBJECTS)
            .map(|id| Body {
                anchor: Position {
                    x: (id % SIDE) as i64 * SPACING + SPACING / 2,
                    y: (id / SIDE % SIDE) as i64 * SPACING + SPACING / 2,
                    z: (id / (SIDE * SIDE)) as i64 * SPACING + SPACING / 2,
                },
                phase: random.rand_range(0..PERIOD as u64) as usize,
            })
            .collect();
        // Piecewise linear motion with reflecting turns. Random phases give
        // different positions and velocity directions, without timed RNG calls.
        let triangle = |phase: usize| {
            let phase = (phase % PERIOD) as i64;
            if phase < PERIOD as i64 / 2 {
                phase - PERIOD as i64 / 4
            } else {
                3 * PERIOD as i64 / 4 - phase
            }
        };
        let offsets = (0..PERIOD)
            .map(|phase| Position {
                x: triangle(phase),
                y: triangle(phase + PERIOD / 3),
                z: triangle(phase + 2 * PERIOD / 3),
            })
            .collect();
        Self { bodies, offsets }
    }

    fn position(&self, id: usize) -> Position {
        let body = &self.bodies[id];
        let offset = self.offsets[body.phase];
        Position {
            x: body.anchor.x + offset.x,
            y: body.anchor.y + offset.y,
            z: body.anchor.z + offset.z,
        }
    }

    fn build<M: MovingMap>(&self) -> M {
        let mut map = M::new();
        for id in 0..OBJECTS {
            map.update(id as u64, self.position(id));
        }
        map
    }

    fn frame<M: MovingMap>(&mut self, map: &mut M, step: usize) {
        for id in 0..OBJECTS {
            self.bodies[id].phase = (self.bodies[id].phase + step) % PERIOD;
            map.update(black_box(id as u64), black_box(self.position(id)));
        }
    }

    fn check<M: MovingMap>(&self, map: &M) {
        // Check every object at its current location and verify that stale cells
        // or duplicate handles have not changed the total population.
        for id in 0..OBJECTS {
            map.check_at(id as u64, self.position(id));
        }
        assert_eq!(map.count_all(), OBJECTS);
    }
}

trait MovingMap {
    fn new() -> Self;
    fn update(&mut self, id: u64, position: Position);
    fn check_at(&self, id: u64, position: Position);
    fn count_all(&self) -> usize;
}

impl MovingMap for NeighborMap<u64> {
    fn new() -> Self {
        Self::new()
    }
    fn update(&mut self, id: u64, position: Position) {
        self.insert(id, position);
    }
    fn check_at(&self, id: u64, position: Position) {
        let mut items = self.nearest(position, 0);
        assert_eq!(items.next(), Some((&id, &position)));
        assert!(items.next().is_none());
    }
    fn count_all(&self) -> usize {
        self.nearest(Position { x: 0, y: 0, z: 0 }, i64::MAX)
            .count()
    }
}

impl MovingMap for LuminosityMap<u64> {
    fn new() -> Self {
        Self::new()
    }
    fn update(&mut self, id: u64, position: Position) {
        // Keep brightness fixed so this measures position updates, not migration
        // between brightness buckets. Positive brightness at zero distance is visible.
        self.insert(id, position, 1.0);
    }
    fn check_at(&self, id: u64, position: Position) {
        let mut items = self.nearest_visible(position, 2.0);
        assert_eq!(items.next(), Some(&id));
        assert!(items.next().is_none());
    }
    fn count_all(&self) -> usize {
        self.nearest_visible(Position { x: 0, y: 0, z: 0 }, 1e-30)
            .count()
    }
}

#[cfg(not(feature = "bench-allocations"))]
fn benchmark_map<M: MovingMap>(c: &mut Criterion, name: &str) {
    let mut group = c.benchmark_group(format!("movement/{name}"));
    // One Criterion iteration is one frame, with exactly OBJECTS updates.
    group.throughput(Throughput::Elements(OBJECTS as u64));
    for (case, step) in CASES {
        let mut simulation = None;
        group.bench_function(BenchmarkId::new(case, OBJECTS), |b| {
            let (motion, map) = simulation.get_or_insert_with(|| {
                let motion = Motion::new();
                let map = motion.build::<M>();
                (motion, map)
            });
            b.iter(|| motion.frame(black_box(map), black_box(step)));
        });
        // Outside every timer, after all warmup and measured updates.
        if let Some((motion, map)) = simulation {
            motion.check(&map);
        }
    }
    group.finish();
}

#[cfg(feature = "bench-allocations")]
fn count_allocations<M: MovingMap>(name: &str) {
    for (case, step) in CASES {
        let mut motion = Motion::new();
        let mut map = motion.build::<M>();
        for _ in 0..32 {
            motion.frame(&mut map, step);
        }
        // Sample consecutive frames over independently randomized initial phases.
        // All deallocation within insert is counted.
        const FRAMES: usize = 128;
        let counts = allocations::measure(|| {
            for _ in 0..FRAMES {
                motion.frame(&mut map, step);
            }
        });
        let updates = (FRAMES * OBJECTS) as f64;
        println!(
            "{name}/{case}: {:.8} allocations, {:.8} deallocations, {:.8} reallocations, {:.2} allocated bytes per update; totals: {} allocs, {} frees, {} reallocs, {} bytes over {} updates",
            counts.allocations as f64 / updates,
            counts.deallocations as f64 / updates,
            counts.reallocations as f64 / updates,
            counts.bytes as f64 / updates,
            counts.allocations,
            counts.deallocations,
            counts.reallocations,
            counts.bytes,
            FRAMES * OBJECTS,
        );
        motion.check(&map);
    }
}

fn benchmark(c: &mut Criterion) {
    #[cfg(not(feature = "bench-allocations"))]
    {
        benchmark_map::<NeighborMap<u64>>(c, "neighbor_map");
        benchmark_map::<LuminosityMap<u64>>(c, "luminosity_map");
    }
    #[cfg(feature = "bench-allocations")]
    {
        let _ = c;
        count_allocations::<NeighborMap<u64>>("neighbor_map");
        count_allocations::<LuminosityMap<u64>>("luminosity_map");
    }
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
