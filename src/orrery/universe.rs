use super::{
    catalogue::{CatalogueTree, Entry},
    orrery_cfg::{Body, BodyClass, Orbit, OrreryCfg},
    solver::Orrery,
};
use crate::precision::GalacticPosition;
use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    math::{DQuat, DVec3},
    prelude::*,
};
use hifitime::Epoch;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Deserialize;
use smol_str::SmolStr;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const PARSEC: f64 = 3.085_677_581_491_367e16;
pub const SOLAR_LUMENS: f64 = 3.6e28;
// Visual magnitude calibration for the synthetic catalogue (no extinction model).
pub fn star_colour(body: &Body) -> [f32; 3] {
    let c = body
        .spectral_class
        .map(|class| class.linear_rgb())
        .unwrap_or(body.surface_color);
    let luminance = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    if luminance > 0.0 {
        c.map(|v| v / luminance)
    } else {
        [1.0; 3]
    }
}
pub use toy_sim_stars::min_brightness;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniverseCfg {
    pub systems: Vec<String>,
    pub starting_system: String,
    pub gravity_cutoff: f64,
    pub synthetic: SyntheticCfg,
    pub sky: SkyCfg,
    #[serde(default)]
    pub gaia: Option<crate::gaia::GaiaConfig>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticCfg {
    pub count: usize,
    pub seed: u64,
    pub min_distance_pc: f64,
    pub max_distance_pc: f64,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SkyCfg {
    pub magnitude_limit: f64,
    pub brightness: f32,
}
impl UniverseCfg {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.gravity_cutoff.is_finite() && self.gravity_cutoff > 0.0,
            "gravity_cutoff must be positive"
        );
        if let Some(gaia) = &self.gaia {
            anyhow::ensure!(
                !gaia.path.is_empty() && (1..=1_000_000).contains(&gaia.max_stars),
                "Gaia path must be nonempty and max_stars in 1..1000000"
            );
        }
        let s = &self.synthetic;
        anyhow::ensure!(
            s.min_distance_pc.is_finite()
                && s.max_distance_pc.is_finite()
                && s.min_distance_pc > 0.0
                && s.max_distance_pc >= s.min_distance_pc,
            "invalid synthetic distance range"
        );
        anyhow::ensure!(
            self.sky.magnitude_limit.is_finite()
                && self.sky.brightness.is_finite()
                && self.sky.brightness > 0.0,
            "invalid sky settings"
        );
        Ok(())
    }
}
#[derive(Asset, TypePath)]
pub struct UniverseAsset {
    pub cfg: UniverseCfg,
    pub systems: Vec<Handle<OrreryCfg>>,
}
#[derive(Default, TypePath)]
pub struct UniverseLoader;
impl AssetLoader for UniverseLoader {
    type Asset = UniverseAsset;
    type Settings = ();
    type Error = anyhow::Error;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let cfg: UniverseCfg = toml::from_slice(&bytes)?;
        cfg.validate()?;
        let systems = cfg
            .systems
            .iter()
            .map(|p| context.load(p.clone()))
            .collect();
        Ok(UniverseAsset { cfg, systems })
    }
    fn extensions(&self) -> &[&str] {
        &["toml"]
    }
}
pub struct SystemDefinition {
    pub solver: Orrery,
    pub influence: f64,
    pub star_name: SmolStr,
}
#[derive(Resource)]
pub struct Universe {
    pub systems: Vec<SystemDefinition>,
    names: BTreeMap<SmolStr, usize>,
    pub tree: Arc<CatalogueTree>,
    pub scenario: Option<super::atmosphere::ScenarioCfg>,
}
impl Universe {
    pub fn init(cfg: OrreryCfg) -> anyhow::Result<Self> {
        let name = cfg.name.clone();
        Self::from_configs(vec![cfg], &name, 1e-8)
    }
    pub fn from_configs(
        configs: Vec<OrreryCfg>,
        starting: &str,
        cutoff: f64,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(cutoff.is_finite() && cutoff > 0.0, "invalid gravity cutoff");
        let mut systems = Vec::new();
        let mut names = BTreeMap::new();
        let mut entries = Vec::new();
        let mut scenario = None;
        let mut system_names = BTreeSet::new();
        for cfg in configs {
            anyhow::ensure!(
                system_names.insert(cfg.name.clone()),
                "duplicate system name {}",
                cfg.name
            );
            if cfg.name == starting {
                scenario = cfg.scenario.clone();
            }
            let solver = Orrery::init(cfg)?;
            let stars: Vec<_> = solver
                .iter()
                .filter(|b| matches!(b.class_params, BodyClass::Star { .. }))
                .collect();
            anyhow::ensure!(
                stars.len() == 1,
                "{} must have exactly one fixed root star",
                solver.name
            );
            let star = stars[0];
            anyhow::ensure!(
                star.parent.is_none() && star.orbit.semi_major == 0.0,
                "star must be fixed at the system anchor"
            );
            let BodyClass::Star { lumens } = star.class_params else {
                unreachable!()
            };
            anyhow::ensure!(
                lumens.is_finite() && lumens > 0.0,
                "invalid stellar luminosity"
            );
            let mut mass = 0.0;
            let mut extent: f64 = 0.0;
            for b in solver.iter() {
                anyhow::ensure!(
                    names.insert(b.name.clone(), systems.len()).is_none(),
                    "duplicate celestial name {}",
                    b.name
                );
                anyhow::ensure!(
                    b.mass.is_finite() && b.mass > 0.0 && b.radius.is_finite() && b.radius > 0.0,
                    "invalid mass/radius for {}",
                    b.name
                );
                let o = b.orbit;
                anyhow::ensure!(
                    [
                        o.semi_major,
                        o.period,
                        o.eccentricity,
                        o.inclination,
                        o.ascending_node,
                        o.arg_of_pericenter,
                        o.mean_anomaly,
                        o.epoch,
                        b.rotation.rotation_period,
                        b.rotation.obliquity,
                        b.rotation.eq_ascend_node,
                        b.rotation.rotation_epoch
                    ]
                    .iter()
                    .all(|v| v.is_finite())
                        && o.semi_major >= 0.0
                        && (0.0..1.0).contains(&o.eccentricity),
                    "invalid orbit/rotation for {}",
                    b.name
                );
                anyhow::ensure!(
                    o.semi_major == 0.0 || o.period > 0.0,
                    "orbit period must be positive for {}",
                    b.name
                );
                anyhow::ensure!(
                    b.surface_color
                        .iter()
                        .all(|c| c.is_finite() && *c >= 0.0 && *c <= 1.0),
                    "invalid surface colour for {}",
                    b.name
                );
                if b.name != star.name {
                    anyhow::ensure!(
                        b.parent.is_some(),
                        "nonstellar body {} needs a parent",
                        b.name
                    );
                }
                mass += b.mass;
                let mut reach = b.radius;
                let mut current = Some(b);
                while let Some(body) = current {
                    reach += body.orbit.semi_major * (1.0 + body.orbit.eccentricity);
                    current = body.parent.as_ref().and_then(|p| solver.get_body(p));
                }
                extent = extent.max(reach);
            }
            let influence =
                extent + (crate::physics::GRAVITATIONAL_CONSTANT * mass / cutoff).sqrt();
            anyhow::ensure!(influence.is_finite(), "invalid system extent");
            entries.push(Entry {
                position: solver.anchor,
                luminosity: lumens,
                influence,
                radius: star.radius,
            });
            let star_name = star.name.clone();
            systems.push(SystemDefinition {
                solver,
                influence,
                star_name,
            });
        }
        anyhow::ensure!(
            system_names.contains(starting),
            "unknown starting system {starting}"
        );
        Ok(Self {
            systems,
            names,
            tree: Arc::new(CatalogueTree::new(entries)),
            scenario,
        })
    }
    pub fn system_for(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }
    pub fn get_body(&self, name: &str) -> Option<&Body> {
        self.systems[self.system_for(name)?].solver.get_body(name)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Body> {
        self.systems.iter().flat_map(|s| s.solver.iter())
    }
    pub fn solve_position(&self, name: &str, t: Epoch) -> Option<GalacticPosition> {
        self.systems[self.system_for(name)?]
            .solver
            .solve_position(name, t)
    }
    pub fn solve_velocity(&self, name: &str, t: Epoch) -> Option<DVec3> {
        self.systems[self.system_for(name)?]
            .solver
            .solve_velocity(name, t)
    }
    pub fn solve_rotation(&self, name: &str, t: Epoch) -> Option<DQuat> {
        self.systems[self.system_for(name)?]
            .solver
            .solve_rotation(name, t)
    }
    pub fn atmospheric_velocity_at_point(
        &self,
        name: &str,
        p: GalacticPosition,
        t: Epoch,
    ) -> Option<DVec3> {
        self.systems[self.system_for(name)?]
            .solver
            .atmospheric_velocity_at_point(name, p, t)
    }
    pub fn gravity_applies(&self, name: &str, position: GalacticPosition) -> bool {
        self.system_for(name).is_some_and(|id| {
            let s = &self.systems[id];
            position.relative_to(s.solver.anchor).length_squared() <= s.influence.powi(2)
        })
    }
}
pub fn generate(cfg: &SyntheticCfg, origin: GalacticPosition) -> Vec<OrreryCfg> {
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed);
    (0..cfg.count)
        .map(|i| {
            let name = format!("Synth {:03}", i + 1);
            let z: f64 = rng.random_range(-1.0..1.0);
            let a: f64 = rng.random_range(0.0..std::f64::consts::TAU);
            let distance = rng
                .random_range(cfg.min_distance_pc.powi(3)..=cfg.max_distance_pc.powi(3))
                .cbrt()
                * PARSEC;
            let position_um = origin.offset_by(
                DVec3::new(
                    (1.0 - z * z).sqrt() * a.cos(),
                    z,
                    (1.0 - z * z).sqrt() * a.sin(),
                ) * distance,
            );
            let mass: f64 = rng.random_range(0.5..2.0);
            let mut bodies = vec![Body {
                name: name.clone().into(),
                class_params: BodyClass::Star {
                    lumens: SOLAR_LUMENS * mass.powf(3.5),
                },
                mass: 1.9885e30 * mass,
                radius: 6.96e8 * mass.powf(0.8),
                spectral_class: Some(if mass < 0.6 {
                    super::orrery_cfg::SpectralClass::M
                } else if mass < 0.85 {
                    super::orrery_cfg::SpectralClass::K
                } else if mass < 1.05 {
                    super::orrery_cfg::SpectralClass::G
                } else if mass < 1.5 {
                    super::orrery_cfg::SpectralClass::F
                } else {
                    super::orrery_cfg::SpectralClass::A
                }),
                surface_color: [
                    1.0,
                    (0.65 + mass * 0.15) as f32,
                    (0.4 + mass * 0.3).min(1.0) as f32,
                ],
                ..Default::default()
            }];
            for (j, roman) in ["I", "II", "III"].iter().enumerate() {
                let m: f64 = rng.random_range(0.3..3.0);
                bodies.push(Body {
                    name: format!("{name} {roman}").into(),
                    parent: Some(name.clone().into()),
                    mass: m * 5.9722e24,
                    radius: 6.0e6 * m.cbrt(),
                    orbit: Orbit {
                        semi_major: 1.495978707e11 * 0.5 * 3f64.powi(j as i32),
                        eccentricity: rng.random_range(0.0..0.1),
                        inclination: rng.random_range(-0.2..0.2),
                        mean_anomaly: rng.random_range(0.0..std::f64::consts::TAU),
                        ..Default::default()
                    },
                    surface_color: [
                        rng.random_range(0.1..0.6),
                        rng.random_range(0.1..0.6),
                        rng.random_range(0.1..0.6),
                    ],
                    ..Default::default()
                });
            }
            OrreryCfg {
                name: name.into(),
                position_um,
                bodies,
                scenario: None,
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_catalogue_is_repeatable_and_unique() {
        let c = SyntheticCfg {
            count: 100,
            seed: 42,
            min_distance_pc: 2.0,
            max_distance_pc: 30.0,
        };
        let a = generate(&c, GalacticPosition::ZERO);
        let b = generate(&c, GalacticPosition::ZERO);
        assert_eq!(a.len(), 100);
        assert_eq!(a[99].position_um, b[99].position_um);
        let u = Universe::from_configs(a, "Synth 001", 1e-8).unwrap();
        assert_eq!(u.iter().count(), 400);
        assert!(u.systems.iter().all(|s| {
            let d = s.solver.anchor.relative_to(GalacticPosition::ZERO).length() / PARSEC;
            (2.0..=30.0).contains(&d)
        }));
        let mut configs = generate(&c, GalacticPosition::ZERO);
        configs[1].bodies[1].name = configs[0].bodies[1].name.clone();
        assert!(Universe::from_configs(configs, "Synth 001", 1e-8).is_err());
    }
}

#[cfg(test)]
mod colour_tests {
    use super::*;
    use crate::orrery::orrery_cfg::SpectralClass;

    #[test]
    fn spectral_presets_preserve_luminance_and_order_blue_to_red() {
        let colour = |class| {
            star_colour(&Body {
                spectral_class: Some(class),
                ..Default::default()
            })
        };
        for class in [
            SpectralClass::O,
            SpectralClass::B,
            SpectralClass::A,
            SpectralClass::F,
            SpectralClass::G,
            SpectralClass::K,
            SpectralClass::M,
        ] {
            let rgb = colour(class);
            let y = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
            assert!((y - 1.0).abs() < 1e-6);
            assert!(rgb.iter().all(|v| v.is_finite() && *v >= 0.0));
        }
        assert!(colour(SpectralClass::O)[2] > colour(SpectralClass::O)[0]);
        assert!(colour(SpectralClass::M)[0] > colour(SpectralClass::M)[2]);
        let body: Body =
            toml::from_str("name = 'test'\nclass = 'star'\nlumens = 100.0\nspectral_class = 'G'")
                .unwrap();
        assert_eq!(star_colour(&body), colour(SpectralClass::G));
        assert_eq!(star_colour(&Body::default()), [1.0; 3]);
    }
}
