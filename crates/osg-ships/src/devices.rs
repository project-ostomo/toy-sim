//! Native hardware descriptors and actuator settings. These are simulator types, not guest records.

pub const MAX_DEVICES: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceHandle(pub u16);

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceKind {
    Weapon,
    Rcs {
        thrust_n: f64,
        propellant_kg_s: f64,
        power_w: f64,
    },
    Accelerometer,
    Computer,
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
    },
    Torquer {
        torque_nm: f64,
    },
    Generator {
        power_w: f64,
    },
    Shield {
        deployed_mass_kg: f64,
        radiator_area_m2: f64,
    },
    Sensor {
        range_m: f64,
    },
}
#[derive(Clone, Debug)]
pub struct DeviceDescriptor {
    pub handle: DeviceHandle,
    /// Zero identifies integrated avionics; physical parts have nonzero IDs.
    pub part_id: u64,
    pub control_enabled: bool,
    pub alias: String,
    pub groups: Vec<String>,
    pub kind: DeviceKind,
    /// Ship-local metres from the dry centre of mass.
    pub position_m: [f64; 3],
    /// Rotation from device-local axes to ship-local axes (xyzw).
    pub rotation: [f64; 4],
}
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceReading {
    Rcs {
        thrust_n: [f64; 3],
    },
    Weapon(osg_ship_api::abi::WeaponReading),
    Accelerometer {
        sample: Option<AccelerometerSample>,
    },
    Computer,
    Storage,
    Battery,
    Engine {
        thrust_n: f64,
    },
    Torquer {
        torque_nm: f64,
    },
    Generator {
        power_w: f64,
    },
    Shield {
        state: u64,
        temperature_k: f64,
        reserve_kg: f64,
        reserve_capacity_kg: f64,
        strength: f64,
        ablation_kg_s: f64,
        radiated_power_w: f64,
        power_w: f64,
    },
    Sensor {
        range_m: f64,
    },
}
/// Ideal specific-force measurement at the device mount, in device-local m/s².
/// Includes thrust, drag and rotational acceleration, excludes free-fall gravity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccelerometerSample {
    pub time_s: f64,
    pub acceleration_m_s2: [f64; 3],
}
#[derive(Clone, Debug)]
pub struct DeviceStatus {
    pub operational: bool,
    pub powered: bool,
    pub reading: DeviceReading,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DeviceSetting {
    RcsThrust([f64; 3]),
    Weapon(#[serde(with = "crate::weapons::SavedWeaponSetting")] osg_ship_api::abi::WeaponSetting),
    Throttle(f64),
    /// Requested torque in device-local axes; clamped to per-axis hardware limits.
    TorqueNm([f64; 3]),
    GeneratorDemand(f64),
    ShieldEnabled(bool),
    SensorEnabled(bool),
}
impl DeviceSetting {
    pub fn finite(&self) -> bool {
        match self {
            Self::Weapon(value) => crate::weapons::valid_setting(value),
            Self::RcsThrust(value) => value.iter().all(|v| v.is_finite()),
            Self::Throttle(v) | Self::GeneratorDemand(v) => v.is_finite(),
            Self::TorqueNm(v) => v.iter().all(|v| v.is_finite()),
            _ => true,
        }
    }
    pub fn supports(&self, kind: &DeviceKind) -> bool {
        matches!(
            (self, kind),
            (Self::RcsThrust(_), DeviceKind::Rcs { .. })
                | (Self::Weapon(_), DeviceKind::Weapon)
                | (Self::Throttle(_), DeviceKind::Engine { .. })
                | (Self::TorqueNm(_), DeviceKind::Torquer { .. })
                | (Self::GeneratorDemand(_), DeviceKind::Generator { .. })
                | (Self::ShieldEnabled(_), DeviceKind::Shield { .. })
                | (Self::SensorEnabled(_), DeviceKind::Sensor { .. })
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCommand {
    pub device: DeviceHandle,
    pub setting: DeviceSetting,
}

impl DeviceKind {
    pub fn abi_tag(&self) -> u64 {
        match self {
            Self::Weapon => 9,
            Self::Rcs { .. } => 10,
            Self::Accelerometer => 0,
            Self::Computer => 1,
            Self::Storage { .. } => 2,
            Self::Battery { .. } => 3,
            Self::Engine { .. } => 4,
            Self::Torquer { .. } => 5,
            Self::Generator { .. } => 6,
            Self::Shield { .. } => 7,
            Self::Sensor { .. } => 8,
        }
    }
}

impl From<&DeviceDescriptor> for osg_ship_api::abi::DeviceInfo {
    fn from(device: &DeviceDescriptor) -> Self {
        Self {
            id: u64::from(device.handle.0) + 1,
            part_id: device.part_id,
            kind: device.kind.abi_tag(),
            flags: u64::from(device.control_enabled),
            position_m: device.position_m,
            rotation: device.rotation,
            alias: osg_ship_api::abi::Text64::new(&device.alias),
            group_count: device.groups.len() as u64,
        }
    }
}

impl DeviceStatus {
    pub fn abi_record(&self) -> Vec<u8> {
        use osg_ship_api::abi::{self as abi, Record};

        let status = abi::DeviceStatus {
            flags: u64::from(self.operational) | (u64::from(self.powered) << 1),
        };

        match self.reading {
            DeviceReading::Rcs { thrust_n } => {
                abi::RcsReading { status, thrust_n }.bytes().to_vec()
            }
            DeviceReading::Weapon(mut reading) => {
                reading.status = status;
                reading.bytes().to_vec()
            }
            DeviceReading::Accelerometer { sample } => abi::AccelerometerReading {
                status,
                sample_present: u64::from(sample.is_some()),
                sample_time_s: sample.map_or(0., |sample| sample.time_s),
                acceleration: sample.map_or([0.; 3], |sample| sample.acceleration_m_s2),
            }
            .bytes()
            .to_vec(),
            DeviceReading::Engine { thrust_n } => {
                abi::EngineReading { status, thrust_n }.bytes().to_vec()
            }
            DeviceReading::Torquer { torque_nm } => abi::TorquerReading {
                status,
                torque_magnitude_nm: torque_nm,
            }
            .bytes()
            .to_vec(),
            DeviceReading::Generator { power_w } => {
                abi::GeneratorReading { status, power_w }.bytes().to_vec()
            }
            DeviceReading::Shield {
                state,
                temperature_k,
                reserve_kg,
                reserve_capacity_kg,
                strength,
                ablation_kg_s,
                radiated_power_w,
                power_w,
            } => abi::ShieldReading {
                status,
                state,
                temperature_k,
                reserve_kg,
                reserve_capacity_kg,
                strength,
                ablation_kg_s,
                radiated_power_w,
                power_w,
            }
            .bytes()
            .to_vec(),
            DeviceReading::Sensor { range_m } => {
                abi::SensorReading { status, range_m }.bytes().to_vec()
            }
            DeviceReading::Computer | DeviceReading::Storage | DeviceReading::Battery => {
                status.bytes().to_vec()
            }
        }
    }
}
