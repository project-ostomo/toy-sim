use super::*;
use crate::{solver::Orrery, universe::Universe};
use hifitime::{Duration, Epoch};
use std::collections::BTreeMap;

fn star() -> CatalogueStar {
    CatalogueStar {
        id: "test-star".into(),
        name: "Test".into(),
        position_ly: [100.0, -20.0, 1.0],
        luminosity_solar: 1.0,
        temperature_k: 5772.0,
        companions: Vec::new(),
    }
}

#[test]
fn independent_object_streams_and_companion_order_reproduce_identical_systems() {
    let mut source = star();
    source.companions = vec![
        CatalogueCompanion {
            id: "test-C".into(),
            luminosity_solar: 0.3,
            temperature_k: 4500.0,
            separation_au: 900.0,
        },
        CatalogueCompanion {
            id: "test-B".into(),
            luminosity_solar: 0.8,
            temperature_k: 5500.0,
            separation_au: 20.0,
        },
    ];
    let first = system(&source, "Test");
    let serialized = toml::to_string(&first).unwrap();
    let mut unrelated = star();
    unrelated.id = "unrelated".into();
    let _ = system(&unrelated, "Unrelated");
    source.companions.reverse();
    assert_eq!(
        serialized,
        toml::to_string(&system(&source, "Test")).unwrap()
    );
    assert_ne!(seed("planet", b"ab"), seed("planeta", b"b"));
    assert_ne!(seed("planet", b"ab"), seed("planet", b"ac"));
    assert_ne!(seed("planet", b"ab"), seed("climate", b"ab"));
}

#[test]
fn binary_components_share_period_and_preserve_center_of_mass() {
    let mut source = star();
    source.companions.push(CatalogueCompanion {
        id: "test-B".into(),
        luminosity_solar: 0.08,
        temperature_k: 3700.0,
        separation_au: 4.0,
    });
    let config = system(&source, "Test");
    config.validate_system().unwrap();
    let solver = Orrery::init(config.clone()).unwrap();
    let root = config
        .bodies
        .iter()
        .find(|body| body.parent.is_none())
        .unwrap();
    assert!(matches!(root.class_params, BodyClass::Barycenter));
    let components: Vec<_> = config
        .bodies
        .iter()
        .filter(|body| matches!(body.class_params, BodyClass::Star { .. }))
        .collect();
    let a = components[0];
    let b = components[1];
    assert_eq!(a.orbit.period, b.orbit.period);
    assert_eq!(
        a.stellar.as_ref().unwrap().age_years,
        b.stellar.as_ref().unwrap().age_years
    );
    assert!((a.mass * a.orbit.semi_major / (b.mass * b.orbit.semi_major) - 1.0).abs() < 1e-14);
    for fraction in [0.0, 0.17, 0.5, 0.9, 37.0] {
        let epoch = Epoch::from_mjd_utc(0.0) + Duration::from_seconds(fraction * a.orbit.period);
        let pa = solver
            .solve_position(&a.name, epoch)
            .unwrap()
            .relative_to(solver.anchor);
        let pb = solver
            .solve_position(&b.name, epoch)
            .unwrap()
            .relative_to(solver.anchor);
        let center = (pa * a.mass + pb * b.mass) / (a.mass + b.mass);
        assert!(center.length() < 0.01, "center drift {center:?}");
        for body in &components {
            let delta = Duration::from_seconds(2.0);
            let measured = solver
                .solve_position(&body.name, epoch + delta)
                .unwrap()
                .relative_to(solver.solve_position(&body.name, epoch - delta).unwrap())
                / 4.0;
            assert!(measured.distance(solver.solve_velocity(&body.name, epoch).unwrap()) < 0.02);
        }
    }
    let root_name = root.name.clone();
    let universe = Universe::init(config).unwrap();
    assert_eq!(
        universe
            .iter()
            .filter(|body| matches!(body.class_params, BodyClass::Barycenter))
            .count(),
        0
    );
    assert!(universe.get_body(&root_name).is_some());
}

#[test]
fn eccentric_solver_velocity_matches_position_derivative() {
    let mut config = crate::example_config();
    config.bodies.truncate(2);
    config.bodies[1].orbit.eccentricity = 0.65;
    config.bodies[1].orbit.inclination = 0.43;
    let solver = Orrery::init(config).unwrap();
    let name = "Helion I Neris";
    for seconds in [0.0, 1000.0, 1e6, 1e7, 1e9] {
        let epoch = Epoch::from_mjd_utc(0.0) + Duration::from_seconds(seconds);
        let delta = Duration::from_seconds(1.0);
        let measured = solver
            .solve_position(name, epoch + delta)
            .unwrap()
            .relative_to(solver.solve_position(name, epoch - delta).unwrap())
            / 2.0;
        assert!(
            measured.distance(solver.solve_velocity(name, epoch).unwrap()) < 0.05,
            "velocity mismatch at {seconds}"
        );
    }
}

#[test]
fn stellar_inference_distinguishes_compact_remnants_and_keeps_radiative_consistency() {
    for (luminosity, temperature, expected) in [
        (1.0, 5772.0, StellarKind::MainSequence),
        (0.00205, 15000.0, StellarKind::WhiteDwarf),
        (80.0, 4500.0, StellarKind::Giant),
        (0.00005, 2200.0, StellarKind::BrownDwarf),
    ] {
        let body = stellar_body("Test", "test", luminosity, temperature);
        assert_eq!(body.stellar.as_ref().unwrap().kind, expected);
        let radiated = 4.0 * PI * body.radius.powi(2) * STEFAN_BOLTZMANN * temperature.powi(4);
        assert!((radiated / (luminosity * SOLAR_WATTS) - 1.0).abs() < 0.001);
        if expected == StellarKind::WhiteDwarf {
            assert!(body.mass > 0.5 * SOLAR_MASS);
        }
    }
}

#[test]
fn generated_orbits_respect_spacing_hill_roche_and_atmosphere_relations() {
    for source in crate::civilization::stars().iter().step_by(13) {
        let config = system(source, &source.name);
        config.validate_system().unwrap();
        let by_name: BTreeMap<_, _> = config
            .bodies
            .iter()
            .map(|body| (body.name.as_str(), body))
            .collect();
        let mut siblings: BTreeMap<&str, Vec<&Body>> = BTreeMap::new();
        for body in &config.bodies {
            if !matches!(body.class_params, BodyClass::Planet) {
                continue;
            }
            let parent = by_name[body.parent.as_ref().unwrap().as_str()];
            if body.rotation.rotation_period == body.orbit.period {
                assert_eq!(body.rotation.obliquity, 0.0);
            }
            let peri = body.orbit.semi_major * (1.0 - body.orbit.eccentricity);
            assert!(peri > parent.radius + body.radius);
            let body_density = body.mass / (4.0 / 3.0 * PI * body.radius.powi(3));
            let roche = 2.44 * (3.0 * parent.mass / (4.0 * PI * body_density)).cbrt();
            assert!(peri > roche);
            let expected_period = period(body.orbit.semi_major, parent.mass + body.mass);
            assert!((expected_period / body.orbit.period - 1.0).abs() < 1e-12);
            assert!((0.0..=PI).contains(&body.orbit.inclination));
            assert!((0.0..TAU).contains(&body.orbit.ascending_node));
            siblings.entry(parent.name.as_str()).or_default().push(body);
            if matches!(parent.class_params, BodyClass::Planet) {
                let parent_normal = (DQuat::from_rotation_z(parent.orbit.ascending_node)
                    * DQuat::from_rotation_x(parent.orbit.inclination)
                    * DQuat::from_rotation_z(parent.orbit.arg_of_pericenter)
                    * DQuat::from_rotation_z(parent.rotation.eq_ascend_node)
                    * DQuat::from_rotation_x(parent.rotation.obliquity))
                    * DVec3::Z;
                let moon_normal = (DQuat::from_rotation_z(body.orbit.ascending_node)
                    * DQuat::from_rotation_x(body.orbit.inclination))
                    * DVec3::Z;
                assert!(parent_normal.dot(moon_normal) > 0.05_f64.cos());
                let grandparent = by_name[parent.parent.as_ref().unwrap().as_str()];
                let hill = parent.orbit.semi_major
                    * (1.0 - parent.orbit.eccentricity)
                    * (parent.mass / (3.0 * grandparent.mass)).cbrt();
                assert!(body.orbit.semi_major * (1.0 + body.orbit.eccentricity) <= hill * 0.25);
                let density = body.mass / (4.0 / 3.0 * PI * body.radius.powi(3));
                let roche = 2.44 * (3.0 * parent.mass / (4.0 * PI * density)).cbrt();
                assert!(peri > roche);
            }
            if let Some(atmosphere) = &body.atmosphere {
                let gravity = G * body.mass / body.radius.powi(2);
                assert!(
                    (atmosphere.scale_height * gravity
                        / (atmosphere.specific_gas_constant * atmosphere.temperature)
                        - 1.0)
                        .abs()
                        < 1e-12
                );
                assert!(atmosphere.height < body.radius * 0.5);
            }
        }
        for (parent_name, mut children) in siblings {
            children.sort_by(|a, b| a.orbit.semi_major.total_cmp(&b.orbit.semi_major));
            let parent = by_name[parent_name];
            let clearance = if matches!(parent.class_params, BodyClass::Planet) {
                6.0
            } else {
                8.0
            };
            for pair in children.windows(2) {
                let inner = pair[0];
                let outer = pair[1];
                let mutual_hill = ((inner.mass + outer.mass) / (3.0 * parent.mass)).cbrt()
                    * (inner.orbit.semi_major + outer.orbit.semi_major)
                    * 0.5;
                let gap = outer.orbit.semi_major * (1.0 - outer.orbit.eccentricity)
                    - inner.orbit.semi_major * (1.0 + inner.orbit.eccentricity);
                assert!(gap >= clearance * mutual_hill * (1.0 - 1e-12));
            }
        }
    }
}

#[test]
fn complete_inhabited_map_validates_preserves_authored_bodies_and_sol_plane() {
    let started = std::time::Instant::now();
    let configs = crate::bundled_configs().unwrap();
    assert_eq!(configs.len(), crate::civilization::INHABITED_SYSTEMS);
    let count: usize = configs.iter().map(|config| config.bodies.len()).sum();
    assert!(count > 20_000, "{count} generated bodies");
    let sol = configs.iter().find(|system| system.name == "Sol").unwrap();
    let earth = sol.bodies.iter().find(|body| body.name == "Earth").unwrap();
    assert!(earth.orbit.inclination < 1e-5);
    assert!((earth.rotation.obliquity - 23.439281_f64.to_radians()).abs() < 1e-14);
    for (authored, config) in crate::handcrafted_configs().iter().zip(&configs) {
        assert_eq!(authored.name, config.name);
        assert_eq!(
            serde_json::to_string(&authored.bodies).unwrap(),
            serde_json::to_string(&config.bodies[..authored.bodies.len()]).unwrap()
        );
    }
    let universe = Universe::from_configs(configs, 1e-8).unwrap();
    assert_eq!(universe.systems.len(), 3000);
    let physical = universe.iter().count();
    assert!(physical <= count);
    eprintln!(
        "Generated and validated 3000 systems: {count} bodies, {physical} physical, {:?}",
        started.elapsed()
    );
}

#[test]
fn bundled_stellar_stubs_gain_planets_without_replacing_stars_or_custom_configs() {
    let authored = crate::handcrafted_configs();
    let bundled = crate::bundled_configs().unwrap();
    let second = crate::bundled_configs().unwrap();
    let mut completed = 0;
    for ((original, populated), repeated) in authored.iter().zip(&bundled).zip(&second) {
        if original.bodies.len() > 1 {
            continue;
        }
        completed += 1;
        assert!(
            populated.bodies.len() >= 4,
            "{} remained empty",
            original.name
        );
        assert_eq!(
            serde_json::to_string(&original.bodies[0]).unwrap(),
            serde_json::to_string(&populated.bodies[0]).unwrap(),
        );
        assert_eq!(
            serde_json::to_string(&populated.bodies).unwrap(),
            serde_json::to_string(&repeated.bodies).unwrap(),
        );
        let mut original = original.clone();
        original.position_um = populated.position_um;
        let prior = Orrery::init(original).unwrap();
        let current = Orrery::init(populated.clone()).unwrap();
        let star = &populated.bodies[0].name;
        for seconds in [0.0, 100.0, 1e9] {
            let epoch = Epoch::from_mjd_utc(0.0) + Duration::from_seconds(seconds);
            assert_eq!(
                prior.solve_position(star, epoch),
                current.solve_position(star, epoch)
            );
            assert_eq!(
                prior.solve_velocity(star, epoch),
                current.solve_velocity(star, epoch)
            );
        }
    }
    assert_eq!(completed, 8);

    let mut custom = authored[2].clone();
    custom.name = "Deliberately barren custom system".into();
    let custom = Universe::init(custom).unwrap();
    assert_eq!(custom.iter().count(), 1);
}

#[test]
fn authored_surfaces_have_explicit_climates_and_biospheres() {
    let configs = crate::handcrafted_configs();
    let sol = configs.iter().find(|config| config.name == "Sol").unwrap();
    let earth = sol.bodies.iter().find(|body| body.name == "Earth").unwrap();
    let earth_surface = earth.planet.as_ref().unwrap();
    assert_eq!(earth_surface.kind, PlanetKind::Ocean);
    assert_eq!(earth_surface.ocean_fraction, 0.71);
    assert!(earth_surface.biosphere);
    assert!(earth_surface.cloud_fraction > 0.5);
    let atmosphere = earth.atmosphere.as_ref().unwrap();
    let pressure =
        atmosphere.surface_density * atmosphere.specific_gas_constant * atmosphere.temperature;
    assert!((pressure - 101325.0).abs() < 0.01);

    let helion = &configs[0];
    let neris = helion
        .bodies
        .iter()
        .find(|body| body.name == "Helion I Neris")
        .unwrap();
    let surface = neris.planet.as_ref().unwrap();
    assert_eq!(surface.kind, PlanetKind::Ocean);
    assert!(surface.ocean_fraction > 0.5);
    assert!(!surface.biosphere);
    assert!(neris.atmosphere.is_some());
    for moon in helion
        .bodies
        .iter()
        .filter(|body| body.parent.as_deref() == Some("Helion I Neris"))
    {
        assert!(moon.atmosphere.is_none());
        assert_eq!(moon.planet.as_ref().unwrap().cloud_fraction, 0.0);
    }
    let mut seeds = std::collections::BTreeSet::new();
    for config in configs {
        config.validate_system().unwrap();
        for body in config.bodies {
            if !matches!(body.class_params, BodyClass::Planet) {
                continue;
            }
            let planet = body
                .planet
                .as_ref()
                .expect("authored planet has physical appearance metadata");
            assert!(seeds.insert(planet.seed));
            assert_eq!(planet.biosphere, body.name == "Earth");
        }
    }
}

#[test]
fn generated_weather_matches_atmosphere_and_does_not_imply_life() {
    let mut atmospheric = 0;
    let mut bare = 0;
    let mut giants = 0;
    for source in crate::civilization::stars().iter().step_by(29) {
        let config = system(source, &source.name);
        config.validate_system().unwrap();
        for body in &config.bodies {
            let Some(planet) = &body.planet else {
                continue;
            };
            assert!(!planet.biosphere);
            if matches!(planet.kind, PlanetKind::GasGiant | PlanetKind::IceGiant) {
                giants += 1;
                assert_eq!(planet.relief_m, 0.0);
                assert_eq!(planet.cloud_fraction, 0.0);
            } else if let Some(atmosphere) = &body.atmosphere {
                atmospheric += 1;
                assert!(planet.cloud_fraction > 0.0);
                assert!(
                    planet.cloud_altitude_m > 0.0 && planet.cloud_altitude_m < atmosphere.height
                );
                assert!(planet.cloud_rotation_period_s.abs() >= 60.0);
            } else {
                bare += 1;
                assert_eq!(planet.cloud_fraction, 0.0);
                assert_eq!(planet.cloud_altitude_m, 0.0);
                assert_eq!(planet.cloud_rotation_period_s, 0.0);
            }
        }
    }
    assert!(atmospheric > 10 && bare > 10 && giants > 10);
}
