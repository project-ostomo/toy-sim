//! Headless loader/query benchmark; does not build or link Bevy.
use osg_stars::{GalacticPosition, StarCatalogue, VisibilityQuery};
use std::time::Instant;
fn main() -> anyhow::Result<()> {
    let start = Instant::now();
    let catalogue = match std::env::args_os().nth(1) {
        Some(path) => StarCatalogue::load(path)?,
        None => StarCatalogue::embedded()?,
    };
    println!(
        "{} stars, {} buckets; load+index {:.3} s; record memory {} MiB",
        catalogue.len(),
        catalogue.bucket_count(),
        start.elapsed().as_secs_f64(),
        catalogue.len() * std::mem::size_of::<osg_stars::Star>() / (1024 * 1024)
    );
    for origin in [
        GalacticPosition::ZERO,
        GalacticPosition::new(100_000_000_000_000_000_000_000, 0, 0),
    ] {
        for magnitude in [6., 6.05, 9.] {
            for run in 0..3 {
                let start = Instant::now();
                let result = catalogue.visible(origin, VisibilityQuery::magnitude(magnitude))?;
                let nearest = catalogue.nearest(origin)?;
                println!(
                    "origin={origin:?} mag={magnitude} run={run} matches={} candidates={} query+nearest={:.3} ms nearest_m={:?}",
                    result.matched,
                    result.candidates,
                    start.elapsed().as_secs_f64() * 1000.,
                    nearest.map(|(_, d)| d)
                );
            }
        }
    }
    Ok(())
}
