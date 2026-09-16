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
            FixedPostUpdate,
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
    let configs: Vec<_> = asset
        .systems
        .iter()
        .map(|h| cfgs.get(h).expect("system asset missing").clone())
        .collect();
    let universe =
        Universe::from_configs(configs, crate::physics::GRAVITY_CUTOFF).expect("invalid universe");
    info!(
        "Loaded {} systems; celestial orbits remain inactive until needed",
        universe.systems.len()
    );
    commands.insert_resource(activity::CelestialMesh(
        meshes.add(Sphere::new(1.0).mesh().uv(128, 64)),
    ));
    commands.insert_resource(universe);
}
// Forces have already sampled the old ephemerides; publish end-of-tick poses now.
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
    toml::from_str(include_str!("../../../assets/stars/helion.star.toml")).unwrap()
}

#[cfg(test)]
pub(crate) fn remote_test_config() -> OrreryCfg {
    toml::from_str(include_str!("../../../tests/fixtures/remote.star.toml")).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::GRAVITATIONAL_CONSTANT;

    #[test]
    fn gravity_samples_start_positions_before_ephemerides_advance() {
        use crate::{
            physics::{PhysicsPlugin, RigidBody, Velocity},
            simulation::SimulationPlugin,
        };
        use bevy::{math::DVec3, state::app::StatesPlugin, time::TimeUpdateStrategy};
        let universe = Universe::init(example_config()).unwrap();
        let name = crate::scenario::INITIAL_SCENARIO.body;
        let system = universe.system_for(name).unwrap();
        let definition = &universe.systems[system];
        let epoch = hifitime::Epoch::from_mjd_utc(0.0);
        let initial = universe.solve_position(name, epoch).unwrap();
        let final_position = universe
            .solve_position(name, epoch + hifitime::Duration::from_seconds(0.1))
            .unwrap();
        let body = universe.get_body(name).unwrap().clone();
        let mu = GRAVITATIONAL_CONSTANT * body.mass;
        let state = activity::CelestialState {
            body,
            system,
            anchor: definition.solver.anchor,
            influence: definition.influence,
            velocity: universe.solve_velocity(name, epoch).unwrap(),
        };
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            SimulationPlugin,
            PhysicsPlugin,
        ))
        .insert_state(GameState::Game)
        .insert_resource(universe)
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(100),
        ))
        .add_systems(
            FixedPostUpdate,
            move_orrery.in_set(SimulationSystems::Celestials),
        );
        let planet = app
            .world_mut()
            .spawn((
                Celestial(name.into()),
                state,
                PreciseTransform {
                    translation_um: initial,
                    ..default()
                },
            ))
            .id();
        let distance: f64 = 10_000_000.0;
        let ship = app
            .world_mut()
            .spawn((
                RigidBody,
                PreciseTransform {
                    translation_um: initial.offset_by(DVec3::Z * distance),
                    ..default()
                },
            ))
            .id();
        app.update(); // First clock update establishes the epoch.
        app.update(); // First fixed tick advances 0.0 -> 0.1 s.
        let expected_velocity = -DVec3::Z * (mu / distance.powi(2)) * 0.1;
        assert!(
            app.world()
                .get::<Velocity>(ship)
                .unwrap()
                .0
                .abs_diff_eq(expected_velocity, 1e-12)
        );
        assert_eq!(
            app.world()
                .get::<PreciseTransform>(planet)
                .unwrap()
                .translation_um,
            final_position
        );
        assert_eq!(
            app.world()
                .get::<PreciseTransform>(ship)
                .unwrap()
                .translation_um,
            initial
                .offset_by(DVec3::Z * distance)
                .offset_by(expected_velocity * 0.1)
        );
    }

    #[test]
    fn startup_scenario_is_circular_in_vacuum_and_moons_are_airless() {
        let system = Universe::init(example_config()).unwrap();
        let scenario = &crate::scenario::INITIAL_SCENARIO;
        scenario.validate(&system).unwrap();
        let planet = system.get_body(&scenario.body).unwrap();
        let (position, velocity) = scenario.relative_state(planet.radius, planet.mass).unwrap();
        assert!(position.normalize().dot(velocity.normalize()).abs() < 1e-12);
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
        let moons: Vec<_> = system
            .iter()
            .filter(|b| b.parent.as_deref() == Some(planet.name.as_str()))
            .collect();
        assert_eq!(moons.len(), 5);
        assert!(moons.iter().all(|moon| moon.atmosphere.is_none()));
    }
}
