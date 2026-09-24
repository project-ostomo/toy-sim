pub mod activity;
pub use orrery_cfg::BodyClass;
pub use osg_universe::{orrery_cfg, universe};
#[derive(bevy::prelude::Resource, Clone, bevy::prelude::Deref)]
pub struct Universe(pub std::sync::Arc<universe::Universe>);

static BUNDLED: std::sync::OnceLock<std::sync::Arc<universe::Universe>> =
    std::sync::OnceLock::new();

impl Universe {
    pub fn bundled() -> Self {
        Self(
            BUNDLED
                .get_or_init(|| {
                    std::sync::Arc::new(universe::Universe::bundled().expect("valid universe"))
                })
                .clone(),
        )
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
                    .before(SimulationSystems::PrepareBodies)
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
    active: Res<activity::ActiveSystems>,
    time: Res<Time<Fixed>>,
    mut bodies: Query<
        (
            &Celestial,
            &mut PreciseTransform,
            &mut activity::CelestialState,
        ),
        Without<activity::SystemRegion>,
    >,
    mut regions: Query<(&activity::SystemRegion, &mut PreciseTransform), Without<Celestial>>,
) {
    let epoch = sim_time(&time);
    let _profile = super::diagnostics::ProfileScope::new("move_orrery");
    bodies
        .par_iter_mut()
        .for_each(|(body, mut pose, mut state)| {
            let definition = active
                .definitions
                .get(&state.system)
                .expect("active system");
            let solver = &definition.solver;
            pose.translation_um = solver.solve_position(&body.0, epoch).unwrap();
            pose.rotation = solver.solve_rotation(&body.0, epoch).unwrap();
            state.velocity = solver.solve_velocity(&body.0, epoch).unwrap();
        });
    regions.par_iter_mut().for_each(|(region, mut pose)| {
        let definition = active
            .definitions
            .get(&region.system)
            .expect("active root region");
        pose.translation_um = definition
            .solver
            .solve_position(&region.name, epoch)
            .unwrap();
    });
}
#[derive(Component, Default)]
pub struct Celestial(pub SmolStr);

#[derive(Component)]
#[require(Celestial)]
pub struct Star {
    pub lumens: f64,
    pub color_temp: f64,
}
