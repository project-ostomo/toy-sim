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
            cargo_quantity: state.inventory.cargo[index],
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
                        _ => 0.0,
                    };
                    DeviceReading::Battery {
                        energy_j: state.inventory.energy_j * capacity_j / design.battery_j.max(1.0),
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
    let computer = if world.get::<super::travel::Dormant>(entity).is_some() {
        ComputerStatus::Paused
    } else if !state.computer_running(design) {
        ComputerStatus::Unpowered
    } else if let Some(fault) = &software.controller.fault {
        ComputerStatus::Fault(bounded(fault, 4096))
    } else if software.controller.is_booting() {
        ComputerStatus::Booting {
            progress: software.controller.boot_progress(),
        }
    } else {
        ComputerStatus::Running {
            gas_used: software.last_gas_used,
            gas_limit: software.last_gas_limit,
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
    Some(ShipPresentation {
        cargo_capacity_m3: design.capacity_m3,
        cargo_used_m3: state.inventory.cargo_volume(catalogue),
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
        power_consumed_w: power.map_or(0.0, |power| power.supplied_w),
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
            mode: instrument.mode,
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

pub fn visual(world: &World, entity: Entity, contact: ContactRef) -> Option<TrackVisual> {
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
    Some(TrackVisual {
        contact,
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
