pub mod activity;
pub mod atmosphere;
pub mod catalogue;
mod orrery_cfg;
mod solver;
pub mod universe;
use crate::{
    GameState,
    assets::TomlAssetLoader,
    physics::sim_time,
    precision::{InterpolatedTransform, PreciseTransform},
    simulation::SimulationSystems,
};
use bevy::prelude::*;
use bevy_asset_loader::{
    asset_collection::AssetCollection,
    loading_state::{
        LoadingStateAppExt,
        config::{ConfigureLoadingState, LoadingStateConfig},
    },
};
pub use orrery_cfg::BodyClass;
use orrery_cfg::OrreryCfg;
use smol_str::SmolStr;
pub use universe::Universe;

pub struct OrreryPlugin;
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoadOrrery;
impl Plugin for OrreryPlugin {
    fn build(&self, app: &mut App) {
        app.configure_loading_state(
            LoadingStateConfig::new(GameState::Loading).load_collection::<StarSysAssets>(),
        )
        .init_asset::<OrreryCfg>()
        .register_asset_loader(TomlAssetLoader::<OrreryCfg>::new("star.toml"))
        .init_asset::<universe::UniverseAsset>()
        .register_asset_loader(universe::UniverseLoader)
        .init_resource::<activity::ActiveSystems>()
        .init_resource::<activity::UniverseDebug>()
        .add_systems(OnEnter(GameState::Game), load_orrery.in_set(LoadOrrery))
        .add_systems(
            FixedUpdate,
            (activity::relocate, activity::activate)
                .chain()
                .before(SimulationSystems::History)
                .run_if(in_state(GameState::Game)),
        )
        .add_systems(
            FixedUpdate,
            move_orrery
                .in_set(SimulationSystems::Celestials)
                .run_if(in_state(GameState::Game)),
        );
    }
}
#[derive(Resource, AssetCollection)]
struct StarSysAssets {
    #[asset(path = "universe.toml")]
    universe: Handle<universe::UniverseAsset>,
}
fn load_orrery(
    mut commands: Commands,
    handles: Res<StarSysAssets>,
    universes: Res<Assets<universe::UniverseAsset>>,
    cfgs: Res<Assets<OrreryCfg>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let asset = universes
        .get(&handles.universe)
        .expect("universe asset missing");
    let mut configs: Vec<_> = asset
        .systems
        .iter()
        .map(|h| cfgs.get(h).expect("system asset missing").clone())
        .collect();
    if let Some(config) = &asset.cfg.gaia {
        commands.insert_resource(crate::gaia::GaiaCatalogue::new(config.clone()));
    }
    let origin = configs
        .iter()
        .find(|c| c.name == asset.cfg.starting_system)
        .expect("starting system missing")
        .position_um;
    configs.extend(universe::generate(&asset.cfg.synthetic, origin));
    let universe = Universe::from_configs(
        configs,
        &asset.cfg.starting_system,
        asset.cfg.gravity_cutoff,
    )
    .expect("invalid universe");
    assert!(
        universe.scenario.is_some(),
        "starting system needs a scenario"
    );
    info!(
        "Loaded {} systems; celestial orbits remain inactive until needed",
        universe.systems.len()
    );
    commands.insert_resource(crate::starfield::SkySettings(asset.cfg.sky));
    commands.insert_resource(activity::CelestialMesh(
        meshes.add(Sphere::new(1.0).mesh().uv(128, 64)),
    ));
    commands.insert_resource(universe);
}
fn move_orrery(
    universe: Res<Universe>,
    time: Res<Time<Fixed>>,
    mut bodies: Query<(
        &Celestial,
        &mut PreciseTransform,
        &mut activity::CelestialState,
    )>,
) {
    let epoch = sim_time(&time);
    for (body, mut pose, mut state) in &mut bodies {
        let solver = &universe.systems[state.system].solver;
        pose.translation_um = solver.solve_position(&body.0, epoch).unwrap();
        pose.rotation = solver.solve_rotation(&body.0, epoch).unwrap();
        state.velocity = solver.solve_velocity(&body.0, epoch).unwrap();
    }
}
#[derive(Component, Default)]
#[require(InterpolatedTransform)]
pub struct Celestial(pub SmolStr);

#[derive(Component)]
#[require(Celestial)]
pub struct Star {
    pub lumens: f64,
    pub color_temp: f64,
}

#[cfg(test)]
pub(crate) fn example_config() -> OrreryCfg {
    toml::from_str(include_str!("../assets/stars/helion.star.toml")).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{physics::GRAVITATIONAL_CONSTANT, precision::ToMicrometersExt};

    #[test]
    fn configured_scenario_is_circular_in_vacuum_with_five_visible_airless_moons() {
        let system = Universe::init(example_config()).unwrap();
        let scenario = system.scenario.as_ref().unwrap();
        let planet = system.get_body(&scenario.body).unwrap();
        let (position, velocity) = scenario.relative_state(planet.radius, planet.mass).unwrap();
        assert!(position.dot(velocity).abs() < 1e-6);
        assert!(
            (velocity.length_squared() * position.length()
                / (GRAVITATIONAL_CONSTANT * planet.mass)
                - 1.0)
                .abs()
                < 1e-12
        );
        assert_eq!(
            planet
                .atmosphere
                .as_ref()
                .unwrap()
                .density(scenario.altitude),
            0.0
        );
        assert_eq!(system.iter().filter(|b| b.atmosphere.is_some()).count(), 1);
        let epoch = sim_time(&Time::<Fixed>::default());
        let ship = system.solve_position(&planet.name, epoch).unwrap() + position.to_micrometers();
        let moons: Vec<_> = system
            .iter()
            .filter(|b| b.parent.as_deref() == Some(planet.name.as_str()))
            .collect();
        assert_eq!(moons.len(), 5);
        let half_height = (std::f64::consts::PI / 8.0).tan();
        for moon in moons {
            assert!(moon.atmosphere.is_none());
            let offset = (system.solve_position(&moon.name, epoch).unwrap() - ship).to_meters_64();
            assert!(offset.z < 0.0);
            assert!(
                offset.y.abs() + moon.radius < -offset.z * half_height,
                "{} outside vertical view",
                moon.name
            );
            assert!(
                offset.x.abs() + moon.radius < -offset.z * half_height * 16.0 / 9.0,
                "{} outside horizontal view",
                moon.name
            );
            let projected_separation = offset.truncate().length();
            assert!(
                projected_separation > planet.radius + moon.radius,
                "{} hidden behind planet",
                moon.name
            );
        }
    }
}
