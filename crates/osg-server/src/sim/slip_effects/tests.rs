use super::*;

#[test]
fn opaque_geometry_splits_a_wake_without_hiding_its_visible_ends() {
    let mut world = World::new();
    let observer = world.spawn_empty().id();
    let blocker = world.spawn_empty().id();
    let mut index = spatial::SpatialIndex::default();
    index.insert(spatial::SpatialObject {
        entity: blocker,
        position: GalacticPosition::ZERO.offset_by(DVec3::Z * 500.0),
        radius_m: 100.0,
        occludes: true,
        optical_occludes: true,
        optical_luminosity_w: 0.0,
    });
    index.finish_geometry();
    world.insert_resource(index);
    let wake = SlipWake {
        view: 1,
        id: Id::new(),
        start: GalacticPosition::ZERO.offset_by(DVec3::new(-1000.0, 0.0, 1000.0)),
        end: GalacticPosition::ZERO.offset_by(DVec3::new(1000.0, 0.0, 1000.0)),
        start_ns: 0,
        end_ns: osg_model::TICK_NS,
        drift_m_s: [0.0; 3],
        radius_m: 10.0,
        seed: 0,
        offset_m: 0.0,
    };
    let mut pieces = Vec::new();
    visible_pieces(
        &world,
        observer,
        GalacticPosition::ZERO,
        &wake,
        1_000_000_000,
        0,
        &mut pieces,
    );
    assert!(pieces.len() >= 2);
    assert!(pieces.iter().all(|piece| !blocked(
        &world,
        observer,
        GalacticPosition::ZERO,
        piece.position(0.5, 1_000_000_000)
    )));
    assert!(pieces.iter().any(|piece| piece.start == wake.start));
    assert!(pieces.iter().any(|piece| piece.end == wake.end));
}

#[test]
fn swept_wakes_coalesce_drift_trim_and_survive_the_ship_and_serialization() {
    let mut world = World::new();
    world.init_resource::<SimulationCounters>();
    let source = world.spawn(identity::Identity(Id::new())).id();
    let origin = GalacticPosition::splat(1_000_000_000_000_000_000_000);
    let drift = [30_000.0, 0.0, 0.0];
    for i in 0..10 {
        record_span(
            &mut world,
            source,
            origin.offset_by(DVec3::Z * i as f64 * 1e12),
            origin.offset_by(DVec3::Z * (i + 1) as f64 * 1e12),
            i * osg_model::TICK_NS,
            (i + 1) * osg_model::TICK_NS,
            drift,
        );
    }
    world.despawn(source);
    let history = world.resource::<SlipHistory>();
    assert_eq!(history.spans.len(), 1);
    let wake = &history.spans[0].wake;
    assert!((wake.position(0.0, 100_000_000_000).relative_to(origin).x - 3e6).abs() < 0.01);
    let bytes = postcard::to_stdvec(history).unwrap();
    let mut restored: SlipHistory = postcard::from_bytes(&bytes).unwrap();
    restored.validate(1_000_000_000).unwrap();
    restored.prune_at(300_500_000_000);
    assert_eq!(restored.spans.len(), 1);
    assert_eq!(restored.spans[0].wake.start_ns, 500_000_000);
    assert!((restored.spans[0].wake.offset_m - 5e12).abs() < 1.0);
    restored.prune_at(301_000_000_000);
    assert!(restored.spans.is_empty());
}

#[test]
fn direction_changes_do_not_draw_a_chord_across_the_turn() {
    let mut world = World::new();
    let source = world.spawn(identity::Identity(Id::new())).id();
    let a = GalacticPosition::ZERO;
    let b = a.offset_by(DVec3::X * 1e9);
    let c = b.offset_by(DVec3::Z * 1e9);
    record_span(&mut world, source, a, b, 0, osg_model::TICK_NS, [0.0; 3]);
    record_span(
        &mut world,
        source,
        b,
        c,
        osg_model::TICK_NS,
        2 * osg_model::TICK_NS,
        [0.0; 3],
    );
    assert_eq!(world.resource::<SlipHistory>().spans.len(), 2);
}

#[test]
fn slipping_observer_receives_its_own_trailing_wake() {
    let account = Id::new();
    let mut app = crate::sim::bootstrap::provision_combat_fixture(&[account], None, None).unwrap();
    let world = app.world_mut();
    let observer = world
        .query_filtered::<Entity, With<super::super::vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let id = world.get::<identity::Identity>(observer).unwrap().0;
    let origin = world
        .get::<super::super::precision::PreciseTransform>(observer)
        .unwrap()
        .translation_um;
    world.resource_mut::<SimulationCounters>().ticks += 1;
    let time = now(world);
    let distance = osg_model::travel::slip::CRUISE_SPEED_LY_S * osg_model::travel::slip::LY_M * 0.1;
    let start = origin.offset_by(DVec3::Z * distance);
    let other = world.spawn(identity::Identity(Id::new())).id();
    for source in [observer, other] {
        record_span(
            world,
            source,
            start,
            origin,
            time - osg_model::TICK_NS,
            time,
            [0.0; 3],
        );
    }
    let own_wake = world.resource::<SlipHistory>().spans[0].wake.id;
    travel::set_dormant(
        world,
        observer,
        osg_model::travel::Presence::SlipTransit(Id::new()),
    );
    world.entity_mut(observer).insert(travel::Transit {
        origin: start,
        position: origin,
        destination: origin.offset_by(DVec3::NEG_Z * distance),
        departed: 0,
        advanced_tick: 1,
        direction: DVec3::NEG_Z.to_array(),
        nominal_direction: DVec3::NEG_Z.to_array(),
        variance_m2: 0.0,
        speed_ly_s: osg_model::travel::slip::CRUISE_SPEED_LY_S,
        retained_velocity: [0.0; 3],
        departure_mass_kg: 1000.0,
        distance_ly: 0.003,
        consumed_fuel_g: 0.0,
        navigation_beacon: None,
        beacon_lost: false,
        intended_capture: None,
        risk_target: None,
        capture_radius_m: 0.0,
        planned_log_loss: 0.0,
    });
    let views = [ViewState {
        id: 7,
        revision: 1,
        focused_ship: Some(id),
        origin,
    }];
    let publication = observe(world, account, &views);
    assert!(!publication.wakes.is_empty());
    assert!(
        publication
            .wakes
            .iter()
            .all(|wake| wake.id == own_wake && wake.view == 7)
    );
    assert!(publication.wakes.iter().any(|wake| wake.end == origin));
    assert!(publication.wakes.iter().any(|wake| wake.start == start));
    assert!(observe(world, Id::new(), &views).wakes.is_empty());
    world.get_mut::<travel::PresenceState>(observer).unwrap().0 =
        osg_model::travel::Presence::Destroyed;
    assert!(observe(world, account, &views).wakes.is_empty());
}

#[test]
fn a_late_observer_sees_a_swept_flyby_after_source_despawns() {
    let account = Id::new();
    let mut app = crate::sim::bootstrap::provision_combat_fixture(&[account], None, None).unwrap();
    let world = app.world_mut();
    let observer = world
        .query_filtered::<Entity, With<super::super::vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let observer_id = world.get::<identity::Identity>(observer).unwrap().0;
    let origin = world
        .get::<super::super::precision::PreciseTransform>(observer)
        .unwrap()
        .translation_um;
    let source = world.spawn(identity::Identity(Id::new())).id();
    let t = now(world);
    record_span(
        world,
        source,
        origin.offset_by(DVec3::new(-1e12, 10_000.0, 0.0)),
        origin.offset_by(DVec3::new(1e12, 10_000.0, 0.0)),
        t,
        t + osg_model::TICK_NS,
        [0.0; 3],
    );
    world.despawn(source);
    world.resource_mut::<SimulationCounters>().ticks += 600;
    let views = [ViewState {
        id: 7,
        revision: 1,
        focused_ship: Some(observer_id),
        origin,
    }];
    let visible = observe(world, account, &views);
    assert!(!visible.wakes.is_empty());
    assert!(visible.wakes.iter().all(|w| w.view == 7));
    assert!(
        visible
            .wakes
            .iter()
            .all(|w| w.offset_m < 4096.0 * w.radius_m.max(8.0))
    );
    assert!(visible.transitions.is_empty());
    let checkpoint = crate::persistence::world::capture(world).unwrap();
    crate::persistence::world::restore(world, &checkpoint).unwrap();
    assert_eq!(observe(world, account, &views), visible);
    assert!(observe(world, Id::new(), &views).wakes.is_empty());
    world.resource_mut::<SimulationCounters>().ticks += 3000;
    prune(world);
    assert!(observe(world, account, &views).wakes.is_empty());
}
