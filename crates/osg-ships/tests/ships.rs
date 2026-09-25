use glam::DMat3;
use osg_ships::*;
fn fixture() -> (Catalogue, CompiledShipDesign, ShipState) {
    let c = Catalogue::builtin();
    let d = starter(EXAMPLE_CONTROLLER.to_vec()).compile(&c).unwrap();
    let mut s = ShipState::new(&d, &c);
    s.test_loadout(&d, &c);
    (c, d, s)
}

#[test]
fn expedition_patrol_has_attitude_authority_for_full_thrust() {
    let design = expedition_patrol().compile(&Catalogue::builtin()).unwrap();
    let mut engine_moment = glam::DVec3::ZERO;
    let mut attitude_authority = glam::DVec3::ZERO;

    for device in &design.device_catalogue {
        let rotation = glam::DQuat::from_array(device.rotation);
        match device.kind {
            DeviceKind::Engine { thrust_n, .. } => {
                let force = rotation * glam::DVec3::NEG_Z * thrust_n;
                engine_moment += glam::DVec3::from_array(device.position_m).cross(force);
            }
            DeviceKind::Torquer { torque_nm, .. } => {
                for axis in glam::DVec3::AXES {
                    attitude_authority += (rotation * axis * torque_nm).abs();
                }
            }
            _ => {}
        }
    }

    // Retain half the attitude authority for steering during a full burn.
    assert!(
        engine_moment.abs().cmple(attitude_authority * 0.5).all(),
        "full-thrust moment {engine_moment:?} exceeds steering reserve from {attitude_authority:?}"
    );
}

#[test]
fn command_batch_rejects_mismatched_settings_before_mutation() {
    let (_, design, state) = fixture();
    let mut settings = state.settings;
    settings.pop();
    let before = settings.clone();
    let engine = design
        .device_catalogue
        .iter()
        .find(|device| matches!(device.kind, DeviceKind::Engine { .. }))
        .unwrap();

    let result = apply_device_commands(
        &design,
        &mut settings,
        &[DeviceCommand {
            device: engine.handle,
            setting: DeviceSetting::Throttle(0.5),
        }],
    );

    assert!(result.is_err());
    assert_eq!(settings, before);
}

#[test]
fn control_configuration_roundtrips() {
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
    bad.parts[1].attachment.as_mut().unwrap().plug = "front".into();
    bad.parts[1].attachment.as_mut().unwrap().socket = "front".into();
    assert!(
        bad.compile(&c)
            .unwrap_err()
            .to_string()
            .contains("occupied")
    );
    let mut bad = s.clone();
    bad.parts[6].attachment.as_mut().unwrap().parent = 999;
    assert!(bad.compile(&c).unwrap_err().to_string().contains("parent"));
    let mut bad = s;
    bad.parts[0].prototype = "untrusted-super-engine".into();
    assert!(bad.compile(&c).is_err());
}
#[test]
fn integer_cargo_transfers_are_atomic_and_cannot_take_consumables() {
    let cat = Catalogue::builtin();
    let mut a = Inventory::empty(&cat);
    let mut b = Inventory::empty(&cat);
    a.quantities[0] = 100;
    a.insert_cargo(0, 10, 1.0, &cat).unwrap();
    a.transfer_cargo(&mut b, 0, 3, 1.0, &cat).unwrap();
    assert_eq!(a.cargo[0], 7);
    assert_eq!(b.cargo[0], 3);
    assert_eq!(a.quantities[0], 100);
    assert!(a.transfer_cargo(&mut b, 0, 8, 1.0, &cat).is_err());
    assert!(a.transfer_cargo(&mut b, 0, 1, 0.003, &cat).is_err());
    assert_eq!(a.cargo[0], 7);
    assert_eq!(b.cargo[0], 3);
    assert!(a.insert_cargo(0, u64::MAX, f64::MAX, &cat).is_err());
}

#[test]
fn fractional_consumption_is_unbiased_and_bounded() {
    let cat = Catalogue::builtin();
    let mut inventory = Inventory::empty(&cat);
    inventory.quantities[0] = 1_000_000;
    let before = inventory.quantities[0];
    for _ in 0..100_000 {
        let previous = inventory.quantities[0];
        let used = inventory.consume(0, 2.4);
        assert_eq!(used, previous - inventory.quantities[0]);
        assert!((2..=3).contains(&(previous - inventory.quantities[0])));
    }
    let used = before - inventory.quantities[0];
    assert!((used as f64 - 240_000.0).abs() < 1500.0);
    inventory.quantities[0] = 1;
    assert_eq!(inventory.consume(0, 2.4), 1);
    assert_eq!(inventory.quantities[0], 0);
    assert_eq!(inventory.consume(0, 2.4), 0);
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
fn weapon_energy_reading_recovers_when_the_shared_battery_recharges() {
    use osg_ship_api::abi;

    let catalogue = Catalogue::builtin();
    let design = armed_starter().compile(&catalogue).unwrap();
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    state.weapons[0].inhibit_flags = abi::WEAPON_ENERGY;

    let device = design.part_devices[design.weapon_parts[0]].unwrap();
    for (battery, expected) in [(0, abi::WEAPON_ENERGY), (design.battery_j, 0)] {
        state.inventory.energy_j = battery;
        let DeviceReading::Weapon(reading) = state.snapshot(&design)[device].reading else {
            panic!("expected weapon reading");
        };
        assert_eq!(reading.inhibit_flags & abi::WEAPON_ENERGY, expected);
    }
}

#[test]
fn model_scaling_is_validated_and_does_not_change_physics_dimensions() {
    let mut catalogue = Catalogue::builtin();
    assert_eq!(catalogue.part("fuselage_8m").unwrap().model_scale, 1.);
    assert_eq!(
        catalogue.part("micropulse_engine_4m").unwrap().model_scale,
        0.5
    );
    let blueprint = armed_starter();
    let before = blueprint.compile(&catalogue).unwrap();
    catalogue.parts[0].model_scale = 0.5;
    let after = blueprint.compile(&catalogue).unwrap();
    assert_eq!(before.dry_mass, after.dry_mass);
    assert_eq!(before.inertia, after.inertia);
    assert_eq!(
        before.parts[0].definition.dimensions,
        after.parts[0].definition.dimensions
    );
    for scale in [0., -1., f32::NAN, f32::INFINITY] {
        catalogue.parts[0].model_scale = scale;
        assert!(catalogue.validate().is_err());
    }
}
