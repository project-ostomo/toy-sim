use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityDef {
    Factory {
        capability: osg_model::industry::IndustryCapability,
        power_per_lane_w: u64,
        lanes: u32,
    },
    Shipyard {
        power_per_lane_w: u64,
        lanes: u32,
        max_radius_m: f64,
    },
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
    DirectoryTransmitter {
        power_w: f64,
    },
    NavigationBeacon {
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
            Self::Factory {
                capability,
                power_per_lane_w,
                lanes,
            } => {
                return capability != osg_model::industry::IndustryCapability::Shipyard
                    && power_per_lane_w > 0
                    && (1..=16).contains(&lanes)
                    && power_per_lane_w.checked_mul(u64::from(lanes)).is_some();
            }
            Self::Shipyard {
                power_per_lane_w,
                lanes,
                max_radius_m,
            } => {
                return power_per_lane_w > 0
                    && (1..=16).contains(&lanes)
                    && power_per_lane_w.checked_mul(u64::from(lanes)).is_some()
                    && max_radius_m.is_finite()
                    && max_radius_m > 0.;
            }
            Self::SlipDrive { power_w }
            | Self::Command { power_w }
            | Self::DirectoryTransmitter { power_w }
            | Self::NavigationBeacon { power_w } => {
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
            Self::CargoHandler {
                transfer_kg_s,
                power_w,
            } => vec![transfer_kg_s, power_w],
            Self::PowerCoupler { transfer_w } => vec![transfer_w],
        };
        values.iter().all(|v| v.is_finite() && *v > 0.)
    }
}
