use bevy::prelude::*;

use super::{
    physics::collision::CollisionStats, simulation::SimulationCounters, vessel::ShipSoftware,
};

pub fn tick(world: &mut World, duration_ms: f64) {
    let slow = duration_ms >= 100.0;
    if !slow
        && !bevy::log::tracing::enabled!(target: "osg_server::timing", bevy::log::tracing::Level::DEBUG)
    {
        return;
    }

    let counters = world.resource::<SimulationCounters>();
    let tick = counters.ticks;
    let sim_time_s = world.resource::<Time<Fixed>>().elapsed_secs_f64();
    let collision = world.resource::<CollisionStats>().clone();
    let mut ships =
        world.query_filtered::<(Entity, &ShipSoftware), Without<super::travel::Dormant>>();
    let mut software_count = 0;
    let mut software_work_ms = 0.0;
    let mut worst_ship = None;
    let mut worst_ship_ms = 0.0;
    let mut worst_callback_ms = 0.0;
    let mut worst_prepare_ms = 0.0;
    let mut worst_gas = 0;
    for (entity, software) in ships.iter(world) {
        software_count += 1;
        let ms = software.last_seconds * 1000.0;
        software_work_ms += ms;
        if ms > worst_ship_ms {
            worst_ship = Some(entity);
            worst_ship_ms = ms;
            worst_callback_ms = software.timings.callback * 1000.0;
            worst_prepare_ms = software.timings.prepare * 1000.0;
            worst_gas = software.last_gas_used;
        }
    }

    macro_rules! report {
        ($level:ident) => {
            bevy::log::$level!(target: "osg_server::timing",
                tick, sim_time_s, duration_ms, software_count, software_work_ms,
                ?worst_ship, worst_ship_ms, worst_callback_ms, worst_prepare_ms, worst_gas,
                collision_ms = collision.total_seconds * 1000.0,
                index_ms = collision.index_seconds * 1000.0,
                query_ms = collision.query_seconds * 1000.0,
                solve_ms = collision.solve_seconds * 1000.0,
                bodies = collision.bodies, candidates = collision.candidates,
                detailed_queries = collision.detailed_queries, impacts = collision.impacts,
                "server tick completed (software work sums parallel workers)");
        };
    }
    if slow {
        report!(warn);
    } else {
        report!(debug);
    }
}

pub fn publication(tick: u64, duration_ms: f64, display_ms: f64, sessions: usize) {
    if duration_ms >= 100.0 {
        bevy::log::warn!(target: "osg_server::timing", tick, duration_ms, display_ms, sessions,
            "slow server display and snapshot publication");
    } else {
        bevy::log::debug!(target: "osg_server::timing", tick, duration_ms, display_ms, sessions,
            "server display and snapshot publication");
    }
}
