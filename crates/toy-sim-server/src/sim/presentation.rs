use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use toy_sim_model::{GalacticPosition, presentation::*};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::spatial;
use toy_sim_ships::{DeviceKind, DeviceReading as Reading, DeviceSetting};

use super::{
    hardware,
    identity::{Control, Identity},
    physics::{MassProps, Velocity},
    precision::PreciseTransform,
    simulation::SimulationCounters,
    vessel::{ShipCatalogue, ShipDesign, ShipSoftware},
};

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

pub fn ship(world: &World, entity: Entity, include_instruments: bool) -> Option<ShipPresentation> {
    let id = world.get::<Identity>(entity)?.0;
    let design = &world.get::<ShipDesign>(entity)?.0;
    let state = hardware::snapshot(world, entity)?;
    let software = world.get::<ShipSoftware>(entity)?;
    let catalogue = &world.resource::<ShipCatalogue>().0;
    let mass = world.get::<MassProps>(entity)?;
    let tick = world.resource::<SimulationCounters>().ticks;
    let inventory: Vec<_> = catalogue
        .resources
        .iter()
        .zip(&state.inventory.quantities)
        .enumerate()
        .map(|(index, (resource, quantity))| ResourceAmount {
            name: bounded(&resource.title, 128),
            unit_mass_kg: resource.mass_kg,
            unit_volume_m3: resource.volume_m3,
            resource: resource.id.clone(),
            quantity: *quantity,
            amount_kg: *quantity as f64 * resource.mass_kg,
            capacity_kg: if resource.volume_m3 > 0.0 {
                state.inventory.tank_capacities_m3[index] / resource.volume_m3 * resource.mass_kg
            } else {
                0.0
            },
        })
        .collect();
    let devices = design
        .device_catalogue
        .iter()
        .zip(state.snapshot(design))
        .map(|(descriptor, status)| {
            let part_index = design.part_for_device(descriptor.handle);
            let part = part_index.map(|index| &design.parts[index]);
            let setting = state
                .settings
                .get(descriptor.handle.0 as usize)
                .and_then(Option::as_ref);
            let reading = match status.reading {
                Reading::Engine { thrust_n } => DeviceReading::Engine {
                    throttle: match descriptor.kind {
                        DeviceKind::Engine {
                            thrust_n: maximum, ..
                        } => (thrust_n / maximum).clamp(0.0, 1.0),
                        _ => 0.0,
                    },
                    thrust_n,
                },
                Reading::Rcs { thrust_n } => DeviceReading::Rcs { thrust_n },
                Reading::Accelerometer { .. } => DeviceReading::Accelerometer {
                    acceleration_m_s2: (status.operational && status.powered)
                        .then(|| world.get::<super::physics::AccelerometerState>(entity))
                        .flatten()
                        .and_then(|accelerometer| {
                            accelerometer.at_mount(
                                DVec3::from_array(descriptor.position_m),
                                DQuat::from_array(descriptor.rotation),
                            )
                        })
                        .map(|sample| sample.acceleration_m_s2),
                },
                Reading::Torquer { torque_nm } => {
                    let requested = match setting {
                        Some(DeviceSetting::TorqueNm(torque)) => DVec3::from_array(*torque),
                        _ => DVec3::ZERO,
                    };
                    DeviceReading::Torquer {
                        torque_nm: (requested.normalize_or_zero() * torque_nm).to_array(),
                    }
                }
                Reading::Generator { power_w } => DeviceReading::Generator { output_w: power_w },
                Reading::Battery => {
                    let capacity_j = match descriptor.kind {
                        DeviceKind::Battery { capacity_j } => capacity_j,
                        _ => 0,
                    };
                    DeviceReading::Battery {
                        energy_j: ((state.inventory.energy_j as u128 * capacity_j as u128)
                            / design.battery_j.max(1) as u128)
                            as u64,
                        capacity_j,
                    }
                }
                Reading::Shield {
                    temperature_k,
                    reserve_kg,
                    strength,
                    ablation_kg_s,
                    ..
                } => DeviceReading::Shield {
                    temperature_k,
                    area_m2: match descriptor.kind {
                        DeviceKind::Shield {
                            radiator_area_m2, ..
                        } => radiator_area_m2 * strength,
                        _ => 0.0,
                    },
                    reserve_kg,
                    feed_kg_s: if reserve_kg > 0.0 {
                        design.shield_feed_kg_s.min(ablation_kg_s)
                    } else {
                        0.0
                    },
                    ablation_kg_s,
                },
                Reading::Weapon(reading) => {
                    let spec = part_index
                        .and_then(|index| design.part_weapons[index])
                        .map(|index| &design.weapon_specs[index]);
                    let now = tick as f64 * 0.1;
                    let interval = spec.map_or(0.0, |spec| spec.cycle_interval_s);
                    let last_fire = reading.next_fire_s - interval;
                    DeviceReading::Weapon {
                        yaw_rad: reading.yaw_rad,
                        pitch_rad: reading.pitch_rad,
                        loaded: reading.ammunition_units > 0 && reading.next_fire_s <= now,
                        firing: reading.shots_fired > 0 && last_fire >= now - 0.1,
                        progress: if interval > 0.0 {
                            (1.0 - (reading.next_fire_s - now) / interval).clamp(0.0, 1.0)
                        } else {
                            1.0
                        },
                    }
                }
                Reading::Sensor { range_m } => DeviceReading::Sensor { range_m },
                Reading::Storage => DeviceReading::Storage {
                    contents: inventory.clone(),
                },
                Reading::Computer => DeviceReading::Avionics,
            };
            let power = part_index
                .and_then(|index| world.get::<hardware::PartDevices>(entity)?.0.get(index))
                .and_then(|part| world.get::<hardware::DevicePower>(*part));
            DeviceTelemetry {
                part: descriptor.part_id,
                name: bounded(
                    &if descriptor.alias.is_empty() {
                        part.map_or_else(
                            || format!("{:?}", descriptor.kind),
                            |part| part.definition.title.clone(),
                        )
                    } else {
                        descriptor.alias.clone()
                    },
                    128,
                ),
                enabled: status.operational && status.powered,
                power_requested_w: power.map_or(0.0, |power| power.requested_w),
                power_delivered_w: power.map_or(0.0, |power| power.supplied_w),
                reading,
            }
        })
        .collect();
    let reboot_remaining_s =
        software.controller.boot_remaining_gas() as f64 / software.last_gas_limit as f64 * 0.1;
    let suspended = world
        .get::<super::travel::SystemsSuspended>(entity)
        .is_some();
    let shared_computer = world
        .get_resource::<super::missiles::Callbacks>()
        .and_then(|callbacks| callbacks.0.get(&entity))
        .is_some_and(|callbacks| !callbacks.is_empty());
    let computer_powered = (!suspended && state.computer_running(design)) || shared_computer;
    let computer = if suspended && !shared_computer {
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
            gas_used: software.last_gas_used,
            gas_limit: software.last_gas_limit,
            execution: match software.controller.execution_status() {
                toy_sim_model::ExecutionStatus::WaitingForGas if software.display_limited => {
                    toy_sim_model::ExecutionStatus::Suspended
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
        let minimum = (preparation.started + 100).saturating_sub(tick) as f64 * 0.1;
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
        serial: software.controller.state.serial.screen.clone(),
        memory_limit_bytes: toy_sim_ship_wasm::MEMORY_LIMIT as u64,
        cargo: state
            .inventory
            .cargo_stacks(catalogue)
            .expect("valid ship cargo"),
        cargo_capacity_m3: design.capacity_m3,
        cargo_used_m3: state.inventory.cargo_volume(catalogue),
        propulsion: {
            let mut reading = world
                .get::<hardware::propulsion::InstalledRatings>(entity)?
                .0
                .clone();
            if world.get::<super::travel::Dormant>(entity).is_none() && state.hull > 0. {
                let output = world.get::<hardware::propulsion::ActuatorOutput>(entity)?;
                reading.force_n = output.force.to_array();
                reading.torque_nm = output.torque.to_array();
            }
            reading.drives = hardware::propulsion::reserves(design, mass.mass, &inventory);
            reading
        },
        ship: id,
        revision: world.get::<Control>(entity)?.revision,
        sim_time_ns: tick.saturating_mul(100_000_000),
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
            hull_hp: state.hull,
            hull_max_hp: design.hull,
            shield_reserve_capacity_kg: design.shield_reserve_capacity_kg,
            shield_strength: state.shield_strength(design),
        }),
        execution: Some(ExecutionMetrics {
            memory_bytes: software.controller.memory_bytes() as u64,
            step_us: software.last_seconds * 1e6,
            prepare_us: software.timings.prepare * 1e6,
            callback_us: software.timings.callback * 1e6,
            publish_us: software.timings.publish * 1e6,
            hardware_us: software.timings.hardware * 1e6,
            scan_us: software.timings.scan * 1e6,
        }),
        mass_kg: mass.mass,
        inertia_kg_m2: mass.inertia.to_cols_array(),
        control_rotation: DQuat::from_mat3(&toy_sim_ships::orientation(
            design.blueprint.avionics.control_orientation,
        ))
        .to_array(),
        hull_heat_capacity_j: design.hull_heat_capacity_j,
        battery_capacity_j: design.battery_j,
        power_generated_w: power.map_or(0.0, |power| power.generated_w),
        generation_capacity_w: design
            .device_catalogue
            .iter()
            .zip(state.snapshot(design))
            .filter_map(|(descriptor, status)| match descriptor.kind {
                DeviceKind::Generator { power_w } if status.operational => {
                    let reactor = design
                        .part_for_device(descriptor.handle)
                        .and_then(|index| world.get::<hardware::PartDevices>(entity)?.0.get(index))
                        .and_then(|part| world.get::<hardware::reactors::Reactor>(*part));
                    Some(reactor.map_or(power_w, |reactor| {
                        let sink = hardware::reactors::sink_temperature(&state.thermal, design);
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
                    .and_then(|handle| state.settings.get(handle))
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
                    coolant_temperature_k: hardware::reactors::sink_temperature(
                        &state.thermal,
                        design,
                    ),
                    operating_temperature_k: reactor.spec.hot_temperature_k,
                    shutdown_temperature_k: reactor.spec.shutdown_temperature_k,
                })
            })
            .collect(),
        slip_cooldown_s: drive.map(|drive| drive.ready_tick.saturating_sub(tick) as f64 * 0.1),
        power_consumed_w: power.map_or(0.0, |power| power.supplied_w) + slip_input,
        power_requested_w: power.map_or(0., |power| power.requested_w)
            + if preparation.is_some() {
                drive.map_or(0., |drive| drive.power_w)
            } else {
                slip_input
            },
        slip_charge,
        inventory,
        devices,
        computer,
        instruments: include_instruments.then(|| instruments(world, entity, software)),
        screens,
    })
}

fn instruments(world: &World, ship: Entity, software: &ShipSoftware) -> Instruments {
    let tick = world.resource::<SimulationCounters>().ticks;
    let now = tick as f64 * 0.1;
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
        .unwrap_or(tick * 100_000_000);
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

pub fn visual(world: &World, entity: Entity) -> Option<ShipVisual> {
    let design = &world.get::<ShipDesign>(entity)?.0;
    let state = hardware::snapshot(world, entity)?;
    let mut engines = Vec::new();
    let mut turrets = Vec::new();
    for (descriptor, status) in design.device_catalogue.iter().zip(state.snapshot(design)) {
        match (descriptor.kind.clone(), status.reading) {
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
        engines,
        turrets,
        shield: state.shield_active().then(|| ShieldVisual {
            temperature_k: state.shield_temperature(design),
            coverage: state.shield_strength(design),
        }),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn manual_flight_keeps_device_telemetry_valid_after_sustained_power_use() {
        let account = toy_sim_model::Id::new();
        let mut app = super::super::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let entity = world
            .query_filtered::<Entity, With<super::super::vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
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
                    .get_mut::<ShipSoftware>(entity)
                    .unwrap()
                    .command(toy_sim_ship_wasm::Command::Manual {
                        throttle: if phase == 2 { 0.0 } else { 0.8 },
                        steering,
                    });
            }
            app.update();
            let frame = super::super::session::frame(app.world_mut(), session).unwrap();
            assert_eq!(frame.presentation.ships.len(), 1);
            super::super::session::prune_events(app.world_mut());
            toy_sim_protocol::validate_frame(&frame)
                .unwrap_or_else(|error| panic!("tick {step}: {error:#}"));
            super::super::session::input(
                app.world_mut(),
                session,
                toy_sim_model::InputFrame {
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
        let mut state = toy_sim_ship_api::abi::AttitudeState::default();
        assert_eq!(super::attitude_reading(state).reference, None);
        state.present = toy_sim_ship_api::abi::ATTITUDE_REFERENCE;
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
        let now = tick as f64 * 0.1;
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
