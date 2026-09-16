use glam::{DMat3, DVec3};
use toy_sim_ships::*;
fn fixture() -> (Catalogue, CompiledShipDesign, ShipState) {
    let c = Catalogue::builtin();
    let d = starter(EXAMPLE_CONTROLLER.to_vec()).compile(&c).unwrap();
    let mut s = ShipState::new(&d, &c);
    s.test_loadout(&d, &c);
    (c, d, s)
}
#[test]
fn standard_avionics_are_automatic_and_have_mass_power_and_logical_devices() {
    let (cat, design, mut state) = fixture();
    assert!(matches!(
        ShipBlueprint::default().firmware,
        Firmware::Standard
    ));
    assert_eq!(design.parts.len(), 8);
    let physical: f64 = design.parts.iter().map(|p| p.definition.mass_kg).sum();
    assert_eq!(design.dry_mass, physical + 71.);
    for h in design.avionics_handles {
        assert!(design.part_for_device(h).is_none());
        assert_eq!(design.device_catalogue[h.0 as usize].part_id, 0);
    }
    state.inventory.quantities[1] = 0.;
    let before = state.inventory.energy_j;
    state.step(&design, &cat, 0.1);
    // The enabled shield now draws 1 MW continuously, including while full.
    assert!((before - state.inventory.energy_j - 100_110.1).abs() < 1e-6);
    state
        .apply_commands(
            &design,
            &[DeviceCommand {
                device: design.avionics_handles[2],
                setting: DeviceSetting::SensorEnabled(false),
            }],
        )
        .unwrap();
    let before = state.inventory.energy_j;
    state.step(&design, &cat, 0.1);
    assert!((before - state.inventory.energy_j - 100_010.1).abs() < 1e-6);
    assert_eq!(state.sensor_range, 0.);
    state.avionics.operational = false;
    state.step(&design, &cat, 0.1);
    assert!(!state.computer_running(&design));
    assert!(
        state
            .snapshot(&design)
            .iter()
            .zip(&design.device_sources)
            .all(|(s, source)| !matches!(source, DeviceSource::Avionics) || !s.operational)
    );
}
#[test]
fn old_designs_are_rejected_and_control_configuration_roundtrips() {
    let (cat, d, _) = fixture();
    let mut ship = d.blueprint;
    ship.avionics.control_orientation = 7;
    ship.avionics.excluded_actuators.push(7);
    ship.avionics.sensor_enabled = false;
    let loaded = ShipBlueprint::from_bytes(&ship.to_bytes().unwrap()).unwrap();
    let d = loaded.compile(&cat).unwrap();
    assert!(
        !d.device_catalogue
            .iter()
            .find(|d| d.part_id == 7)
            .unwrap()
            .control_enabled
    );
    assert_eq!(loaded.avionics, ship.avionics);
    ship.format_version = 1;
    assert!(
        ShipBlueprint::from_bytes(&ship.to_bytes().unwrap())
            .unwrap_err()
            .to_string()
            .contains("incompatible ship format")
    );
}
#[test]
fn rotations_are_the_24_unique_proper_cube_rotations() {
    let mut rotations = vec![];
    for i in 0..24 {
        let r = orientation(i);
        assert_eq!(r.determinant(), 1.);
        assert_eq!(r * r.transpose(), DMat3::IDENTITY);
        assert!(!rotations.contains(&r));
        rotations.push(r);
    }
    assert_eq!(orientation(0), DMat3::IDENTITY);
}
#[test]
fn ship_cbor_roundtrip_and_validation() {
    let s = starter(EXAMPLE_CONTROLLER.to_vec());
    let bytes = s.to_bytes().unwrap();
    let decoded = ShipBlueprint::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.to_bytes().unwrap(), bytes);
    decoded.compile(&Catalogue::builtin()).unwrap();
    for end in [0, 8, 15, bytes.len() - 1] {
        assert!(ShipBlueprint::from_bytes(&bytes[..end]).is_err());
    }
    assert!(ShipBlueprint::from_bytes(b"TOYSHIP\0").is_err());
    let mut invalid = bytes;
    invalid.push(0);
    assert!(ShipBlueprint::from_bytes(&invalid).is_err());
}
#[test]
fn geometry_rejects_overlap_disconnection_and_unknown_parts() {
    let c = Catalogue::builtin();
    let s = starter(EXAMPLE_CONTROLLER.to_vec());
    let mut bad = s.clone();
    bad.parts[1].position = bad.parts[0].position;
    assert!(bad.compile(&c).unwrap_err().to_string().contains("overlap"));
    let mut bad = s.clone();
    bad.parts[6].position[0] += 100;
    assert!(
        bad.compile(&c)
            .unwrap_err()
            .to_string()
            .contains("connected")
    );
    let mut bad = s;
    bad.parts[0].prototype = "untrusted-super-engine".into();
    assert!(bad.compile(&c).is_err());
}
#[test]
fn inventory_uses_fractional_si_quantities_and_atomic_capacity_checks() {
    let c = Catalogue::builtin();
    let mut a = Inventory::empty(&c);
    let mut b = Inventory::empty(&c);
    a.insert(0, 1.25, 1., &c).unwrap();
    assert_eq!(a.mass(&c), 1.25);
    assert_eq!(a.volume(&c), 0.00125);
    a.transfer(&mut b, 0, 0.125, 1., &c).unwrap();
    assert_eq!(a.quantities[0], 1.125);
    assert_eq!(b.quantities[0], 0.125);
    assert!(a.transfer(&mut b, 0, 1., 0.0005, &c).is_err());
    assert_eq!(a.quantities[0], 1.125);
    assert_eq!(b.quantities[0], 0.125);
    for n in [-1., f64::INFINITY, f64::NAN] {
        assert!(a.insert(0, n, 1., &c).is_err());
    }
}
#[test]
fn last_fraction_of_propellant_scales_thrust_and_energy_together() {
    let (c, d, mut s) = fixture();
    s.inventory.quantities[0] = 0.05;
    let energy = s.inventory.energy_j;
    s.apply_commands(
        &d,
        &[DeviceCommand {
            device: d
                .device_catalogue
                .iter()
                .find(|device| device.alias == "main_engine")
                .unwrap()
                .handle,
            setting: DeviceSetting::Throttle(1.),
        }],
    )
    .unwrap();
    let out = s.step(&d, &c, 0.1);
    assert!((out.force.z + 100000.).abs() < 1e-8);
    assert!(out.torque.length() < 1e-8);
    assert_eq!(s.inventory.quantities[0], 0.);
    assert!((energy - s.inventory.energy_j - 10_100_110.1).abs() < 1e-6);
    assert_eq!(s.sensor_range, 100_000_000.);
    let (mass, inertia) = s.mass_properties(&d, &c);
    assert!((inertia - d.inertia * (mass / d.dry_mass)).abs_diff_eq(DMat3::ZERO, 1e-8));
}
#[test]
fn off_axis_engine_generates_lever_arm_torque() {
    let c = Catalogue::builtin();
    let mut b = starter(EXAMPLE_CONTROLLER.to_vec());
    b.parts[6].position = [10, 0, 50];
    let d = b.compile(&c).unwrap();
    let mut s = ShipState::new(&d, &c);
    s.test_loadout(&d, &c);
    s.apply_commands(
        &d,
        &[DeviceCommand {
            device: d
                .device_catalogue
                .iter()
                .find(|device| device.alias == "main_engine")
                .unwrap()
                .handle,
            setting: DeviceSetting::Throttle(1.),
        }],
    )
    .unwrap();
    let out = s.step(&d, &c, 0.1);
    assert!(out.torque.y.abs() > 1000.);
    assert_eq!(out.torque, (d.parts[6].centre - d.centre).cross(out.force));
}
#[test]
fn computer_power_loss_and_failure_neutralize_outputs() {
    for failed in [false, true] {
        let (c, d, mut s) = fixture();
        s.inventory.energy_j = if failed { d.battery_j } else { 5. };
        s.inventory.quantities[1] = 0.;
        s.avionics.operational = !failed;
        let engine = d
            .device_catalogue
            .iter()
            .find(|device| device.alias == "main_engine")
            .unwrap()
            .handle;
        s.apply_commands(
            &d,
            &[DeviceCommand {
                device: engine,
                setting: DeviceSetting::Throttle(1.),
            }],
        )
        .unwrap();
        let out = s.step(&d, &c, 0.1);
        assert_eq!(out.force, DVec3::ZERO);
        assert!(!s.computer_running(&d));
        assert_eq!(
            s.settings[engine.0 as usize],
            Some(DeviceSetting::Throttle(0.))
        );
    }
}
#[test]
fn passive_parts_are_absent_from_tick_plan_and_priority_is_stable() {
    let (c, d, _) = fixture();
    assert_eq!(d.active_parts.len(), 4);
    let mut b = d.blueprint.clone();
    b.parts.reverse();
    let other = b.compile(&c).unwrap();
    assert_eq!(
        d.active_parts
            .iter()
            .map(|&i| d.parts[i].placed.id)
            .collect::<Vec<_>>(),
        other
            .active_parts
            .iter()
            .map(|&i| other.parts[i].placed.id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn device_metadata_roundtrips_without_changing_execution() {
    let mut ship = starter(EXAMPLE_CONTROLLER.to_vec());
    ship.parts[1].name = "Navigation computer".into();
    ship.parts[1].alias = "navigation".into();
    ship.parts[1].groups = vec!["avionics".into(), "essential".into()];
    let bytes = ship.to_bytes().unwrap();
    let decoded = ShipBlueprint::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.to_bytes().unwrap(), bytes);
    decoded.compile(&Catalogue::builtin()).unwrap();
    let value: ciborium::Value = ciborium::from_reader(bytes.as_slice()).unwrap();
    let ciborium::Value::Map(mut fields) = value else {
        panic!("expected named map")
    };
    assert!(
        fields
            .iter()
            .any(|(key, value)| key.as_text() == Some("firmware") && value.as_map().is_some())
    );
    fields.push((
        ciborium::Value::Text("future_metadata".into()),
        ciborium::Value::Bool(true),
    ));
    let mut extended = Vec::new();
    ciborium::into_writer(&ciborium::Value::Map(fields), &mut extended).unwrap();
    assert_eq!(
        ShipBlueprint::from_bytes(&extended)
            .unwrap()
            .to_bytes()
            .unwrap(),
        bytes
    );
}

#[test]
fn device_handles_are_dense_and_commands_are_checked_atomically() {
    let (c, d, mut s) = fixture();
    let directory = &d.device_catalogue;
    assert_eq!(directory.len(), d.parts.len() + 1);
    for (i, descriptor) in directory.iter().enumerate() {
        assert_eq!(descriptor.handle.0 as usize, i);
    }
    let engine = directory
        .iter()
        .find(|device| device.alias == "main_engine")
        .unwrap()
        .handle;
    let radar = directory
        .iter()
        .find(|device| device.alias == "radar")
        .unwrap()
        .handle;
    let before = s.settings.clone();
    for bad in [
        DeviceCommand {
            device: radar,
            setting: DeviceSetting::Throttle(1.),
        },
        DeviceCommand {
            device: DeviceHandle(u16::MAX),
            setting: DeviceSetting::Throttle(1.),
        },
        DeviceCommand {
            device: engine,
            setting: DeviceSetting::Throttle(f64::NAN),
        },
    ] {
        let first = DeviceCommand {
            device: engine,
            setting: DeviceSetting::Throttle(0.5),
        };
        assert!(s.apply_commands(&d, &[first, bad]).is_err());
        assert_eq!(s.settings, before);
    }
    s.apply_commands(
        &d,
        &[DeviceCommand {
            device: engine,
            setting: DeviceSetting::Throttle(0.5),
        }],
    )
    .unwrap();
    assert!(s.step(&d, &c, 0.1).force.z < -9000.);
    // No new commands means hardware keeps the setting.
    assert!(s.step(&d, &c, 0.1).force.z < -9000.);
    let part = d.part_for_device(engine).unwrap();
    s.devices[part].operational = false;
    assert_eq!(s.step(&d, &c, 0.1).force, DVec3::ZERO);
    assert!(!s.snapshot(&d).get(engine.0 as usize).unwrap().operational);
}

#[test]
fn device_aliases_are_unique_and_groups_resolve_to_handles() {
    let (c, d, _) = fixture();
    let mut ship = d.blueprint;
    ship.parts[4].groups = vec!["propulsion".into()];
    ship.parts[6].groups = vec!["propulsion".into()];
    let compiled = ship.compile(&c).unwrap();
    let directory = compiled.device_catalogue;
    assert_eq!(
        directory
            .iter()
            .filter(|device| device.groups.iter().any(|group| group == "propulsion"))
            .count(),
        2
    );
    ship.parts[4].alias = ship.parts[6].alias.clone();
    assert!(
        ship.compile(&c)
            .unwrap_err()
            .to_string()
            .contains("duplicate device alias")
    );
}

#[test]
fn sensor_reading_tracks_power_and_enable_setting() {
    let (catalogue, design, mut state) = fixture();
    let radar = design.avionics_handles[2];
    state.step(&design, &catalogue, 0.1);
    assert!(matches!(state.snapshot(&design)[radar.0 as usize].reading,
        DeviceReading::Sensor { range_m } if range_m > 0.));

    state
        .apply_commands(
            &design,
            &[DeviceCommand {
                device: radar,
                setting: DeviceSetting::SensorEnabled(false),
            }],
        )
        .unwrap();
    state.step(&design, &catalogue, 0.1);
    assert_eq!(state.sensor_range, 0.);

    state.avionics.operational = false;
    assert!(!state.snapshot(&design)[radar.0 as usize].operational);
}

#[test]
fn rcs_scales_all_axes_when_fuel_runs_out_and_applies_mount_torque() {
    let catalogue = Catalogue::builtin();
    let design = armed_starter().compile(&catalogue).unwrap();
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    state.inventory.quantities[0] = 0.05;
    let device = design
        .device_catalogue
        .iter()
        .find(|device| matches!(device.kind, DeviceKind::Rcs { .. }))
        .unwrap();
    state
        .apply_commands(
            &design,
            &[DeviceCommand {
                device: device.handle,
                setting: DeviceSetting::RcsThrust([2000.0, -2000.0, 0.0]),
            }],
        )
        .unwrap();

    let part = &design.parts[design.part_for_device(device.handle).unwrap()];
    let expected = part.rotation * DVec3::new(1000.0, -1000.0, 0.0);
    let output = state.step(&design, &catalogue, 0.1);
    assert!(output.force.distance(expected) < 1e-8);
    assert!(
        output
            .torque
            .distance((part.centre - design.centre).cross(expected))
            < 1e-8
    );
    assert_eq!(state.inventory.quantities[0], 0.0);

    let output = state.step(&design, &catalogue, 0.1);
    assert_eq!(output.force, DVec3::ZERO);
    assert_eq!(output.torque, DVec3::ZERO);
}

#[test]
fn every_shield_generator_must_be_powered_for_the_combined_field() {
    let cat = Catalogue::builtin();
    let mut ship = starter(EXAMPLE_CONTROLLER.to_vec());
    let mut second = ship
        .parts
        .iter()
        .find(|p| p.prototype == "shield")
        .unwrap()
        .clone();
    second.id = 15;
    second.alias = "second_shield".into();
    second.position = [0, 0, -20];
    ship.parts.push(second);
    let d = ship.compile(&cat).unwrap();
    let mut state = ShipState::new(&d, &cat);
    state.test_loadout(&d, &cat);
    state.step(&d, &cat, 0.1);
    assert!(state.thermal.shield_powered);

    let second_index = d.parts.iter().position(|p| p.placed.id == 15).unwrap();
    state.devices[second_index].operational = false;
    state.step(&d, &cat, 0.1);
    assert!(!state.thermal.shield_powered);
    state.shield_activation(&d, true);
    assert!(!state.shield_active());
}

#[test]
fn weapon_energy_reading_recovers_when_the_shared_battery_recharges() {
    use toy_sim_ship_api::abi;

    let catalogue = Catalogue::builtin();
    let design = armed_starter().compile(&catalogue).unwrap();
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    state.weapons[0].inhibit_flags = abi::WEAPON_ENERGY;

    let device = design.part_devices[design.weapon_parts[0]].unwrap();
    for (battery, expected) in [(0.0, abi::WEAPON_ENERGY), (design.battery_j, 0)] {
        state.inventory.energy_j = battery;
        let DeviceReading::Weapon(reading) = state.snapshot(&design)[device].reading else {
            panic!("expected weapon reading");
        };
        assert_eq!(reading.inhibit_flags & abi::WEAPON_ENERGY, expected);
    }
}
