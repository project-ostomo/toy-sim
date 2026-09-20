use crate::{orrery_cfg::OrreryCfg, solver::Orrery};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SIMULATION_EPOCH_MJD_UTC: f64 = 0.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyIdentity {
    pub name: String,
    pub id: [u8; 16],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemAsset {
    pub version: u16,
    pub system_id: [u8; 16],
    pub body_ids: Vec<BodyIdentity>,
    pub config: OrreryCfg,
}

impl SystemAsset {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        self.config.validate_system()?;
        let bytes = toml::to_string(self)?.into_bytes();
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "system definition exceeds asset limit"
        );
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "system definition exceeds asset limit"
        );
        let asset: Self = toml::from_str(std::str::from_utf8(bytes)?)?;
        asset.validate()?;
        asset.config.validate_system()?;
        Ok(asset)
    }

    pub fn solver(&self) -> Result<Orrery> {
        self.validate()?;
        self.config.validate_system()?;
        Orrery::init(self.config.clone())
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported system definition version");
        ensure!(
            !self.config.name.is_empty() && self.config.name.len() <= 128,
            "invalid system name"
        );
        ensure!(
            !self.config.bodies.is_empty()
                && self.config.bodies.len() <= 4096
                && self.body_ids.len() == self.config.bodies.len(),
            "invalid system body count"
        );
        let names: BTreeSet<_> = self
            .config
            .bodies
            .iter()
            .map(|body| body.name.as_str())
            .collect();
        let mut ids = BTreeSet::new();
        let mut mapped = BTreeSet::new();
        ensure!(
            names.len() == self.config.bodies.len(),
            "duplicate body name"
        );
        for body in &self.body_ids {
            ensure!(
                names.contains(body.name.as_str())
                    && mapped.insert(body.name.as_str())
                    && ids.insert(body.id)
                    && body.id != self.system_id,
                "invalid body identity mapping"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hifitime::{Duration, Epoch};

    fn asset() -> SystemAsset {
        let config = crate::example_config();
        let body_ids = config
            .bodies
            .iter()
            .enumerate()
            .map(|(i, body)| BodyIdentity {
                name: body.name.to_string(),
                id: [(i + 1) as u8; 16],
            })
            .collect();
        SystemAsset {
            version: 1,
            system_id: [0; 16],
            body_ids,
            config,
        }
    }

    #[test]
    fn full_system_definition_reproduces_analytic_motion() {
        let asset = asset();
        let original = asset.solver().unwrap();
        let decoded = SystemAsset::decode(&asset.encode().unwrap()).unwrap();
        let reconstructed = decoded.solver().unwrap();
        for seconds in [0., 0.05, 17.125, 86400., 1e9] {
            let epoch = Epoch::from_mjd_utc(0.) + Duration::from_seconds(seconds);
            for body in original.iter() {
                assert_eq!(
                    original.solve_position(&body.name, epoch),
                    reconstructed.solve_position(&body.name, epoch)
                );
                assert_eq!(
                    original.solve_velocity(&body.name, epoch),
                    reconstructed.solve_velocity(&body.name, epoch)
                );
                assert_eq!(
                    original.solve_rotation(&body.name, epoch),
                    reconstructed.solve_rotation(&body.name, epoch)
                );
            }
        }
        assert_eq!(
            toml::to_string(&asset.config).unwrap(),
            toml::to_string(&decoded.config).unwrap()
        );
        assert_eq!(asset.body_ids.len(), decoded.body_ids.len());
        assert_eq!(asset.config.position_um, decoded.config.position_um);
    }

    #[test]
    fn rejects_ambiguous_identity_and_unknown_asset_version() {
        let mut asset = asset();
        asset.version = 2;
        assert!(asset.encode().is_err());
        asset.version = 1;
        asset.body_ids[0].id = asset.system_id;
        assert!(asset.encode().is_err());
        asset.body_ids[0].id = [1; 16];
        asset.body_ids[0].name = "unknown".into();
        assert!(asset.encode().is_err());
    }

    #[test]
    fn decoding_rejects_invalid_orbit_and_parent_cycle() {
        let mut asset = asset();
        asset.config.bodies[1].orbit.eccentricity = f64::NAN;
        let malformed = toml::to_string(&asset).unwrap();
        assert!(SystemAsset::decode(malformed.as_bytes()).is_err());
        assert!(asset.solver().is_err());
        asset.config.bodies[1].orbit.eccentricity = 0.;
        asset.config.bodies[1].parent = Some(asset.config.bodies[1].name.clone());
        let malformed = toml::to_string(&asset).unwrap();
        assert!(SystemAsset::decode(malformed.as_bytes()).is_err());
    }
}
