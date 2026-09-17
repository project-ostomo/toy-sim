use glam::DMat3;
use toy_sim_ships::*;
fn fixture() -> (Catalogue, CompiledShipDesign, ShipState) {
    let c = Catalogue::builtin();
    let d = starter(EXAMPLE_CONTROLLER.to_vec()).compile(&c).unwrap();
    let mut s = ShipState::new(&d, &c);
    s.test_loadout(&d, &c);
    (c, d, s)
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
