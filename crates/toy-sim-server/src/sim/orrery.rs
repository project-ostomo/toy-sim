pub mod activity;
pub use orrery_cfg::BodyClass;
pub use toy_sim_universe::{orrery_cfg, universe};
#[derive(bevy::prelude::Resource, Clone, bevy::prelude::Deref)]
pub struct Universe(pub std::sync::Arc<universe::Universe>);

static BUNDLED: std::sync::OnceLock<std::sync::Arc<universe::Universe>> =
    std::sync::OnceLock::new();

impl Universe {
    pub fn bundled() -> Self {
        Self(
            BUNDLED
                .get_or_init(|| {
                    let mut universe = universe::Universe::from_configs(
                        toy_sim_universe::bundled_configs().expect("bundled systems"),
                        super::physics::GRAVITY_CUTOFF,
                    )
                    .expect("valid universe");
                    let mut entries = universe.index.entries.clone();
                    for (entry, system) in entries.iter_mut().zip(&universe.systems) {
                        entry.influence = entry
                            .influence
                            .max(super::infrastructure::gate_activation_extent(system));
                    }
                    universe.index = std::sync::Arc::new(
                        toy_sim_universe::catalogue::CatalogueIndex::new(entries),
                    );
                    std::sync::Arc::new(universe)
                })
                .clone(),
        )
    }

    pub fn is_bundled(&self) -> bool {
        BUNDLED
            .get()
            .is_some_and(|bundled| std::sync::Arc::ptr_eq(&self.0, bundled))
    }

    pub fn from_configs(configs: Vec<orrery_cfg::OrreryCfg>, cutoff: f64) -> anyhow::Result<Self> {
        Ok(Self(std::sync::Arc::new(universe::Universe::from_configs(
            configs, cutoff,
        )?)))
    }
    pub fn init(config: orrery_cfg::OrreryCfg) -> anyhow::Result<Self> {
        Ok(Self(std::sync::Arc::new(universe::Universe::init(config)?)))
    }
}
use super::{
    GameState, physics::sim_time, precision::PreciseTransform, simulation::SimulationSystems,
};
use bevy::prelude::*;
use smol_str::SmolStr;

pub struct OrreryPlugin;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoadOrrery;

impl Plugin for OrreryPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Universe::bundled())
            .init_resource::<activity::ActiveSystems>()
            .init_resource::<activity::UniverseDebug>()
            .add_systems(
                FixedUpdate,
                activity::activate
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
pub struct Celestial(pub SmolStr);

#[derive(Component)]
#[require(Celestial)]
pub struct Star {
    pub lumens: f64,
    pub color_temp: f64,
}
