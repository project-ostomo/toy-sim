use std::io::Write;

/// Dumps initial settlement placement as JSON for offline tools, including
/// the procedurally generated planets at those locations.
fn main() -> anyhow::Result<()> {
    let map = osg_universe::civilization::map();
    let stars = osg_universe::civilization::stars();
    let star_by_id: std::collections::HashMap<_, _> =
        stars.iter().map(|star| (&star.id, star)).collect();

    let mut systems = Vec::with_capacity(map.systems.len());
    for (index, system) in map.systems.iter().enumerate() {
        let star = star_by_id.get(&system.catalogue_id);
        let orrery = star.map(|star| osg_universe::generation::system(star, &system.name));

        let stellar: Vec<serde_json::Value> = orrery
            .iter()
            .flat_map(|orrery| orrery.bodies.iter())
            .filter_map(|body| {
                body.stellar.as_ref().map(|stellar| {
                    serde_json::json!({
                        "name": body.name,
                        "kind": format!("{:?}", stellar.kind),
                        "spectral_class": format!("{:?}", body.spectral_class),
                        "mass_solar": body.mass / 1.98847e30,
                        "radius_solar": body.radius / 6.957e8,
                        "temperature_k": stellar.effective_temperature_k,
                        "age_years": stellar.age_years,
                    })
                })
            })
            .collect();

        let planets: Vec<serde_json::Value> = orrery
            .iter()
            .flat_map(|orrery| orrery.bodies.iter())
            .filter(|body| {
                matches!(
                    body.class_params,
                    osg_universe::orrery_cfg::BodyClass::Planet
                )
            })
            .map(|body| {
                let planet = body.planet.as_ref().unwrap();
                let moons = orrery
                    .iter()
                    .flat_map(|orrery| orrery.bodies.iter())
                    .filter(|other| other.parent.as_deref() == Some(&*body.name))
                    .count();
                let locked =
                    body.rotation.rotation_period == body.orbit.period && body.orbit.period > 0.0;
                serde_json::json!({
                    "name": body.name,
                    "kind": format!("{:?}", planet.kind),
                    "mass_earth": body.mass / 5.9722e24,
                    "radius_earth": body.radius / 6.371e6,
                    "semi_major_au": body.orbit.semi_major / 1.495978707e11,
                    "eccentricity": body.orbit.eccentricity,
                    "period_days": body.orbit.period / 86400.0,
                    "inclination_deg": body.orbit.inclination.to_degrees(),
                    "temperature_k": planet.temperature_k,
                    "ocean_fraction": planet.ocean_fraction,
                    "cloud_fraction": planet.cloud_fraction,
                    "biosphere": planet.biosphere,
                    "atmosphere": body.atmosphere.is_some(),
                    "tidally_locked": locked,
                    "moons": moons,
                })
            })
            .collect();

        systems.push(serde_json::json!({
            "index": index,
            "name": system.name,
            "catalogue_id": system.catalogue_id,
            "sovereignty": system.sovereignty,
            "alignment": format!("{:?}", system.alignment),
            "position_ly": star.map(|s| s.position_ly),
            "luminosity_solar": star.map(|s| s.luminosity_solar),
            "temperature_k": star.map(|s| s.temperature_k),
            "companions": star.map(|s| s.companions.len()),
            "stellar": stellar,
            "planets": planets,
        }));
    }

    let json = serde_json::json!({
        "generation_version": map.generation_version,
        "systems": systems,
    });
    let mut file = std::io::BufWriter::new(std::fs::File::create(
        "/tmp/opencode/inhabited-map-dump.json",
    )?);
    serde_json::to_writer(&mut file, &json)?;
    file.flush()?;
    println!("wrote {} initial settlements", map.systems.len());
    Ok(())
}
