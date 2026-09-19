use toy_sim_ships::*;

#[test]
fn micropulse_charge_tanks_and_engine_metadata_use_complete_charge_mass() {
    let catalogue = Catalogue::builtin();
    let charge_index = catalogue
        .resources
        .iter()
        .position(|resource| resource.id == "micropulse_charge")
        .unwrap();
    assert_eq!(charge_index, 4);

    let blueprint = ShipBlueprint {
        parts: vec![
            PlacedPart {
                id: 1,
                name: String::new(),
                alias: String::new(),
                groups: vec![],
                prototype: "fuselage_8m".into(),
                attachment: None,
                tanks: vec![Tank {
                    resource: "micropulse_charge".into(),
                    volume_m3: 100.,
                    initial_fill: 0.5,
                }],
            },
            PlacedPart {
                id: 2,
                name: String::new(),
                alias: "main_engine".into(),
                groups: vec![],
                prototype: "micropulse_engine_8m".into(),
                attachment: Some(Attachment {
                    parent: 1,
                    socket: "aft".into(),
                    plug: "fore".into(),
                    roll: 0,
                }),
                tanks: vec![],
            },
        ],
        ..Default::default()
    };
    let design = blueprint.compile(&catalogue).unwrap();
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    assert_eq!(state.inventory.quantities[charge_index], 90_000);
    assert_eq!(state.inventory.quantities[0], 0);
    assert_eq!(state.inventory.mass(&catalogue), 90_000.);

    let device = design
        .device_catalogue
        .iter()
        .find(|device| device.part_id == 2)
        .unwrap();
    let DeviceKind::Engine {
        propellant_resource,
        thrust_n,
        propellant_kg_s,
        power_w,
    } = &device.kind
    else {
        panic!("micropulse engine must expose an engine actuator");
    };
    assert_eq!(propellant_resource, "micropulse_charge");
    assert_eq!(*power_w, 0.);
    assert!((thrust_n / propellant_kg_s - 49_033.25).abs() < 1e-8);
    state
        .apply_commands(
            &design,
            &[DeviceCommand {
                device: device.handle,
                setting: DeviceSetting::Throttle(0.5),
            }],
        )
        .unwrap();
}

#[test]
fn micropulse_energy_budget_rejects_unphysical_or_nonfinite_rates() {
    for (isp, energy, electric, heat) in [
        (5000., 1e9, 0.0005, 0.001),
        (5000., 3e9, 0.6, 0.1),
        (f64::NAN, 3e9, 0.0005, 0.001),
        (1e-300, 3e9, 0.0005, 0.001),
        (5000., f64::INFINITY, 0.0005, 0.001),
        (5000., 3e9, -0.1, 0.001),
        (5000., 3e9, 0.0005, f64::NAN),
    ] {
        let mut catalogue = Catalogue::builtin();
        catalogue
            .parts
            .iter_mut()
            .find(|part| part.id == "micropulse_engine_8m")
            .unwrap()
            .equipment = Equipment::MicropulseEngine {
            thrust_n: 1e7,
            specific_impulse_s: isp,
            charge_energy_j_kg: energy,
            electric_fraction: electric,
            absorbed_heat_fraction: heat,
            plume: None,
        };
        assert!(catalogue.validate().is_err());
    }

    let catalogue = Catalogue::builtin();
    let encoded = toml::to_string(&catalogue).unwrap();
    let decoded: Catalogue = toml::from_str(&encoded).unwrap();
    decoded.validate().unwrap();
    assert!(matches!(
        decoded.part("micropulse_engine_4m").unwrap().equipment,
        Equipment::MicropulseEngine { .. }
    ));

    let mut missing_charges = catalogue;
    missing_charges
        .resources
        .retain(|resource| resource.id != "micropulse_charge");
    assert!(missing_charges.validate().is_err());
}

#[test]
fn micropulse_demonstrator_has_startup_power_cooling_and_charge_reserves() {
    let catalogue = Catalogue::builtin();
    let blueprint = micropulse_starter();
    let design = blueprint.compile(&catalogue).unwrap();
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    assert_eq!(state.inventory.mass(&catalogue), 180_000.);
    assert_eq!(state.inventory.energy_j, 300_000_000);
    assert_eq!(state.inventory.quantities[0], 0);
    assert!(design.shield_radiator_area_m2 > 0.);
    assert!(design.shield_reserve_capacity_kg > 0.);
    assert!(design.max_torque > 0.);
}
