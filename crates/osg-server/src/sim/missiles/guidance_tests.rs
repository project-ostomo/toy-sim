use super::integration_tests::Fixture;
use super::*;

#[test]
fn guidance_uses_real_power_and_engine_state_and_exhausted_rounds_coast() {
    let mut fixture = Fixture::new();
    let missile = fixture.launch();
    fixture.app.update();
    let world = fixture.app.world_mut();
    world.get_mut::<Missile>(missile).unwrap().age_s = 10_000.;
    let capacity = world.get::<ShipDesign>(missile).unwrap().0.battery_j;
    world
        .get_mut::<hardware::ShipInventory>(missile)
        .unwrap()
        .0
        .energy_j = capacity;
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .get::<Missile>(missile)
            .unwrap()
            .guidance_enabled
    );

    let world = fixture.app.world_mut();
    let design = world.get::<ShipDesign>(missile).unwrap().0.clone();
    let engine_index = design
        .parts
        .iter()
        .position(|part| matches!(part.definition.equipment, Equipment::Engine { .. }))
        .unwrap();
    let engine = world.get::<hardware::PartDevices>(missile).unwrap().0[engine_index];
    world
        .get_mut::<hardware::Device>(engine)
        .unwrap()
        .0
        .operational = false;
    fixture.app.update();
    let world = fixture.app.world();
    let handle = world.get::<Missile>(missile).unwrap().handle;
    assert_eq!(
        world.resource::<Callbacks>().0[&fixture.parent][&handle].maximum_acceleration_m_s2,
        0.
    );

    let world = fixture.app.world_mut();
    world
        .get_mut::<hardware::Device>(engine)
        .unwrap()
        .0
        .operational = true;
    world
        .get_mut::<hardware::ShipInventory>(missile)
        .unwrap()
        .0
        .energy_j = 1;
    let before = world
        .get::<PreciseTransform>(missile)
        .unwrap()
        .translation_um;
    fixture.app.update();
    let world = fixture.app.world();
    assert!(!world.get::<Missile>(missile).unwrap().guidance_enabled);
    assert_eq!(world.get::<Missile>(missile).unwrap().throttle, 0.);
    assert!(world.get::<Velocity>(missile).is_some());
    assert!(
        world
            .get::<super::super::physics::collision::CollisionBody>(missile)
            .is_some()
    );
    assert!(world.get::<travel::Dormant>(missile).is_none());
    assert!(world.get::<hardware::Device>(engine).unwrap().0.operational);
    assert_eq!(world.get::<hardware::SensorRange>(missile).unwrap().0, 0.);
    assert!(
        world
            .resource::<Callbacks>()
            .0
            .get(&fixture.parent)
            .is_none()
    );
    assert_ne!(
        world
            .get::<PreciseTransform>(missile)
            .unwrap()
            .translation_um,
        before
    );
}

#[test]
fn computer_fault_neutralizes_shared_missile_actuators_without_destroying_them() {
    let mut fixture = Fixture::new();
    let missile = fixture.launch();
    for _ in 0..3 {
        fixture.app.update();
    }
    assert!(
        fixture
            .app
            .world()
            .get::<Missile>(missile)
            .unwrap()
            .throttle
            > 0.
    );
    fixture
        .app
        .world_mut()
        .get_mut::<ShipSoftware>(fixture.parent)
        .unwrap()
        .controller
        .fail("intentional invalid-memory regression".into());
    fixture.app.update();
    let world = fixture.app.world();
    assert_eq!(world.get::<Missile>(missile).unwrap().throttle, 0.);
    assert!(world.get::<Missile>(missile).unwrap().guidance_enabled);
    let design = &world.get::<ShipDesign>(missile).unwrap().0;
    let settings = &world.get::<hardware::DeviceSettings>(missile).unwrap().0;
    for descriptor in &design.device_catalogue {
        match descriptor.kind {
            DeviceKind::Engine { .. } => assert!(matches!(
                settings[descriptor.handle.0 as usize],
                Some(DeviceSetting::Throttle(0.))
            )),
            DeviceKind::Torquer { .. } => assert!(matches!(
                settings[descriptor.handle.0 as usize],
                Some(DeviceSetting::TorqueNm([0., 0., 0.]))
            )),
            _ => {}
        }
    }
}

#[test]
fn stock_guidance_intercepts_a_crossing_target_with_ordinary_torque_and_propulsion() {
    let mut fixture = Fixture::new();
    let world = fixture.app.world_mut();
    let design = world.get::<ShipDesign>(fixture.parent).unwrap().0.clone();
    let mut controller = world
        .resource_mut::<super::super::vessel::WasmRuntime>()
        .0
        .instantiate(design.blueprint.controller_bytes())
        .unwrap();
    controller.configure_hardware(&design, &world.resource::<ShipCatalogue>().0);
    world
        .entity_mut(fixture.parent)
        .insert(ShipSoftware::new(controller));
    for _ in 0..65 {
        fixture.app.update();
    }
    let world = fixture.app.world_mut();
    assert!(
        !world
            .get::<ShipSoftware>(fixture.parent)
            .unwrap()
            .controller
            .is_booting()
    );
    let origin = world
        .get::<PreciseTransform>(fixture.parent)
        .unwrap()
        .translation_um
        .offset_by(DVec3::Y * 100_000_000.);
    world
        .get_mut::<PreciseTransform>(fixture.parent)
        .unwrap()
        .translation_um = origin;
    world.get_mut::<Velocity>(fixture.parent).unwrap().0 = DVec3::ZERO;
    world.get_mut::<AngularVelocity>(fixture.parent).unwrap().0 = DVec3::ZERO;
    world
        .get_mut::<PreciseTransform>(fixture.target)
        .unwrap()
        .translation_um = origin.offset_by(DVec3::X * 10_000.);
    world.get_mut::<Velocity>(fixture.target).unwrap().0 = DVec3::Y * 200.;
    world.get_mut::<AngularVelocity>(fixture.target).unwrap().0 = DVec3::ZERO;
    for _ in 0..3 {
        fixture.app.update();
    }
    let missile = fixture.launch();
    let world = fixture.app.world();
    let fuel = world
        .resource::<ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|resource| resource.id == spec::PROPELLANT)
        .unwrap();
    let initial_fuel = world
        .get::<hardware::ShipInventory>(missile)
        .unwrap()
        .0
        .quantities[fuel];
    let mut closest = f64::INFINITY;
    let mut hit = false;
    for _ in 0..600 {
        fixture.app.update();
        let world = fixture.app.world();
        let software = world.get::<ShipSoftware>(fixture.parent).unwrap();
        assert!(
            software.controller.fault.is_none(),
            "{:?}",
            software.controller.fault
        );
        let position = world
            .get::<PreciseTransform>(missile)
            .unwrap()
            .translation_um;
        let target_position = world
            .get::<PreciseTransform>(fixture.target)
            .unwrap()
            .translation_um;
        closest = closest.min(position.relative_to(target_position).length());
        let angular = world
            .get::<AngularVelocity>(missile)
            .map_or(DVec3::ZERO, |a| a.0);
        assert!(angular.is_finite());
        if world.get::<travel::PresenceState>(missile).unwrap().0
            == osg_model::travel::Presence::Destroyed
        {
            hit = true;
            break;
        }
    }
    let world = fixture.app.world();
    let remaining = world
        .get::<hardware::ShipInventory>(missile)
        .unwrap()
        .0
        .quantities[fuel];
    eprintln!(
        "crossing interceptor: closest={closest:.3}m fuel={remaining}/{initial_fuel} hit={hit}"
    );
    assert!(
        hit && closest < 100.,
        "missile missed; closest={closest:.3}m"
    );
    assert!(
        remaining < initial_fuel && remaining > 0,
        "intercept requires finite propellant use"
    );
}
