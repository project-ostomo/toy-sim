use crate::{
    atmosphere::AtmosphereCfg,
    civilization::LIGHT_YEAR_M,
    orrery_cfg::{
        Body, BodyClass, Orbit, OrreryCfg, PlanetKind, PlanetParameters, Rotation, SpectralClass,
        StellarKind, StellarParameters,
    },
};
use glam::{DQuat, DVec3};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};
use std::f64::consts::{PI, TAU};
use toy_sim_space::GalacticPosition;

const SOLAR_MASS: f64 = 1.98847e30;
const EARTH_MASS: f64 = 5.9722e24;
const SOLAR_RADIUS: f64 = 6.957e8;
const SOLAR_WATTS: f64 = 3.828e26;
const AU: f64 = 149_597_870_700.0;
const G: f64 = 6.67430e-11;
const STEFAN_BOLTZMANN: f64 = 5.670374419e-8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogueStar {
    pub id: String,
    pub name: String,
    pub position_ly: [f64; 3],
    pub luminosity_solar: f64,
    pub temperature_k: f64,
    pub companions: Vec<CatalogueCompanion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogueCompanion {
    pub id: String,
    pub luminosity_solar: f64,
    pub temperature_k: f64,
    pub separation_au: f64,
}

/// Independent domains keep changes to one object's features from consuming
/// random draws belonging to a different object or generation stage.
pub fn seed(domain: &str, identity: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(domain.len() + identity.len() + 8);
    input.extend_from_slice(&(domain.len() as u64).to_le_bytes());
    input.extend_from_slice(domain.as_bytes());
    input.extend_from_slice(identity);
    blake3::derive_key("toy-sim astronomical generation v1", &input)
}

fn rng(domain: &str, identity: &str) -> ChaCha20Rng {
    ChaCha20Rng::from_seed(seed(domain, identity.as_bytes()))
}

fn log_range(random: &mut ChaCha20Rng, minimum: f64, maximum: f64) -> f64 {
    random.random_range(minimum.ln()..maximum.ln()).exp()
}

#[derive(Clone)]
struct PlanetHost {
    body: Body,
    identity: String,
    luminosity_solar: f64,
    inner_m: f64,
    outer_m: f64,
}

pub fn system(star: &CatalogueStar, name: &str) -> OrreryCfg {
    let primary = stellar_body(name, &star.id, star.luminosity_solar, star.temperature_k);
    let mut bodies = vec![primary.clone()];
    let mut hosts = vec![PlanetHost {
        body: primary.clone(),
        identity: star.id.clone(),
        luminosity_solar: star.luminosity_solar,
        inner_m: primary.radius * 8.0,
        outer_m: 120.0 * AU,
    }];

    let mut companions: Vec<_> = star.companions.iter().collect();
    companions.sort_by(|a, b| {
        a.separation_au
            .total_cmp(&b.separation_au)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut group_index = 0;
    let mut group_extent = primary.radius;
    let mut total_luminosity = star.luminosity_solar;
    for (index, companion) in companions.iter().enumerate() {
        let component_name = format!("{name} {}", char::from(b'B' + index as u8));
        let mut component = stellar_body(
            &component_name,
            &companion.id,
            companion.luminosity_solar,
            companion.temperature_k,
        );
        let group_mass = bodies[group_index].mass;
        let total_mass = group_mass + component.mass;
        let root_name = if index + 1 == companions.len() {
            format!("{name} barycenter")
        } else {
            format!("{name} barycenter {}", index + 1)
        };

        let mut random = rng("binary-orbit", &companion.id);
        let mut mutual = orbit(
            &mut random,
            companion.separation_au * AU,
            total_mass,
            0.25,
            None,
        );
        mutual.semi_major = mutual
            .semi_major
            .max((group_extent * 8.0 + component.radius * 8.0) / (1.0 - mutual.eccentricity));
        mutual.period = period(mutual.semi_major, total_mass);
        let pericenter = mutual.semi_major * (1.0 - mutual.eccentricity);

        for host in &mut hosts {
            host.outer_m = host
                .outer_m
                .min(pericenter * 0.1 * (host.body.mass / total_mass).cbrt());
        }
        hosts.push(PlanetHost {
            body: component.clone(),
            identity: companion.id.clone(),
            luminosity_solar: companion.luminosity_solar,
            inner_m: component.radius * 8.0,
            outer_m: pericenter * 0.1 * (component.mass / total_mass).cbrt(),
        });

        bodies[group_index].parent = Some(root_name.clone().into());
        bodies[group_index].orbit = Orbit {
            semi_major: mutual.semi_major * component.mass / total_mass,
            arg_of_pericenter: (mutual.arg_of_pericenter + PI).rem_euclid(TAU),
            ..mutual
        };
        component.parent = Some(root_name.clone().into());
        component.orbit = Orbit {
            semi_major: mutual.semi_major * group_mass / total_mass,
            ..mutual
        };
        bodies.push(component);
        group_index = bodies.len();
        bodies.push(Body {
            name: root_name.into(),
            class_params: BodyClass::Barycenter,
            mass: total_mass,
            radius: 0.0,
            ..Default::default()
        });
        group_extent += mutual.semi_major * (1.0 + mutual.eccentricity);
        total_luminosity += companion.luminosity_solar;
    }

    if !companions.is_empty() {
        hosts.push(PlanetHost {
            body: bodies[group_index].clone(),
            identity: format!("{}/circumbinary", star.id),
            luminosity_solar: total_luminosity,
            inner_m: group_extent * 4.0,
            outer_m: 120.0 * AU,
        });
    }

    let common_age = bodies
        .iter()
        .filter_map(|body| body.stellar.as_ref())
        .map(|stellar| stellar.age_years)
        .fold(f64::INFINITY, f64::min);
    for body in &mut bodies {
        if let Some(stellar) = &mut body.stellar {
            stellar.age_years = common_age;
        }
    }

    for host in hosts {
        append_planets(&mut bodies, &host);
    }

    OrreryCfg {
        name: name.into(),
        position_um: GalacticPosition::from_meters(
            DVec3::from_array(star.position_ly) * LIGHT_YEAR_M,
        ),
        bodies,
    }
}

pub(crate) fn populate_bundled_system(
    config: &mut OrreryCfg,
    identity: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.bodies.len() == 1,
        "bundled population requires one authored star"
    );
    let star = config.bodies[0].clone();
    let BodyClass::Star { lumens } = star.class_params else {
        anyhow::bail!("bundled population requires a stellar host");
    };
    anyhow::ensure!(
        star.parent.is_none(),
        "bundled stellar host must be the root"
    );
    append_planets(
        &mut config.bodies,
        &PlanetHost {
            inner_m: star.radius * 8.0,
            outer_m: 120.0 * AU,
            body: star,
            identity: identity.to_owned(),
            luminosity_solar: lumens / toy_sim_stars::SOLAR_LUMENS,
        },
    );
    config.validate_system()
}

fn stellar_body(name: &str, identity: &str, luminosity: f64, temperature: f64) -> Body {
    assert!(luminosity.is_finite() && luminosity > 0.0);
    assert!(temperature.is_finite() && temperature > 0.0);
    let radius_solar = luminosity.sqrt() * (5772.0 / temperature).powi(2);
    let main_sequence_mass = if luminosity < 0.033 {
        (luminosity / 0.23).powf(1.0 / 2.3)
    } else if luminosity < 16.0 {
        luminosity.powf(0.25)
    } else {
        (luminosity / 1.5).powf(1.0 / 3.5)
    };
    let (kind, solar_mass) = if radius_solar < 0.03 && temperature > 6000.0 {
        // Invert the cold white-dwarf mass/radius relation; catalogue temperatures
        // set luminosity and radius, while degeneracy sets the mass scale.
        let r2 = (radius_solar / 0.0112).powi(2);
        let fraction = 2.0 / (r2 + (r2 * r2 + 4.0).sqrt());
        (
            StellarKind::WhiteDwarf,
            (1.44 * fraction.powf(1.5)).clamp(0.17, 1.37),
        )
    } else if radius_solar > 3.0 && temperature < 6500.0 {
        (StellarKind::Giant, main_sequence_mass.clamp(0.8, 8.0))
    } else if main_sequence_mass < 0.075 {
        (
            StellarKind::BrownDwarf,
            main_sequence_mass.clamp(0.015, 0.075),
        )
    } else {
        (
            StellarKind::MainSequence,
            main_sequence_mass.clamp(0.075, 40.0),
        )
    };

    let class = match temperature {
        t if t >= 30_000.0 => SpectralClass::O,
        t if t >= 10_000.0 => SpectralClass::B,
        t if t >= 7500.0 => SpectralClass::A,
        t if t >= 6000.0 => SpectralClass::F,
        t if t >= 5200.0 => SpectralClass::G,
        t if t >= 3700.0 => SpectralClass::K,
        _ => SpectralClass::M,
    };

    let mut random = rng("stellar-age", identity);
    let age_limit = if matches!(kind, StellarKind::MainSequence) {
        (8e9 * solar_mass / luminosity).clamp(1e7, 12e9)
    } else {
        12e9
    };
    Body {
        name: name.into(),
        class_params: BodyClass::Star {
            lumens: luminosity * toy_sim_stars::SOLAR_LUMENS,
        },
        mass: solar_mass * SOLAR_MASS,
        radius: radius_solar * SOLAR_RADIUS,
        spectral_class: Some(class),
        surface_color: class.linear_rgb(),
        rotation: Rotation {
            rotation_period: 25.0 * 86400.0 / solar_mass.sqrt(),
            ..Default::default()
        },
        stellar: Some(StellarParameters {
            kind,
            effective_temperature_k: temperature,
            age_years: log_range(&mut random, age_limit * 0.05, age_limit),
        }),
        ..Default::default()
    }
}

fn period(semi_major: f64, total_mass: f64) -> f64 {
    TAU * (semi_major.powi(3) / (G * total_mass)).sqrt()
}

fn orbit(
    random: &mut ChaCha20Rng,
    semi_major: f64,
    mass: f64,
    max_e: f64,
    plane: Option<(f64, f64)>,
) -> Orbit {
    let (inclination, ascending_node) = plane.unwrap_or_else(|| {
        (
            random.random_range(-1.0_f64..1.0).acos(),
            random.random_range(0.0..TAU),
        )
    });
    Orbit {
        semi_major,
        period: period(semi_major, mass),
        eccentricity: random.random::<f64>().powi(3) * max_e,
        inclination: (inclination + random.random_range(-0.025..0.025)).clamp(0.0, PI),
        ascending_node: (ascending_node + random.random_range(-0.025..0.025)).rem_euclid(TAU),
        arg_of_pericenter: random.random_range(0.0..TAU),
        mean_anomaly: random.random_range(0.0..TAU),
        epoch: 0.0,
    }
}

fn spaced_semi_major(
    previous: &Body,
    mass: f64,
    central_mass: f64,
    eccentricity: f64,
    hill_radii: f64,
) -> f64 {
    let hill_fraction = ((mass + previous.mass) / (3.0 * central_mass)).cbrt() * hill_radii * 0.5;
    let denominator = 1.0 - eccentricity - hill_fraction;
    if denominator <= 0.0 {
        return f64::INFINITY;
    }
    previous.orbit.semi_major * (1.0 + previous.orbit.eccentricity + hill_fraction) / denominator
}

fn append_planets(bodies: &mut Vec<Body>, host: &PlanetHost) {
    let mut architecture = rng("planetary-architecture", &host.identity);
    let plane = (
        architecture.random_range(-1.0_f64..1.0).acos(),
        architecture.random_range(0.0..TAU),
    );
    let count = architecture.random_range(3..=10);
    let snow_line = 2.7 * host.luminosity_solar.sqrt() * AU;
    let mut semi_major =
        (host.luminosity_solar.sqrt() * log_range(&mut architecture, 0.12, 0.4) * AU)
            .max(host.inner_m * 1.25)
            .max(0.001 * AU);
    let mut previous: Option<Body> = None;
    for index in 0..count {
        let identity = format!("{}/planet/{index}", host.identity);
        let mut random = rng("planet-mass", &identity);
        let icy = semi_major > snow_line;
        let giant = random.random_bool(if icy { 0.43 } else { 0.06 });
        let earth_masses = if giant {
            log_range(
                &mut random,
                10.0,
                (host.body.mass / EARTH_MASS * 0.003).clamp(11.0, 1000.0),
            )
        } else {
            log_range(&mut random, 0.03, 8.0)
        };
        let mass = earth_masses * EARTH_MASS;
        let radius = if giant && earth_masses >= 80.0 {
            6.9911e7 * (earth_masses / 317.8).powf(-0.04)
        } else if giant {
            6.371e6 * earth_masses.powf(0.47)
        } else {
            6.371e6 * earth_masses.powf(0.27) * if icy { 1.2 } else { 1.0 }
        };

        let mut orbit_rng = rng("planet-orbit", &identity);
        let mut motion = orbit(
            &mut orbit_rng,
            semi_major,
            host.body.mass + mass,
            0.12,
            Some(plane),
        );
        if let Some(previous) = &previous {
            semi_major = semi_major.max(spaced_semi_major(
                previous,
                mass,
                host.body.mass,
                motion.eccentricity,
                8.0,
            ));
        }
        let density = mass / (4.0 / 3.0 * PI * radius.powi(3));
        let roche = 2.44 * (3.0 * host.body.mass / (4.0 * PI * density)).cbrt();
        let inner_limit = host.inner_m.max(roche * 1.15) + radius;
        semi_major = semi_major.max(inner_limit / (1.0 - motion.eccentricity));
        if semi_major * (1.0 + motion.eccentricity) > host.outer_m {
            break;
        }
        motion.semi_major = semi_major;
        motion.period = period(semi_major, host.body.mass + mass);

        let climate = climate(
            &identity,
            mass,
            radius,
            host.luminosity_solar,
            semi_major,
            motion.eccentricity,
            giant,
            icy,
        );
        let mut rotation_rng = rng("planet-rotation", &identity);
        let tidally_locked = semi_major < 0.15 * AU * (host.body.mass / SOLAR_MASS).sqrt();
        let rotation_period = if tidally_locked {
            motion.period
        } else {
            log_range(
                &mut rotation_rng,
                if giant { 7.0 } else { 10.0 },
                if giant { 20.0 } else { 80.0 },
            ) * 3600.0
        };
        let planet = Body {
            name: format!("{} {}", host.body.name, index + 1).into(),
            parent: Some(host.body.name.clone()),
            class_params: BodyClass::Planet,
            mass,
            radius,
            orbit: motion,
            rotation: Rotation {
                rotation_period,
                obliquity: if tidally_locked {
                    0.0
                } else {
                    rotation_rng.random_range(0.0..0.75)
                },
                eq_ascend_node: rotation_rng.random_range(0.0..TAU),
                ..Default::default()
            },
            atmosphere: climate.atmosphere,
            surface_color: climate.color,
            planet: Some(climate.parameters),
            ..Default::default()
        };
        append_moons(bodies, &planet, &identity, host, giant);
        bodies.push(planet.clone());
        previous = Some(planet);
        semi_major *= architecture.random_range(1.65..2.5);
    }
}

struct Climate {
    parameters: PlanetParameters,
    atmosphere: Option<AtmosphereCfg>,
    color: [f32; 3],
}

fn climate(
    identity: &str,
    mass: f64,
    radius: f64,
    luminosity: f64,
    semi_major: f64,
    eccentricity: f64,
    giant: bool,
    icy: bool,
) -> Climate {
    let mut random = rng("climate", identity);
    let albedo = random.random_range(if icy { 0.35..0.65 } else { 0.12..0.4 });
    let equilibrium = (luminosity * SOLAR_WATTS * (1.0 - albedo)
        / (16.0
            * PI
            * STEFAN_BOLTZMANN
            * semi_major.powi(2)
            * (1.0 - eccentricity.powi(2)).sqrt()))
    .powf(0.25)
    .max(3.0);
    let gas_constant = if giant {
        3600.0
    } else if random.random_bool(0.4) {
        188.9
    } else {
        296.8
    };
    let escape_parameter = G * mass / (radius * gas_constant * (equilibrium * 4.0).max(400.0));
    let retained = giant
        || (mass > 0.12 * EARTH_MASS
            && escape_parameter > 18.0
            && equilibrium < 1000.0
            && random.random_bool(0.8));
    let pressure = if giant {
        1e5
    } else if retained {
        log_range(&mut random, 100.0, 5e6)
    } else {
        0.0
    };
    let greenhouse = if giant {
        0.0
    } else {
        (if gas_constant < 200.0 { 45.0 } else { 15.0 }) * (1.0 + pressure / 1e4).ln()
    };
    let intrinsic = if giant {
        random.random_range(40.0_f64..120.0)
    } else {
        0.0
    };
    let temperature = (equilibrium.powi(4) + intrinsic.powi(4)).powf(0.25) + greenhouse;
    let boiling = if pressure > 611.0 {
        (1.0 / 373.15 - 461.5 / 2.26e6 * (pressure / 101325.0).ln()).recip()
    } else {
        0.0
    };
    let ocean_fraction =
        if !giant && retained && temperature > 273.15 && temperature < boiling.min(600.0) {
            random.random_range(0.1..0.85)
        } else {
            0.0
        };
    let kind = if giant {
        if mass > 80.0 * EARTH_MASS {
            PlanetKind::GasGiant
        } else {
            PlanetKind::IceGiant
        }
    } else if ocean_fraction > 0.0 {
        PlanetKind::Ocean
    } else if icy && temperature < 190.0 {
        PlanetKind::Ice
    } else {
        PlanetKind::Rocky
    };
    let tint = random.random_range(0.85_f32..1.15);
    let color = match kind {
        PlanetKind::Ocean => [0.36, 0.29, 0.21],
        PlanetKind::Ice => [0.62, 0.69, 0.73],
        PlanetKind::IceGiant => [0.22, 0.48, 0.59],
        PlanetKind::GasGiant => [0.61, 0.49, 0.34],
        PlanetKind::Rocky => [0.32, 0.24, 0.18],
    }
    .map(|value| value * tint);
    let atmosphere = retained.then(|| {
        let scale = gas_constant * temperature * radius.powi(2) / (G * mass);
        let density = pressure / (gas_constant * temperature);
        let scattering =
            (density / 1.2 * gas_constant / 287.0 * if giant { 0.2 } else { 1.0 }) as f32;
        AtmosphereCfg {
            height: scale * 10.0,
            surface_density: density,
            scale_height: scale,
            temperature,
            specific_gas_constant: gas_constant,
            heat_capacity_ratio: 1.4,
            rayleigh_scattering: [5.8e-6, 13.5e-6, 33.1e-6].map(|value| value * scattering),
            mie_scattering: (random.random_range(0.02..0.3) / scale) as f32,
            mie_absorption: (0.02 / scale) as f32,
            mie_scale_height: scale * 0.16,
            mie_asymmetry: 0.76,
            ground_albedo: color,
        }
    });
    let gravity = G * mass / radius.powi(2);
    let mut geology = rng("surface-geology", identity);
    let relief_m = if giant {
        0.0
    } else {
        (90_000.0 / gravity * geology.random_range(0.5..1.5))
            .clamp(200.0, 50_000.0)
            .min(radius * 0.025)
    };
    let mut weather = rng("surface-weather", identity);
    let cloud_fraction = if giant || !retained {
        0.0
    } else if pressure > 5e5 {
        weather.random_range(0.8..0.98)
    } else if ocean_fraction > 0.0 {
        weather.random_range(0.4..0.75)
    } else if pressure < 1000.0 {
        weather.random_range(0.02..0.12)
    } else {
        weather.random_range(0.08..0.4)
    };
    let (cloud_altitude_m, cloud_rotation_period_s) = if cloud_fraction > 0.0 {
        let altitude = atmosphere.as_ref().unwrap().scale_height * weather.random_range(0.4..1.4);
        let wind_speed = weather.random_range(5.0..60.0);
        let sign = if weather.random_bool(0.5) { 1.0 } else { -1.0 };
        (altitude, sign * (TAU * radius / wind_speed).max(60.0))
    } else {
        (0.0, 0.0)
    };
    Climate {
        parameters: PlanetParameters {
            seed: seed("planet-surface", identity.as_bytes()),
            kind,
            equilibrium_temperature_k: equilibrium,
            temperature_k: temperature,
            bond_albedo: albedo,
            ocean_fraction,
            relief_m,
            cloud_fraction,
            cloud_altitude_m,
            cloud_rotation_period_s,
            biosphere: false,
        },
        atmosphere,
        color,
    }
}

fn append_moons(
    bodies: &mut Vec<Body>,
    planet: &Body,
    identity: &str,
    host: &PlanetHost,
    giant: bool,
) {
    let mut architecture = rng("moon-architecture", identity);
    let count = if giant {
        architecture.random_range(2..=6)
    } else {
        architecture.random_range(0..=2)
    };
    let hill = planet.orbit.semi_major
        * (1.0 - planet.orbit.eccentricity)
        * (planet.mass / (3.0 * host.body.mass)).cbrt();
    let normal = (DQuat::from_rotation_z(planet.orbit.ascending_node)
        * DQuat::from_rotation_x(planet.orbit.inclination)
        * DQuat::from_rotation_z(planet.orbit.arg_of_pericenter)
        * DQuat::from_rotation_z(planet.rotation.eq_ascend_node)
        * DQuat::from_rotation_x(planet.rotation.obliquity))
        * DVec3::Z;
    let plane = (normal.z.clamp(-1.0, 1.0).acos(), normal.x.atan2(-normal.y));
    let mut semi_major = planet.radius * architecture.random_range(4.0..7.0);
    let mut previous: Option<Body> = None;
    let temperature = planet.planet.as_ref().unwrap().temperature_k;
    for index in 0..count {
        let identity = format!("{identity}/moon/{index}");
        let mut random = rng("moon-mass", &identity);
        let mass = planet.mass * log_range(&mut random, 1e-7, if giant { 2e-4 } else { 0.008 });
        let icy = temperature < 190.0;
        let density = if icy { 1700.0 } else { 3300.0 };
        let radius = (3.0 * mass / (4.0 * PI * density)).cbrt();
        let roche = 2.44 * (3.0 * planet.mass / (4.0 * PI * density)).cbrt();
        let mut orbit_rng = rng("moon-orbit", &identity);
        let mut motion = orbit(
            &mut orbit_rng,
            semi_major,
            planet.mass + mass,
            0.035,
            Some(plane),
        );
        if let Some(previous) = &previous {
            semi_major = semi_major.max(spaced_semi_major(
                previous,
                mass,
                planet.mass,
                motion.eccentricity,
                6.0,
            ));
        }
        semi_major = semi_major
            .max((roche * 1.15 + radius).max(planet.radius + radius) / (1.0 - motion.eccentricity));
        if semi_major * (1.0 + motion.eccentricity) > hill * 0.25 {
            break;
        }
        motion.semi_major = semi_major;
        motion.period = period(semi_major, planet.mass + mass);

        let climate = climate(
            &identity,
            mass,
            radius,
            host.luminosity_solar,
            planet.orbit.semi_major,
            planet.orbit.eccentricity,
            false,
            icy,
        );
        let moon = Body {
            name: format!("{} {}", planet.name, char::from(b'a' + index as u8)).into(),
            parent: Some(planet.name.clone()),
            mass,
            radius,
            orbit: motion,
            rotation: Rotation {
                rotation_period: motion.period,
                ..Default::default()
            },
            atmosphere: climate.atmosphere,
            surface_color: climate.color,
            planet: Some(climate.parameters),
            ..Default::default()
        };
        bodies.push(moon.clone());
        previous = Some(moon);
        semi_major *= architecture.random_range(1.8..2.7);
    }
}

#[cfg(test)]
mod tests;
