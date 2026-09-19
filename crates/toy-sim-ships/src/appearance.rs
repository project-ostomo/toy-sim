use crate::{Catalogue, CompiledShipDesign, Equipment, GRID, MAX_FILE, PartDef};
use anyhow::{Context, Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ShipAppearance {
    pub catalogue_revision: u32,
    pub parts: Vec<AppearancePart>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearancePart {
    pub id: u64,
    pub prototype: String,
    pub position_m: [f64; 3],
    pub rotation: [f64; 4],
}

pub struct PreparedAppearance {
    pub parts: Vec<PreparedAppearancePart>,
    pub radius: f64,
    pub shield_radius: Option<f64>,
}

pub struct PreparedAppearancePart {
    pub id: u64,
    pub definition: PartDef,
    pub position: DVec3,
    pub rotation: DMat3,
}

impl From<&CompiledShipDesign> for ShipAppearance {
    fn from(design: &CompiledShipDesign) -> Self {
        Self {
            catalogue_revision: design.blueprint.catalogue_revision,
            parts: design
                .parts
                .iter()
                .map(|part| AppearancePart {
                    id: part.placed.id,
                    prototype: part.definition.id.clone(),
                    position_m: (part.centre - design.centre).to_array(),
                    rotation: DQuat::from_mat3(&part.rotation).to_array(),
                })
                .collect(),
        }
    }
}

impl ShipAppearance {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let bytes = toml::to_string(self)?.into_bytes();
        ensure!(bytes.len() <= MAX_FILE, "ship appearance too large");
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_FILE,
            "invalid ship appearance length"
        );
        Ok(toml::from_str(std::str::from_utf8(bytes)?)?)
    }

    pub fn prepare(&self, catalogue: &Catalogue) -> Result<PreparedAppearance> {
        ensure!(
            self.catalogue_revision == catalogue.revision,
            "appearance catalogue revision mismatch"
        );
        ensure!(
            !self.parts.is_empty() && self.parts.len() <= 4096,
            "appearance needs 1–4096 parts"
        );
        let mut ids = HashSet::with_capacity(self.parts.len());
        let mut parts = Vec::with_capacity(self.parts.len());
        let mut radius = 0.0_f64;
        let mut has_shield = false;
        for part in &self.parts {
            ensure!(ids.insert(part.id), "duplicate appearance part ID");
            let definition = catalogue
                .part(&part.prototype)
                .context("unknown appearance part")?;
            let position = DVec3::from_array(part.position_m);
            let rotation = DQuat::from_array(part.rotation);
            ensure!(
                position.is_finite()
                    && rotation.is_finite()
                    && (rotation.length_squared() - 1.0).abs() <= 1e-10,
                "invalid appearance transform"
            );
            let rotation = DMat3::from_quat(rotation);
            let half = DVec3::from_array(definition.dimensions.map(|n| n as f64 * GRID)) * 0.5;
            radius = radius.max((position.abs() + rotation.abs() * half).length());
            has_shield |= matches!(definition.equipment, Equipment::Shield { .. });
            parts.push(PreparedAppearancePart {
                id: part.id,
                definition: definition.clone(),
                position,
                rotation,
            });
        }
        ensure!(
            radius.is_finite() && radius > 0.0,
            "invalid appearance bounds"
        );
        Ok(PreparedAppearance {
            parts,
            radius,
            shield_radius: has_shield.then(|| crate::thermal::shield_radius(radius)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Firmware, ShipBlueprint, Tank};

    fn private_design(catalogue: &Catalogue) -> CompiledShipDesign {
        let mut blueprint = ShipBlueprint::default();
        blueprint.name = "private ship designation".into();
        blueprint.firmware = Firmware::Custom(b"private flight computer".to_vec());
        blueprint.avionics.sensor_enabled = false;
        blueprint.attach("fuselage_2m", 0, "", "", 0);
        blueprint.attach("fuselage_2m", 1, "aft", "fore", 0);
        blueprint.parts[0].name = "secret reactor fuel storage".into();
        blueprint.parts[0].tanks.push(Tank {
            resource: "reactor_fuel".into(),
            volume_m3: 1.0,
            initial_fill: 0.731,
        });
        blueprint.compile(catalogue).unwrap()
    }

    #[test]
    fn public_appearance_contains_only_visual_fields_and_preserves_the_physical_origin() {
        let catalogue = Catalogue::builtin();
        let design = private_design(&catalogue);
        let appearance = ShipAppearance::from(&design);
        let bytes = appearance.to_bytes().unwrap();
        let value: toml::Value = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
        let root = value.as_table().unwrap();
        assert_eq!(root.len(), 2);
        assert!(root.contains_key("catalogue_revision") && root.contains_key("parts"));
        for part in root["parts"].as_array().unwrap() {
            let fields = part.as_table().unwrap();
            assert_eq!(fields.len(), 4);
            for key in ["id", "prototype", "position_m", "rotation"] {
                assert!(fields.contains_key(key));
            }
        }
        for secret in [
            "reactor_fuel",
            "initial_fill",
            "private",
            "tanks",
            "avionics",
        ] {
            assert!(!std::str::from_utf8(&bytes).unwrap().contains(secret));
        }

        let restored = ShipAppearance::from_bytes(&bytes).unwrap();
        let prepared = restored.prepare(&catalogue).unwrap();
        assert!((prepared.radius - design.radius).abs() < 1e-12);
        for (visual, physical) in prepared.parts.iter().zip(&design.parts) {
            assert_eq!(visual.id, physical.placed.id);
            assert!(
                visual
                    .position
                    .abs_diff_eq(physical.centre - design.centre, 1e-12)
            );
            assert!(visual.rotation.abs_diff_eq(physical.rotation, 1e-12));
        }

        let mut without_containment = design.blueprint.clone();
        without_containment.parts[0].tanks.clear();
        let without_containment = without_containment.compile(&catalogue).unwrap();
        assert!((without_containment.centre - design.centre).length() > 0.001);

        let mut different_inventory = design.blueprint.clone();
        different_inventory.parts[0].tanks[0].resource = "spent_fuel".into();
        different_inventory.parts[0].tanks[0].initial_fill = 0.02;
        let different_inventory = different_inventory.compile(&catalogue).unwrap();
        assert_eq!(
            ShipAppearance::from(&different_inventory)
                .to_bytes()
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn public_appearance_rejects_private_fields_and_invalid_geometry() {
        let catalogue = Catalogue::builtin();
        let mut appearance = ShipAppearance::from(&private_design(&catalogue));
        let mut text = String::from_utf8(appearance.to_bytes().unwrap()).unwrap();
        text.push_str("\ninitial_fill = 0.5\n");
        assert!(ShipAppearance::from_bytes(text.as_bytes()).is_err());
        appearance.parts[0].rotation = [0.0; 4];
        assert!(appearance.prepare(&catalogue).is_err());
        appearance.parts[0].rotation = [0.0, 0.0, 0.0, 1.0];
        appearance.parts[0].position_m[0] = f64::INFINITY;
        assert!(appearance.prepare(&catalogue).is_err());
    }
}
