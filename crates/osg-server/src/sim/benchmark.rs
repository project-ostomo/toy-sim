//! Opt-in measurements of the production application and snapshot publication.
use super::*;
use bevy::math::DVec3;
use rand::{SeedableRng, seq::SliceRandom};
use rand_chacha::ChaCha8Rng;
use std::io::Write;
use std::time::Instant;

fn percentile(samples: &mut [f64], fraction: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * fraction).round() as usize]
}

#[test]
#[ignore = "serial fresh debug world slip and publication profile"]
fn fresh_world_slip_load() {
    diagnostics::samples::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let account = osg_model::Id::new();
    let mut app = provision(&[account], None, None).unwrap();
    app.update();
    let world = app.world_mut();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let ship_id = world.get::<identity::Identity>(ship).unwrap().0;
    let origin = world
        .get::<precision::PreciseTransform>(ship)
        .unwrap()
        .translation_um;
    let universe = world.resource::<orrery::Universe>().0.clone();
    let current = universe.containing_segment(origin, DVec3::ZERO);
    let target = universe
        .systems
        .iter()
        .enumerate()
        .filter(|(index, _)| !current.contains(index))
        .min_by(|(_, a), (_, b)| {
            a.position
                .relative_to(origin)
                .length_squared()
                .total_cmp(&b.position.relative_to(origin).length_squared())
        })
        .map(|(_, system)| osg_model::Id(system.id))
        .unwrap();
    let session = session::connect(world, account, Default::default()).unwrap();
    world
        .get_mut::<session::Session>(session)
        .unwrap()
        .screens
        .insert((ship_id, 0), 10);
    world
        .get_mut::<session::Session>(session)
        .unwrap()
        .instruments
        .insert(ship_id);
    let mut elapsed = Vec::new();
    let mut scopes = std::collections::BTreeMap::<&str, f64>::new();
    for frame in 0..700 {
        if frame == 100 {
            let world = app.world_mut();
            let state = &mut world.get_mut::<travel::Travel>(ship).unwrap().0;
            state.enabled = true;
            state.directive_revision += 1;
            state.itinerary = vec![osg_model::travel::ItineraryEntry {
                directive: osg_model::travel::Directive::SlipToSystem(target),
                label: "Performance probe".into(),
                max_loss_ppm: 100.0,
                fuel_allowance_kg: 1000.0,
                estimated_duration_ticks: None,
            }];
        }
        diagnostics::samples::take();
        let started = Instant::now();
        app.update();
        let world = app.world_mut();
        infrastructure::publish_navigation(world);
        displays::update(world);
        let snapshot = session::frame(world, session).unwrap();
        std::hint::black_box(snapshot);
        elapsed.push(started.elapsed().as_secs_f64() * 1000.0);
        for (name, start, end) in diagnostics::samples::take() {
            *scopes.entry(name).or_default() += end.duration_since(start).as_secs_f64() * 1000.0;
        }
        if frame % 100 == 99 {
            let state = &world.get::<travel::Travel>(ship).unwrap().0;
            println!(
                "slip_profile frame={frame} p50={:.2} p95={:.2} phase={:?} summary={} failure={:?} presence={:?}",
                percentile(&mut elapsed, 0.5),
                percentile(&mut elapsed, 0.95),
                state.status.phase,
                state.status.summary,
                state.failure,
                world.get::<travel::PresenceState>(ship).unwrap().0
            );
            let mut ranked: Vec<_> = scopes
                .iter()
                .map(|(key, value)| (*key, *value / 100.0))
                .collect();
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
            println!("slip_profile scopes={:?}", &ranked[..ranked.len().min(12)]);
            elapsed.clear();
            scopes.clear();
        }
    }
    diagnostics::samples::ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);
}

fn scatter_players(world: &mut World, accounts: &[osg_model::AccountId]) {
    let universe = world.resource::<orrery::Universe>().clone();
    let epoch = physics::sim_time(world.resource::<Time<Fixed>>());
    let mut systems: Vec<_> = (0..universe.systems.len()).collect();
    let mut rng = ChaCha8Rng::seed_from_u64(0x5343_4154_5445_5231);
    systems.shuffle(&mut rng);
    assert!(accounts.len() <= systems.len(), "one player per system");

    let ships: std::collections::HashMap<_, _> = world
        .query_filtered::<(Entity, &identity::Control), (
            With<vessel::ShipDesign>, Without<travel::Dormant>,
        )>()
        .iter(world)
        .map(|(entity, control)| (control.account, entity))
        .collect();
    let radial = DVec3::new(1.0, 2.0, 3.0).normalize();
    let tangent = radial.cross(DVec3::Y).normalize();
    for (&account, system) in accounts.iter().zip(systems) {
        let ship = ships[&account];
        let summary = &universe.systems[system];
        let primary = universe.body(summary.primary).expect("system primary");
        let position = universe.solve_position(summary.primary, epoch).unwrap();
        let velocity = universe.solve_velocity(summary.primary, epoch).unwrap();
        // Place ships outside the primary with a local circular-orbit velocity.
        // The shuffled systems provide the galactic distribution.
        let radius = 149_597_870_700.0_f64.max(4.0 * primary.radius);
        let speed = (physics::GRAVITATIONAL_CONSTANT * primary.mass / radius).sqrt();
        world
            .get_mut::<precision::PreciseTransform>(ship)
            .unwrap()
            .translation_um = position.offset_by(radial * radius);
        world.get_mut::<physics::Velocity>(ship).unwrap().0 = velocity + tangent * speed;
    }
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
    let warmup: usize = std::env::var("OSG_BENCH_WARMUP")
        .unwrap_or("70".into())
        .parse()
        .unwrap();
    assert!(frames > 0, "measure at least one frame");
    assert!(matches!(scene.as_str(), "sparse" | "dense" | "rays"));
    let mut capture = std::env::var_os("OSG_BENCH_SPATIAL_CAPTURE").map(|path| {
        assert_eq!(repeats, 1, "capture one world per replay file");
        let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        file.write_all(b"OSGRPLY1").unwrap();
        file.write_all(&(frames as u64).to_le_bytes()).unwrap();
        file
    });
    for repeat in 0..repeats {
        let accounts: Vec<_> = (0..players).map(|_| osg_model::Id::new()).collect();
        let started = Instant::now();
        let mut app = provision(&accounts, None, None).unwrap();
        let world = app.world_mut();
        if let Ok(backend) = std::env::var("OSG_BENCH_BVH") {
            world.resource_mut::<spatial::SpatialIndex>().geometry = match backend.as_str() {
                "static" => osg_spatial::GalacticIndex::bvh(),
                "dynamic" => osg_spatial::GalacticIndex::dynamic_bvh(),
                _ => panic!("OSG_BENCH_BVH must be static or dynamic"),
            };
        }
        scatter_players(world, &accounts);
        let first_player = world
            .query::<(Entity, &identity::Control)>()
            .iter(world)
            .find(|(_, control)| control.account == accounts[0])
            .unwrap()
            .0;
        let anchor = world
            .get::<precision::PreciseTransform>(first_player)
            .unwrap()
            .translation_um;

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
        let records = world.resource::<spatial::SpatialIndex>().geometry.len();
        let catalogue = world.resource::<orrery::Universe>().systems.len();
        assert!(catalogue >= 1_000_000);
        println!(
            "setup players={players} placement=scattered_systems placement_seed=0x5343415454455231 scene={scene} repeat={repeat} seconds={:.3} catalogue={catalogue} records={records}",
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
        for frame in 0..warmup + frames {
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
                let _profile = diagnostics::ProfileScope::new("session.serialization");
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
                                &mut osg_spatial::QueryBudget::new(usize::MAX),
                            )
                            .unwrap(),
                    );
                }
            }
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            // Capture is a separate diagnostic run, outside the measured frame.
            // I/O still affects caches; do not use its timings as a baseline.
            if frame >= warmup
                && let Some(file) = &mut capture
            {
                let index = world.resource::<spatial::SpatialIndex>();
                let mut records: Vec<_> = index
                    .geometry
                    .iter()
                    .map(|(key, record)| {
                        let spatial::SpatialKey::Entity(entity) = key else {
                            panic!("active scene unexpectedly contains a catalogue record");
                        };
                        (
                            entity.to_bits(),
                            *record,
                            index.collision_radii.get(entity).copied().unwrap_or(-1.0),
                        )
                    })
                    .collect();
                records.sort_unstable_by_key(|(id, _, _)| *id);
                file.write_all(&(records.len() as u64).to_le_bytes())
                    .unwrap();
                for (id, record, collision_radius) in records {
                    file.write_all(&id.to_le_bytes()).unwrap();
                    for coordinate in [record.position.x, record.position.y, record.position.z] {
                        file.write_all(&coordinate.to_le_bytes()).unwrap();
                    }
                    for value in [record.radius_m, record.luminosity, collision_radius] {
                        file.write_all(&value.to_le_bytes()).unwrap();
                    }
                }
            }
            if frame >= warmup {
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
            "population records={} entities={} occupied_systems={}",
            app.world()
                .resource::<spatial::SpatialIndex>()
                .geometry
                .len(),
            app.world().entities().len(),
            app.world()
                .resource::<orrery::activity::ActiveSystems>()
                .occupied_systems
                .len()
        );
        println!(
            "result players={players} scene={scene} repeat={repeat} frames={frames} warmup={warmup} total_p50_ms={:.3} total_p95_ms={:.3} physics_p50_ms={:.3} discovery_p50_ms={:.3} solver_p50_ms={:.3} bodies={:.1} grouped={:.1} groups={:.1} reused={:.1} contact_pairs={:.1}",
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
    if let Some(mut file) = capture {
        file.flush().unwrap();
    }
}
