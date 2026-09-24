//! This firmware's hardware discovery cache and measured actuator availability.
use osg_ship_api::abi;

#[derive(Clone, Debug)]
pub enum Capability {
    Passive,
    Gun(abi::GunSpec),
    Laser(abi::LaserSpec),
    Engine(abi::EngineSpec),
    Torquer(abi::TorquerSpec),
    Rcs(abi::RcsSpec),
    Generator(abi::GeneratorSpec),
}

#[derive(Clone, Debug)]
pub struct Device {
    pub info: abi::DeviceInfo,
    pub capability: Capability,
    pub status: abi::DeviceStatus,
    pub accelerometer: abi::AccelerometerReading,
    pub thrust_n: f64,
    pub resource_mass_kg: f64,
}

impl Device {
    pub fn available(&self) -> bool {
        self.status.flags & (abi::OPERATIONAL | abi::POWERED) == abi::OPERATIONAL | abi::POWERED
    }
}

#[derive(Default)]
pub struct Hardware {
    pub rcs_readings: std::collections::BTreeMap<u64, abi::RcsReading>,
    pub weapon_readings: std::collections::BTreeMap<u64, abi::WeaponReading>,
    pub shield_readings: std::collections::BTreeMap<u64, abi::ShieldReading>,
    pub devices: Vec<Device>,
    pub propellants: Vec<abi::ResourceInfo>,
    #[cfg(target_arch = "wasm32")]
    next_device: u32,
    #[cfg(target_arch = "wasm32")]
    next_resource: u32,
}

impl Hardware {
    pub fn get(&self, id: u64) -> Option<&Device> {
        self.devices.iter().find(|device| device.info.id == id)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn discover(&mut self, tick: &abi::TickContext) -> Result<bool, i32> {
        use osg_ship_api::sdk;

        // Discovery is ordinary firmware state across completed callbacks.
        // Pending user requests remain in the host until discovery finishes.
        for _ in 0..16 {
            if self.next_resource < tick.resource_count as u32 {
                self.next_resource += 1;
            } else if self.next_device < tick.device_count as u32 {
                let info = sdk::device(self.next_device)?;
                let capability = match info.kind {
                    abi::DEVICE_RCS => Capability::Rcs(sdk::device_spec(info.id, info.kind)?),
                    abi::DEVICE_GUN => Capability::Gun(sdk::device_spec(info.id, info.kind)?),
                    abi::DEVICE_LASER => Capability::Laser(sdk::device_spec(info.id, info.kind)?),
                    abi::DEVICE_ENGINE => Capability::Engine(sdk::device_spec(info.id, info.kind)?),
                    abi::DEVICE_TORQUER => {
                        Capability::Torquer(sdk::device_spec(info.id, info.kind)?)
                    }
                    abi::DEVICE_GENERATOR => {
                        Capability::Generator(sdk::device_spec(info.id, info.kind)?)
                    }
                    _ => Capability::Passive,
                };
                let resource_id = match capability {
                    Capability::Engine(spec) => spec.propellant_resource,
                    Capability::Rcs(spec) => spec.propellant_resource,
                    Capability::Generator(spec) => spec.fuel_resource,
                    _ => 0,
                };
                if let Capability::Engine(spec) = capability {
                    if spec.propellant_resource > 0
                        && !self
                            .propellants
                            .iter()
                            .any(|r| r.id == spec.propellant_resource)
                    {
                        self.propellants
                            .push(sdk::resource_info((spec.propellant_resource - 1) as u32)?);
                    }
                }
                let resource_mass_kg = if resource_id == 0 {
                    0.
                } else {
                    sdk::resource_info((resource_id - 1) as u32)?.unit_mass_kg
                };
                self.devices.push(Device {
                    info,
                    capability,
                    status: abi::DeviceStatus::default(),
                    accelerometer: abi::AccelerometerReading::default(),
                    thrust_n: 0.,
                    resource_mass_kg,
                });
                self.next_device += 1;
            } else {
                return Ok(true);
            }
        }

        Ok(self.next_resource == tick.resource_count as u32
            && self.next_device == tick.device_count as u32)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn measure(&mut self) -> Result<(), i32> {
        use osg_ship_api::sdk;

        for device in &mut self.devices {
            let id = device.info.id;
            let kind = device.info.kind;
            device.status = match kind {
                abi::DEVICE_GUN | abi::DEVICE_LASER => {
                    let reading: abi::WeaponReading = sdk::device_read(id, kind)?;
                    self.weapon_readings.insert(id, reading);
                    reading.status
                }
                abi::DEVICE_ENGINE => {
                    let reading: abi::EngineReading = sdk::device_read(id, kind)?;
                    device.thrust_n = reading.thrust_n;
                    reading.status
                }
                abi::DEVICE_TORQUER => sdk::device_read::<abi::TorquerReading>(id, kind)?.status,
                abi::DEVICE_RCS => {
                    let reading: abi::RcsReading = sdk::device_read(id, kind)?;
                    self.rcs_readings.insert(id, reading);
                    reading.status
                }
                abi::DEVICE_GENERATOR => {
                    sdk::device_read::<abi::GeneratorReading>(id, kind)?.status
                }
                abi::DEVICE_SHIELD => {
                    let reading: abi::ShieldReading = sdk::device_read(id, kind)?;
                    self.shield_readings.insert(id, reading);
                    reading.status
                }
                abi::DEVICE_SENSOR => sdk::device_read::<abi::SensorReading>(id, kind)?.status,
                abi::DEVICE_ACCELEROMETER => {
                    device.accelerometer = sdk::device_read(id, kind)?;
                    device.accelerometer.status
                }
                _ => sdk::device_read(id, kind)?,
            };
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub tick: abi::TickContext,
    pub flight: abi::FlightState,
    pub propellant_kg: f64,
}

impl std::ops::Deref for Sample {
    type Target = abi::FlightState;

    fn deref(&self) -> &Self::Target {
        &self.flight
    }
}

impl std::ops::DerefMut for Sample {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.flight
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Actuation {
    Rcs {
        device: u64,
        value: abi::RcsSetting,
    },
    Throttle {
        device: u64,
        value: abi::ThrottleSetting,
    },
    Torque {
        device: u64,
        value: abi::TorqueSetting,
    },
}

impl Actuation {
    #[cfg(target_arch = "wasm32")]
    pub fn apply(&self) -> Result<(), i32> {
        use osg_ship_api::sdk;

        match self {
            Self::Rcs { device, value } => sdk::device_write(*device, abi::SET_RCS, value),
            Self::Throttle { device, value } => {
                sdk::device_write(*device, abi::SET_THROTTLE, value)
            }
            Self::Torque { device, value } => sdk::device_write(*device, abi::SET_TORQUE, value),
        }
    }
}
