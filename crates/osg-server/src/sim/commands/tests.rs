use super::*;
use crate::sim::{
    self,
    ownership::{AssetOwner, Directory},
    travel::{PresenceState, Travel},
};
use bevy::math::DVec3;

fn fixture() -> (App, Id, Id, Entity, Id) {
    let owner = Id::new();
    let officer = Id::new();
    let mut app = sim::provision(&[owner, officer], None, None).unwrap();
    app.update();
    let world = app.world_mut();
    let (ship, id) = world
        .query::<(Entity, &Identity, &Control)>()
        .iter(world)
        .find(|(_, _, control)| control.account == owner)
        .map(|(entity, id, _)| (entity, id.0))
        .unwrap();
    let organization = world.resource::<Directory>().0.players[&owner]
        .organization
        .unwrap();
    world
        .resource_mut::<Directory>()
        .0
        .organizations
        .get_mut(&organization)
        .unwrap()
        .officers
        .insert(officer);
    world
        .entity_mut(ship)
        .insert(AssetOwner(ownership::Principal::Organization(organization)));
    (app, owner, officer, ship, id)
}

#[test]
fn officer_executes_without_session_and_stale_or_revoked_authority_cannot_mutate() {
    let (mut app, member, officer, ship, id) = fixture();
    let world = app.world_mut();
    assert_eq!(
        world.query::<&sim::session::Session>().iter(world).count(),
        0
    );
    let state = telemetry(world, officer, id).unwrap();
    let revision = state.telemetry.authority_revision;
    assert_eq!(state.presentation.ship, id);
    assert!(!state.presentation.inventory.is_empty());
    assert!(telemetry(world, member, id).is_err());
    assert!(source(world, member, id).is_err());
    assert!(execute(world, officer, id, revision + 1, ShipCommand::StopFiring).is_err());
    assert!(
        execute(
            world,
            officer,
            id,
            revision,
            ShipCommand::SetThrottle(f64::NAN)
        )
        .is_err()
    );

    let before = world.get::<Travel>(ship).unwrap().0.revision;
    world
        .get_mut::<ShipSoftware>(ship)
        .unwrap()
        .schedule
        .completed(Some(60.));
    assert!(
        !world
            .get::<ShipSoftware>(ship)
            .unwrap()
            .schedule
            .ready(false)
    );
    let order = travel::Order::Sublight(travel::Destination::Galactic(GalacticPosition::ZERO));
    execute(
        world,
        officer,
        id,
        revision,
        ShipCommand::SetTravel {
            preferences: Default::default(),
            engage: false,
            expected_revision: before,
            orders: vec![order.clone()],
        },
    )
    .unwrap();
    assert_eq!(world.get::<Travel>(ship).unwrap().0.revision, before + 1);
    assert_eq!(world.get::<Travel>(ship).unwrap().0.orders[0].action, order);
    assert!(
        world
            .get::<ShipSoftware>(ship)
            .unwrap()
            .schedule
            .ready(false)
    );
    assert!(
        execute(
            world,
            officer,
            id,
            revision,
            ShipCommand::SetTravel {
                preferences: Default::default(),
                engage: false,
                expected_revision: before,
                orders: vec![],
            }
        )
        .is_err()
    );
    assert_eq!(world.get::<Travel>(ship).unwrap().0.orders.len(), 1);

    let organization = world.resource::<Directory>().0.players[&member]
        .organization
        .unwrap();
    world
        .resource_mut::<Directory>()
        .0
        .organizations
        .get_mut(&organization)
        .unwrap()
        .officers
        .remove(&officer);
    assert!(source(world, officer, id).is_err());
    assert!(telemetry(world, officer, id).is_err());
    assert!(execute(world, officer, id, revision, ShipCommand::StopFiring).is_err());
}

#[test]
fn target_handles_and_director_queries_use_own_observed_tracks_and_fresh_docked_state() {
    let (mut app, _, officer, ship, id) = fixture();
    let world = app.world_mut();
    let observed = 42;
    let hidden = 43;
    let observation = SensorObservation {
        spatial_instance: Id::new(),
        id: observed,
        entity: None,
        pose: Pose::default(),
        radius_m: 10.,
        iff: None,
    };
    let mut snapshot = sim::sensors::ObservationSnapshot::default();
    snapshot.contacts.insert(observed, observation);
    world
        .entity_mut(ship)
        .insert(sim::sensors::Observations(Arc::new(snapshot)));
    let reader = source(world, officer, id).unwrap();
    let reference = ContactRef {
        observer: id,
        contact: observed,
    };
    for target in [
        ContactRef {
            observer: Id::new(),
            ..reference
        },
        ContactRef {
            contact: hidden,
            ..reference
        },
    ] {
        assert!(
            reader
                .query(
                    ProgramQuery::Contact(target),
                    false,
                    osg_model::wasm_world::ReplyCapacity::UNLIMITED
                )
                .is_err()
        );
    }
    let ProgramReply::Contact { handle, .. } = reader
        .query(
            ProgramQuery::Contact(reference),
            false,
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap()
    else {
        panic!("expected contact");
    };
    let revision = world.get::<Control>(ship).unwrap().revision;
    assert!(
        execute(
            world,
            officer,
            id,
            revision,
            ShipCommand::MarkTarget {
                target: ContactRef {
                    contact: hidden,
                    ..reference
                },
                maximum_flight_time_s: 1.,
            }
        )
        .is_err()
    );
    execute(
        world,
        officer,
        id,
        revision,
        ShipCommand::MarkTarget {
            target: reference,
            maximum_flight_time_s: 1.,
        },
    )
    .unwrap();
    assert!(
        matches!(world.get::<ShipSoftware>(ship).unwrap().inbox.last().unwrap().command, Command::MarkTarget { contact, .. } if contact == handle)
    );

    let host = world
        .query_filtered::<Entity, With<sim::travel::DockingBays>>()
        .iter(world)
        .next()
        .unwrap();
    let host_id = world.get::<Identity>(host).unwrap().0;
    let position = GalacticPosition::from_meters(DVec3::X * 1e9);
    world
        .get_mut::<sim::precision::PreciseTransform>(host)
        .unwrap()
        .translation_um = position;
    world.entity_mut(ship).insert((
        PresenceState(travel::Presence::Docked {
            host: host_id,
            bay: 0,
        }),
        sim::travel::Dormant,
    ));
    world.get_mut::<Travel>(ship).unwrap().0.revision = 77;
    let reader = source(world, officer, id).unwrap();
    let ProgramReply::Travel {
        state,
        pose,
        slip_ready,
        ..
    } = reader
        .query(
            ProgramQuery::Travel,
            true,
            osg_model::wasm_world::ReplyCapacity::UNLIMITED,
        )
        .unwrap()
    else {
        panic!("expected current travel state")
    };
    assert_eq!(state.revision, 77);
    assert_eq!(pose, sim::session::ship_pose(world, ship).unwrap());
    assert!(pose.position.relative_to(position).length() < 1000.);
    assert!(!slip_ready);
    assert!(matches!(
        telemetry(world, officer, id).unwrap().telemetry.presence,
        travel::Presence::Docked { .. }
    ));
}

#[test]
fn rejected_two_request_autopilot_change_leaves_queue_and_slip_preparation_intact() {
    let (mut app, _, officer, ship, id) = fixture();
    let world = app.world_mut();
    let revision = world.get::<Control>(ship).unwrap().revision;
    let travel = world.get::<Travel>(ship).unwrap().0.clone();
    {
        let mut software = world.get_mut::<ShipSoftware>(ship).unwrap();
        software.inbox.clear();
        for _ in 0..254 {
            software.command(Command::StopFiring);
        }
    }
    let request_id = world.get::<ShipSoftware>(ship).unwrap().request_id;
    world
        .get_mut::<sim::travel::SlipDrive>(ship)
        .unwrap()
        .preparation = Some(sim::travel::Preparation {
        navigation_beacon: None,
        destination: GalacticPosition::ZERO,
        started: 7,
        mass: 1000.,
        work_j: 10.,
        required_j: 100.,
    });
    for command in [
        ShipCommand::SetTravel {
            preferences: Default::default(),
            engage: true,
            expected_revision: travel.revision,
            orders: vec![],
        },
        ShipCommand::SetAutopilot(true),
    ] {
        assert!(
            execute(world, officer, id, revision, command)
                .unwrap_err()
                .to_string()
                .contains("queue full")
        );
        let software = world.get::<ShipSoftware>(ship).unwrap();
        assert_eq!(software.inbox.len(), 254);
        assert_eq!(software.request_id, request_id);
        assert_eq!(world.get::<Travel>(ship).unwrap().0, travel);
        assert_eq!(
            world
                .get::<sim::travel::SlipDrive>(ship)
                .unwrap()
                .preparation
                .as_ref()
                .unwrap()
                .work_j,
            10.
        );
    }
}
