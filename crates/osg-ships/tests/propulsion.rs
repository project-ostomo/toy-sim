use osg_ships::{Catalogue, DeviceKind, Equipment, STANDARD_GRAVITY_M_S2};

#[test]
fn propulsion_catalogue_preserves_energy_and_names_real_consumables() {
    let catalogue = Catalogue::builtin();
    for part in &catalogue.parts {
        match &part.equipment {
            Equipment::Engine {
                thrust_n,
                propellant_kg_s,
                power_w,
                propellant_resource,
                propellant_energy_j_kg,
                ..
            } => {
                assert!(
                    0.5 * thrust_n.powi(2) / propellant_kg_s
                        <= *power_w + propellant_kg_s * propellant_energy_j_kg
                );
                assert!(
                    catalogue
                        .resources
                        .iter()
                        .any(|resource| resource.id == *propellant_resource)
                );
            }
            Equipment::Rcs {
                thrust_n,
                propellant_kg_s,
                power_w,
                propellant_resource,
            } => {
                assert!(0.5 * thrust_n.powi(2) / propellant_kg_s <= *power_w);
                assert!(
                    catalogue
                        .resources
                        .iter()
                        .any(|resource| resource.id == *propellant_resource)
                );
            }
            Equipment::ThermalEngine {
                thrust_n,
                specific_impulse_s,
                propellant_resource,
                ..
            } => {
                let DeviceKind::Engine {
                    propellant_resource: resource,
                    propellant_kg_s,
                    power_w,
                    ..
                } = part.equipment.device_kind().unwrap()
                else {
                    panic!("thermal engine actuator missing");
                };
                assert_eq!(resource, *propellant_resource);
                assert_eq!(power_w, 0.0);
                assert!(
                    (propellant_kg_s * specific_impulse_s * STANDARD_GRAVITY_M_S2 - thrust_n).abs()
                        < 1e-7
                );
            }
            _ => {}
        }
    }
}

#[test]
fn catalogue_rejects_missing_propellant_and_free_jet_energy() {
    let mut catalogue = Catalogue::builtin();
    let engine = catalogue
        .parts
        .iter_mut()
        .find(|p| p.id == "engine")
        .unwrap();
    let Equipment::Engine {
        propellant_resource,
        ..
    } = &mut engine.equipment
    else {
        unreachable!()
    };
    *propellant_resource = "nonexistent".into();
    assert!(catalogue.validate().is_err());

    let mut catalogue = Catalogue::builtin();
    let engine = catalogue
        .parts
        .iter_mut()
        .find(|p| p.id == "engine")
        .unwrap();
    let Equipment::Engine { power_w, .. } = &mut engine.equipment else {
        unreachable!()
    };
    *power_w = 1.0;
    assert!(catalogue.validate().is_err());
}
