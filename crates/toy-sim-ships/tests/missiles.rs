use toy_sim_ships::{
    Catalogue, DeviceKind, Equipment, Inventory, ShipBlueprint, ShipState, missiles,
    utilities::UtilityDef,
};

fn resource(catalogue: &Catalogue, name: &str) -> usize {
    catalogue
        .resources
        .iter()
        .position(|r| r.id == name)
        .unwrap()
}

#[test]
fn packaged_round_matches_spawned_mass_and_has_useful_transfer_endurance() {
    let catalogue = Catalogue::builtin();
    let blueprint = missiles::blueprint();
    let design = blueprint.compile(&catalogue).unwrap();
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    let propellant = resource(&catalogue, missiles::PROPELLANT);
    let ammunition = resource(&catalogue, missiles::AMMUNITION);
    let wet_mass = design.dry_mass + state.inventory.mass(&catalogue);

    assert_eq!(design.dry_mass, 160.);
    assert_eq!(state.inventory.quantities[propellant], missiles::FUEL_KG);
    assert_eq!(wet_mass, catalogue.resources[ammunition].mass_kg);
    assert_eq!(state.inventory.energy_j, missiles::BATTERY_J);
    assert_eq!(design.shield_deployed_kg, 0.);
    assert!(blueprint.avionics.sensor_enabled);
    assert!(matches!(
        design.device_catalogue[design.avionics_handles[2].0 as usize].kind,
        DeviceKind::Sensor { range_m } if range_m == missiles::SENSOR_RANGE_M
    ));

    let engine = design
        .device_catalogue
        .iter()
        .find_map(|device| match device.kind {
            DeviceKind::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
                ..
            } => Some((device, thrust_n, propellant_kg_s, power_w)),
            _ => None,
        })
        .unwrap();
    assert_eq!(engine.1, missiles::THRUST_N);
    assert!((engine.1 / engine.2 - missiles::EXHAUST_M_S).abs() < 1e-6);
    // Symmetric auxiliary parts keep the motor's thrust on the mass centre.
    assert!(engine.0.position_m[0].abs() < 1e-12);
    assert!(engine.0.position_m[1].abs() < 1e-12);

    let delta_v = missiles::EXHAUST_M_S * (wet_mass / design.dry_mass).ln();
    let burn_s = missiles::FUEL_KG as f64 / engine.2;
    assert!(delta_v > 2700. && delta_v < 2800.);
    assert!(1_000_000. / delta_v + burn_s < missiles::GUIDANCE_ENDURANCE_S);

    let seeker_power = design
        .parts
        .iter()
        .find_map(|part| match part.definition.equipment {
            Equipment::Utility {
                utility:
                    UtilityDef::Sensor {
                        range_m, power_w, ..
                    },
            } => {
                assert_eq!(range_m, missiles::SENSOR_RANGE_M);
                Some(power_w)
            }
            _ => None,
        })
        .unwrap();
    let torque_power: f64 = design
        .parts
        .iter()
        .filter_map(|part| match part.definition.equipment {
            Equipment::Torquer { power_w, .. } => Some(power_w),
            _ => None,
        })
        .sum();
    let command_power: f64 = design
        .parts
        .iter()
        .filter_map(|part| match part.definition.equipment {
            Equipment::Utility {
                utility: UtilityDef::Command { power_w },
            } => Some(power_w),
            _ => None,
        })
        .sum();
    assert!(command_power > 0.);
    let worst_case_energy = (seeker_power + command_power + torque_power)
        * missiles::GUIDANCE_ENDURANCE_S
        + engine.3 * burn_s;
    assert!(worst_case_energy < design.battery_j as f64);
}

#[test]
fn patrol_and_installation_magazines_contain_discrete_rounds_with_real_mass() {
    let catalogue = Catalogue::builtin();
    let ammunition = resource(&catalogue, missiles::AMMUNITION);
    for (blueprint, launcher_count) in [
        (missiles::missile_patrol(), 2),
        (missiles::missile_defense_station(), 4),
    ] {
        let design = blueprint.compile(&catalogue).unwrap();
        let inventory = Inventory::for_design(&design, &catalogue);
        let launcher_parts: Vec<_> = design
            .parts
            .iter()
            .filter(|part| {
                matches!(
                    part.definition.equipment,
                    Equipment::Utility {
                        utility: UtilityDef::MissileLauncher { .. }
                    }
                )
            })
            .collect();
        assert_eq!(launcher_parts.len(), launcher_count);
        assert_eq!(
            inventory.quantities[ammunition],
            launcher_count as u64 * missiles::MAGAZINE_ROUNDS
        );
        assert_eq!(inventory.cargo[ammunition], 0);
        assert_eq!(inventory.tank_room(ammunition, &catalogue), 0);

        for part in launcher_parts {
            let Equipment::Utility {
                utility: UtilityDef::MissileLauncher { spec },
            } = part.definition.equipment
            else {
                unreachable!()
            };
            assert!(spec.maximum_range_m >= 1_000_000.);
            assert!(
                part.placed
                    .tanks
                    .iter()
                    .all(|tank| tank.resource == missiles::AMMUNITION)
            );
        }
        let encoded = blueprint.to_bytes().unwrap();
        ShipBlueprint::from_bytes(&encoded)
            .unwrap()
            .compile(&catalogue)
            .unwrap();
    }
}

#[test]
fn chemical_energy_and_launcher_capacity_are_validated() {
    let catalogue = Catalogue::builtin();
    for invalid_energy in [-1., f64::NAN, f64::INFINITY, 0.] {
        let mut invalid = catalogue.clone();
        let part = invalid
            .parts
            .iter_mut()
            .find(|p| p.id == "interceptor_engine")
            .unwrap();
        let Equipment::Engine {
            propellant_energy_j_kg,
            ..
        } = &mut part.equipment
        else {
            unreachable!()
        };
        *propellant_energy_j_kg = invalid_energy;
        assert!(invalid.validate().is_err());
    }

    let mut invalid = catalogue.clone();
    invalid.resources.retain(|r| r.id != missiles::AMMUNITION);
    assert!(invalid.validate().is_err());

    let mut invalid = catalogue;
    invalid
        .parts
        .iter_mut()
        .find(|p| p.id == missiles::LAUNCHER_PART)
        .unwrap()
        .tank_volume_m3 = 0.1;
    assert!(invalid.validate().is_err());
}
