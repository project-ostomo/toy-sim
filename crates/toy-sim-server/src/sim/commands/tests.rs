use super::*;
use crate::sim::{
    self,
    ownership::{AssetOwner, Directory},
    travel::{PresenceState, Travel},
};
use bevy::math::DVec3;
use std::collections::BTreeSet;

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
    let group_entity = world.get::<Membership>(ship).unwrap().0;
    let group_id = world.get::<Group>(group_entity).unwrap().id;
    let other_group_entity = world
        .query::<(Entity, &Control, &Membership)>()
        .iter(world)
        .find(|(_, control, _)| control.account == officer)
        .unwrap()
        .2
        .0;
    let other_group_id = world.get::<Group>(other_group_entity).unwrap().id;
    let observed = Id::new();
    let hidden = Id::new();
    for (group, track) in [(group_entity, observed), (other_group_entity, hidden)] {
        Arc::make_mut(&mut world.get_mut::<Group>(group).unwrap().snapshot).put(Track {
            spatial_instance: Id::new(),
            id: track,
            entity: None,
            pose: Pose::default(),
            position_sigma_m: 100.,
            velocity_sigma_m_s: 10.,
            observed_tick: 0,
            estimate_tick: 0,
            tags: BTreeSet::from([Tag::Kind("ship".into())]),
            provenance: Provenance::Sensor,
            radius_m: Some(10.),
            appearance: None,
        });
    }
    let reader = source(world, officer, id).unwrap();
    assert!(
        reader
            .query(
                ProgramQuery::Contact(ContactRef {
                    group: other_group_id,
                    track: hidden
                }),
                true,
                65536
            )
            .is_err()
    );
    let ProgramReply::Tracks(page) = reader
        .query(
            ProgramQuery::Tracks(TrackQuery {
                track: Some(hidden),
                limit: 1,
                work: 100_000,
                ..Default::default()
            }),
            true,
            65536,
        )
        .unwrap()
    else {
        panic!("expected tracks")
    };
    assert!(page.tracks.is_empty());
    let ProgramReply::Contact { handle, .. } = reader
        .query(
            ProgramQuery::Contact(ContactRef {
                group: group_id,
                track: observed,
            }),
            true,
            65536,
        )
        .unwrap()
    else {
        panic!("expected observed contact")
    };
    let revision = world.get::<Control>(ship).unwrap().revision;
    assert!(
        execute(
            world,
            officer,
            id,
            revision,
            ShipCommand::MarkTarget {
                group: group_id,
                track: hidden,
                maximum_flight_time_s: 1.
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
            group: group_id,
            track: observed,
            maximum_flight_time_s: 1.,
        },
    )
    .unwrap();
    assert!(
        matches!(world.get::<ShipSoftware>(ship).unwrap().inbox.last().unwrap().command, Command::MarkTarget {contact, ..} if contact == handle)
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
    } = reader.query(ProgramQuery::Travel, true, 65536).unwrap()
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
