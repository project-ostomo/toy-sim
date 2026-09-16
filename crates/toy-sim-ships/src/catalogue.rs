use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceDef {
    pub id: String,
    pub title: String,
    pub mass_kg: f64,
    pub volume_m3: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartDef {
    pub id: String,
    pub title: String,
    pub dimensions: [u32; 3],
    pub mass_kg: f64,
    pub hull: f64,
    pub color: [f32; 3],
    #[serde(default)]
    pub model: Option<String>,
    #[serde(flatten)]
    pub equipment: Equipment,
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
    Structure,
    CoolantTank {
        capacity_kg: f64,
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
        capacity_j: f64,
    },
    Engine {
        thrust_n: f64,
        propellant_kg_s: f64,
        power_w: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plume: Option<VacuumPlume>,
    },
    Rcs {
        thrust_n: f64,
        propellant_kg_s: f64,
        power_w: f64,
    },
    Torquer {
        torque_nm: f64,
        power_w: f64,
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
            Self::Structure | Self::CoolantTank { .. } | Self::HeatSink { .. } => return None,
            Self::Weapon { .. } => D::Weapon,
            Self::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
            } => D::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
            },
            Self::Storage { capacity_m3 } => D::Storage { capacity_m3 },
            Self::Battery { capacity_j } => D::Battery { capacity_j },
            Self::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => D::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
            },
            Self::Torquer { torque_nm, .. } => D::Torquer { torque_nm },
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
            Self::Generator { .. } => Some(0),
            Self::Engine { .. } | Self::Torquer { .. } | Self::Rcs { .. } => Some(3),
            Self::Shield { .. } => Some(4),
            _ => None,
        }
    }
    fn validate(&self) -> bool {
        let values: Vec<f64> = match *self {
            Self::Structure => vec![],
            Self::CoolantTank { capacity_kg } => vec![capacity_kg],
            Self::HeatSink { capacity_j } => vec![capacity_j],
            Self::Weapon { ref weapon } => return weapon.valid(),
            Self::Storage { capacity_m3 } => vec![capacity_m3],
            Self::Battery { capacity_j } => vec![capacity_j],
            Self::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => vec![thrust_n, propellant_kg_s, power_w],
            Self::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
            } => vec![thrust_n, propellant_kg_s, power_w],
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
        let c: Self =
            toml::from_str(include_str!("../data/catalogue.toml")).expect("bundled part catalogue");
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
        }
        ids.clear();
        for p in &self.parts {
            if let Equipment::Weapon { weapon } = &p.equipment {
                weapon
                    .spec(self)
                    .ok_or_else(|| anyhow::anyhow!("unknown ammunition"))?;
            }
            if let Equipment::Engine {
                plume: Some(plume), ..
            } = &p.equipment
            {
                ensure!(plume.valid(), "invalid vacuum plume for {}", p.id);
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
            ensure!(
                p.dimensions.iter().all(|&d| d > 0 && d <= 1000),
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
            let c: Catalogue = toml::from_str(&source.replace(from, to)).unwrap();
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
