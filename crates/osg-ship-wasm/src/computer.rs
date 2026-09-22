//! Native simulation inputs, player requests, and committed hardware actions.

use crate::screens::ScreenImage;
use osg_ship_api::abi;
use osg_ships::{DeviceCommand, DeviceStatus};

#[derive(Clone, Debug)]
pub enum Command {
    SetThrottle(f64),
    Manual {
        throttle: f64,
        steering: [f64; 3],
    },
    MarkTarget {
        contact: u64,
        maximum_flight_time_s: f64,
    },
    StopFiring,
    UnmarkTarget,
    StartFiring,
    HoldAttitude,
    StopGuidance,
    AimDirection([f64; 3]),
    AimContact(u64),
    SelectTarget(u64),
    EngageNavigation {
        throttle_limit: f64,
        stand_off_m: f64,
    },
}

impl Command {
    pub fn payload(&self) -> (u64, Vec<u8>) {
        use abi::Record;

        match *self {
            Self::SetThrottle(throttle) => (abi::REQUEST_THROTTLE, throttle.to_le_bytes().to_vec()),
            Self::MarkTarget {
                contact,
                maximum_flight_time_s,
            } => (
                abi::REQUEST_MARK_TARGET,
                abi::MarkTargetRequest {
                    contact,
                    maximum_flight_time_s,
                }
                .bytes()
                .to_vec(),
            ),
            Self::UnmarkTarget => (abi::REQUEST_UNMARK_TARGET, Vec::new()),
            Self::StartFiring => (abi::REQUEST_START_FIRING, Vec::new()),
            Self::StopFiring => (abi::REQUEST_STOP_FIRING, Vec::new()),
            Self::Manual { throttle, steering } => (
                abi::REQUEST_MANUAL,
                abi::ManualRequest { throttle, steering }.bytes().to_vec(),
            ),
            Self::HoldAttitude => (abi::REQUEST_HOLD_ATTITUDE, Vec::new()),
            Self::StopGuidance => (abi::REQUEST_STOP_GUIDANCE, Vec::new()),
            Self::AimDirection(direction) => (
                abi::REQUEST_AIM_DIRECTION,
                abi::DirectionRequest { direction }.bytes().to_vec(),
            ),
            Self::AimContact(contact) => (
                abi::REQUEST_AIM_CONTACT,
                abi::ContactRequest { contact }.bytes().to_vec(),
            ),
            Self::SelectTarget(contact) => (
                abi::REQUEST_SELECT_TARGET,
                abi::ContactRequest { contact }.bytes().to_vec(),
            ),
            Self::EngageNavigation {
                throttle_limit,
                stand_off_m,
            } => (
                abi::REQUEST_ENGAGE_NAVIGATION,
                abi::NavigationRequest {
                    throttle_limit,
                    stand_off_m,
                }
                .bytes()
                .to_vec(),
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}

#[derive(Clone, Debug)]
pub struct RequestReply {
    pub id: u64,
    pub result: u64,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Observation {
    pub time_s: f64,
    pub flight: abi::FlightState,
    pub resources: abi::ShipResources,
    pub inventory: Vec<u64>,
}

impl std::ops::Deref for Observation {
    type Target = abi::FlightState;

    fn deref(&self) -> &Self::Target {
        &self.flight
    }
}

#[derive(Clone, Debug)]
pub struct Input {
    pub tick: u64,
    pub dt: f64,
    pub physics_dt: f64,
    pub observation: Observation,
    pub devices: Vec<DeviceStatus>,
    pub commands: Vec<Request>,
    pub screen_events: Vec<abi::ScreenEvent>,
    pub requested_screens: Vec<u8>,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            tick: 0,
            dt: 0.1,
            physics_dt: 0.1,
            observation: Observation::default(),
            devices: Vec::new(),
            commands: Vec::new(),
            screen_events: Vec::new(),
            requested_screens: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackKind {
    Ship,
    Display,
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub world_actions: Vec<osg_model::ProgramAction>,
    pub devices: Vec<DeviceCommand>,
    pub replies: Vec<RequestReply>,
    pub screens: Vec<ScreenImage>,
    pub cleared_screens: Vec<u64>,
    pub tick_interval_seconds: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct SensorContact {
    pub iff: Option<(osg_model::EntityId, osg_model::IffIdentity)>,
    pub measured: abi::Contact,
    pub name: String,
}

impl std::ops::Deref for SensorContact {
    type Target = abi::Contact;

    fn deref(&self) -> &Self::Target {
        &self.measured
    }
}

impl crate::Controller {
    pub fn has_pending_input(&self) -> bool {
        !self.pending_requests.is_empty() || !self.pending_events.is_empty()
    }

    pub fn enqueue_screen_event(&mut self, event: abi::ScreenEvent) -> anyhow::Result<()> {
        self.queue_input(Vec::new(), vec![event])
    }

    pub(crate) fn queue_input(
        &mut self,
        requests: Vec<Request>,
        events: Vec<abi::ScreenEvent>,
    ) -> anyhow::Result<()> {
        let mut pending = self.pending_requests.clone();

        for request in requests {
            anyhow::ensure!(
                !pending.iter().any(|queued| queued.id == request.id),
                "duplicate request ID"
            );

            if matches!(request.command, Command::Manual { .. }) {
                pending.retain(|queued| !matches!(queued.command, Command::Manual { .. }));
            }

            anyhow::ensure!(
                pending.len() < abi::MAX_INPUTS as usize,
                "request queue full"
            );
            pending.push(request);
        }

        let mut pending_events = self.pending_events.clone();

        for mut event in events {
            anyhow::ensure!(
                event.screen < u64::from(abi::MAX_SCREENS)
                    && event.kind <= abi::EVENT_RESET
                    && event.x.is_finite()
                    && event.y.is_finite()
                    && event.text.as_str().is_some(),
                "invalid screen event"
            );
            anyhow::ensure!(
                !pending_events.iter().any(|queued| queued.id == event.id),
                "duplicate screen event ID"
            );

            if event.kind == abi::EVENT_POINTER_MOVE
                && pending_events.last().is_some_and(|previous| {
                    previous.screen == event.screen && previous.kind == abi::EVENT_POINTER_MOVE
                })
            {
                pending_events.pop();
            }

            if pending_events.len() >= abi::MAX_INPUTS as usize {
                // Reset the affected screen instead of losing a release event.
                // Use the incoming event ID, so acknowledgements remain unambiguous.
                pending_events.retain(|queued| queued.screen != event.screen);
                event = abi::ScreenEvent {
                    id: event.id,
                    screen: event.screen,
                    kind: abi::EVENT_RESET,
                    ..Default::default()
                };

                // A new screen can arrive while other screens fill the queue.
                // Collapse each existing screen to one reset using its newest ID.
                if pending_events.len() >= abi::MAX_INPUTS as usize {
                    let mut resets = std::collections::BTreeMap::new();

                    for queued in pending_events.drain(..) {
                        resets.insert(
                            queued.screen,
                            abi::ScreenEvent {
                                id: queued.id,
                                screen: queued.screen,
                                kind: abi::EVENT_RESET,
                                ..Default::default()
                            },
                        );
                    }

                    pending_events.extend(resets.into_values());
                }
            }

            pending_events.push(event);
        }

        self.pending_requests = pending;
        self.pending_events = pending_events;
        Ok(())
    }

    /// Install immutable hardware metadata when the physical design changes.
    pub fn configure_hardware(
        &mut self,
        design: &osg_ships::CompiledShipDesign,
        catalogue: &osg_ships::Catalogue,
    ) {
        use abi::Record;
        use osg_ships::{DeviceKind, DeviceSource, Equipment};

        self.catalogue = design.device_catalogue.clone().into();
        let sensor_power_w = design
            .parts
            .iter()
            .filter_map(|part| match part.definition.equipment {
                Equipment::Utility {
                    utility: osg_ships::utilities::UtilityDef::Sensor { power_w, .. },
                } => Some(power_w),
                _ => None,
            })
            .reduce(|total, power| total + power)
            .unwrap_or(osg_ships::SENSOR_POWER_W);

        self.resource_specs = catalogue
            .resources
            .iter()
            .enumerate()
            .map(|(index, resource)| abi::ResourceInfo {
                id: index as u64 + 1,
                key: abi::Text64::new(&resource.id),
                unit_mass_kg: resource.mass_kg,
                unit_volume_m3: resource.volume_m3,
            })
            .collect();

        self.device_specs = design
            .device_sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                if let DeviceSource::MicropulseGenerator(part) = source {
                    let Equipment::MicropulseEngine {
                        thrust_n,
                        specific_impulse_s,
                        charge_energy_j_kg,
                        ..
                    } = design.parts[*part].definition.equipment
                    else {
                        unreachable!("micropulse generator belongs to a micropulse engine")
                    };
                    let resource = catalogue
                        .resources
                        .iter()
                        .position(|resource| resource.id == "micropulse_charge")
                        .unwrap();
                    let exhaust_velocity = osg_ships::STANDARD_GRAVITY_M_S2 * specific_impulse_s;
                    let power = 0.005 * thrust_n * exhaust_velocity;
                    return abi::GeneratorSpec {
                        fuel_resource: resource as u64 + 1,
                        max_power_w: power,
                        efficiency: 0.005 * exhaust_velocity * exhaust_velocity
                            / charge_energy_j_kg,
                        fuel_units_s: thrust_n
                            / exhaust_velocity
                            / catalogue.resources[resource].mass_kg,
                    }
                    .bytes()
                    .to_vec();
                }
                let DeviceSource::Part(part) = source else {
                    return match design.device_catalogue[index].kind {
                        DeviceKind::Sensor { range_m } => abi::SensorSpec {
                            max_range_m: range_m,
                            power_w: sensor_power_w,
                        }
                        .bytes()
                        .to_vec(),
                        _ => Vec::new(),
                    };
                };

                match design.parts[*part].definition.equipment {
                    Equipment::Weapon { .. } => design.weapon_specs
                        [design.part_weapons[*part].unwrap()]
                    .bytes()
                    .to_vec(),
                    Equipment::Rcs {
                        ref propellant_resource,
                        thrust_n,
                        propellant_kg_s,
                        power_w,
                    } => abi::RcsSpec {
                        propellant_resource: catalogue
                            .resources
                            .iter()
                            .position(|r| r.id == *propellant_resource)
                            .expect("validated RCS resource")
                            as u64
                            + 1,
                        per_axis_thrust_n: thrust_n,
                        per_axis_propellant_units_s: propellant_kg_s
                            / catalogue
                                .resources
                                .iter()
                                .find(|r| r.id == *propellant_resource)
                                .expect("validated RCS resource")
                                .mass_kg,
                        per_axis_power_w: power_w,
                    }
                    .bytes()
                    .to_vec(),
                    Equipment::Utility { .. }
                    | Equipment::Structure
                    | Equipment::CoolantTank { .. }
                    | Equipment::Radiator { .. }
                    | Equipment::EmergencyCooling { .. }
                    | Equipment::HeatSink { .. } => Vec::new(),
                    Equipment::Storage { capacity_m3 } => {
                        abi::StorageSpec { capacity_m3 }.bytes().to_vec()
                    }
                    Equipment::Battery { capacity_j } => {
                        abi::BatterySpec { capacity_j }.bytes().to_vec()
                    }
                    Equipment::Engine { .. }
                    | Equipment::MicropulseEngine { .. }
                    | Equipment::ThermalEngine { .. } => {
                        let DeviceKind::Engine {
                            thrust_n,
                            propellant_resource,
                            propellant_kg_s,
                            power_w,
                        } = &design.device_catalogue[index].kind
                        else {
                            unreachable!("engine equipment has an engine descriptor");
                        };
                        let resource_index = catalogue
                            .resources
                            .iter()
                            .position(|resource| resource.id == *propellant_resource)
                            .expect("validated engine resource");
                        abi::EngineSpec {
                            propellant_resource: resource_index as u64 + 1,
                            max_thrust_n: *thrust_n,
                            propellant_units_s: propellant_kg_s
                                / catalogue.resources[resource_index].mass_kg,
                            max_power_w: *power_w,
                        }
                        .bytes()
                        .to_vec()
                    }
                    Equipment::Torquer { torque_nm, power_w } => abi::TorquerSpec {
                        per_axis_limit_nm: torque_nm,
                        max_power_w: power_w,
                    }
                    .bytes()
                    .to_vec(),
                    Equipment::Reactor { spec } => abi::GeneratorSpec {
                        fuel_resource: catalogue
                            .resources
                            .iter()
                            .position(|r| r.id == "reactor_fuel")
                            .unwrap() as u64
                            + 1,
                        max_power_w: spec.thermal_power_w * spec.efficiency(300.0),
                        efficiency: spec.efficiency(300.0),
                        fuel_units_s: spec.thermal_power_w / spec.fuel_energy_j_kg,
                    }
                    .bytes()
                    .to_vec(),
                    Equipment::FuelProcessor { .. } => Vec::new(),
                    Equipment::Generator {
                        power_w,
                        fuel_kg_s,
                        efficiency,
                    } => abi::GeneratorSpec {
                        fuel_resource: 2,
                        max_power_w: power_w,
                        efficiency,
                        fuel_units_s: fuel_kg_s / catalogue.resources[1].mass_kg,
                    }
                    .bytes()
                    .to_vec(),
                    Equipment::Shield {
                        deployed_mass_kg,
                        radiator_area_m2,
                        feed_rate_kg_s,
                        power_w,
                    } => abi::ShieldSpec {
                        deployed_mass_kg,
                        radiator_area_m2,
                        feed_rate_kg_s,
                        emissivity: osg_ships::thermal::EMISSIVITY,
                        sustain_power_w: power_w,
                    }
                    .bytes()
                    .to_vec(),
                }
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer() -> crate::Controller {
        let bytes = wat::parse_str(format!(
            r#"(module
            (memory (export "memory") 1)
            (func (export "game_version") (result i32) i32.const {})
            (func (export "ship_tick")))"#,
            osg_ship_api::GAME_VERSION as u32,
        ))
        .unwrap();
        crate::ControllerRuntime::new()
            .unwrap()
            .instantiate(&bytes)
            .unwrap()
    }

    #[test]
    fn screen_overflow_resets_held_input_and_retains_unique_acknowledgement_ids() {
        let mut computer = computer();

        for id in 1..=256 {
            computer
                .enqueue_screen_event(abi::ScreenEvent {
                    id,
                    screen: 0,
                    kind: abi::EVENT_POINTER_PRESS,
                    ..Default::default()
                })
                .unwrap();
        }

        computer
            .enqueue_screen_event(abi::ScreenEvent {
                id: 257,
                screen: 1,
                kind: abi::EVENT_POINTER_RELEASE,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(computer.pending_events.len(), 2);
        assert!(
            computer
                .pending_events
                .iter()
                .all(|event| event.kind == abi::EVENT_RESET)
        );
        assert_ne!(computer.pending_events[0].id, computer.pending_events[1].id);

        for id in 258..300 {
            computer
                .enqueue_screen_event(abi::ScreenEvent {
                    id,
                    screen: 0,
                    kind: abi::EVENT_POINTER_MOVE,
                    x: id as f64,
                    ..Default::default()
                })
                .unwrap();
        }

        assert_eq!(computer.pending_events.len(), 3);
        assert_eq!(computer.pending_events.last().unwrap().id, 299);
        computer.reboot();
        assert!(computer.pending_events.is_empty());
    }

    #[test]
    fn manual_samples_coalesce_without_dropping_unacknowledged_guidance_requests() {
        let mut computer = computer();
        computer
            .queue_input(
                vec![Request {
                    id: 1,
                    command: Command::HoldAttitude,
                }],
                Vec::new(),
            )
            .unwrap();

        for id in 2..300 {
            computer
                .queue_input(
                    vec![Request {
                        id,
                        command: Command::Manual {
                            throttle: 0.1,
                            steering: [0.; 3],
                        },
                    }],
                    Vec::new(),
                )
                .unwrap();
        }

        assert_eq!(computer.pending_requests.len(), 2);
        assert_eq!(computer.pending_requests[0].id, 1);
        assert_eq!(computer.pending_requests[1].id, 299);
        computer.reboot();
        assert!(computer.pending_requests.is_empty());
        assert!(!computer.has_pending_input());
    }
}

#[derive(Debug)]
pub enum WorldQueryError {
    BufferTooSmall,
    LimitExceeded,
}

impl std::fmt::Display for WorldQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BufferTooSmall => formatter.write_str("world query reply buffer is too small"),
            Self::LimitExceeded => formatter.write_str("world query exceeds its work limit"),
        }
    }
}

impl std::error::Error for WorldQueryError {}

pub fn query_work(query: &osg_model::ProgramQuery) -> u64 {
    use osg_model::ProgramQuery;

    match query {
        ProgramQuery::Orrery { .. } => osg_model::local_space::QUERY_GAS,
        ProgramQuery::RouteRequest(_) => osg_model::routing::REQUEST_GAS,
        ProgramQuery::RoutePoll { .. } => osg_model::routing::POLL_GAS,
        ProgramQuery::SlipEligibility { .. } => 131_072,
        ProgramQuery::Beacons { limit, .. } => 100 + 1008 * u64::from((*limit).min(256)),
        ProgramQuery::Beacon(_)
        | ProgramQuery::Contact(_)
        | ProgramQuery::Travel
        | ProgramQuery::Resolve { .. } => 1000,
    }
}
