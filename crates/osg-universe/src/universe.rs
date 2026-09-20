use super::{
    catalogue::{CatalogueIndex, Entry},
    orrery_cfg::{Body, BodyClass, OrreryCfg},
    solver::Orrery,
};
use crate::precision::GalacticPosition;
use glam::{DQuat, DVec3};
use hifitime::Epoch;
use serde::Deserialize;
use smol_str::SmolStr;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
pub use osg_stars::min_brightness;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniverseCfg {
    pub systems: Vec<String>,
}
impl UniverseCfg {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.systems.is_empty() && self.systems.iter().all(|p| !p.is_empty()),
            "universe needs nonempty system paths"
        );
        Ok(())
    }
}
pub struct SystemDefinition {
    pub solver: Orrery,
    pub influence: f64,
    pub star_name: SmolStr,
    pub root_name: SmolStr,
}
pub struct Universe {
    pub systems: Vec<SystemDefinition>,
    names: BTreeMap<SmolStr, usize>,
    pub index: Arc<CatalogueIndex>,
}
impl Universe {
    pub fn init(cfg: OrreryCfg) -> anyhow::Result<Self> {
        Self::from_configs(vec![cfg], crate::physics::GRAVITY_CUTOFF)
    }
    pub fn from_configs(configs: Vec<OrreryCfg>, cutoff: f64) -> anyhow::Result<Self> {
        anyhow::ensure!(cutoff.is_finite() && cutoff > 0.0, "invalid gravity cutoff");
        let mut systems = Vec::new();
        let mut names = BTreeMap::new();
        let mut entries = Vec::new();
        let mut system_names = BTreeSet::new();
        for cfg in configs {
            cfg.validate_system()?;
            anyhow::ensure!(
                system_names.insert(cfg.name.clone()),
                "duplicate system name {}",
                cfg.name
            );
            let solver = Orrery::init(cfg)?;
            let star = solver
                .iter()
                .filter(|body| matches!(body.class_params, BodyClass::Star { .. }))
                .max_by(|a, b| {
                    let lumens = |body: &Body| match body.class_params {
                        BodyClass::Star { lumens } => lumens,
                        _ => 0.0,
                    };
                    lumens(a)
                        .total_cmp(&lumens(b))
                        .then_with(|| b.name.cmp(&a.name))
                })
                .expect("validated luminous system");
            let root_name = solver
                .iter()
                .find(|body| body.parent.is_none())
                .expect("validated system root")
                .name
                .clone();
            let mut mass = 0.0;
            let mut extent: f64 = 0.0;
            let mut lumens = 0.0;
            for body in solver.iter() {
                anyhow::ensure!(
                    names.insert(body.name.clone(), systems.len()).is_none(),
                    "duplicate celestial name {}",
                    body.name
                );
                anyhow::ensure!(
                    body.orbit.semi_major == 0.0 || body.orbit.period > 0.0,
                    "orbit period must be positive for {}",
                    body.name
                );
                if matches!(body.class_params, BodyClass::Barycenter) {
                    continue;
                }
                if let BodyClass::Star { lumens: luminosity } = body.class_params {
                    lumens += luminosity;
                }
                mass += body.mass;
                let mut reach = body.radius;
                let mut current = Some(body);
                while let Some(body) = current {
                    reach += body.orbit.semi_major * (1.0 + body.orbit.eccentricity);
                    current = body
                        .parent
                        .as_ref()
                        .and_then(|parent| solver.get_body(parent));
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
                root_name,
            });
        }
        anyhow::ensure!(!systems.is_empty(), "universe needs at least one system");
        Ok(Self {
            systems,
            names,
            index: Arc::new(CatalogueIndex::new(entries)),
        })
    }
    pub fn system_for(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }
    pub fn get_body(&self, name: &str) -> Option<&Body> {
        self.systems[self.system_for(name)?].solver.get_body(name)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Body> {
        self.systems
            .iter()
            .flat_map(|s| s.solver.iter())
            .filter(|body| !matches!(body.class_params, BodyClass::Barycenter))
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_contains_only_universe_data() {
        let text = include_str!("../../../assets/universe.toml");
        let cfg: UniverseCfg = toml::from_str(text).unwrap();
        cfg.validate().unwrap();
        for obsolete in [
            "[synthetic]\ncount = 0",
            "[sky]\nbrightness = 1.0",
            "[gaia]\npath = 'stars'",
            "gravity_cutoff = 1e-8",
            "starting_system = 'Helion system'",
        ] {
            assert!(toml::from_str::<UniverseCfg>(&format!("{text}\n{obsolete}\n")).is_err());
        }
        let system = include_str!("../../../assets/stars/helion.star.toml");
        assert!(
            toml::from_str::<OrreryCfg>(&format!("{system}\n[scenario]\nbody = 'Helion'\n"))
                .is_err()
        );
    }

    #[test]
    fn authored_systems_require_globally_unique_body_names() {
        let local = crate::example_config();
        let mut remote = crate::remote_test_config();
        Universe::from_configs(vec![local.clone(), remote.clone()], 1e-8).unwrap();
        remote.bodies[1].name = local.bodies[1].name.clone();
        assert!(Universe::from_configs(vec![local, remote], 1e-8).is_err());
    }
}

#[cfg(test)]
mod colour_tests {
    use super::*;
    use crate::orrery_cfg::SpectralClass;

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
