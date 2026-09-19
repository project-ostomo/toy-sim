use super::{identity::AppearanceAssets, orrery::Universe};
use bevy::prelude::*;
use smol_str::SmolStr;
use std::{collections::HashMap, sync::Arc};
use toy_sim_model::{
    CelestialSystemRef, Id, Pose, UniverseBody, UniverseCatalogue, UniverseSystem, ViewState,
};

#[derive(Resource, Clone)]
pub struct UniverseRegistry {
    pub universe: Arc<toy_sim_universe::universe::Universe>,
    pub names: Arc<HashMap<Id, SmolStr>>,
    pub catalogue: [u8; 32],
    pub definitions: Arc<Vec<(Id, [u8; 32])>>,
}

pub fn identity(name: &str) -> Id {
    let hash = blake3::derive_key("toy-sim celestial identity v1", name.as_bytes());
    let mut bytes: [u8; 16] = hash[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id(bytes)
}

pub fn system_identity(name: &str) -> Id {
    let hash = blake3::derive_key("toy-sim system identity v1", name.as_bytes());
    let mut bytes: [u8; 16] = hash[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id(bytes)
}

pub fn gate_identity(local_catalogue: &str, remote_catalogue: &str) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("toy-sim gate mouth identity v1");
    hash.update(local_catalogue.as_bytes());
    hash.update(&[0]);
    hash.update(remote_catalogue.as_bytes());
    Id(hash.finalize().as_bytes()[..16].try_into().unwrap())
}

struct PreparedRegistry {
    registry: UniverseRegistry,
    assets: HashMap<[u8; 32], Vec<u8>>,
}

static BUNDLED_REGISTRY: std::sync::OnceLock<PreparedRegistry> = std::sync::OnceLock::new();

pub fn initialize(world: &mut World) -> anyhow::Result<()> {
    let universe = world.resource::<Universe>().clone();
    let prepared = if universe.is_bundled() {
        BUNDLED_REGISTRY
            .get_or_init(|| prepare(universe.0.clone()).expect("valid bundled catalogue"))
    } else {
        return install(world, &prepare(universe.0.clone())?);
    };
    install(world, prepared)
}

fn install(world: &mut World, prepared: &PreparedRegistry) -> anyhow::Result<()> {
    world
        .resource::<AppearanceAssets>()
        .extend(prepared.assets.clone());
    world.insert_resource(prepared.registry.clone());
    Ok(())
}

fn prepare(
    universe: Arc<toy_sim_universe::universe::Universe>,
) -> anyhow::Result<PreparedRegistry> {
    let mut assets = HashMap::new();
    let mut names = HashMap::new();
    let mut definitions = Vec::new();
    let mut catalogue = UniverseCatalogue {
        systems: Vec::new(),
    };
    for (system_index, system) in universe.systems.iter().enumerate() {
        let mut bodies = Vec::new();
        for body in system.solver.iter() {
            let id = identity(&body.name);
            names.insert(id, body.name.clone());
            bodies.push(UniverseBody {
                id,
                name: body.name.to_string(),
                kind: match body.class_params {
                    super::orrery::BodyClass::Star { .. } => "star",
                    super::orrery::BodyClass::Planet => "planet",
                    super::orrery::BodyClass::Barycenter => "barycenter",
                }
                .into(),
                radius_m: body.radius,
                mass_kg: body.mass,
                parent: body.parent.as_ref().map(|name| identity(name)),
            });
        }
        let system_id = system_identity(&system.solver.name);
        let asset = toy_sim_universe::replication::SystemAsset {
            version: 1,
            system_id: system_id.0,
            body_ids: system
                .solver
                .iter()
                .map(|body| toy_sim_universe::replication::BodyIdentity {
                    name: body.name.to_string(),
                    id: identity(&body.name).0,
                })
                .collect(),
            config: toy_sim_universe::orrery_cfg::OrreryCfg {
                name: system.solver.name.clone(),
                position_um: system.solver.anchor,
                bodies: system.solver.iter().cloned().collect(),
            },
        };
        let bytes = asset.encode()?;
        let hash = *blake3::hash(&bytes).as_bytes();
        assets.insert(hash, bytes);
        definitions.push((system_id, hash));
        catalogue.systems.push(UniverseSystem {
            id: system_identity(&system.solver.name),
            name: system.solver.name.to_string(),
            position: system.solver.anchor,
            influence_radius_m: universe.index.entries[system_index].influence,
            bodies,
        });
    }
    let bytes = postcard::to_stdvec(&catalogue)?;
    let hash = *blake3::hash(&bytes).as_bytes();
    assets.insert(hash, bytes);
    Ok(PreparedRegistry {
        registry: UniverseRegistry {
            universe,
            names: Arc::new(names),
            catalogue: hash,
            definitions: Arc::new(definitions),
        },
        assets,
    })
}

impl UniverseRegistry {
    pub fn celestial_ref(&self, view: u64, body: Id) -> Option<CelestialSystemRef> {
        let name = self.names.get(&body)?;
        self.system_ref(view, self.universe.system_for(name)?)
    }

    fn system_ref(&self, view: u64, index: usize) -> Option<CelestialSystemRef> {
        let &(system, definition) = self.definitions.get(index)?;
        Some(CelestialSystemRef {
            view,
            system,
            definition,
            epoch_mjd_utc: toy_sim_universe::replication::SIMULATION_EPOCH_MJD_UTC,
            sim_time_origin_ns: 0,
        })
    }

    pub fn system_refs(
        &self,
        views: &mut [ViewState],
        inspected: Option<&str>,
    ) -> Vec<CelestialSystemRef> {
        let mut result = Vec::new();
        for view in views {
            let mut systems: std::collections::BTreeSet<_> = self
                .universe
                .index
                .containing_segment(view.origin, bevy::math::DVec3::ZERO)
                .into_iter()
                .collect();
            if let Some(index) = inspected.and_then(|name| self.universe.system_for(name)) {
                systems.insert(index);
            }
            let mut systems: Vec<_> = systems.into_iter().collect();
            systems.sort_by(|&a, &b| {
                let inspected_system = inspected.and_then(|name| self.universe.system_for(name));
                let distance = |index: usize| {
                    self.universe.systems[index]
                        .solver
                        .anchor
                        .relative_to(view.origin)
                        .length_squared()
                };
                (Some(a) != inspected_system)
                    .cmp(&(Some(b) != inspected_system))
                    .then_with(|| distance(a).total_cmp(&distance(b)))
                    .then(a.cmp(&b))
            });
            if systems.len() > 32 {
                systems.truncate(32);
                view.completion = toy_sim_model::Completion::ResultLimit;
            }
            for index in systems {
                result.push(self.system_ref(view.id, index).expect("registered system"));
            }
        }
        result
    }

    pub fn pose(&self, id: Id, epoch: hifitime::Epoch) -> Option<Pose> {
        let name = self.names.get(&id)?;
        let position = self.universe.solve_position(name, epoch)?;
        let velocity = self.universe.solve_velocity(name, epoch)?;
        let rotation = self.universe.solve_rotation(name, epoch)?;
        let dt = hifitime::Duration::from_seconds(0.01);
        let next = self.universe.solve_rotation(name, epoch + dt)?;
        let angular_velocity = (next * rotation.inverse()).to_scaled_axis() / 0.01;
        Some(Pose {
            position,
            rotation: rotation.to_array(),
            velocity: velocity.to_array(),
            angular_velocity: angular_velocity.to_array(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::{Completion, GalacticPosition};

    #[test]
    fn inactive_remote_system_is_published_for_its_view_without_sensor_contacts() {
        let mut remote = toy_sim_universe::example_config();
        remote.name = "Remote".into();
        remote.position_um = GalacticPosition::from_meters(bevy::math::DVec3::new(1e18, 0., 0.));
        for body in &mut remote.bodies {
            body.name = format!("Remote {}", body.name).into();
            body.parent = body
                .parent
                .as_ref()
                .map(|name| format!("Remote {name}").into());
        }
        let remote_position = remote.position_um;
        let mut world = World::new();
        world.insert_resource(
            Universe::from_configs(
                vec![toy_sim_universe::example_config(), remote],
                super::super::physics::GRAVITY_CUTOFF,
            )
            .unwrap(),
        );
        world.insert_resource(AppearanceAssets::default());
        initialize(&mut world).unwrap();
        let registry = world.resource::<UniverseRegistry>();
        let mut views = vec![ViewState {
            focused_ship: None,
            origin: remote_position,
            id: 7,
            revision: 0,
            group: Id([0; 16]),
            tracks: Vec::new(),
            completion: Completion::Complete,
        }];
        let references = registry.system_refs(&mut views, None);
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].system, system_identity("Remote"));
        assert_eq!(references[0].view, 7);
        let bytes = world
            .resource::<AppearanceAssets>()
            .get(&references[0].definition)
            .unwrap();
        assert_eq!(*blake3::hash(&bytes).as_bytes(), references[0].definition);
        let asset = toy_sim_universe::replication::SystemAsset::decode(&bytes).unwrap();
        let solver = asset.solver().unwrap();
        assert_eq!(solver.anchor, remote_position);
        assert_eq!(asset.body_ids.len(), solver.iter().count());
        let epoch = hifitime::Epoch::from_mjd_utc(references[0].epoch_mjd_utc)
            + hifitime::Duration::from_seconds(0.125);
        for mapping in asset.body_ids {
            let expected = registry.pose(Id(mapping.id), epoch).unwrap();
            assert_eq!(
                expected.position,
                solver.solve_position(mapping.name.as_str(), epoch).unwrap()
            );
            assert_eq!(
                expected.velocity,
                solver
                    .solve_velocity(mapping.name.as_str(), epoch)
                    .unwrap()
                    .to_array()
            );
        }
    }

    #[test]
    fn overlapping_systems_have_bounded_explicit_completion() {
        let template = toy_sim_universe::example_config();
        let mut configs = Vec::new();
        for index in 0..33 {
            let mut config = template.clone();
            config.name = format!("System {index}").into();
            config.bodies.retain(|body| body.parent.is_none());
            config.bodies[0].name = format!("Star {index}").into();
            configs.push(config);
        }
        let universe = toy_sim_universe::universe::Universe::from_configs(
            configs,
            super::super::physics::GRAVITY_CUTOFF,
        )
        .unwrap();
        let definitions = universe
            .systems
            .iter()
            .map(|system| (system_identity(&system.solver.name), [0; 32]))
            .collect();
        let registry = UniverseRegistry {
            universe: Arc::new(universe),
            names: Arc::new(HashMap::new()),
            catalogue: [0; 32],
            definitions: Arc::new(definitions),
        };
        let mut views = [ViewState {
            focused_ship: None,
            origin: GalacticPosition::ZERO,
            id: 0,
            revision: 0,
            group: Id([0; 16]),
            tracks: Vec::new(),
            completion: Completion::Complete,
        }];
        let references = registry.system_refs(&mut views, Some("Star 32"));
        assert_eq!(references.len(), 32);
        assert_eq!(views[0].completion, Completion::ResultLimit);
        assert_eq!(references[0].system, system_identity("System 32"));
    }
}
