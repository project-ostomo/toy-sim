pub mod atmosphere;
pub mod catalogue;
pub mod orrery_cfg;
pub mod replication;
pub mod solver;
pub mod universe;

mod precision {
    pub use toy_sim_space::{GalacticPosition, ToMicrometersExt};
}

mod physics {
    pub const GRAVITATIONAL_CONSTANT: f64 = 6.67430e-11;
    pub const GRAVITY_CUTOFF: f64 = 1e-8;
}

pub fn example_config() -> orrery_cfg::OrreryCfg {
    toml::from_str(include_str!("../../../assets/stars/helion.star.toml")).unwrap()
}

#[cfg(test)]
fn remote_test_config() -> orrery_cfg::OrreryCfg {
    toml::from_str(include_str!("../../../tests/fixtures/remote.star.toml")).unwrap()
}
