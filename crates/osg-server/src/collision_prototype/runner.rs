use super::*;

pub fn run() -> Result<()> {
    let mut settings = Settings {
        f32: false,
        cached: true,
        substep: 0.01,
        rapier_singletons: false,
    };
    let mut players = 1;
    let mut ticks = 300;
    let mut warmup = 70;
    let mut scripted = false;
    let mut baseline = false;
    let mut include_catalogue = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--f32" => settings.f32 = true,
            "--fresh" => settings.cached = false,
            "--scripted" => scripted = true,
            "--baseline" => baseline = true,
            "--no-catalogue" => include_catalogue = false,
            "--ships" => players = args.next().expect("--ships value").parse()?,
            "--ticks" => ticks = args.next().expect("--ticks value").parse()?,
            "--warmup" => warmup = args.next().expect("--warmup value").parse()?,
            "--step-ms" => {
                settings.substep = args.next().expect("--step-ms value").parse::<f64>()? / 1000.0
            }
            _ => anyhow::bail!("unknown option {arg}"),
        }
    }
    ensure!(
        [0.1, 0.01, 0.005].contains(&settings.substep),
        "step must be 100, 10, or 5 ms"
    );
    ensure!(
        ticks > 0 && players > 0 && players <= 1024,
        "invalid workload size"
    );
    if baseline {
        return baseline_run(players, warmup, ticks);
    }
    let fixtures::Fixture {
        bodies,
        sources,
        catalogue,
    } = if scripted {
        fixtures::Fixture {
            bodies: fixtures::scripted(),
            sources: vec![],
            catalogue: vec![],
        }
    } else {
        fixtures::production(players, include_catalogue)?
    };
    let catalogue_count = catalogue.len();
    eprintln!(
        "fixture bodies={} active_sources={} catalogue={}",
        bodies.len(),
        sources.len(),
        catalogue.len()
    );
    eprintln!("fixture_rss_kib={}", rss_kib());
    let initial = bodies.clone();
    let mut app = application(bodies, sources, settings, ExternalForces::default());
    let seed_started = Instant::now();
    {
        let mut spatial = app.world_mut().resource_mut::<Spatial>();
        for source in catalogue {
            spatial.map.insert(
                source.id,
                hash_position(source.position)?,
                source.brightness,
            );
        }
    }
    eprintln!(
        "catalogue_seed_ms={:.3}",
        seed_started.elapsed().as_secs_f64() * 1000.0
    );
    let mut samples = Vec::new();
    let mut totals = [0.0; 9];
    let mut max_group = 0;
    let mut impacts = 0;
    let mut groups = 0;
    let mut isolated = 0;
    let mut pairs = 0;
    #[cfg(feature = "collision-prototype-allocations")]
    let mut allocation_totals = [0_u64; 4];
    for tick in 0..warmup + ticks {
        if scripted {
            // Repeat the same collision workload, preserving allocations and
            // group membership but starting a new contact episode each tick.
            for mut body in app
                .world_mut()
                .query::<&mut Body>()
                .iter_mut(app.world_mut())
            {
                *body = initial.iter().find(|b| b.id == body.id).unwrap().clone();
            }
            app.world_mut().resource_mut::<Engines>().contacts.clear();
        }
        let start = Instant::now();
        #[cfg(not(feature = "collision-prototype-allocations"))]
        app.update();
        #[cfg(feature = "collision-prototype-allocations")]
        {
            let counts = super::allocations::measure(|| {
                app.update();
            });
            if tick >= warmup {
                for (sum, count) in allocation_totals.iter_mut().zip([
                    counts.allocations,
                    counts.deallocations,
                    counts.reallocations,
                    counts.bytes,
                ]) {
                    *sum += count;
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if tick == 0 {
            eprintln!(
                "first_update_ms={elapsed:.3} seeded_rss_kib={} indexed_records={}",
                rss_kib(),
                app.world().resource::<Spatial>().map.len()
            );
        }
        if tick >= warmup {
            samples.push(elapsed);
            let m = app.world().resource::<Metrics>();
            for (sum, value) in totals.iter_mut().zip([
                m.force_ms,
                m.index_ms,
                m.grouping_ms,
                m.physics_ms,
                m.construction_ms,
                m.solver_ms,
                m.writeback_ms,
                m.updates as f64,
                m.speed_violations as f64,
            ]) {
                *sum += value;
            }
            max_group = max_group.max(m.largest);
            impacts += m.impacts;
            groups += m.groups;
            isolated += m.isolated;
            pairs += m.pairs;
        }
    }
    samples.sort_by(f64::total_cmp);
    let average = samples.iter().sum::<f64>() / ticks as f64;
    let p95 = samples[((ticks - 1) as f64 * 0.95).ceil() as usize];
    let rss = std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .unwrap_or("")
        .to_owned();
    println!(
        "workload,precision,cached,step_ms,ticks,mean_ms,p95_ms,force_ms,index_ms,grouping_ms,physics_ms,construction_worker_ms,solver_worker_ms,writeback_ms,updates,speed_violations,groups,max_group,pairs,impacts,catalogue_records,isolated_bodies,smallest_cell_km"
    );
    print!(
        "{},{},{},{},{},{average:.6},{p95:.6}",
        if scripted {
            "scripted".to_owned()
        } else {
            format!("ships-{players}")
        },
        if settings.f32 { "f32" } else { "f64" },
        settings.cached,
        settings.substep * 1000.0,
        ticks
    );
    for sum in totals {
        print!(",{:.6}", sum / ticks as f64);
    }
    println!(
        ",{:.3},{max_group},{:.3},{impacts},{catalogue_count},{:.3},{}",
        groups as f64 / ticks as f64,
        pairs as f64 / ticks as f64,
        isolated as f64 / ticks as f64,
        1_u64 << MINIMUM_CELL_SHIFT
    );
    eprintln!("{rss}");
    #[cfg(feature = "collision-prototype-allocations")]
    eprintln!(
        "allocations_per_tick={} deallocations_per_tick={} reallocations_per_tick={} allocated_bytes_per_tick={}",
        allocation_totals[0] / ticks as u64,
        allocation_totals[1] / ticks as u64,
        allocation_totals[2] / ticks as u64,
        allocation_totals[3] / ticks as u64
    );
    Ok(())
}

fn rss_kib() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

fn baseline_run(players: usize, warmup: usize, ticks: usize) -> Result<()> {
    let accounts: Vec<_> = (0..players)
        .map(|i| osg_model::Id((i as u128 + 1).to_le_bytes()))
        .collect();
    let mut app = crate::scenario(&accounts, accounts.first().copied(), None)?;
    let mut samples = Vec::new();
    let mut stages = [0.0; 4];
    for tick in 0..warmup + ticks {
        let start = Instant::now();
        app.update();
        if tick >= warmup {
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            let c = app
                .world()
                .resource::<crate::sim::physics::collision::CollisionStats>();
            for (sum, value) in stages.iter_mut().zip([
                c.total_seconds,
                c.index_seconds,
                c.query_seconds,
                c.solve_seconds,
            ]) {
                *sum += value * 1000.0;
            }
        }
    }
    samples.sort_by(f64::total_cmp);
    println!("workload,ticks,total_ms,p95_ms,collision_ms,index_ms,query_ms,solve_ms");
    print!(
        "ships-{players},{ticks},{:.6},{:.6}",
        samples.iter().sum::<f64>() / ticks as f64,
        samples[((ticks - 1) as f64 * 0.95).ceil() as usize]
    );
    for sum in stages {
        print!(",{:.6}", sum / ticks as f64);
    }
    println!();
    Ok(())
}
