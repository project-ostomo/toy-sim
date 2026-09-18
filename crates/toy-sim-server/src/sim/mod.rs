pub mod bootstrap;
pub mod combat;
pub mod displays;
#[cfg(test)]
mod firmware_tests;
pub mod hardware;
pub mod infrastructure;
pub mod presentation;
pub mod registry;
pub mod services;
pub mod session;
pub mod travel;
pub use bootstrap::{ScenarioConfig, apply_debug_requests, provision};
pub mod identity;
pub mod intelligence;
pub mod orrery;
pub mod physics;
pub mod precision;
pub mod scenario;
pub mod sensors;
pub mod simulation;
pub mod spatial;
pub mod spatial_tree;
pub mod vessel;

use bevy::{prelude::*, state::app::StatesPlugin, time::TimeUpdateStrategy};
use std::time::Duration;

#[derive(Clone, Eq, PartialEq, Debug, Hash, Default, States)]
pub enum GameState {
    #[default]
    Loading,
    Game,
}

pub fn application(ship: Option<std::path::PathBuf>) -> App {
    let mut app = App::new();
    identity::initialize(app.world_mut(), &[]);
    app.init_resource::<session::Clock>()
        .init_resource::<session::Events>()
        .init_resource::<travel::TravelEvents>()
        .init_resource::<services::PublishedWorld>();
    app.add_plugins((MinimalPlugins, StatesPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )))
        .insert_resource(vessel::ShipLaunch(ship))
        .init_state::<GameState>()
        .add_plugins((
            simulation::SimulationPlugin,
            spatial::SpatialPlugin,
            sensors::SensorsPlugin,
            orrery::OrreryPlugin,
            physics::PhysicsPlugin,
            vessel::VesselsPlugin,
        ));
    registry::initialize(app.world_mut()).expect("valid universe catalogue");
    app.add_systems(
        FixedUpdate,
        (services::publish_indexes, services::prepare_sources)
            .chain()
            .before(simulation::SimulationSystems::PrepareBodies),
    );
    app.add_systems(
        FixedPostUpdate,
        infrastructure::move_gates.in_set(simulation::SimulationSystems::Celestials),
    );
    app.add_systems(
        FixedPostUpdate,
        services::dispatch_actions.before(simulation::SimulationSystems::Integrate),
    );
    app.add_systems(
        FixedFirst,
        travel::advance.run_if(in_state(GameState::Game)),
    );
    app.add_systems(
        FixedLast,
        (
            intelligence::collect_unused_groups,
            travel::geometry::refresh,
            identity::identify_celestials,
            identity::clean_indexes,
            intelligence::acquire,
            intelligence::coast,
            intelligence::fuse,
            intelligence::publish,
            combat::flush_travel,
        )
            .chain()
            .in_set(simulation::SimulationSystems::Intelligence)
            .after(simulation::SimulationSystems::Complete)
            .run_if(in_state(GameState::Game)),
    );
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::Game);
    app
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_ecs_runs_orbital_ships_without_rendering() {
        let mut app = application(None);
        app.update();
        app.update();
        let ship = app
            .world_mut()
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(app.world())
            .unwrap();
        let initial = *app
            .world()
            .get::<precision::PreciseTransform>(ship)
            .unwrap();

        for _ in 0..60 {
            app.update();
        }

        let world = app.world();
        let pose = world.get::<precision::PreciseTransform>(ship).unwrap();
        assert!(
            pose.translation_um
                .relative_to(initial.translation_um)
                .length()
                > 1.0
        );
        assert!(world.get::<physics::MassProps>(ship).unwrap().mass > 0.0);
        assert!(world.get::<hardware::Hull>(ship).unwrap().0 > 0.0);
        assert!(
            !world
                .get::<vessel::ShipSoftware>(ship)
                .unwrap()
                .controller
                .is_booting()
        );
        assert!(world.resource::<simulation::SimulationCounters>().ticks >= 60);
        assert!(world.contains_resource::<physics::collision::CollisionReport>());
    }
}
