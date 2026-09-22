pub mod bootstrap;
pub mod chat;
pub mod combat;
pub mod commands;
pub mod diagnostics;
pub mod displays;
#[cfg(test)]
mod firmware_tests;
pub mod gas;
pub mod hardware;
pub mod industry;
pub mod infrastructure;
pub mod presentation;
pub mod registry;
pub mod route_service;
pub mod routing;
pub mod services;
pub mod session;
pub mod travel;
pub use bootstrap::{ScenarioConfig, apply_debug_requests, provision};
pub mod identity;
pub mod orrery;
pub mod ownership;
pub mod physics;
pub mod precision;
pub mod scenario;
pub mod sensors;
pub mod simulation;
pub mod slip_effects;
pub mod spatial;
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
        .init_resource::<chat::ChatService>()
        .init_resource::<travel::TravelEvents>()
        .init_resource::<slip_effects::SlipHistory>()
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
    route_service::install(&mut app);
    registry::initialize(app.world_mut()).expect("valid universe catalogue");
    app.add_systems(
        FixedUpdate,
        (
            services::publish_indexes,
            chat::refresh,
            services::prepare_sources,
        )
            .chain()
            .before(simulation::SimulationSystems::PrepareBodies),
    );
    app.add_systems(
        FixedPostUpdate,
        (services::dispatch_actions, chat::flush)
            .chain()
            .before(simulation::SimulationSystems::Integrate),
    );
    app.add_systems(
        FixedFirst,
        (travel::advance, slip_effects::prune, travel::plan_orders)
            .chain()
            .run_if(in_state(GameState::Game)),
    );
    app.add_systems(
        FixedLast,
        (
            identity::identify_celestials,
            identity::clean_indexes,
            sensors::publish,
            combat::flush_travel,
        )
            .chain()
            .in_set(simulation::SimulationSystems::Observations)
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
        let account = osg_model::Id::new();
        let mut app = provision(&[account], None, None).unwrap();
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
