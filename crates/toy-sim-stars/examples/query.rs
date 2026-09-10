//! Headless loader/query benchmark; does not build or link Bevy.
use std::time::Instant;
use toy_sim_stars::{GalacticPosition, StarCatalogue, VisibilityQuery};
fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: cargo run -p toy-sim-stars --release --example query -- CATALOGUE.stars");
    let start = Instant::now();
    let catalogue = StarCatalogue::load(path)?;
    println!(
        "{} stars, {} buckets; load+index {:.3} s; record memory {} MiB",
        catalogue.len(),
        catalogue.bucket_count(),
        start.elapsed().as_secs_f64(),
        catalogue.len() * std::mem::size_of::<toy_sim_stars::Star>() / (1024 * 1024)
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
