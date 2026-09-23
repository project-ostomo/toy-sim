use crate::{
    catalogue::{CatalogueIndex, Entry},
    generation::{self, CatalogueStar},
    orrery_cfg::{Body, BodyClass, OrreryCfg},
    precision::GalacticPosition,
    solver::Orrery,
};
use anyhow::{Context, Result, ensure};
use glam::{DQuat, DVec3};
use hifitime::Epoch;
use osg_stars::{Star, StarCatalogue};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, Weak},
};

pub type SystemId = [u8; 16];
pub type LocalBodyId = [u8; 16];
pub const GENERATOR_REVISION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CelestialId {
    pub system: SystemId,
    pub local: LocalBodyId,
}

pub fn star_colour(body: &Body) -> [f32; 3] {
    let color = body
        .spectral_class
        .map(|class| class.linear_rgb())
        .unwrap_or(body.surface_color);
    let luminance = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
    if luminance > 0.0 {
        color.map(|value| value / luminance)
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
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.systems.is_empty() && self.systems.iter().all(|p| !p.is_empty()),
            "universe needs nonempty system paths"
        );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct SystemSummary {
    pub id: SystemId,
    pub primary: CelestialId,
    pub name: SmolStr,
    pub position: GalacticPosition,
    pub influence_bound: f64,
    pub capture_bound: f64,
    pub star_radius: f64,
    pub stellar_mass: f64,
    pub luminosity: f64,
    pub temperature_k: f64,
    pub colour: [f32; 3],
}

pub struct SystemDefinition {
    pub id: SystemId,
    pub solver: Orrery,
    pub influence: f64,
    pub capture_bound: f64,
    pub star_name: SmolStr,
    pub root_name: SmolStr,
    bodies: BTreeMap<LocalBodyId, SmolStr>,
    identities: BTreeMap<SmolStr, LocalBodyId>,
}

impl SystemDefinition {
    fn new(id: SystemId, config: OrreryCfg, cutoff: f64) -> Result<Self> {
        config.validate_system()?;
        let solver = Orrery::init(config)?;
        let star = solver
            .iter()
            .filter(|body| matches!(body.class_params, BodyClass::Star { .. }))
            .max_by(|a, b| stellar_luminosity(a).total_cmp(&stellar_luminosity(b)))
            .context("system has no star")?;
        let star_name = star.name.clone();
        let root_name = solver
            .iter()
            .find(|body| body.parent.is_none())
            .context("system has no root")?
            .name
            .clone();
        let mut bodies = BTreeMap::new();
        let mut identities = BTreeMap::new();
        let mut mass = 0.0;
        let mut extent: f64 = 0.0;
        let mut capture_bound: f64 = 0.0;
        for body in solver.iter() {
            ensure!(
                !body.key.is_empty(),
                "body {} requires a stable key",
                body.name
            );
            let local = local_body_identity(&body.key);
            ensure!(
                bodies.insert(local, body.name.clone()).is_none(),
                "duplicate body key {}",
                body.key
            );
            identities.insert(body.name.clone(), local);
            if matches!(body.class_params, BodyClass::Barycenter) {
                continue;
            }
            mass += body.mass;
            let mut reach = body.radius;
            let mut current = Some(body);
            while let Some(ancestor) = current {
                reach += ancestor.orbit.semi_major * (1.0 + ancestor.orbit.eccentricity);
                current = ancestor
                    .parent
                    .as_ref()
                    .and_then(|name| solver.get_body(name));
            }
            extent = extent.max(reach);
            let exclusion = 0.008 * 149_597_870_700.0 * (body.mass / 1.98847e30).cbrt();
            capture_bound = capture_bound.max(reach - body.radius + body.radius.max(exclusion));
        }
        let influence = extent + (crate::physics::GRAVITATIONAL_CONSTANT * mass / cutoff).sqrt();
        ensure!(influence.is_finite(), "invalid influence extent");
        Ok(Self {
            id,
            solver,
            influence,
            capture_bound,
            star_name,
            root_name,
            bodies,
            identities,
        })
    }

    pub fn body_id(&self, name: &str) -> Option<CelestialId> {
        Some(CelestialId {
            system: self.id,
            local: *self.identities.get(name)?,
        })
    }

    pub fn body(&self, local: LocalBodyId) -> Option<&Body> {
        self.solver.get_body(self.bodies.get(&local)?)
    }
}

enum DefinitionSource {
    Authored {
        config: Arc<OrreryCfg>,
        populate: bool,
    },
    Catalogue {
        source_id: u64,
        temperature_k: f64,
    },
    Enriched(usize),
}

#[derive(Default)]
struct DefinitionCache {
    entries: HashMap<SystemId, Arc<SystemDefinition>>,
    resident: HashMap<SystemId, Weak<SystemDefinition>>,
    recent: VecDeque<SystemId>,
}

impl DefinitionCache {
    fn get(&mut self, id: SystemId) -> Option<Arc<SystemDefinition>> {
        let Some(definition) = self.entries.get(&id).cloned() else {
            return self.resident.get(&id)?.upgrade();
        };
        self.recent.retain(|entry| *entry != id);
        self.recent.push_back(id);
        Some(definition)
    }

    fn insert(&mut self, id: SystemId, definition: Arc<SystemDefinition>) {
        self.resident
            .retain(|_, definition| definition.strong_count() > 0);
        self.resident.insert(id, Arc::downgrade(&definition));
        self.recent.retain(|entry| *entry != id);
        self.recent.push_back(id);
        self.entries.insert(id, definition);
        while self.entries.len() > 128 {
            if let Some(oldest) = self.recent.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

pub struct Universe {
    pub systems: Vec<SystemSummary>,
    pub index: Arc<CatalogueIndex>,
    pub fingerprint: [u8; 32],
    maximum_capture_bound: f64,
    sources: Vec<DefinitionSource>,
    identities: HashMap<SystemId, usize>,
    names: HashMap<SmolStr, SystemId>,
    authored_bodies: HashMap<SmolStr, CelestialId>,
    cache: Mutex<DefinitionCache>,
    cutoff: f64,
}

impl Universe {
    pub fn init(config: OrreryCfg) -> Result<Self> {
        Self::from_configs(vec![config], crate::physics::GRAVITY_CUTOFF)
    }

    pub fn from_configs(configs: Vec<OrreryCfg>, cutoff: f64) -> Result<Self> {
        ensure!(cutoff.is_finite() && cutoff > 0.0, "invalid gravity cutoff");
        let mut systems = Vec::new();
        let mut sources = Vec::new();
        let mut fingerprint = blake3::Hasher::new_derive_key("OpenSpaceGame universe v2");
        fingerprint.update(&cutoff.to_le_bytes());
        fingerprint.update(&GENERATOR_REVISION.to_le_bytes());
        for config in configs {
            fingerprint.update(&serde_json::to_vec(&config)?);
            let id = system_identity(&config.key);
            systems.push(authored_summary(id, &config, false, cutoff)?);
            sources.push(DefinitionSource::Authored {
                config: Arc::new(config),
                populate: false,
            });
        }
        Self::assemble(systems, sources, cutoff, *fingerprint.finalize().as_bytes())
    }

    pub fn bundled() -> Result<Self> {
        let cutoff = crate::physics::GRAVITY_CUTOFF;
        let mut systems = Vec::with_capacity(1_001_760);
        let mut sources = Vec::with_capacity(1_001_760);
        let settlements = crate::civilization::map();
        let mut fingerprint = blake3::Hasher::new_derive_key("OpenSpaceGame universe v2");
        fingerprint.update(StarCatalogue::embedded_bytes());
        fingerprint.update(include_bytes!("../data/inhabited-stars.json"));
        fingerprint.update(include_bytes!("../data/star-names.json"));
        fingerprint.update(&cutoff.to_le_bytes());
        fingerprint.update(&GENERATOR_REVISION.to_le_bytes());
        for (mut config, settlement) in crate::handcrafted_configs()
            .into_iter()
            .zip(&settlements.systems)
        {
            config.position_um = settlement.position;
            fingerprint.update(&serde_json::to_vec(&config)?);
            let id = system_identity(&config.key);
            let populate = config.bodies.len() == 1;
            systems.push(authored_summary(id, &config, populate, cutoff)?);
            sources.push(DefinitionSource::Authored {
                config: Arc::new(config),
                populate,
            });
        }
        let enriched = crate::civilization::stars();
        let enriched_ids: HashMap<_, _> = enriched
            .iter()
            .enumerate()
            .map(|(index, star)| Ok((gaia_source_id(&star.id)?, index)))
            .collect::<Result<_>>()?;
        let companions: HashSet<_> = enriched
            .iter()
            .flat_map(|star| &star.companions)
            .map(|star| gaia_source_id(&star.id))
            .collect::<Result<_>>()?;
        let mut matched = HashSet::new();
        for star in StarCatalogue::embedded_records()? {
            if companions.contains(&star.id.value) {
                continue;
            }
            let id = catalogue_identity(star.id.namespace, star.id.value);
            if let Some(&enrichment) = enriched_ids.get(&star.id.value) {
                matched.insert(enrichment);
                let source = &enriched[enrichment];
                let name = &settlements.systems[10 + enrichment].name;
                systems.push(procedural_summary(id, source, name, cutoff));
                sources.push(DefinitionSource::Enriched(enrichment));
            } else {
                systems.push(catalogue_summary(id, &star, cutoff));
                sources.push(DefinitionSource::Catalogue {
                    source_id: star.id.value,
                    temperature_k: star.temperature_k,
                });
            }
        }
        for (enrichment, star) in enriched.iter().enumerate() {
            if matched.contains(&enrichment) {
                continue;
            }
            let id = catalogue_identity(2, gaia_source_id(&star.id)?);
            let name = &settlements.systems[10 + enrichment].name;
            systems.push(procedural_summary(id, star, name, cutoff));
            sources.push(DefinitionSource::Enriched(enrichment));
        }
        Self::assemble(systems, sources, cutoff, *fingerprint.finalize().as_bytes())
    }

    fn assemble(
        systems: Vec<SystemSummary>,
        sources: Vec<DefinitionSource>,
        cutoff: f64,
        fingerprint: [u8; 32],
    ) -> Result<Self> {
        ensure!(!systems.is_empty(), "universe needs at least one system");
        let mut identities = HashMap::with_capacity(systems.len());
        let mut names = HashMap::with_capacity(systems.len());
        let mut authored_bodies = HashMap::new();
        let mut entries = Vec::with_capacity(systems.len());
        for (index, (summary, source)) in systems.iter().zip(&sources).enumerate() {
            ensure!(
                identities.insert(summary.id, index).is_none(),
                "duplicate system identity"
            );
            ensure!(
                names.insert(summary.name.clone(), summary.id).is_none(),
                "duplicate system name {}",
                summary.name
            );
            entries.push(Entry {
                position: summary.position,
                luminosity: summary.luminosity,
                influence: summary.influence_bound.max(summary.capture_bound),
                radius: summary.star_radius,
            });
            if let DefinitionSource::Authored { config, .. } = source {
                for body in &config.bodies {
                    authored_bodies.insert(
                        body.name.clone(),
                        CelestialId {
                            system: summary.id,
                            local: local_body_identity(&body.key),
                        },
                    );
                }
            }
        }
        Ok(Self {
            maximum_capture_bound: systems
                .iter()
                .map(|system| system.capture_bound)
                .fold(0.0, f64::max),
            systems,
            sources,
            identities,
            names,
            authored_bodies,
            index: Arc::new(CatalogueIndex::new(entries)),
            fingerprint,
            cache: Mutex::new(DefinitionCache::default()),
            cutoff,
        })
    }

    pub fn system_index(&self, id: SystemId) -> Option<usize> {
        self.identities.get(&id).copied()
    }

    pub fn system_id_for_name(&self, name: &str) -> Option<SystemId> {
        self.names.get(name).copied()
    }

    pub fn authored_body(&self, name: &str) -> Option<CelestialId> {
        self.authored_bodies.get(name).copied()
    }

    pub fn reference_by_name(&self, system: &str, body: &str) -> Option<CelestialId> {
        self.resolve(self.system_id_for_name(system)?)
            .ok()?
            .body_id(body)
    }

    pub fn resolve(&self, id: SystemId) -> Result<Arc<SystemDefinition>> {
        self.resolve_index(self.system_index(id).context("unknown system identity")?)
    }

    pub fn resolve_index(&self, index: usize) -> Result<Arc<SystemDefinition>> {
        let summary = self.systems.get(index).context("unknown system index")?;
        if let Some(definition) = self.cache.lock().unwrap().get(summary.id) {
            return Ok(definition);
        }
        let mut config = match &self.sources[index] {
            DefinitionSource::Authored { config, populate } => {
                let mut config = (**config).clone();
                if *populate {
                    let identity = config.key.to_string();
                    generation::populate_bundled_system(&mut config, &identity)?;
                }
                config
            }
            DefinitionSource::Enriched(enrichment) => {
                generation::system(&crate::civilization::stars()[*enrichment], &summary.name)
            }
            DefinitionSource::Catalogue {
                source_id,
                temperature_k,
            } => {
                let star = CatalogueStar {
                    id: format!("gaia-dr3-{source_id}"),
                    name: summary.name.to_string(),
                    position_ly: [0.0; 3],
                    luminosity_solar: summary.luminosity / osg_stars::SOLAR_LUMENS,
                    temperature_k: *temperature_k,
                    companions: Vec::new(),
                };
                generation::system(&star, &summary.name)
            }
        };
        config.position_um = summary.position;
        let definition = Arc::new(SystemDefinition::new(summary.id, config, self.cutoff)?);
        ensure!(
            definition.influence <= summary.influence_bound * (1.0 + 1e-12),
            "generated system exceeds influence bound"
        );
        let mut cache = self.cache.lock().unwrap();
        if let Some(existing) = cache.get(summary.id) {
            return Ok(existing);
        }
        cache.insert(summary.id, definition.clone());
        Ok(definition)
    }

    pub fn body(&self, id: CelestialId) -> Option<Body> {
        self.resolve(id.system).ok()?.body(id.local).cloned()
    }

    pub fn solve_position(&self, id: CelestialId, epoch: Epoch) -> Option<GalacticPosition> {
        let definition = self.resolve(id.system).ok()?;
        definition
            .solver
            .solve_position(&definition.body(id.local)?.name, epoch)
    }

    pub fn solve_velocity(&self, id: CelestialId, epoch: Epoch) -> Option<DVec3> {
        let definition = self.resolve(id.system).ok()?;
        definition
            .solver
            .solve_velocity(&definition.body(id.local)?.name, epoch)
    }

    pub fn solve_rotation(&self, id: CelestialId, epoch: Epoch) -> Option<DQuat> {
        let definition = self.resolve(id.system).ok()?;
        definition
            .solver
            .solve_rotation(&definition.body(id.local)?.name, epoch)
    }

    pub fn atmospheric_velocity_at_point(
        &self,
        id: CelestialId,
        position: GalacticPosition,
        epoch: Epoch,
    ) -> Option<DVec3> {
        let definition = self.resolve(id.system).ok()?;
        definition.solver.atmospheric_velocity_at_point(
            &definition.body(id.local)?.name,
            position,
            epoch,
        )
    }

    pub fn gravity_applies(&self, id: CelestialId, position: GalacticPosition) -> bool {
        self.resolve(id.system).is_ok_and(|definition| {
            position
                .relative_to(definition.solver.anchor)
                .length_squared()
                <= definition.influence.powi(2)
        })
    }

    /// Conservative orbital envelopes for slip captures; exact moving-body
    /// intersections are evaluated by the caller after resolving each system.
    pub fn capture_candidates(
        &self,
        start: GalacticPosition,
        delta: DVec3,
        ship_radius: f64,
        budget: &mut osg_space::spatial::QueryBudget,
    ) -> Result<Vec<usize>, osg_space::spatial::QueryError> {
        self.index.spatial.segment_candidates_filtered(
            start,
            delta,
            ship_radius,
            self.maximum_capture_bound,
            budget,
            |index| Some(self.systems[index].capture_bound),
        )
    }

    pub fn containing_segment(&self, start: GalacticPosition, delta: DVec3) -> Vec<usize> {
        self.index
            .containing_segment(start, delta)
            .into_iter()
            .filter(|&index| {
                let Ok(definition) = self.resolve_index(index) else {
                    return false;
                };
                let offset = definition.solver.anchor.relative_to(start);
                let t = if delta.length_squared() > 0.0 {
                    (offset.dot(delta) / delta.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (offset - delta * t).length_squared() <= definition.influence.powi(2)
            })
            .collect()
    }

    pub fn cached_definitions(&self) -> usize {
        self.cache.lock().unwrap().entries.len()
    }
}

pub fn system_identity(key: &str) -> SystemId {
    blake3::derive_key("OpenSpaceGame system identity v2", key.as_bytes())[..16]
        .try_into()
        .unwrap()
}

pub fn local_body_identity(key: &str) -> LocalBodyId {
    blake3::derive_key("OpenSpaceGame local body identity v2", key.as_bytes())[..16]
        .try_into()
        .unwrap()
}

fn catalogue_identity(namespace: u64, value: u64) -> SystemId {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&namespace.to_le_bytes());
    bytes[8..].copy_from_slice(&value.to_le_bytes());
    bytes
}

fn gaia_source_id(id: &str) -> Result<u64> {
    id.strip_prefix("gaia-edr3-")
        .context("unsupported enrichment identity")?
        .parse()
        .context("invalid source identity")
}

fn stellar_luminosity(body: &Body) -> f64 {
    match body.class_params {
        BodyClass::Star { lumens } => lumens,
        _ => 0.0,
    }
}

fn catalogue_summary(id: SystemId, star: &Star, cutoff: f64) -> SystemSummary {
    let properties = generation::stellar_properties(
        star.luminosity / osg_stars::SOLAR_LUMENS,
        star.temperature_k,
    );
    let extent = properties.radius + 240.0 * 149_597_870_700.0 + 6.957e8;
    let mass = properties.mass + 11_000.0 * 5.9722e24;
    SystemSummary {
        id,
        primary: CelestialId {
            system: id,
            local: local_body_identity(&format!("gaia-dr3-{}", star.id.value)),
        },
        name: format!("Gaia DR3 {}", star.id.value).into(),
        position: star.position,
        influence_bound: extent + (crate::physics::GRAVITATIONAL_CONSTANT * mass / cutoff).sqrt(),
        capture_bound: extent + 0.008 * 149_597_870_700.0 * (mass / 1.98847e30).cbrt(),
        star_radius: properties.radius,
        stellar_mass: properties.mass,
        luminosity: star.luminosity,
        temperature_k: star.temperature_k,
        colour: star.colour,
    }
}

fn procedural_summary(
    id: SystemId,
    star: &CatalogueStar,
    name: &str,
    cutoff: f64,
) -> SystemSummary {
    let properties = generation::stellar_properties(star.luminosity_solar, star.temperature_k);
    let (extent, mass) = generation::system_bounds(star);
    let luminosity = (star.luminosity_solar
        + star
            .companions
            .iter()
            .map(|c| c.luminosity_solar)
            .sum::<f64>())
        * osg_stars::SOLAR_LUMENS;
    SystemSummary {
        id,
        primary: CelestialId {
            system: id,
            local: local_body_identity(&star.id),
        },
        name: name.into(),
        position: GalacticPosition::from_meters(
            DVec3::from_array(star.position_ly) * crate::civilization::LIGHT_YEAR_M,
        ),
        influence_bound: extent + (crate::physics::GRAVITATIONAL_CONSTANT * mass / cutoff).sqrt(),
        capture_bound: extent + 0.008 * 149_597_870_700.0 * (mass / 1.98847e30).cbrt(),
        star_radius: properties.radius,
        stellar_mass: properties.mass,
        luminosity,
        temperature_k: star.temperature_k,
        colour: {
            let colour = properties.spectral_class.linear_rgb();
            let luminance = 0.2126 * colour[0] + 0.7152 * colour[1] + 0.0722 * colour[2];
            colour.map(|channel| channel / luminance)
        },
    }
}

fn authored_summary(
    id: SystemId,
    config: &OrreryCfg,
    populate: bool,
    cutoff: f64,
) -> Result<SystemSummary> {
    ensure!(!config.key.is_empty(), "authored system needs a stable key");
    let definition = SystemDefinition::new(id, config.clone(), cutoff)?;
    let star = definition.solver.get_body(&definition.star_name).unwrap();
    let extra_extent = if populate {
        240.0 * 149_597_870_700.0
    } else {
        0.0
    };
    let extra_gravity = if populate {
        (crate::physics::GRAVITATIONAL_CONSTANT * 11_000.0 * 5.9722e24 / cutoff).sqrt()
    } else {
        0.0
    };
    Ok(SystemSummary {
        id,
        primary: CelestialId {
            system: id,
            local: local_body_identity(&star.key),
        },
        name: config.name.clone(),
        position: config.position_um,
        influence_bound: definition.influence + extra_extent + extra_gravity,
        capture_bound: definition.capture_bound
            + extra_extent
            + if populate {
                0.008 * 149_597_870_700.0 * (11_000.0_f64 * 5.9722e24 / 1.98847e30).cbrt()
            } else {
                0.0
            },
        star_radius: star.radius,
        stellar_mass: star.mass,
        luminosity: definition.solver.iter().map(stellar_luminosity).sum(),
        temperature_k: star.stellar.as_ref().map_or_else(
            || {
                ((stellar_luminosity(star) / 93.0)
                    / (4.0 * std::f64::consts::PI * star.radius.powi(2) * 5.670374419e-8))
                    .powf(0.25)
            },
            |parameters| parameters.effective_temperature_k,
        ),
        colour: star_colour(star),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_survive_display_renames_and_resolve_without_global_body_names() {
        let original = crate::example_config();
        let first = Universe::init(original.clone()).unwrap();
        let reference = first
            .reference_by_name(&original.name, &original.bodies[1].name)
            .unwrap();
        let mut renamed = original.clone();
        renamed.name = "Renamed system".into();
        let prior_name = renamed.bodies[1].name.clone();
        renamed.bodies[1].name = "Renamed planet".into();
        for body in &mut renamed.bodies {
            if body.parent.as_ref() == Some(&prior_name) {
                body.parent = Some("Renamed planet".into());
            }
        }
        let second = Universe::init(renamed).unwrap();
        assert_eq!(
            reference,
            second
                .reference_by_name("Renamed system", "Renamed planet")
                .unwrap()
        );
        let epoch = Epoch::from_mjd_utc(42.0);
        assert_eq!(
            first.solve_position(reference, epoch),
            second.solve_position(reference, epoch)
        );

        let mut remote = crate::remote_test_config();
        let previous = remote.bodies[0].name.clone();
        remote.bodies[0].name = original.bodies[0].name.clone();
        for body in &mut remote.bodies {
            if body.parent.as_ref() == Some(&previous) {
                body.parent = Some(original.bodies[0].name.clone());
            }
        }
        let combined = Universe::from_configs(vec![original, remote], 1e-8).unwrap();
        assert_ne!(
            combined.resolve_index(0).unwrap().id,
            combined.resolve_index(1).unwrap().id
        );
    }

    #[test]
    fn cache_eviction_preserves_external_references_and_regeneration_at_current_epoch() {
        let configs = (0..140)
            .map(|index| {
                let mut config = crate::example_config();
                config.key = format!("test/system/{index}").into();
                config.name = format!("System {index}").into();
                config
            })
            .collect();
        let universe = Universe::from_configs(configs, 1e-8).unwrap();
        assert_eq!(universe.cached_definitions(), 0);
        let pinned = universe.resolve_index(0).unwrap();
        let reference = pinned.body_id("Helion I Neris").unwrap();
        let epoch = Epoch::from_mjd_utc(100.0);
        let position = universe.solve_position(reference, epoch).unwrap();
        for index in 1..140 {
            universe.resolve_index(index).unwrap();
        }
        assert_eq!(universe.cached_definitions(), 128);
        assert!(Arc::ptr_eq(&pinned, &universe.resolve_index(0).unwrap()));
        drop(pinned);
        assert_eq!(universe.solve_position(reference, epoch).unwrap(), position);
        assert_ne!(
            universe
                .solve_position(reference, Epoch::from_mjd_utc(0.0))
                .unwrap(),
            position
        );
        assert!(universe.cached_definitions() <= 128);
    }

    #[test]
    fn companion_bounds_cover_generated_orbits_and_gravity() {
        for source in crate::civilization::stars()
            .iter()
            .filter(|star| !star.companions.is_empty())
        {
            let id = system_identity(&source.id);
            let summary = procedural_summary(id, source, &source.name, 1e-8);
            let definition =
                SystemDefinition::new(id, generation::system(source, &source.name), 1e-8).unwrap();
            assert!(
                definition.influence <= summary.influence_bound,
                "{} exceeds bounds",
                source.id
            );
            for body in definition.solver.iter() {
                for days in [0.0, 10_000.0, 1e8] {
                    let position = definition
                        .solver
                        .solve_position(&body.name, Epoch::from_mjd_utc(days))
                        .unwrap();
                    let exclusion = 0.008 * 149_597_870_700.0 * (body.mass / 1.98847e30).cbrt();
                    assert!(
                        position.relative_to(summary.position).length()
                            + body.radius.max(exclusion)
                            <= summary.capture_bound
                    );
                }
            }
        }
    }
}
