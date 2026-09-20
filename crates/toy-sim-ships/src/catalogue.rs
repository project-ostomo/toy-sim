use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const STANDARD_GRAVITY_M_S2: f64 = 9.80665;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceDef {
    pub id: String,
    pub title: String,
    pub mass_kg: f64,
    pub volume_m3: f64,
    #[serde(default)]
    pub exportable_product: bool,
    #[serde(default)]
    pub storage: ResourceStorage,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    #[default]
    Bulk,
    Liquid,
    Cryogenic,
    Nuclear,
    Charges,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceStorage {
    pub class: StorageClass,
    pub usable_fraction: f64,
    pub containment_kg_m3: f64,
}

impl Default for ResourceStorage {
    fn default() -> Self {
        Self {
            class: StorageClass::Bulk,
            usable_fraction: 1.,
            containment_kg_m3: 0.,
        }
    }
}

impl ResourceStorage {
    fn valid(&self) -> bool {
        self.usable_fraction.is_finite()
            && self.usable_fraction > 0.
            && self.usable_fraction <= 1.
            && self.containment_kg_m3.is_finite()
            && self.containment_kg_m3 >= 0.
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartDef {
    #[serde(default)]
    pub collision: crate::collision::CollisionVolume,
    #[serde(default)]
    pub nodes: Vec<crate::AttachmentNode>,
    pub id: String,
    pub title: String,
    pub dimensions: [u32; 3],
    #[serde(default)]
    pub tank_volume_m3: f64,
    pub mass_kg: f64,
    pub hull: f64,
    pub color: [f32; 3],
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_model_scale")]
    pub model_scale: f32,
    #[serde(flatten)]
    pub equipment: Equipment,
}

fn default_model_scale() -> f32 {
    1.0
}

/// Full-output vacuum plume appearance. These are authored visual parameters,
/// not gas/plasma simulation inputs, and do not change the engine's performance.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VacuumPlume {
    /// Nozzle opening relative to the part's centre, in metres. Exhaust is +Z.
    pub origin_m: [f32; 3],
    pub length_m: f32,
    pub nozzle_radius_m: f32,
    /// Expanding envelope: radius(z) = nozzle_radius + z * tan(half_angle).
    pub expansion_half_angle_rad: f32,
    /// Linear RGB tint, independently scaled by intensity for HDR rendering.
    pub color_linear_rgb: [f32; 3],
    /// Artistic HDR emission multiplier; not a calibrated physical radiance.
    pub intensity: f32,
    /// Positive exponents for normalized axial and radial emission falloff.
    pub axial_falloff: f32,
    pub radial_falloff: f32,
    /// Fractional noise modulation, 0–1; zero disables variation.
    pub noise_strength: f32,
    pub noise_scale_m: f32,
    /// Apparent downstream texture motion, not the physical exhaust velocity.
    pub noise_speed_m_s: f32,
}

impl VacuumPlume {
    fn valid(&self) -> bool {
        self.origin_m.iter().all(|v| v.is_finite())
            && [
                self.length_m,
                self.nozzle_radius_m,
                self.axial_falloff,
                self.radial_falloff,
                self.noise_scale_m,
            ]
            .iter()
            .all(|v| v.is_finite() && *v > 0.)
            && self.expansion_half_angle_rad.is_finite()
            && (0.0..std::f32::consts::FRAC_PI_2).contains(&self.expansion_half_angle_rad)
            && (self.nozzle_radius_m + self.length_m * self.expansion_half_angle_rad.tan())
                .is_finite()
            && self
                .color_linear_rgb
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && self.intensity.is_finite()
            && self.intensity >= 0.
            && self.noise_strength.is_finite()
            && (0.0..=1.0).contains(&self.noise_strength)
            && self.noise_speed_m_s.is_finite()
            && self.noise_speed_m_s >= 0.
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Equipment {
    Utility {
        utility: crate::utilities::UtilityDef,
    },
    Structure,
    CoolantTank {
        capacity_kg: f64,
    },
    Radiator {
        area_m2: f64,
        emissivity: f64,
    },
    EmergencyCooling {
        max_flow_kg_s: f64,
        heat_removed_j_kg: f64,
        activation_fraction: f64,
    },
    HeatSink {
        capacity_j: f64,
    },
    Weapon {
        weapon: crate::weapons::WeaponDef,
    },
    Storage {
        capacity_m3: f64,
    },
    Battery {
        capacity_j: u64,
    },
    Engine {
        propellant_resource: String,
        thrust_n: f64,
        propellant_kg_s: f64,
        power_w: f64,
        #[serde(default)]
        propellant_energy_j_kg: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plume: Option<VacuumPlume>,
    },
    ThermalEngine {
        thrust_n: f64,
        specific_impulse_s: f64,
        propellant_resource: String,
        thermal_efficiency: f64,
        fuel_energy_j_kg: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plume: Option<VacuumPlume>,
    },
    MicropulseEngine {
        thrust_n: f64,
        specific_impulse_s: f64,
        charge_energy_j_kg: f64,
        electric_efficiency: f64,
        absorbed_heat_fraction: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plume: Option<VacuumPlume>,
    },
    Rcs {
        propellant_resource: String,
        thrust_n: f64,
        propellant_kg_s: f64,
        power_w: f64,
    },
    Torquer {
        torque_nm: f64,
        power_w: f64,
    },
    Reactor {
        spec: crate::reactors::ReactorSpec,
    },
    FuelProcessor {
        spec: crate::reactors::FuelProcessorSpec,
    },
    Generator {
        power_w: f64,
        fuel_kg_s: f64,
        efficiency: f64,
    },
    Shield {
        deployed_mass_kg: f64,
        radiator_area_m2: f64,
        feed_rate_kg_s: f64,
        power_w: f64,
    },
}

impl Equipment {
    pub fn device_kind(&self) -> Option<crate::DeviceKind> {
        use crate::DeviceKind as D;
        Some(match *self {
            Self::Radiator { .. }
            | Self::EmergencyCooling { .. }
            | Self::Utility { .. }
            | Self::Structure
            | Self::CoolantTank { .. }
            | Self::HeatSink { .. } => return None,
            Self::Weapon { .. } => D::Weapon,
            Self::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => D::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
            },
            Self::Storage { capacity_m3 } => D::Storage { capacity_m3 },
            Self::Battery { capacity_j } => D::Battery { capacity_j },
            Self::Engine {
                ref propellant_resource,
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => D::Engine {
                propellant_resource: propellant_resource.clone(),
                thrust_n,
                propellant_kg_s,
                power_w,
            },
            Self::ThermalEngine {
                thrust_n,
                specific_impulse_s,
                ref propellant_resource,
                ..
            } => D::Engine {
                propellant_resource: propellant_resource.clone(),
                thrust_n,
                propellant_kg_s: thrust_n / (STANDARD_GRAVITY_M_S2 * specific_impulse_s),
                power_w: 0.0,
            },
            Self::MicropulseEngine {
                thrust_n,
                specific_impulse_s,
                ..
            } => D::Engine {
                propellant_resource: "micropulse_charge".into(),
                thrust_n,
                propellant_kg_s: thrust_n / (STANDARD_GRAVITY_M_S2 * specific_impulse_s),
                power_w: 0.,
            },
            Self::Torquer { torque_nm, .. } => D::Torquer { torque_nm },
            Self::Reactor { spec } => D::Generator {
                power_w: spec.thermal_power_w * spec.efficiency(300.0),
            },
            Self::FuelProcessor { .. } => return None,
            Self::Generator { power_w, .. } => D::Generator { power_w },
            Self::Shield {
                deployed_mass_kg,
                radiator_area_m2,
                ..
            } => D::Shield {
                deployed_mass_kg,
                radiator_area_m2,
            },
        })
    }

    pub fn priority(&self) -> Option<u8> {
        match self {
            Self::Weapon { .. } => Some(5),
            Self::Generator { .. } | Self::Reactor { .. } | Self::FuelProcessor { .. } => Some(0),
            Self::Engine { .. }
            | Self::MicropulseEngine { .. }
            | Self::ThermalEngine { .. }
            | Self::Torquer { .. }
            | Self::Rcs { .. } => Some(3),
            Self::Shield { .. } => Some(4),
            _ => None,
        }
    }
    fn validate(&self) -> bool {
        let values: Vec<f64> = match *self {
            Self::Reactor { spec } => return spec.valid(),
            Self::FuelProcessor { spec } => return spec.valid(),
            Self::Utility { ref utility } => return utility.valid(),
            Self::Radiator {
                area_m2,
                emissivity,
            } => {
                if emissivity > 1.0 {
                    return false;
                }
                vec![area_m2, emissivity]
            }
            Self::EmergencyCooling {
                max_flow_kg_s,
                heat_removed_j_kg,
                activation_fraction,
            } => vec![max_flow_kg_s, heat_removed_j_kg, activation_fraction],
            Self::Structure => vec![],
            Self::CoolantTank { capacity_kg } => vec![capacity_kg],
            Self::HeatSink { capacity_j } => vec![capacity_j],
            Self::Weapon { ref weapon } => return weapon.valid(),
            Self::Storage { capacity_m3 } => vec![capacity_m3],
            Self::Battery { capacity_j } => vec![capacity_j as f64],
            Self::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
                propellant_energy_j_kg,
                ..
            } => {
                let supplied_power = power_w + propellant_kg_s * propellant_energy_j_kg;
                if !propellant_energy_j_kg.is_finite()
                    || propellant_energy_j_kg < 0.
                    || !power_w.is_finite()
                    || power_w < 0.
                    || !supplied_power.is_finite()
                    || 0.5 * thrust_n.powi(2) / propellant_kg_s > supplied_power * (1.0 + 1e-10)
                {
                    return false;
                }
                vec![thrust_n, propellant_kg_s]
            }
            Self::ThermalEngine {
                thrust_n,
                specific_impulse_s,
                thermal_efficiency,
                fuel_energy_j_kg,
                ..
            } => {
                if thermal_efficiency > 0.94 {
                    return false;
                }
                vec![
                    thrust_n,
                    specific_impulse_s,
                    thermal_efficiency,
                    fuel_energy_j_kg,
                ]
            }
            Self::MicropulseEngine {
                thrust_n,
                specific_impulse_s,
                charge_energy_j_kg,
                electric_efficiency,
                absorbed_heat_fraction,
                ..
            } => {
                let exhaust_velocity = STANDARD_GRAVITY_M_S2 * specific_impulse_s;
                let jet_energy_j_kg = 0.5 * exhaust_velocity * exhaust_velocity;
                let charge_flow_kg_s = thrust_n / exhaust_velocity;
                let reaction_power_w = charge_flow_kg_s * charge_energy_j_kg;
                if ![electric_efficiency, absorbed_heat_fraction]
                    .iter()
                    .all(|fraction| fraction.is_finite() && (0.0..=1.0).contains(fraction))
                    || electric_efficiency <= 0.01
                    || !jet_energy_j_kg.is_finite()
                    || !reaction_power_w.is_finite()
                    || jet_energy_j_kg / charge_energy_j_kg + absorbed_heat_fraction > 1.0
                {
                    return false;
                }
                vec![thrust_n, specific_impulse_s, charge_energy_j_kg]
            }
            Self::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => {
                if 0.5 * thrust_n.powi(2) / propellant_kg_s > power_w * (1.0 + 1e-10) {
                    return false;
                }
                vec![thrust_n, propellant_kg_s, power_w]
            }
            Self::Torquer { torque_nm, power_w } => vec![torque_nm, power_w],
            Self::Generator {
                power_w,
                fuel_kg_s,
                efficiency,
            } => {
                if efficiency > 1.0 {
                    return false;
                }
                vec![power_w, fuel_kg_s, efficiency]
            }
            Self::Shield {
                deployed_mass_kg,
                radiator_area_m2,
                feed_rate_kg_s,
                power_w,
            } => vec![deployed_mass_kg, radiator_area_m2, feed_rate_kg_s, power_w],
        };
        values.iter().all(|v| v.is_finite() && *v > 0.)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalogue {
    pub revision: u32,
    pub resources: Vec<ResourceDef>,
    pub parts: Vec<PartDef>,
}
impl Catalogue {
    pub fn builtin() -> Self {
        let mut c: Self =
            toml::from_str(include_str!("../data/catalogue.toml")).expect("bundled part catalogue");
        let common_sky: Self = toml::from_str(include_str!("../data/common-sky.toml"))
            .expect("Common Sky part catalogue");
        c.resources.extend(common_sky.resources);
        c.parts.extend(common_sky.parts);
        c.validate().expect("invalid bundled catalogue");
        c
    }
    pub fn part(&self, id: &str) -> Option<&PartDef> {
        self.parts.iter().find(|p| p.id == id)
    }
    pub fn validate(&self) -> Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        ensure!(
            self.resources.first().is_some_and(|r| r.id == "propellant")
                && self.resources.get(1).is_some_and(|r| r.id == "fuel"),
            "catalogue must define propellant and fuel in slots 0 and 1"
        );
        for r in &self.resources {
            ensure!(
                !r.id.is_empty() && ids.insert(&r.id),
                "duplicate/empty resource ID"
            );
            ensure!(
                r.mass_kg.is_finite()
                    && r.mass_kg > 0.
                    && r.volume_m3.is_finite()
                    && r.volume_m3 > 0.,
                "invalid resource units"
            );
            ensure!(r.storage.valid(), "invalid resource storage: {}", r.id);
            let mass_mg = crate::industry::mass_mg(r.mass_kg)?;
            ensure!(
                mass_mg > 0
                    && (mass_mg as f64 * crate::industry::MILLIGRAM_KG - r.mass_kg).abs()
                        <= r.mass_kg * 1e-12,
                "resource unit mass must use whole milligrams"
            );
        }
        ids.clear();
        for p in &self.parts {
            ensure!(p.collision.valid(), "invalid collision volume");
            let nodes = p.attachment_nodes();
            ensure!(nodes.len() <= 128, "too many attachment nodes");
            let mut names = std::collections::BTreeSet::new();
            for node in &nodes {
                let normal = glam::DVec3::from_array(node.normal);
                ensure!(
                    !node.name.is_empty() && node.name.len() <= 64 && names.insert(&node.name),
                    "duplicate/invalid attachment node"
                );
                ensure!(
                    !node.connector.is_empty() && node.connector.len() <= 64,
                    "invalid connector type"
                );
                ensure!(
                    node.position_m
                        .iter()
                        .all(|p| p.is_finite() && p.abs() <= 100_000.0),
                    "invalid node position"
                );
                ensure!(
                    normal.is_finite()
                        && normal.abs().max_element() == 1.0
                        && normal.length_squared() == 1.0,
                    "node normal must be a unit assembly axis"
                );
            }
            ensure!(
                p.model_scale.is_finite() && p.model_scale > 0.,
                "invalid model scale for {}",
                p.id
            );
            ensure!(
                p.tank_volume_m3.is_finite() && p.tank_volume_m3 >= 0.,
                "invalid tank volume for {}",
                p.id
            );
            if matches!(
                p.equipment,
                Equipment::Reactor { .. } | Equipment::FuelProcessor { .. }
            ) {
                for name in [
                    "reactor_fuel",
                    "spent_fuel",
                    "fertile_feedstock",
                    "bred_fuel",
                ] {
                    ensure!(
                        self.resources
                            .iter()
                            .any(|r| r.id == name && r.mass_kg == 1.0),
                        "nuclear equipment requires kilogram resource {name}"
                    );
                }
            }
            if matches!(p.equipment, Equipment::FuelProcessor { spec } if spec.produces_charges) {
                for name in ["repair_material", "micropulse_charge"] {
                    ensure!(
                        self.resources
                            .iter()
                            .any(|r| r.id == name && r.mass_kg == 1.0),
                        "charge production requires kilogram resource {name}"
                    );
                }
            }
            if let Equipment::Weapon { weapon } = &p.equipment {
                weapon
                    .spec(self)
                    .ok_or_else(|| anyhow::anyhow!("unknown ammunition"))?;
            }
            if matches!(
                p.equipment,
                Equipment::Utility {
                    utility: crate::utilities::UtilityDef::MissileLauncher { .. }
                }
            ) {
                let ammunition = self
                    .resources
                    .iter()
                    .find(|resource| resource.id == crate::missiles::AMMUNITION)
                    .ok_or_else(|| {
                        anyhow::anyhow!("missile launcher requires interceptor ammunition")
                    })?;
                ensure!(
                    p.tank_volume_m3 >= ammunition.volume_m3,
                    "missile launcher needs room for at least one round"
                );
            }
            if let Equipment::Engine {
                propellant_resource,
                ..
            }
            | Equipment::ThermalEngine {
                propellant_resource,
                ..
            }
            | Equipment::Rcs {
                propellant_resource,
                ..
            } = &p.equipment
            {
                ensure!(
                    self.resources.iter().any(|r| r.id == *propellant_resource),
                    "unknown propellant resource for {}",
                    p.id
                );
            }
            if let Equipment::ThermalEngine {
                propellant_resource,
                ..
            } = &p.equipment
            {
                ensure!(
                    propellant_resource != "reactor_fuel" && propellant_resource != "spent_fuel",
                    "thermal engine propellant must differ from reactor fuel"
                );
            }
            if matches!(p.equipment, Equipment::ThermalEngine { .. }) {
                ensure!(
                    self.resources.iter().any(|r| r.id == "reactor_fuel")
                        && self.resources.iter().any(|r| r.id == "spent_fuel"),
                    "thermal engines require reactor and spent fuel resources"
                );
            }
            if let Equipment::Engine {
                plume: Some(plume), ..
            }
            | Equipment::ThermalEngine {
                plume: Some(plume), ..
            }
            | Equipment::MicropulseEngine {
                plume: Some(plume), ..
            } = &p.equipment
            {
                ensure!(plume.valid(), "invalid vacuum plume for {}", p.id);
            }
            if matches!(p.equipment, Equipment::MicropulseEngine { .. }) {
                ensure!(
                    self.resources
                        .iter()
                        .any(|resource| resource.id == "micropulse_charge"),
                    "micropulse engines require micropulse_charge resource"
                );
            }
            ensure!(
                p.equipment.validate(),
                "invalid equipment rates for {}",
                p.id
            );
            ensure!(
                p.color
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "invalid part color"
            );
            ensure!(ids.insert(&p.id), "duplicate part {}", p.id);
            ensure!(
                p.mass_kg.is_finite() && p.mass_kg > 0. && p.hull.is_finite() && p.hull > 0.,
                "invalid mass/hull"
            );
            let mass_mg = crate::industry::mass_mg(p.mass_kg)?;
            ensure!(
                mass_mg > 0
                    && (mass_mg as f64 * crate::industry::MILLIGRAM_KG - p.mass_kg).abs()
                        <= p.mass_kg * 1e-12,
                "part mass must use whole milligrams"
            );
            ensure!(
                p.dimensions.iter().all(|&d| d > 0 && d <= 10000),
                "invalid dimensions"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vacuum_plume_survives_catalogue_roundtrip_and_is_optional() {
        let mut catalogue = Catalogue::builtin();
        let Equipment::Engine {
            plume: Some(plume), ..
        } = &catalogue.part("engine").unwrap().equipment
        else {
            panic!("bundled engine needs a plume");
        };
        assert_eq!(plume.origin_m, [0., 0., 1.]);
        let encoded = toml::to_string(&catalogue).unwrap();
        let decoded: Catalogue = toml::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        let Equipment::Engine {
            plume: Some(decoded),
            ..
        } = &decoded.part("engine").unwrap().equipment
        else {
            panic!("plume was lost");
        };
        assert_eq!(decoded, plume);
        let engine = catalogue
            .parts
            .iter_mut()
            .find(|p| p.id == "engine")
            .unwrap();
        let Equipment::Engine { plume, .. } = &mut engine.equipment else {
            unreachable!()
        };
        *plume = None;
        let decoded: Catalogue = toml::from_str(&toml::to_string(&catalogue).unwrap()).unwrap();
        decoded.validate().unwrap();
        assert!(matches!(
            decoded.part("engine").unwrap().equipment,
            Equipment::Engine { plume: None, .. }
        ));
    }

    #[test]
    fn invalid_vacuum_plume_parameters_are_rejected() {
        let source = include_str!("../data/catalogue.toml");
        for (from, to) in [
            ("nozzle_radius_m = 0.35", "nozzle_radius_m = -0.1"),
            ("length_m = 80.0", "length_m = 0.0"),
            ("origin_m = [0.0, 0.0, 1.0]", "origin_m = [nan, 0.0, 1.0]"),
            (
                "expansion_half_angle_rad = 0.0120573494",
                "expansion_half_angle_rad = 1.5707964",
            ),
            (
                "color_linear_rgb = [0.15, 0.35, 1.0]",
                "color_linear_rgb = [0.15, 0.35, 2.0]",
            ),
            ("noise_strength = 0.1", "noise_strength = nan"),
            ("intensity = 80.0", "intensity = inf"),
            ("radial_falloff = 2.0", "radial_falloff = -1.0"),
        ] {
            assert!(source.contains(from));
            let c: Catalogue = toml::from_str(&source.replacen(from, to, 1)).unwrap();
            assert!(
                c.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("invalid vacuum plume"),
                "{to}"
            );
        }
        assert!(
            toml::from_str::<Catalogue>(
                &source.replace("noise_strength = 0.1", "noise_strenght = 0.1")
            )
            .is_err()
        );
    }
}
