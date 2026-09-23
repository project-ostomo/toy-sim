//! Opt-in measurements of the production application and snapshot publication.
use super::*;
use bevy::math::DVec3;
use std::time::Instant;

fn percentile(samples: &mut [f64], fraction: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * fraction).round() as usize]
}

#[test]
#[ignore = "serial production benchmark; set OSG_BENCH_PLAYERS and OSG_BENCH_SCENE"]
fn production_tick_and_publication() {
    diagnostics::samples::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let players: usize = std::env::var("OSG_BENCH_PLAYERS")
        .unwrap_or("1".into())
        .parse()
        .unwrap();
    let scene = std::env::var("OSG_BENCH_SCENE").unwrap_or("sparse".into());
    let repeats: usize = std::env::var("OSG_BENCH_REPEATS")
        .unwrap_or("3".into())
        .parse()
        .unwrap();
    let frames: usize = std::env::var("OSG_BENCH_FRAMES")
        .unwrap_or("300".into())
        .parse()
        .unwrap();
    assert!(matches!(scene.as_str(), "sparse" | "dense" | "rays"));
    for repeat in 0..repeats {
        let accounts: Vec<_> = (0..players).map(|_| osg_model::Id::new()).collect();
        let started = Instant::now();
        let mut app = provision(&accounts, None, None).unwrap();
        let world = app.world_mut();
        let ships: Vec<_> = world
            .query_filtered::<(Entity, &identity::Identity), With<vessel::ShipDesign>>()
            .iter(world)
            .map(|(entity, id)| (entity, id.0))
            .collect();
        let anchor = world
            .get::<precision::PreciseTransform>(ships[0].0)
            .unwrap()
            .translation_um;
        for (number, &(ship, _)) in ships.iter().enumerate() {
            if world.get::<travel::Dormant>(ship).is_some() {
                continue;
            }
            let player = world
                .get::<identity::Control>(ship)
                .is_some_and(|control| accounts.contains(&control.account));
            if !player {
                continue;
            }
            world
                .get_mut::<precision::PreciseTransform>(ship)
                .unwrap()
                .translation_um = anchor.offset_by(DVec3::Y * 2e6 * number as f64);
        }

        // Maintain a controlled contact load through the production projectile
        // components. Reset only this synthetic cluster between measured ticks.
        let mut contact_load = Vec::new();
        if scene == "dense" {
            for number in 0..64 {
                let offset = DVec3::new(
                    (number % 4) as f64,
                    ((number / 4) % 4) as f64,
                    (number / 16) as f64,
                ) * 1.8
                    + DVec3::Z * 2e6;
                let mut projectile = physics::collision::Projectile::new(1.0, 10.0);
                projectile.remaining_s = 1e9;
                projectile.hull_hp = 1e12;
                let entity = world
                    .spawn((
                        projectile,
                        precision::PreciseTransform {
                            translation_um: anchor.offset_by(offset),
                            ..Default::default()
                        },
                        physics::MassProps {
                            mass: 10.0,
                            inertia: bevy::math::DMat3::IDENTITY * 4.0,
                            inertia_inv: bevy::math::DMat3::IDENTITY * 0.25,
                        },
                    ))
                    .id();
                contact_load.push((entity, offset));
            }
        }
        let mut sessions = Vec::new();
        for account in accounts {
            let session = session::connect(world, account, Default::default()).unwrap();
            let id = world
                .query::<(&identity::Identity, &identity::Control)>()
                .iter(world)
                .find(|(_, control)| control.account == account)
                .unwrap()
                .0
                .0;
            let mut subscription = world.get_mut::<session::Session>(session).unwrap();
            subscription.screens.insert((id, 0), 10);
            subscription.instruments.insert(id);
            sessions.push(session);
        }
        spatial::rebuild(world);
        let records = world
            .resource::<spatial::SpatialIndex>()
            .hash
            .read()
            .unwrap()
            .len();
        let catalogue = world.resource::<orrery::Universe>().systems.len();
        assert!(catalogue >= 1_000_000);
        println!(
            "setup players={players} scene={scene} repeat={repeat} seconds={:.3} catalogue={catalogue} records={records}",
            started.elapsed().as_secs_f64()
        );

        let mut elapsed = Vec::new();
        let mut breakdown = std::collections::BTreeMap::<String, Vec<f64>>::new();
        let mut query_counts = std::collections::BTreeMap::<&'static str, usize>::new();
        let mut physics_ms = Vec::new();
        let mut discovery_ms = Vec::new();
        let mut solver_ms = Vec::new();
        let mut groups = 0;
        let mut grouped = 0;
        let mut bodies = 0;
        let mut reused = 0;
        let mut contact_pairs = 0;
        for frame in 0..70 + frames {
            diagnostics::samples::take();
            diagnostics::samples::take_counts();
            let started = Instant::now();
            for &(entity, offset) in &contact_load {
                let world = app.world_mut();
                world
                    .get_mut::<precision::PreciseTransform>(entity)
                    .unwrap()
                    .translation_um = anchor.offset_by(offset);
                world.get_mut::<physics::Velocity>(entity).unwrap().0 = DVec3::ZERO;
                world.get_mut::<physics::AngularVelocity>(entity).unwrap().0 = DVec3::ZERO;
            }
            app.update();
            let simulation_ms = started.elapsed().as_secs_f64() * 1000.0;
            let simulation_end = Instant::now();
            if frame == 0 {
                println!(
                    "first simulation tick ms={:.3}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
            let world = app.world_mut();
            let publication_started = Instant::now();
            let navigation_started = Instant::now();
            infrastructure::publish_navigation(world);
            let navigation_ms = navigation_started.elapsed().as_secs_f64() * 1000.0;
            let displays_started = Instant::now();
            displays::update(world);
            let displays_ms = displays_started.elapsed().as_secs_f64() * 1000.0;
            let snapshots_started = Instant::now();
            for &session in &sessions {
                let snapshot = session::frame(world, session).unwrap();
                std::hint::black_box(postcard::to_stdvec(&snapshot).unwrap());
            }
            session::prune_events(world);
            let snapshots_ms = snapshots_started.elapsed().as_secs_f64() * 1000.0;
            let publication_ms = publication_started.elapsed().as_secs_f64() * 1000.0;
            if scene == "rays" {
                let universe = world.resource::<orrery::Universe>();
                for direction in [DVec3::X, DVec3::Y, DVec3::Z, DVec3::ONE.normalize()] {
                    std::hint::black_box(
                        universe
                            .capture_candidates(
                                anchor,
                                direction * 1e17,
                                100.0,
                                &mut osg_space::spatial::QueryBudget::new(usize::MAX),
                            )
                            .unwrap(),
                    );
                }
            }
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            if frame >= 70 {
                for (name, count) in diagnostics::samples::take_counts() {
                    *query_counts.entry(name).or_default() += count;
                }
                let samples = diagnostics::samples::take();
                let mut frame_times = std::collections::BTreeMap::<String, f64>::new();
                let mut intervals = Vec::new();
                for (name, start, end) in samples {
                    if end <= simulation_end {
                        *frame_times.entry(name.into()).or_default() +=
                            end.duration_since(start).as_secs_f64() * 1000.0;
                        intervals.push((start, end));
                    } else {
                        *frame_times
                            .entry(format!("publication.{name}"))
                            .or_default() += end.duration_since(start).as_secs_f64() * 1000.0;
                    }
                }
                intervals.sort_unstable();
                let mut covered = 0.0;
                let mut cursor = started;
                for (start, end) in intervals {
                    let start = start.max(cursor);
                    if end > start {
                        covered += end.duration_since(start).as_secs_f64() * 1000.0;
                    }
                    cursor = cursor.max(end);
                }
                frame_times.insert("SIMULATION_TOTAL".into(), simulation_ms);
                frame_times.insert("SIMULATION_UNSCOPED".into(), simulation_ms - covered);
                frame_times.insert("PUBLICATION_TOTAL".into(), publication_ms);
                let collision = world.resource::<physics::collision::CollisionStats>();
                frame_times.insert("COLLISION_TOTAL".into(), collision.total_seconds * 1000.0);
                frame_times.insert(
                    "collision.discovery".into(),
                    collision.query_seconds * 1000.0,
                );
                frame_times.insert("collision.solver".into(), collision.solve_seconds * 1000.0);
                frame_times.insert(
                    "collision.index_sync".into(),
                    collision.index_seconds * 1000.0,
                );
                frame_times.insert("publication.navigation".into(), navigation_ms);
                frame_times.insert("publication.displays".into(), displays_ms);
                frame_times.insert(
                    "publication.snapshots_serialization_pruning".into(),
                    snapshots_ms,
                );
                for (name, ms) in frame_times {
                    breakdown.entry(name).or_default().push(ms);
                }
                elapsed.push(total_ms);
                let stats = world.resource::<physics::collision::CollisionStats>();
                physics_ms.push(stats.total_seconds * 1000.0);
                discovery_ms.push(stats.query_seconds * 1000.0);
                solver_ms.push(stats.solve_seconds * 1000.0);
                groups += stats.groups;
                grouped += stats.grouped_bodies;
                bodies += stats.bodies;
                reused += stats.reused_worlds;
                contact_pairs += stats.contact_pairs;
            }
            if frame % 50 == 0 {
                let stats = world.resource::<physics::collision::CollisionStats>();
                println!(
                    "progress frame={frame} total_ms={:.3} physics_ms={:.3} groups={} bodies={} grouped={}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    stats.total_seconds * 1000.0,
                    stats.groups,
                    stats.bodies,
                    stats.grouped_bodies
                );
            }
        }
        for (name, mut samples) in breakdown {
            let mean = samples.iter().sum::<f64>() / frames as f64;
            println!(
                "breakdown scope={name} mean_ms={mean:.4} p50_ms={:.4} p95_ms={:.4} samples={}",
                percentile(&mut samples, 0.5),
                percentile(&mut samples, 0.95),
                samples.len()
            );
        }
        for (name, count) in query_counts {
            println!(
                "query_count name={name} per_tick={:.3}",
                count as f64 / frames as f64
            );
        }
        println!(
            "population records={} entities={}",
            app.world()
                .resource::<spatial::SpatialIndex>()
                .hash
                .read()
                .unwrap()
                .len(),
            app.world().entities().len()
        );
        println!(
            "result players={players} scene={scene} repeat={repeat} frames={frames} total_p50_ms={:.3} total_p95_ms={:.3} physics_p50_ms={:.3} discovery_p50_ms={:.3} solver_p50_ms={:.3} bodies={:.1} grouped={:.1} groups={:.1} reused={:.1} contact_pairs={:.1}",
            percentile(&mut elapsed, 0.5),
            percentile(&mut elapsed, 0.95),
            percentile(&mut physics_ms, 0.5),
            percentile(&mut discovery_ms, 0.5),
            percentile(&mut solver_ms, 0.5),
            bodies as f64 / frames as f64,
            grouped as f64 / frames as f64,
            groups as f64 / frames as f64,
            reused as f64 / frames as f64,
            contact_pairs as f64 / frames as f64
        );
        if scene == "dense" {
            assert!(
                contact_pairs >= frames as u64,
                "dense fixture lost its contacts"
            );
        }
    }
}
