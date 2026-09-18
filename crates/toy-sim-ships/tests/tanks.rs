use toy_sim_ships::*;

fn blueprint() -> ShipBlueprint {
    ShipBlueprint {
        parts: vec![PlacedPart {
            id: 1,
            name: String::new(),
            alias: String::new(),
            groups: vec![],
            prototype: "fuselage_8m".into(),
            position: [0; 3],
            orientation: 0,
            tanks: vec![
                Tank {
                    resource: "propellant".into(),
                    volume_m3: 300.,
                    initial_fill: 0.5,
                },
                Tank {
                    resource: "fuel".into(),
                    volume_m3: 200.,
                    initial_fill: 0.25,
                },
            ],
        }],
        ..Default::default()
    }
}

#[test]
fn tanks_round_trip_and_initialize_resources_by_density() {
    let cat = Catalogue::builtin();
    let ship = blueprint();
    let decoded = ShipBlueprint::from_bytes(&ship.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded.parts, ship.parts);
    let design = decoded.compile(&cat).unwrap();
    let mut state = ShipState::new(&design, &cat);
    assert_eq!(state.inventory.quantities[0], 150_000.);
    assert_eq!(state.inventory.quantities[1], 50_000.);
    assert_eq!(state.inventory.mass(&cat), 200_000.);
    assert_eq!(state.inventory.volume(&cat), 200.);
    state.test_loadout(&design, &cat);
    assert_eq!(state.inventory.mass(&cat), 200_000.);
    assert_eq!(state.inventory.capacity_m3(0, design.capacity_m3), 300.);
}

#[test]
fn tank_allocations_are_validated_before_launch() {
    let cat = Catalogue::builtin();
    for volume in [300.001, -1., 0., f64::NAN, f64::INFINITY] {
        let mut ship = blueprint();
        ship.parts[0].tanks[0].volume_m3 = volume;
        assert!(ship.compile(&cat).is_err(), "accepted {volume}");
    }
    for fill in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
        let mut ship = blueprint();
        ship.parts[0].tanks[0].initial_fill = fill;
        assert!(ship.compile(&cat).is_err());
    }
    let mut ship = blueprint();
    ship.parts[0].tanks[0].resource = "unobtainium".into();
    assert!(ship.compile(&cat).is_err());
    let mut ship = blueprint();
    ship.parts[0].prototype = "structure".into();
    assert!(ship.compile(&cat).is_err());
    let mut ship = blueprint();
    ship.parts[0].tanks = vec![
        Tank {
            resource: "fuel".into(),
            volume_m3: 1.,
            initial_fill: 1.
        };
        33
    ];
    assert!(ship.compile(&cat).is_err());
}

#[test]
fn dedicated_capacity_cannot_store_other_resources_and_failed_transfers_are_atomic() {
    let cat = Catalogue::builtin();
    let design = blueprint().compile(&cat).unwrap();
    let mut inventory = Inventory::for_design(&design, &cat);
    inventory.insert(0, 150_000., 0., &cat).unwrap();
    assert!(inventory.insert(0, 1., 0., &cat).is_err());
    assert!(inventory.insert(2, 1., 0., &cat).is_err());
    let mut source = Inventory::empty(&cat);
    source.insert(0, 2., 1., &cat).unwrap();
    let before = inventory.quantities.clone();
    assert!(source.transfer(&mut inventory, 0, 1., 0., &cat).is_err());
    assert_eq!(inventory.quantities, before);
    assert_eq!(source.quantities[0], 2.);
    inventory.transfer(&mut source, 0, 100., 1., &cat).unwrap();
    source.transfer(&mut inventory, 0, 100., 0., &cat).unwrap();
    assert_eq!(inventory.quantities, before);
}

#[test]
fn repeated_resources_share_capacity_and_cargo_uses_only_overflow() {
    let cat = Catalogue::builtin();
    let mut ship = blueprint();
    ship.parts[0].tanks[1].resource = "propellant".into();
    let design = ship.compile(&cat).unwrap();
    let mut inventory = Inventory::for_design(&design, &cat);
    assert_eq!(inventory.capacity_m3(0, 1.), 501.);
    inventory.insert(0, 300_000., 1., &cat).unwrap();
    inventory.insert(1, 500., 1., &cat).unwrap();
    inventory.insert(0, 500., 1., &cat).unwrap();
    assert!((inventory.cargo_volume(&cat) - 1.).abs() < 1e-8);
    assert!(inventory.insert(0, 1., 1., &cat).is_err());
    assert!(inventory.insert(1, 1., 1., &cat).is_err());
}

#[test]
fn end_caps_use_resource_density_and_debug_cargo_preserves_tank_loading() {
    let cat = Catalogue::builtin();
    let mut ship = blueprint();
    ship.parts[0].prototype = "fuselage_end_8m".into();
    ship.parts[0].tanks = vec![Tank {
        resource: "bearing".into(),
        volume_m3: 40.,
        initial_fill: 0.5,
    }];
    let design = ship.compile(&cat).unwrap();
    let inventory = Inventory::for_design(&design, &cat);
    assert_eq!(inventory.quantities[2], 10_000_000.);
    assert_eq!(inventory.mass(&cat), 100_000.);
    ship.parts[0].tanks[0].volume_m3 = 40.1;
    assert!(ship.compile(&cat).is_err());

    let mut ship = blueprint();
    let mut cargo = ship.parts[0].clone();
    cargo.id = 2;
    cargo.prototype = "storage".into();
    cargo.position = [80, 0, 0];
    cargo.tanks.clear();
    ship.parts.push(cargo);
    let design = ship.compile(&cat).unwrap();
    let mut state = ShipState::new(&design, &cat);
    state.test_loadout(&design, &cat);
    assert!((state.inventory.mass(&cat) - 200_800.).abs() < 1e-8);
    assert!(state.inventory.cargo_volume(&cat) <= design.capacity_m3);
}

#[test]
fn containment_occupies_volume_and_mass_even_when_tanks_are_empty() {
    let cat = Catalogue::builtin();
    let mut ship = blueprint();
    ship.parts[0].tanks.clear();
    let empty_hull = ship.compile(&cat).unwrap();
    ship.parts[0].tanks.push(Tank {
        resource: "hydrogen".into(),
        volume_m3: 100.,
        initial_fill: 0.,
    });
    let dry_tank = ship.compile(&cat).unwrap();
    assert_eq!(dry_tank.dry_mass - empty_hull.dry_mass, 2_000.);
    assert!(dry_tank.inertia.x_axis.x > empty_hull.inertia.x_axis.x);
    let hydrogen = cat
        .resources
        .iter()
        .position(|r| r.id == "hydrogen")
        .unwrap();
    let mut inventory = Inventory::for_design(&dry_tank, &cat);
    assert_eq!(inventory.capacity_m3(hydrogen, 0.), 85.);
    let full = 85. / cat.resources[hydrogen].volume_m3;
    inventory.insert(hydrogen, full, 0., &cat).unwrap();
    assert!(inventory.insert(hydrogen, 1., 0., &cat).is_err());
    assert!((inventory.mass(&cat) - full).abs() < 1e-9);
}
