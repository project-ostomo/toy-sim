use osg_ships::*;

#[test]
fn connectors_determine_pose_and_reject_wrong_sizes_and_reused_sockets() {
    let cat = Catalogue::builtin();
    let mut ship = ShipBlueprint::default();
    ship.attach("fuselage_2m", 0, "", "", 0);
    ship.attach("ntr_water_2m", 1, "aft", "fore", 0);
    let design = ship.compile(&cat).unwrap();
    assert!((design.parts[1].centre.z - 3.5).abs() < 1e-10);
    let mut wrong = ship.clone();
    wrong.parts[1].prototype = "ntr_water_4m".into();
    assert!(
        wrong
            .compile(&cat)
            .unwrap_err()
            .to_string()
            .contains("incompatible")
    );
    ship.attach("fuselage_end_2m", 1, "aft", "fore", 0);
    assert!(
        ship.compile(&cat)
            .unwrap_err()
            .to_string()
            .contains("occupied")
    );
}

#[test]
fn attachment_graph_rejects_cycles_and_survives_serialization_and_reordering() {
    let cat = Catalogue::builtin();
    let ship = ntr_patrol();
    let expected = ship.compile(&cat).unwrap();
    let mut decoded = ShipBlueprint::from_bytes(&ship.to_bytes().unwrap()).unwrap();
    decoded.parts.reverse();
    let reordered = decoded.compile(&cat).unwrap();
    for part in &expected.parts {
        let other = reordered
            .parts
            .iter()
            .find(|p| p.placed.id == part.placed.id)
            .unwrap();
        assert_eq!(part.centre, other.centre);
        assert_eq!(part.rotation, other.rotation);
    }
    decoded
        .parts
        .iter_mut()
        .find(|p| p.id == 2)
        .unwrap()
        .attachment
        .as_mut()
        .unwrap()
        .parent = 4;
    assert!(
        decoded
            .compile(&cat)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn station_connectors_cannot_attach_to_ship_equipment_ports() {
    let cat = Catalogue::builtin();
    let mut ship = ntr_patrol();
    ship.attach("station_hangar_64m", 6, "back", "aft", 0);
    assert!(
        ship.compile(&cat)
            .unwrap_err()
            .to_string()
            .contains("incompatible")
    );
}

#[test]
fn coarse_hangar_collision_preserves_the_docking_aperture() {
    let cat = Catalogue::builtin();
    let mut ship = ShipBlueprint::default();
    ship.attach("station_hangar_64m", 0, "", "", 0);
    let design = ship.compile(&cat).unwrap();
    let boxes = collision::voxel_boxes(&design);
    let inside = |point: glam::DVec3| {
        boxes
            .iter()
            .any(|(center, half)| ((point - center).abs() - half).max_element() <= 0.0)
    };
    assert!(!inside(glam::DVec3::new(0.0, 0.0, -40.0)));
    assert!(!inside(glam::DVec3::ZERO));
    assert!(inside(glam::DVec3::new(30.0, 0.0, -40.0)));
    assert!(inside(glam::DVec3::new(0.0, 0.0, 45.0)));
    assert!(boxes.len() < 10_000);
}

#[test]
fn ship_weapons_can_mount_on_station_equipment_ports() {
    let cat = Catalogue::builtin();
    for prototype in [
        "station_core_32m",
        "station_end_32m",
        "station_habitat_200m",
        "station_hangar_64m",
        "directory_transmitter_48m",
    ] {
        let mut ship = ShipBlueprint::default();
        ship.attach(prototype, 0, "", "", 0);
        ship.attach("autocannon_compact", 1, "right", "left", 0);
        ship.compile(&cat)
            .unwrap_or_else(|error| panic!("{prototype}: {error:#}"));
    }
}
