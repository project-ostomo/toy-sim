use std::collections::BTreeMap;
use std::time::Instant;
use toy_sim_universe::{bundled_configs, civilization, orrery_cfg::BodyClass, universe::Universe};

fn main() {
    let start = Instant::now();
    let map = civilization::map();
    println!(
        "map: systems={} links={} elapsed_ms={:.2}",
        map.systems.len(),
        map.links.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );

    let start = Instant::now();
    let configs = bundled_configs().expect("valid inhabited systems");
    let mut classes = BTreeMap::new();
    let mut atmospheres = 0;
    let mut bodies = 0;
    let mut minimum_height = f64::INFINITY;
    let mut maximum_height = 0.0_f64;
    let mut minimum_fraction = f64::INFINITY;
    let mut maximum_fraction = 0.0_f64;
    let mut maximum_optical_depth = 0.0_f64;
    for config in &configs {
        for body in &config.bodies {
            bodies += 1;
            atmospheres += usize::from(body.atmosphere.is_some());
            if let Some(atmosphere) = &body.atmosphere {
                minimum_height = minimum_height.min(atmosphere.height);
                maximum_height = maximum_height.max(atmosphere.height);
                minimum_fraction = minimum_fraction.min(atmosphere.height / body.radius);
                maximum_fraction = maximum_fraction.max(atmosphere.height / body.radius);
                maximum_optical_depth = maximum_optical_depth.max(
                    atmosphere.rayleigh_scattering[2] as f64 * atmosphere.scale_height
                        + atmosphere.mie_scattering as f64 * atmosphere.mie_scale_height,
                );
                assert!((body.radius + atmosphere.height) as f32 > body.radius as f32);
            }
            let class = match body.class_params {
                BodyClass::Star { .. } => body
                    .stellar
                    .as_ref()
                    .map(|value| format!("{:?}", value.kind))
                    .unwrap_or("AuthoredStar".into()),
                BodyClass::Planet => body
                    .planet
                    .as_ref()
                    .map(|value| format!("{:?}", value.kind))
                    .unwrap_or("AuthoredPlanet".into()),
                BodyClass::Barycenter => "Barycenter".into(),
            };
            *classes.entry(class).or_insert(0usize) += 1;
        }
    }
    println!(
        "generation+validation: bodies={bodies} atmospheres={atmospheres} elapsed_ms={:.2}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    println!("classes: {classes:?}");
    println!(
        "atmosphere bounds: height_m={minimum_height:.1}..{maximum_height:.1} height/radius={minimum_fraction:.6}..{maximum_fraction:.6} maximum_blue_optical_depth={maximum_optical_depth:.2}"
    );
    let start = Instant::now();
    let universe = Universe::from_configs(configs, 1e-8).expect("consistent universe");
    println!(
        "solvers+spatial index: physical_bodies={} elapsed_ms={:.2}",
        universe.iter().count(),
        start.elapsed().as_secs_f64() * 1000.0
    );
}
