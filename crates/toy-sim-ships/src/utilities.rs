use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityDef {
    SlipDrive {
        power_w: f64,
    },
    Command {
        power_w: f64,
    },
    Sensor {
        range_m: f64,
        power_w: f64,
        active: bool,
    },
    Beacon {
        power_w: f64,
    },
    Docking {
        radius_m: f64,
        mass_capacity_kg: f64,
    },
    Crew {
        capacity: u32,
    },
    LifeSupport {
        capacity: u32,
        power_w: f64,
        supplies_kg_per_person_s: f64,
    },
    Workshop {
        repair_hp_s: f64,
        material_kg_hp: f64,
        power_w: f64,
    },
    CargoHandler {
        transfer_kg_s: f64,
        power_w: f64,
    },
    PowerCoupler {
        transfer_w: f64,
    },
}

impl UtilityDef {
    pub fn valid(&self) -> bool {
        let values = match *self {
            Self::SlipDrive { power_w } | Self::Command { power_w } | Self::Beacon { power_w } => {
                vec![power_w]
            }
            Self::Sensor {
                range_m, power_w, ..
            } => vec![range_m, power_w],
            Self::Docking {
                radius_m,
                mass_capacity_kg,
            } => vec![radius_m, mass_capacity_kg],
            Self::Crew { capacity } => return capacity > 0,
            Self::LifeSupport {
                capacity,
                power_w,
                supplies_kg_per_person_s,
            } => {
                if capacity == 0 {
                    return false;
                }
                vec![power_w, supplies_kg_per_person_s]
            }
            Self::Workshop {
                repair_hp_s,
                material_kg_hp,
                power_w,
            } => vec![repair_hp_s, material_kg_hp, power_w],
            Self::CargoHandler {
                transfer_kg_s,
                power_w,
            } => vec![transfer_kg_s, power_w],
            Self::PowerCoupler { transfer_w } => vec![transfer_w],
        };
        values.iter().all(|v| v.is_finite() && *v > 0.)
    }
}
