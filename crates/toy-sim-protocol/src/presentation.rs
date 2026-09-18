use super::{pose_valid, position_valid};
use anyhow::{Result, ensure};
use toy_sim_model::*;

fn nonnegative(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite() && *v >= 0.)
}

fn finite(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite())
}

fn rotation(q: &[f64; 4]) -> bool {
    finite(q) && (q.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 1e-5
}

fn attitude_valid(attitude: &AttitudeInstrument) -> bool {
    attitude.reference.as_ref().is_none_or(rotation) && nonnegative(&[attitude.control_error_rad])
}

fn resources(values: &[ResourceAmount]) -> bool {
    values.len() <= 256
        && values.iter().all(|r| {
            r.name.len() <= 128
                && r.resource.len() <= 128
                && nonnegative(&[r.amount_kg, r.capacity_kg, r.unit_mass_kg, r.unit_volume_m3])
        })
}

pub fn validate(p: &PresentationFrame, watermark: u64) -> Result<()> {
    ensure!(
        p.ships.len() <= 64
            && p.visuals.len() <= 8192
            && p.combat.len() <= 16384
            && p.celestial_systems.len() <= 256
            && p.capabilities.len() <= 7,
        "presentation limit"
    );
    ensure!(
        p.navigation.systems.len() <= 65536 && p.navigation.beacons.len() <= 65536,
        "navigation catalogue limit"
    );
    let mut systems = std::collections::BTreeSet::new();
    for system in &p.navigation.systems {
        ensure!(
            systems.insert(system.id)
                && system.name.len() <= 128
                && position_valid(system.position),
            "invalid navigation system"
        );
    }
    let mut beacons = std::collections::BTreeSet::new();
    for beacon in &p.navigation.beacons {
        ensure!(
            beacons.insert(beacon.id)
                && systems.contains(&beacon.system)
                && beacon.name.len() <= 128
                && pose_valid(&beacon.pose)
                && nonnegative(&[beacon.radius_m]),
            "invalid navigation beacon"
        );
    }
    for beacon in &p.navigation.beacons {
        ensure!(
            beacon
                .gate_exit
                .is_none_or(|exit| exit != beacon.id && beacons.contains(&exit)),
            "invalid gate endpoint"
        );
    }
    for ship in &p.ships {
        ensure!(
            nonnegative(&[
                ship.mass_kg,
                ship.hull_heat_capacity_j,
                ship.battery_capacity_j,
                ship.power_generated_w,
                ship.power_consumed_w,
                ship.cargo_capacity_m3,
                ship.cargo_used_m3
            ]) && finite(&ship.inertia_kg_m2)
                && rotation(&ship.control_rotation),
            "invalid hardware totals"
        );
        ensure!(
            resources(&ship.inventory) && ship.devices.len() <= 4096 && ship.screens.len() <= 8,
            "hardware limit"
        );
        match &ship.computer {
            ComputerStatus::Fault(error) => ensure!(error.len() <= 4096, "computer fault limit"),
            ComputerStatus::Booting { progress } => ensure!(
                progress.is_finite() && (0. ..=1.).contains(progress),
                "invalid boot progress"
            ),
            _ => {}
        }
        for screen in &ship.screens {
            ensure!(
                screen.slot < 8
                    && (1..=4096).contains(&screen.width)
                    && (1..=4096).contains(&screen.height)
                    && screen.title.len() <= 64,
                "invalid screen definition"
            );
        }
        if let Some(e) = &ship.environment {
            ensure!(
                e.altitude_m.is_finite()
                    && finite(&e.airspeed_m_s)
                    && nonnegative(&[e.density_kg_m3, e.pressure_pa]),
                "invalid flight environment"
            );
        }
        if let Some(h) = &ship.health {
            ensure!(
                nonnegative(&[
                    h.hull_hp,
                    h.hull_max_hp,
                    h.shield_reserve_capacity_kg,
                    h.shield_strength
                ]) && h.shield_strength <= 1.,
                "invalid ship health"
            );
        }
        if let Some(e) = &ship.execution {
            ensure!(
                nonnegative(&[
                    e.step_us,
                    e.prepare_us,
                    e.callback_us,
                    e.publish_us,
                    e.hardware_us,
                    e.scan_us
                ]),
                "invalid execution metrics"
            );
        }
        for device in &ship.devices {
            ensure!(
                device.name.len() <= 128
                    && nonnegative(&[device.power_requested_w, device.power_delivered_w]),
                "invalid device telemetry"
            );
            let valid = match &device.reading {
                DeviceReading::Rcs { thrust_n } => finite(thrust_n),
                DeviceReading::Accelerometer { acceleration_m_s2 } => {
                    acceleration_m_s2.as_ref().is_none_or(|v| finite(v))
                }
                DeviceReading::Engine { throttle, thrust_n } => {
                    nonnegative(&[*throttle, *thrust_n]) && *throttle <= 1.
                }
                DeviceReading::Torquer { torque_nm } => finite(torque_nm),
                DeviceReading::Generator { output_w } => nonnegative(&[*output_w]),
                DeviceReading::Battery {
                    energy_j,
                    capacity_j,
                } => nonnegative(&[*energy_j, *capacity_j]),
                DeviceReading::Shield {
                    temperature_k,
                    area_m2,
                    reserve_kg,
                    feed_kg_s,
                    ablation_kg_s,
                } => nonnegative(&[
                    *temperature_k,
                    *area_m2,
                    *reserve_kg,
                    *feed_kg_s,
                    *ablation_kg_s,
                ]),
                DeviceReading::Weapon {
                    yaw_rad,
                    pitch_rad,
                    progress,
                    ..
                } => finite(&[*yaw_rad, *pitch_rad, *progress]) && (0. ..=1.).contains(progress),
                DeviceReading::Sensor { range_m } => nonnegative(&[*range_m]),
                DeviceReading::Storage { contents } => resources(contents),
                DeviceReading::Avionics | DeviceReading::Structure => true,
            };
            ensure!(valid, "invalid device reading");
        }
        if let Some(i) = &ship.instruments {
            ensure!(
                i.weapons.len() <= 4096 && i.paths.len() <= 64 && i.markers.len() <= 256,
                "instrument limit"
            );
            if let Some(a) = &i.attitude {
                ensure!(attitude_valid(a), "invalid attitude instrument");
            }
            if let Some(n) = &i.navigation {
                ensure!(
                    nonnegative(&[
                        n.throttle,
                        n.throttle_limit,
                        n.stand_off_m,
                        n.approach_speed_limit_m_s,
                        n.braking_distance_m
                    ]) && n.throttle <= 1.
                        && n.throttle_limit <= 1.
                        && n.predicted_fuel_kg.is_none_or(|v| nonnegative(&[v]))
                        && n.reason.len() <= 1024,
                    "invalid navigation instrument"
                );
            }
            if let Some(w) = &i.weapons_state {
                ensure!(w.reason.len() <= 1024, "weapon reason limit");
            }
            for w in &i.weapons {
                ensure!(
                    nonnegative(&[
                        w.ammunition_units,
                        w.battery_energy_j,
                        w.shot_energy_j,
                        w.pointing_error_rad
                    ]),
                    "invalid weapon resources"
                );
                ensure!(
                    finite(&w.aim_direction) && nonnegative(&[w.flight_time_s]),
                    "invalid weapon instrument"
                );
            }
            let mut vertex_count = 0;
            for path in &i.paths {
                vertex_count += path.vertices.len();
                ensure!(
                    vertex_count <= 16384
                        && path.valid_until_ns >= path.published_at_ns
                        && path.vertices.iter().all(|v| position_valid(v.position)),
                    "invalid trajectory"
                );
                if path.timed {
                    ensure!(
                        path.vertices
                            .windows(2)
                            .all(|v| v[0].sim_time_ns < v[1].sim_time_ns),
                        "unordered timed trajectory"
                    );
                }
            }
            ensure!(
                i.markers
                    .iter()
                    .all(|m| position_valid(m.position) && m.label.len() <= 128),
                "invalid marker"
            );
        }
    }
    for v in &p.visuals {
        ensure!(
            v.engines.len() <= 4096 && v.turrets.len() <= 4096,
            "visual device limit"
        );
        ensure!(
            v.engines.iter().all(|e| finite(&e.thrust_n)
                && e.thrust_fraction.is_finite()
                && (0. ..=1.).contains(&e.thrust_fraction))
                && v.turrets.iter().all(|t| finite(&[t.yaw_rad, t.pitch_rad])),
            "invalid device visual"
        );
        if let Some(s) = &v.shield {
            ensure!(
                nonnegative(&[s.temperature_k, s.coverage]) && s.coverage <= 1.,
                "invalid shield visual"
            );
        }
    }
    for event in &p.combat {
        ensure!(event.sequence <= watermark, "invalid combat sequence");
        let valid = match &event.kind {
            CombatEventKind::Projectile {
                start,
                end,
                end_time_ns,
                radius_m,
                ..
            } => {
                position_valid(*start)
                    && position_valid(*end)
                    && *end_time_ns >= event.sim_time_ns
                    && nonnegative(&[*radius_m])
            }
            CombatEventKind::Fired {
                position, energy_j, ..
            } => position_valid(*position) && nonnegative(&[*energy_j]),
            CombatEventKind::Impact {
                normal,
                position,
                velocity_m_s,
                energy_j,
                ..
            } => {
                position_valid(*position)
                    && finite(normal)
                    && finite(velocity_m_s)
                    && nonnegative(&[*energy_j])
            }
            CombatEventKind::Destroyed {
                pose,
                energy_j,
                mass_kg,
                radius_m,
                ..
            } => pose_valid(pose) && nonnegative(&[*energy_j, *mass_kg, *radius_m]),
        };
        ensure!(valid, "invalid combat event");
    }
    if let Some(u) = &p.universe {
        ensure!(
            u.active_systems.len() <= 8192
                && u.active_systems.iter().all(|s| s.reason.len() <= 256),
            "invalid universe status"
        );
    }
    let mut systems = std::collections::BTreeSet::new();
    for system in &p.celestial_systems {
        ensure!(
            system.epoch_mjd_utc.is_finite() && systems.insert((system.view, system.system)),
            "invalid celestial system reference"
        );
    }
    if let Some(d) = &p.diagnostics {
        ensure!(
            d.collision
                .as_ref()
                .is_none_or(|collision| nonnegative(&[collision.dissipated_j])),
            "invalid collision diagnostics"
        );
        ensure!(
            nonnegative(&[d.tick_duration_ms])
                && d.systems.len() <= 256
                && d.systems
                    .iter()
                    .all(|(name, time)| name.len() <= 128 && nonnegative(&[*time])),
            "invalid diagnostics"
        );
    }
    Ok(())
}

pub fn validate_catalogue(catalogue: &UniverseCatalogue) -> Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    let mut body_count = 0;
    ensure!(catalogue.systems.len() <= 65536, "universe system limit");
    for system in &catalogue.systems {
        ensure!(
            ids.insert(system.id)
                && system.name.len() <= 128
                && position_valid(system.position)
                && nonnegative(&[system.influence_radius_m]),
            "invalid catalogue system"
        );
        body_count += system.bodies.len();
        ensure!(body_count <= 262144, "universe body limit");
        for body in &system.bodies {
            ensure!(
                ids.insert(body.id)
                    && body.name.len() <= 128
                    && body.kind.len() <= 64
                    && nonnegative(&[body.radius_m, body.mass_kg]),
                "invalid catalogue body"
            );
        }
    }
    for system in &catalogue.systems {
        let body_ids: std::collections::BTreeSet<_> = system.bodies.iter().map(|b| b.id).collect();
        for body in &system.bodies {
            ensure!(
                body.parent
                    .is_none_or(|parent| parent != body.id && body_ids.contains(&parent)),
                "invalid catalogue parent"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_attitude_without_reference_roundtrips_and_present_reference_is_validated() {
        let mut attitude = AttitudeInstrument {
            mode: 0,
            reference: None,
            control_error_rad: 0.,
        };
        assert!(attitude_valid(&attitude));
        let bytes = postcard::to_allocvec(&attitude).unwrap();
        let decoded: AttitudeInstrument = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, attitude);
        attitude.reference = Some([0.; 4]);
        assert!(!attitude_valid(&attitude));
        attitude.reference = Some([0., 0., 0., 1.]);
        assert!(attitude_valid(&attitude));
    }
}
