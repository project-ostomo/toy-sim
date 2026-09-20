pub mod bootstrap;
pub mod chat;
pub mod combat;
pub mod commands;
pub mod defense;
pub mod diagnostics;
pub mod displays;
#[cfg(test)]
mod firmware_tests;
pub mod gas;
pub mod hardware;
pub mod industry;
pub mod infrastructure;
pub mod llm;
pub mod missiles;
pub mod npc;
pub mod presentation;
pub mod registry;
pub mod route_service;
pub mod routing;
pub mod services;
pub mod session;
pub mod travel;
pub use bootstrap::{ScenarioConfig, apply_debug_requests, provision};
pub mod identity;
pub mod intelligence;
pub mod orrery;
pub mod ownership;
pub mod physics;
pub mod precision;
pub mod scenario;
pub mod sensors;
pub mod simulation;
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
    missiles::install(&mut app);
    defense::install(&mut app);
    npc::install(&mut app);
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
        (travel::advance, travel::plan_orders)
            .chain()
            .run_if(in_state(GameState::Game)),
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
    #[ignore = "full seeded world performance measurement"]
    fn seeded_world_performance() {
        let account = osg_model::Id::new();
        let mut app = provision(&[account], Some(account), None).unwrap();
        npc::seed::populate(app.world_mut()).unwrap();
        let ship = app
            .world_mut()
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .iter(app.world())
            .next()
            .unwrap();
        let mut durations = Vec::new();
        for tick in 0..120 {
            let started = std::time::Instant::now();
            app.update();
            let simulation_ms = started.elapsed().as_secs_f64() * 1000.;
            infrastructure::publish_navigation(app.world_mut());
            let publication_ms = started.elapsed().as_secs_f64() * 1000. - simulation_ms;
            let position = app
                .world()
                .get::<precision::PreciseTransform>(ship)
                .unwrap()
                .translation_um;
            let index = app.world().resource::<spatial::SpatialIndex>();
            let candidates = index.visible(
                position,
                4. * std::f64::consts::PI * osg_model::optical::MIN_OPTICAL_FLUX_W_M2,
            );
            for target in candidates {
                let _ = index.observed_luminosity(target, position);
                let _ = index.fully_occluded(ship, target, position);
            }
            let complete_ms = started.elapsed().as_secs_f64() * 1000.;
            eprintln!(
                "seeded profile tick={tick} simulation_ms={simulation_ms:.2} publication_ms={publication_ms:.2} complete_ms={complete_ms:.2}"
            );
            if tick >= 60 {
                durations.push(complete_ms);
            }
        }
        durations.sort_by(f64::total_cmp);
        eprintln!(
            "seeded profile median_ms={:.2} p95_ms={:.2} max_ms={:.2}",
            durations[durations.len() / 2],
            durations[durations.len() * 95 / 100],
            durations.last().unwrap()
        );
    }

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
