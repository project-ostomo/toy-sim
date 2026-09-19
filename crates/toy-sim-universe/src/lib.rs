pub mod atmosphere;
pub mod catalogue;
pub mod civilization;
pub mod generation;
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

pub fn handcrafted_configs() -> Vec<orrery_cfg::OrreryCfg> {
    let manifest: universe::UniverseCfg =
        toml::from_str(include_str!("../../../assets/universe.toml"))
            .expect("bundled universe manifest");
    manifest
        .validate()
        .expect("valid bundled universe manifest");
    let sources = [
        (
            "stars/helion.star.toml",
            include_str!("../../../assets/stars/helion.star.toml"),
        ),
        (
            "stars/sol.star.toml",
            include_str!("../../../assets/stars/sol.star.toml"),
        ),
        (
            "stars/vesper.star.toml",
            include_str!("../../../assets/stars/vesper.star.toml"),
        ),
        (
            "stars/aurora.star.toml",
            include_str!("../../../assets/stars/aurora.star.toml"),
        ),
        (
            "stars/lyra.star.toml",
            include_str!("../../../assets/stars/lyra.star.toml"),
        ),
        (
            "stars/cinder.star.toml",
            include_str!("../../../assets/stars/cinder.star.toml"),
        ),
        (
            "stars/meridian.star.toml",
            include_str!("../../../assets/stars/meridian.star.toml"),
        ),
        (
            "stars/havoc.star.toml",
            include_str!("../../../assets/stars/havoc.star.toml"),
        ),
        (
            "stars/elysium.star.toml",
            include_str!("../../../assets/stars/elysium.star.toml"),
        ),
        (
            "stars/terminus.star.toml",
            include_str!("../../../assets/stars/terminus.star.toml"),
        ),
    ];
    manifest
        .systems
        .iter()
        .map(|path| {
            let text = sources
                .iter()
                .find(|(name, _)| name == path)
                .unwrap_or_else(|| panic!("unbundled system: {path}"))
                .1;
            toml::from_str(text).expect("valid bundled authored system")
        })
        .collect()
}

pub fn bundled_configs() -> anyhow::Result<Vec<orrery_cfg::OrreryCfg>> {
    let mut configs = handcrafted_configs();
    let map = civilization::map();
    let authored = configs.len();
    for (config, settlement) in configs.iter_mut().zip(&map.systems) {
        anyhow::ensure!(
            config.name == settlement.name,
            "authored system ordering mismatch"
        );
        config.position_um = settlement.position;
    }
    for (star, settlement) in civilization::stars().iter().zip(&map.systems[authored..]) {
        anyhow::ensure!(
            star.id == settlement.catalogue_id,
            "catalogue ordering mismatch"
        );
        let mut config = generation::system(star, &settlement.name);
        config.position_um = settlement.position;
        configs.push(config);
    }
    anyhow::ensure!(
        configs.len() == map.systems.len(),
        "incomplete inhabited map"
    );
    for config in &configs {
        config.validate_system()?;
    }
    Ok(configs)
}
