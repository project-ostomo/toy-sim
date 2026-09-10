use bevy::prelude::*;

use crate::GameState;

pub const TICK_RATE_HZ: f64 = 10.0;

pub struct SimulationPlugin;

/// Ordered stages within each FixedUpdate, followed by integration in FixedPostUpdate.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimulationSystems {
    History,
    Celestials,
    PrepareBodies,
    Forces,
    GatherForces,
}

/// Application frames and completed gameplay simulation ticks, counted separately.
#[derive(Resource, Default)]
pub struct SimulationCounters {
    pub frames: u64,
    pub ticks: u64,
}

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
            .init_resource::<SimulationCounters>()
            .configure_sets(
                FixedUpdate,
                (
                    SimulationSystems::History,
                    SimulationSystems::Celestials,
                    SimulationSystems::PrepareBodies,
                    SimulationSystems::Forces,
                    SimulationSystems::GatherForces,
                )
                    .chain(),
            )
            .add_systems(First, |mut counters: ResMut<SimulationCounters>| {
                counters.frames += 1;
            })
            .add_systems(
                FixedLast,
                (|mut counters: ResMut<SimulationCounters>| counters.ticks += 1)
                    .run_if(in_state(GameState::Game)),
            );
    }
}
