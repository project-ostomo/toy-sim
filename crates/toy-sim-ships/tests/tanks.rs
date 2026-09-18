use toy_sim_ships::*;

fn blueprint() -> ShipBlueprint {
    ShipBlueprint {
        parts: vec![PlacedPart {
            id: 1,
            name: String::new(),
            alias: String::new(),
            groups: vec![],
            prototype: "fuselage_8m".into(),
            attachment: None,
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
    assert_eq!(state.inventory.quantities[0], 150_000);
    assert_eq!(state.inventory.quantities[1], 50_000);
    assert_eq!(state.inventory.mass(&cat), 200_000.);
    assert_eq!(state.inventory.volume(&cat), 200.);
    state.test_loadout(&design, &cat);
    assert_eq!(state.inventory.mass(&cat), 200_000.);
    assert_eq!(state.inventory.tank_capacities_m3[0], 300.);
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
fn tanks_never_overflow_into_cargo_and_cargo_never_fuels_engines() {
    let cat = Catalogue::builtin();
    let design = blueprint().compile(&cat).unwrap();
    let mut inventory = Inventory::for_design(&design, &cat);
    inventory.insert_consumable(0, 150_000, &cat).unwrap();
    assert!(inventory.insert_consumable(0, 1, &cat).is_err());
    assert!(inventory.insert_consumable(2, 1, &cat).is_err());
    inventory.insert_cargo(0, 100, 1.0, &cat).unwrap();
    assert_eq!(inventory.quantities[0], 300_000);
    assert_eq!(inventory.cargo[0], 100);
    inventory.quantities[0] = 0;
    assert_eq!(inventory.consume(0, 20.0), 0.0);
    assert_eq!(inventory.cargo[0], 100);
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
    assert_eq!(inventory.quantities[2], 10_000_000);
    assert_eq!(inventory.mass(&cat), 100_000.);
    ship.parts[0].tanks[0].volume_m3 = 40.1;
    assert!(ship.compile(&cat).is_err());

    let mut ship = blueprint();
    let mut cargo = ship.parts[0].clone();
    cargo.id = 2;
    cargo.prototype = "storage".into();
    cargo.attachment = Some(Attachment {
        parent: 1,
        socket: "right".into(),
        plug: "left".into(),
        roll: 0,
    });
    cargo.tanks.clear();
    ship.parts.push(cargo);
    let design = ship.compile(&cat).unwrap();
    let mut state = ShipState::new(&design, &cat);
    state.test_loadout(&design, &cat);
    assert!((state.inventory.mass(&cat) - 200_000.).abs() < 1e-8);
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
    assert_eq!(inventory.tank_capacities_m3[hydrogen], 85.);
    let full = 85. / cat.resources[hydrogen].volume_m3;
    inventory
        .insert_consumable(hydrogen, full.floor() as u64, &cat)
        .unwrap();
    assert!(inventory.insert_consumable(hydrogen, 1, &cat).is_err());
    assert!((inventory.mass(&cat) - full.floor()).abs() < 1e-9);
}
