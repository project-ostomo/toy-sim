use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use osg_model::{GalacticPosition, presentation::*};
use osg_ship_api::abi;
use osg_ship_wasm::spatial;
use osg_ships::{DeviceKind, DeviceReading as Reading, DeviceSetting};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use super::{
    hardware,
    identity::{Control, Identity},
    physics::{MassProps, Velocity},
    precision::PreciseTransform,
    simulation::SimulationCounters,
    vessel::{ComputerBudget, ShipCatalogue, ShipDesign, ShipSoftware, SoftwareDiagnostics},
};

/// Account-independent readings, valid only within one publication batch.
/// RPC callers use a fresh instance to observe mutations immediately.
#[derive(Resource, Clone, Default)]
pub struct DeviceReadings(Arc<Mutex<HashMap<Entity, Option<Arc<Vec<osg_ships::DeviceStatus>>>>>>);

impl DeviceReadings {
    fn get(&self, world: &World, entity: Entity) -> Option<Arc<Vec<osg_ships::DeviceStatus>>> {
        if let Some(readings) = self.0.lock().unwrap().get(&entity) {
            #[cfg(test)]
            crate::sim::diagnostics::samples::count("publication.device_readings_hits", 1);
            return readings.clone();
        }
        let _profile = crate::sim::diagnostics::ProfileScope::new("publication.device_readings");
        #[cfg(test)]
        crate::sim::diagnostics::samples::count("publication.device_readings_builds", 1);
        let readings = hardware::device_readings(world, entity).map(Arc::new);
        self.0.lock().unwrap().insert(entity, readings.clone());
        readings
    }
}

fn bounded(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn attitude_reading(instrument: abi::AttitudeState) -> AttitudeInstrument {
    AttitudeInstrument {
        mode: instrument.mode,
        reference: (instrument.present & abi::ATTITUDE_REFERENCE != 0)
            .then_some(instrument.reference),
        control_error_rad: instrument.control_error,
    }
}

fn nanoseconds(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1e9).round() as u64
}

pub fn ship(
    world: &World,
    entity: Entity,
    include_instruments: bool,
    cache: &DeviceReadings,
) -> Option<ShipPresentation> {
    let id = world.get::<Identity>(entity)?.0;
    let design = &world.get::<ShipDesign>(entity)?.0;
    let cargo = &world.get::<hardware::ShipInventory>(entity)?.0;
    let hull = world.get::<hardware::Hull>(entity)?.0;
    let thermal = &world.get::<hardware::ShipThermal>(entity)?.0;
    let settings = &world.get::<hardware::DeviceSettings>(entity)?.0;
    let readings = cache.get(world, entity)?;
    let software = world.get::<ShipSoftware>(entity)?;
    let budget = world.get::<ComputerBudget>(entity)?;
    let diagnostics = world.get::<SoftwareDiagnostics>(entity)?;
    let catalogue = &world.resource::<ShipCatalogue>().0;
    let mass = world.get::<MassProps>(entity)?;
    let tick = world.resource::<SimulationCounters>().ticks;
    let inventory: Vec<_> = catalogue
        .resources
        .iter()
        .zip(&cargo.quantities)
        .enumerate()
        .map(|(index, (resource, quantity))| ResourceAmount {
            name: bounded(&resource.title, 128),
            unit_mass_kg: resource.mass_kg,
            unit_volume_m3: resource.volume_m3,
            resource: resource.id.clone(),
            quantity: *quantity,
            amount_kg: *quantity as f64 * resource.mass_kg,
            capacity_kg: if resource.volume_m3 > 0.0 {
                cargo.tank_capacities_m3[index] / resource.volume_m3 * resource.mass_kg
            } else {
                0.0
            },
        })
        .collect();
    let reboot_remaining_s = software.controller.boot_remaining_gas() as f64
        / budget.gas_limit() as f64
        * osg_model::TICK_SECONDS;
    let suspended = world
        .get::<super::travel::SystemsSuspended>(entity)
        .is_some();
    let computer_powered = !suspended
        && osg_ships::computer_running(hull, &world.get::<hardware::Avionics>(entity)?.0);
    let computer = if suspended {
        ComputerStatus::Paused
    } else if let Some(fault) = &software.controller.fault {
        ComputerStatus::Fault {
            message: bounded(fault, 4096),
            reboot_remaining_s: computer_powered.then_some(reboot_remaining_s),
        }
    } else if !computer_powered {
        ComputerStatus::Unpowered
    } else if software.controller.is_booting() {
        ComputerStatus::Booting {
            progress: software.controller.boot_progress(),
            remaining_s: reboot_remaining_s,
        }
    } else {
        ComputerStatus::Running {
            gas_used: budget.used_gas(),
            gas_limit: budget.gas_limit(),
            execution: match software.controller.execution_status() {
                osg_model::ExecutionStatus::WaitingForGas if budget.display_limited() => {
                    osg_model::ExecutionStatus::Suspended
                }
                status => status,
            },
        }
    };
    let mut screens = super::displays::definitions(world, entity);
    if screens.is_empty() {
        screens = software
            .controller
            .screens
            .iter()
            .filter_map(|screen| {
                Some(ScreenDefinition {
                    slot: u8::try_from(screen.id).ok()?,
                    width: u16::try_from(screen.width).ok()?,
                    height: u16::try_from(screen.height).ok()?,
                    title: screen.title.as_str()?.to_owned(),
                })
            })
            .collect();
    }
    let power = world.get::<hardware::PowerFlow>(entity);
    let slip_input = world
        .get::<super::travel::SlipChargingPower>(entity)
        .map_or(0., |power| power.0);
    let drive = world.get::<super::travel::SlipDrive>(entity);
    let preparation = drive.and_then(|drive| drive.preparation.as_ref());
    let slip_charge = preparation.map(|preparation| {
        let remaining = (preparation.required_j - preparation.work_j).max(0.);
        let elapsed = tick.saturating_sub(preparation.started) as f64 * osg_model::TICK_SECONDS;
        let minimum = (osg_model::travel::slip::MIN_CHARGE_SECONDS - elapsed).max(0.0);
        SlipChargeTelemetry {
            stored_j: preparation.work_j as u64,
            required_j: preparation.required_j.ceil() as u64,
            input_w: slip_input,
            remaining_s: if remaining == 0. {
                Some(minimum)
            } else if slip_input > 0. {
                Some((remaining / slip_input).max(minimum))
            } else {
                None
            },
        }
    });
    Some(ShipPresentation {
        navigation_access: super::services::navigation_access(world, entity),
        serial: software.controller.state.serial.screen.clone(),
        memory_limit_bytes: osg_ship_wasm::MEMORY_LIMIT as u64,
        cargo: cargo.cargo_stacks(catalogue).expect("valid ship cargo"),
        cargo_capacity_m3: design.capacity_m3,
        cargo_used_m3: cargo.cargo_volume(catalogue),
        propulsion: {
            let mut reading = world
                .get::<hardware::propulsion::InstalledRatings>(entity)?
                .0
                .clone();
            if world.get::<super::travel::Dormant>(entity).is_none() && hull > 0. {
                let output = world.get::<hardware::propulsion::ActuatorOutput>(entity)?;
                reading.force_n = output.force.to_array();
                reading.torque_nm = output.torque.to_array();
            }
            reading.drives = hardware::propulsion::reserves(design, mass.mass, &inventory);
            reading
        },
        ship: id,
        revision: world.get::<Control>(entity)?.revision,
        sim_time_ns: tick.saturating_mul(osg_model::TICK_NS),
        environment: world
            .get::<super::physics::aerodynamics::AeroEnv>(entity)
            .map(|env| FlightEnvironment {
                altitude_m: env.altitude,
                airspeed_m_s: env.airspeed.to_array(),
                density_kg_m3: env.density,
                pressure_pa: env.pressure,
            }),
        health: Some(ShipHealth {
            crew_people: world
                .get::<hardware::utilities::Crew>(entity)
                .map_or(0, |crew| crew.people),
            crew_capacity: world
                .get::<hardware::utilities::Crew>(entity)
                .map_or(0, |crew| crew.capacity),
            life_support_fraction: world
                .get::<hardware::utilities::Crew>(entity)
                .map_or(1., |crew| crew.support_fraction),
            hull_hp: hull,
            hull_max_hp: design.hull,
            shield_reserve_capacity_kg: design.shield_reserve_capacity_kg,
            shield_strength: thermal.shield_strength(design.as_ref().into()),
        }),
        execution: Some(ExecutionMetrics {
            memory_bytes: software.controller.memory_bytes() as u64,
            step_us: diagnostics.last_seconds * 1e6,
            prepare_us: diagnostics.timings.prepare * 1e6,
            callback_us: diagnostics.timings.callback * 1e6,
            publish_us: diagnostics.timings.publish * 1e6,
            hardware_us: diagnostics.timings.hardware * 1e6,
            scan_us: diagnostics.timings.scan * 1e6,
        }),
        mass_kg: mass.mass,
        inertia_kg_m2: mass.inertia.to_cols_array(),
        control_rotation: DQuat::from_mat3(&osg_ships::orientation(
            design.blueprint.avionics.control_orientation,
        ))
        .to_array(),
        hull_heat_capacity_j: design.hull_heat_capacity_j,
        battery_capacity_j: design.battery_j,
        power_generated_w: power.map_or(0.0, |power| power.generated_w),
        generation_capacity_w: design
            .device_catalogue
            .iter()
            .zip(readings.iter())
            .filter_map(|(descriptor, status)| match descriptor.kind {
                DeviceKind::Generator { power_w } if status.operational => {
                    let reactor = design
                        .part_for_device(descriptor.handle)
                        .and_then(|index| world.get::<hardware::PartDevices>(entity)?.0.get(index))
                        .and_then(|part| world.get::<hardware::reactors::Reactor>(*part));
                    Some(reactor.map_or(power_w, |reactor| {
                        let sink = hardware::reactors::sink_temperature(thermal, design);
                        let spec = reactor.spec;
                        let heat = (spec.heat_transfer_w_k
                            * (spec.hot_temperature_k - sink).max(0.))
                        .min(spec.thermal_power_w);
                        heat * spec.efficiency(sink)
                    }))
                }
                _ => None,
            })
            .sum(),
        reactors: world
            .get::<hardware::PartDevices>(entity)
            .into_iter()
            .flat_map(|parts| parts.0.iter().enumerate())
            .filter_map(|(index, part)| {
                let reactor = world.get::<hardware::reactors::Reactor>(*part)?;
                let device = world.get::<hardware::Device>(*part)?;
                let setting = design.part_devices[index]
                    .and_then(|handle| settings.get(handle))
                    .cloned()
                    .flatten();
                let status = if !device.0.operational {
                    ReactorStatus::Damaged
                } else if reactor.shutdown
                    || !matches!(setting, Some(DeviceSetting::GeneratorDemand(value)) if value > 0.)
                    || !device.0.powered
                {
                    ReactorStatus::Shutdown
                } else if device.0.actual > 1.0 {
                    ReactorStatus::Running
                } else {
                    ReactorStatus::Standby
                };
                Some(ReactorTelemetry {
                    name: bounded(&design.parts[index].definition.title, 128),
                    status,
                    temperature_k: 300.
                        + reactor.core_energy_j / reactor.spec.core_heat_capacity_j_k,
                    coolant_temperature_k: hardware::reactors::sink_temperature(thermal, design),
                    operating_temperature_k: reactor.spec.hot_temperature_k,
                    shutdown_temperature_k: reactor.spec.shutdown_temperature_k,
                })
            })
            .collect(),
        slip_available: drive.is_some(),
        slip_exotic_fuel_kg: drive.map(|_| {
            let grams = catalogue
                .resources
                .iter()
                .position(|resource| resource.id == osg_model::travel::slip::EXOTIC_RESOURCE)
                .and_then(|index| cargo.quantities.get(index))
                .copied()
                .unwrap_or(0) as f64;
            grams * 0.001
        }),
        slip_navigation_lock: world
            .get::<super::travel::Transit>(entity)
            .map(|transit| transit.navigation_beacon.is_some() && !transit.beacon_lost),
        power_consumed_w: power.map_or(0.0, |power| power.supplied_w) + slip_input,
        power_requested_w: power.map_or(0., |power| power.requested_w)
            + if preparation.is_some() {
                drive.map_or(0., |drive| drive.power_w)
            } else {
                slip_input
            },
        slip_charge,
        slip_transit: world.get::<super::travel::Transit>(entity).map(|transit| {
            SlipTransitTelemetry {
                departed_ns: transit.departed * osg_model::TICK_NS,
                destination: transit.destination,
                failure_ppm: world
                    .get::<super::travel::Travel>(entity)
                    .map_or(0.0, |travel| travel.0.status.planned_loss_ppm),
                direction: transit.direction,
            }
        }),
        inventory,
        computer,
        instruments: include_instruments.then(|| instruments(world, entity, software)),
        screens,
    })
}

fn instruments(world: &World, ship: Entity, software: &ShipSoftware) -> Instruments {
    let tick = world.resource::<SimulationCounters>().ticks;
    let now = tick as f64 * osg_model::TICK_SECONDS;
    let state = &software.controller.state;
    let contact = |handle| super::services::contact_ref(world, ship, handle);
    let current = spatial::Snapshot {
        epoch: now,
        origin: world.get::<PreciseTransform>(ship).map_or([0; 3], |pose| {
            [
                pose.translation_um.x,
                pose.translation_um.y,
                pose.translation_um.z,
            ]
        }),
        velocity: world
            .get::<Velocity>(ship)
            .map_or([0.0; 3], |velocity| velocity.0.to_array()),
        rotation: world
            .get::<PreciseTransform>(ship)
            .map_or([0.0, 0.0, 0.0, 1.0], |pose| pose.rotation.to_array()),
        ..Default::default()
    };
    let attitude = state
        .attitude
        .filter(|instrument| instrument.valid_until_s > now);
    let navigation = state
        .navigation
        .filter(|instrument| instrument.valid_until_s > now);
    let weapons = state
        .weapons
        .filter(|instrument| instrument.valid_until_s > now);
    let selected_contact = state
        .contacts
        .filter(|instrument| instrument.valid_until_s > now)
        .and_then(|instrument| contact(instrument.selected_contact));
    let paths: Vec<_> = state
        .spatial
        .paths
        .iter()
        .filter_map(|(&id, path)| {
            let positions = state.spatial.polyline_at(id, current)?;
            Some(Trajectory {
                id,
                revision: path.revision,
                published_at_ns: nanoseconds(path.published_at),
                valid_until_ns: nanoseconds(path.header.meta.valid_until_s),
                timed: path.header.kind == abi::PATH_TIMED,
                vertices: positions
                    .into_iter()
                    .zip(path.vertices.iter())
                    .map(|(position, vertex)| TrajectoryVertex {
                        sim_time_ns: nanoseconds(if path.header.kind == abi::PATH_TIMED {
                            vertex.time_s
                        } else {
                            now
                        }),
                        position: GalacticPosition::new(position[0], position[1], position[2]),
                    })
                    .collect(),
            })
        })
        .collect();
    let markers = state
        .spatial
        .markers
        .iter()
        .filter_map(|(&id, marker)| {
            let position = state.spatial.marker_at(id, current)?;
            Some(NavigationMarker {
                id,
                kind: marker.record.meta.role,
                position: GalacticPosition::new(position[0], position[1], position[2]),
                sim_time_ns: nanoseconds(if marker.record.time_mode == abi::TIME_FIXED {
                    marker.record.time_s
                } else {
                    now
                }),
                label: marker
                    .record
                    .meta
                    .label
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect();
    let valid_until_ns = attitude
        .iter()
        .map(|instrument| nanoseconds(instrument.valid_until_s))
        .chain(
            navigation
                .iter()
                .map(|instrument| nanoseconds(instrument.valid_until_s)),
        )
        .chain(
            weapons
                .iter()
                .map(|instrument| nanoseconds(instrument.valid_until_s)),
        )
        .chain(paths.iter().map(|path| path.valid_until_ns))
        .max()
        .unwrap_or(tick * osg_model::TICK_NS);
    Instruments {
        valid_until_ns,
        selected_contact,
        attitude: attitude.map(attitude_reading),
        navigation: navigation.map(|instrument| NavigationInstrument {
            status: instrument.status,
            target: contact(instrument.target_contact),
            own_path: (instrument.own_path != 0).then_some(instrument.own_path),
            target_path: (instrument.target_path != 0).then_some(instrument.target_path),
            throttle_limit: instrument.throttle_limit,
            throttle: instrument.throttle,
            stand_off_m: instrument.stand_off_m,
            approach_speed_limit_m_s: instrument.approach_speed_limit_m_s,
            braking_distance_m: instrument.braking_distance_m,
            arrival_time_ns: (instrument.present & abi::NAV_ARRIVAL != 0)
                .then(|| nanoseconds(instrument.arrival_time_s)),
            predicted_fuel_kg: (instrument.present & abi::NAV_FUEL != 0)
                .then_some(instrument.predicted_fuel_kg),
            reason: instrument.reason.as_str().unwrap_or_default().to_owned(),
        }),
        weapons_state: weapons.map(|instrument| WeaponsInstrument {
            firing: instrument.mode == abi::WEAPONS_FIRING,
            target: contact(instrument.target_contact),
            reason: instrument.reason.as_str().unwrap_or_default().to_owned(),
        }),
        weapons: weapons.map_or_else(Vec::new, |instrument| {
            state
                .weapon_rows
                .iter()
                .filter_map(|row| {
                    let design = &world.get::<ShipDesign>(ship)?.0;
                    let descriptor = design
                        .device_catalogue
                        .get(row.device.checked_sub(1)? as usize)?;
                    let aim_direction = state
                        .spatial
                        .marker_at(row.aim_marker, current)
                        .and_then(|position| spatial::relative(position, current.origin))
                        .map_or([0.0; 3], |direction| {
                            DVec3::from_array(direction).normalize_or_zero().to_array()
                        });
                    Some(WeaponInstrument {
                        ammunition_units: row.reading.ammunition_units as f64,
                        battery_energy_j: row.reading.battery_energy_j,
                        shot_energy_j: row.reading.shot_energy_j,
                        pointing_error_rad: row.pointing_error_rad,
                        inhibit_flags: row.reading.inhibit_flags,
                        part: descriptor.part_id,
                        target: contact(instrument.target_contact),
                        status: row.solution_flags,
                        aim_direction,
                        flight_time_s: row.time_of_flight_s,
                    })
                })
                .collect()
        }),
        paths,
        markers,
    }
}

pub fn visual(world: &World, entity: Entity, cache: &DeviceReadings) -> Option<ShipVisual> {
    let design = &world.get::<ShipDesign>(entity)?.0;
    let thermal = &world.get::<hardware::ShipThermal>(entity)?.0;
    let mut engines = Vec::new();
    let mut turrets = Vec::new();
    let readings = cache.get(world, entity)?;
    for (descriptor, status) in design.device_catalogue.iter().zip(readings.iter()) {
        match (descriptor.kind.clone(), status.reading.clone()) {
            (
                DeviceKind::Engine {
                    thrust_n: maximum, ..
                },
                Reading::Engine { thrust_n },
            ) => engines.push(EngineVisual {
                part: descriptor.part_id,
                thrust_n: [0.0, 0.0, thrust_n],
                thrust_fraction: (thrust_n / maximum).clamp(0.0, 1.0),
            }),
            (
                DeviceKind::Rcs {
                    thrust_n: maximum, ..
                },
                Reading::Rcs { thrust_n },
            ) => engines.push(EngineVisual {
                part: descriptor.part_id,
                thrust_fraction: (DVec3::from_array(thrust_n).length() / maximum).clamp(0.0, 1.0),
                thrust_n,
            }),
            (_, Reading::Weapon(weapon)) => turrets.push(TurretVisual {
                part: descriptor.part_id,
                yaw_rad: weapon.yaw_rad,
                pitch_rad: weapon.pitch_rad,
            }),
            _ => {}
        }
    }
    Some(ShipVisual {
        slip_readiness: slip_readiness(world, entity),
        engines,
        turrets,
        shield: (thermal.shield_state == abi::SHIELD_ACTIVE).then(|| ShieldVisual {
            temperature_k: thermal.shield_temperature(design.as_ref().into()),
            coverage: thermal.shield_strength(design.as_ref().into()),
        }),
    })
}

pub fn slip_readiness(world: &World, entity: Entity) -> f64 {
    if world.get::<super::travel::Transit>(entity).is_some() {
        return 1.0;
    }
    let Some(preparation) = world
        .get::<super::travel::SlipDrive>(entity)
        .and_then(|drive| drive.preparation.as_ref())
    else {
        return 0.0;
    };
    let tick = world
        .resource::<super::simulation::SimulationCounters>()
        .ticks;
    let energy = preparation.work_j / preparation.required_j.max(1.0);
    let elapsed = tick.saturating_sub(preparation.started) as f64 * osg_model::TICK_SECONDS;
    energy
        .min(elapsed / osg_model::travel::slip::MIN_CHARGE_SECONDS)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_readings_preserve_instruments_and_refresh_between_batches() {
        let account = osg_model::Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let entity = world
            .query_filtered::<Entity, With<super::super::vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        super::super::session::prepare_publication(world);
        let cache = world.resource::<DeviceReadings>().clone();
        let original = cache.get(world, entity).unwrap();
        for instruments in [false, true] {
            let shared = ship(world, entity, instruments, &cache).unwrap();
            let fresh = ship(world, entity, instruments, &DeviceReadings::default()).unwrap();
            assert_eq!(
                postcard::to_stdvec(&shared).unwrap(),
                postcard::to_stdvec(&fresh).unwrap()
            );
            assert_eq!(shared.instruments.is_some(), instruments);
        }
        let _ = visual(world, entity, &cache).unwrap();
        assert!(Arc::ptr_eq(&original, &cache.get(world, entity).unwrap()));
        world.get_mut::<hardware::SensorRange>(entity).unwrap().0 = 1234.0;
        super::super::session::prepare_publication(world);
        let refreshed = world
            .resource::<DeviceReadings>()
            .get(world, entity)
            .unwrap();
        assert!(!Arc::ptr_eq(&original, &refreshed));
        assert_eq!(
            format!("{refreshed:?}"),
            format!("{:?}", hardware::device_readings(world, entity).unwrap())
        );
    }

    #[test]
    fn manual_flight_keeps_device_telemetry_valid_after_sustained_power_use() {
        let account = osg_model::Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let entity = world
            .query_filtered::<Entity, With<super::super::vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let unrelated: Vec<_> = world
            .query_filtered::<Entity, With<ShipDesign>>()
            .iter(world)
            .filter(|other| *other != entity)
            .collect();
        for other in unrelated {
            world.despawn(other);
        }
        let session = super::super::session::connect(
            world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        let ship_id = world.get::<Identity>(entity).unwrap().0;
        world
            .get_mut::<super::super::session::Session>(session)
            .unwrap()
            .instruments
            .insert(ship_id);
        for step in 0..6000 {
            if step % 10 == 0 {
                let phase = (step / 100) % 4;
                let steering = match phase {
                    0 => [0.2, 0.7, 0.0],
                    1 => [-0.6, 0.3, 0.1],
                    2 => [0.0, 0.0, 0.0],
                    _ => [0.4, -0.5, -0.2],
                };
                app.world_mut()
                    .get_mut::<super::super::vessel::ShipMailbox>(entity)
                    .unwrap()
                    .command(osg_ship_wasm::Command::Manual {
                        throttle: if phase == 2 { 0.0 } else { 0.8 },
                        steering,
                    });
            }
            app.update();
            super::super::infrastructure::publish_navigation(app.world_mut());
            super::super::session::prepare_publication(app.world_mut());
            let frame = super::super::session::frame(app.world_mut(), session).unwrap();
            assert_eq!(frame.presentation.ships.len(), 1);
            super::super::session::prune_events(app.world_mut());
            super::super::session::input(
                app.world_mut(),
                session,
                osg_model::InputFrame {
                    world: frame.world,
                    sequence: step + 1,
                    actions: Vec::new(),
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn manual_attitude_does_not_publish_an_absent_zero_quaternion() {
        let mut state = osg_ship_api::abi::AttitudeState::default();
        assert_eq!(super::attitude_reading(state).reference, None);
        state.present = osg_ship_api::abi::ATTITUDE_REFERENCE;
        state.reference = [0., 0., 0., 1.];
        assert_eq!(
            super::attitude_reading(state).reference,
            Some(state.reference)
        );
    }

    use super::*;
    use std::sync::Arc;

    #[test]
    fn timed_paths_publish_absolute_positions_without_rounding_the_origin() {
        let mut app = super::super::application(None);
        app.update();
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<super::super::vessel::ControlledVessel>>()
            .single(app.world())
            .unwrap();
        let tick = app.world().resource::<SimulationCounters>().ticks;
        let now = tick as f64 * osg_model::TICK_SECONDS;
        let origin = [20_000_000_000_000_000_000_003i128, 11, -23];
        let mut software = app.world_mut().get_mut::<ShipSoftware>(entity).unwrap();
        software.controller.state.spatial.paths.insert(
            7,
            spatial::Path {
                header: abi::SpatialPath {
                    meta: abi::SpatialMeta {
                        id: 7,
                        valid_until_s: now + 30.0,
                        ..Default::default()
                    },
                    frame: abi::SpatialFrame {
                        kind: abi::FRAME_SNAPSHOT,
                        origin_velocity_m_s: [3.0, 0.0, 0.0],
                        ..Default::default()
                    },
                    kind: abi::PATH_TIMED,
                    subject_contact: 0,
                },
                snapshot: Some(spatial::Snapshot {
                    origin,
                    epoch: now,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    ..Default::default()
                }),
                vertices: Arc::from([
                    abi::SpatialVertex {
                        time_s: now,
                        position_m: [0.0, 0.0, 0.0],
                    },
                    abi::SpatialVertex {
                        time_s: now + 10.0,
                        position_m: [10.0, 0.0, 0.0],
                    },
                ]),
                revision: 17,
                published_at: now,
            },
        );
        let world = app.world();
        let result = instruments(world, entity, world.get::<ShipSoftware>(entity).unwrap());
        let path = &result.paths[0];
        assert_eq!(path.revision, 17);
        assert_eq!(
            path.vertices[0].position,
            GalacticPosition::new(origin[0], 11, -23)
        );
        assert_eq!(
            path.vertices[1].position,
            GalacticPosition::new(origin[0] + 40_000_000, 11, -23)
        );
        assert_eq!(path.vertices[1].sim_time_ns, nanoseconds(now + 10.0));
    }
}
