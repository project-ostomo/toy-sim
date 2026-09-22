use bevy::prelude::*;

use crate::sim::GameState;

use osg_model::TICK_RATE_HZ;

pub struct SimulationPlugin;

/// Forces read start-of-tick poses in FixedUpdate. FixedPostUpdate integrates
/// ships, then advances celestial ephemerides to the same end-of-tick instant.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimulationSystems {
    History,
    PrepareBodies,
    Forces,
    GatherForces,
    Integrate,
    Celestials,
    Complete,
    Observations,
}

/// Application frames and completed gameplay simulation ticks, counted separately.
#[derive(Resource, Default)]
pub struct SimulationCounters {
    pub frames: u64,
    pub ticks: u64,
}

#[derive(Resource, Default)]
pub struct TickMetrics {
    pub last_duration_ms: f64,
}

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
            .init_resource::<SimulationCounters>()
            .init_resource::<TickMetrics>()
            .configure_sets(
                FixedUpdate,
                (
                    SimulationSystems::History,
                    SimulationSystems::PrepareBodies,
                    SimulationSystems::Forces,
                    SimulationSystems::GatherForces,
                )
                    .chain(),
            )
            .configure_sets(
                FixedPostUpdate,
                (SimulationSystems::Integrate, SimulationSystems::Celestials).chain(),
            )
            .add_systems(First, |mut counters: ResMut<SimulationCounters>| {
                counters.frames += 1;
            })
            .add_systems(
                FixedLast,
                (|mut counters: ResMut<SimulationCounters>| counters.ticks += 1)
                    .in_set(SimulationSystems::Complete)
                    .after(crate::sim::spatial::SensorSystems::Scan)
                    .run_if(in_state(GameState::Game)),
            );
    }
}
