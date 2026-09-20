use super::*;
use bevy::math::DVec3;
use osg_model::{GalacticPosition, Pose, Provenance, ownership::*};
use osg_ship_api::abi;
use std::{collections::BTreeSet, sync::Arc};

struct Fixture {
    world: World,
    ship: Entity,
    attacker: Entity,
    group: Entity,
    contact: ContactRef,
}

impl Fixture {
    fn new(observed: bool) -> Self {
        let mut world = World::new();
        world.init_resource::<SimulationCounters>();
        world.init_resource::<identity::IdentityIndex>();
        world.init_resource::<AssociationIndex>();
        world.init_resource::<WitnessedLaunches>();
        let officer = Id::new();
        let organization = Id::new();
        let mut directory = OwnershipDirectory::default();
        directory.organizations.insert(
            organization,
            Organization {
                id: organization,
                name: "Defense test".into(),
                sovereignty: Id::new(),
                open_membership: false,
                officers: BTreeSet::from([officer]),
            },
        );
        directory.players.insert(
            officer,
            PlayerAffiliation {
                account: officer,
                name: "Officer".into(),
                organization: Some(organization),
            },
        );
        world.insert_resource(Directory(directory));

        let group_id = Id::new();
        let group = world
            .spawn(Group {
                id: group_id,
                key: None,
                snapshot: Arc::default(),
            })
            .id();
        identity::register(&mut world, group, group_id);
        let design = Arc::new(
            osg_ships::missiles::missile_patrol()
                .compile(&osg_ships::Catalogue::builtin())
                .unwrap(),
        );
        let bytes = wat::parse_str(format!(
            "(module (memory (export \"memory\") 1) (func (export \"ship_api_version\") (result i32) i32.const {}) (func (export \"ship_tick\")))", abi::VERSION,
        )).unwrap();
        let mut runtime = osg_ship_wasm::ControllerRuntime::default();
        let ship = world
            .spawn((
                ShipDesign(design),
                ShipSoftware::new(runtime.instantiate(&bytes).unwrap()),
                PreciseTransform::default(),
                Membership(group),
                Control {
                    account: officer,
                    revision: 1,
                },
                AssetOwner(Principal::Organization(organization)),
                DefenseDuty {
                    account: officer,
                    organization,
                    installation: None,
                    engagement_range_m: 1e6,
                    hostile_iff: false,
                },
            ))
            .id();
        identity::register(&mut world, ship, Id::new());
        let attacker = world
            .spawn(ShipSoftware::new(runtime.instantiate(&bytes).unwrap()))
            .id();
        identity::register(&mut world, attacker, Id::new());
        let contact = ContactRef {
            group: group_id,
            track: Id::new(),
        };
        let mut fixture = Self {
            world,
            ship,
            attacker,
            group,
            contact,
        };
        if observed {
            fixture.observe();
        }
        fixture
    }

    fn observe(&mut self) {
        let track = Track {
            spatial_instance: Id::new(),
            id: self.contact.track,
            entity: None,
            pose: Pose {
                position: GalacticPosition::ZERO.offset_by(DVec3::X * 1000.),
                ..Default::default()
            },
            position_sigma_m: 1.,
            velocity_sigma_m_s: 1.,
            observed_tick: tick(&self.world),
            estimate_tick: tick(&self.world),
            tags: BTreeSet::new(),
            provenance: Provenance::Sensor,
            radius_m: Some(10.),
            appearance: None,
        };
        let estimate = self.world.spawn(TrackEstimate(track.clone())).id();
        let attacker_id = self.world.get::<Identity>(self.attacker).unwrap().0;
        self.world
            .resource_mut::<AssociationIndex>()
            .0
            .insert((self.group, attacker_id), estimate);
        Arc::make_mut(&mut self.world.get_mut::<Group>(self.group).unwrap().snapshot).put(track);
    }

    fn beam_hit(&mut self) {
        let mut report = Report::default();
        report
            .beam_hits
            .push(super::super::physics::collision::weapons::BeamHit {
                source: self.attacker,
                target: self.ship,
            });
        record(&mut self.world, &report);
    }

    fn firing_requests(&self) -> usize {
        self.world
            .get::<ShipSoftware>(self.ship)
            .unwrap()
            .inbox
            .iter()
            .filter(|request| matches!(request.command, Command::StartFiring))
            .count()
    }

    fn establish_firing(&mut self) {
        self.beam_hit();
        update(&mut self.world);
        assert_eq!(self.firing_requests(), 1);
        let handle = self
            .world
            .get::<DefenseState>(self.ship)
            .unwrap()
            .lease
            .as_ref()
            .unwrap()
            .handle;
        let mut software = self.world.get_mut::<ShipSoftware>(self.ship).unwrap();
        software.inbox.clear();
        software.controller.state.weapons = Some(abi::WeaponsState {
            mode: abi::WEAPONS_FIRING,
            target_contact: handle,
            ..Default::default()
        });
    }
}

#[test]
fn firing_intent_and_unidentified_hits_do_not_identify_an_attacker() {
    let mut fixture = Fixture::new(false);
    fixture
        .world
        .get_mut::<ShipSoftware>(fixture.attacker)
        .unwrap()
        .controller
        .state
        .weapons = Some(abi::WeaponsState {
        mode: abi::WEAPONS_FIRING,
        target_contact: 123,
        ..Default::default()
    });
    record(&mut fixture.world, &Report::default());
    update(&mut fixture.world);
    assert_eq!(fixture.firing_requests(), 0);

    fixture.beam_hit();
    assert!(
        fixture
            .world
            .get::<DefenseState>(fixture.ship)
            .unwrap()
            .threats
            .is_empty()
    );
    fixture.observe();
    fixture
        .world
        .get_mut::<DefenseState>(fixture.ship)
        .unwrap()
        .next_scan = 0;
    update(&mut fixture.world);
    assert_eq!(
        fixture.firing_requests(),
        0,
        "later detection cannot reveal an earlier unseen shot"
    );
}

#[test]
fn observed_anonymous_beam_aggression_uses_contact_and_stops_when_track_is_lost() {
    let mut fixture = Fixture::new(true);
    fixture.establish_firing();
    let state = fixture.world.get::<DefenseState>(fixture.ship).unwrap();
    assert_eq!(state.lease.as_ref().unwrap().contact, fixture.contact);
    assert!(
        fixture
            .world
            .get::<Group>(fixture.group)
            .unwrap()
            .snapshot
            .tracks[&fixture.contact.track]
            .entity
            .is_none()
    );

    fixture.world.resource_mut::<SimulationCounters>().ticks = 3;
    update(&mut fixture.world);
    assert!(
        fixture
            .world
            .get::<ShipSoftware>(fixture.ship)
            .unwrap()
            .inbox
            .iter()
            .any(|request| matches!(request.command, Command::StopFiring))
    );
    assert!(
        fixture
            .world
            .get::<DefenseState>(fixture.ship)
            .unwrap()
            .lease
            .is_none()
    );
}

#[test]
fn missile_impact_only_attributes_parent_if_its_launch_was_observed() {
    for witness_launch in [false, true] {
        let mut fixture = Fixture::new(witness_launch);
        let parent = fixture.world.get::<Identity>(fixture.attacker).unwrap().0;
        let missile = fixture
            .world
            .spawn(super::super::missiles::Missile {
                parent,
                target: fixture.contact,
                handle: 1,
                age_s: 2.,
                direction: [1., 0., 0.],
                throttle: 1.,
                guidance_enabled: true,
            })
            .id();
        record_launch(&mut fixture.world, fixture.attacker, missile);
        if !witness_launch {
            fixture.observe();
        }
        let mut report = Report::default();
        report
            .impact_events
            .push(super::super::physics::collision::ImpactEvent {
                entities: [missile, fixture.ship],
                time: 0.,
                position: GalacticPosition::ZERO,
                velocity: DVec3::X * 1000.,
                normal: DVec3::X,
                shields: [false, true],
                surface_positions: [GalacticPosition::ZERO; 2],
                energy_j: 1e8,
            });
        record(&mut fixture.world, &report);
        update(&mut fixture.world);
        assert_eq!(fixture.firing_requests(), usize::from(witness_launch));
    }
}

#[test]
fn transfer_revokes_only_automation_owned_firing() {
    for new_command in [false, true] {
        let mut fixture = Fixture::new(true);
        fixture.establish_firing();
        fixture
            .world
            .entity_mut(fixture.ship)
            .insert(AssetOwner(Principal::Player(Id::new())));
        if new_command {
            fixture
                .world
                .get_mut::<ShipSoftware>(fixture.ship)
                .unwrap()
                .command(Command::MarkTarget {
                    contact: 999,
                    maximum_flight_time_s: 2.,
                });
        }
        update(&mut fixture.world);
        assert!(fixture.world.get::<DefenseDuty>(fixture.ship).is_none());
        let software = fixture.world.get::<ShipSoftware>(fixture.ship).unwrap();
        assert_eq!(
            software
                .inbox
                .iter()
                .any(|request| matches!(request.command, Command::StopFiring)),
            !new_command
        );
        if new_command {
            assert!(software.inbox.iter().any(|request| matches!(
                request.command,
                Command::MarkTarget { contact: 999, .. }
            )));
        }
    }
}

#[test]
fn changed_published_player_target_survives_transfer_cleanup() {
    let mut fixture = Fixture::new(true);
    fixture.establish_firing();
    fixture
        .world
        .entity_mut(fixture.ship)
        .insert(AssetOwner(Principal::Player(Id::new())));
    fixture
        .world
        .get_mut::<ShipSoftware>(fixture.ship)
        .unwrap()
        .controller
        .state
        .weapons
        .as_mut()
        .unwrap()
        .target_contact = 999;
    update(&mut fixture.world);
    assert!(
        fixture
            .world
            .get::<ShipSoftware>(fixture.ship)
            .unwrap()
            .inbox
            .is_empty()
    );
}

#[test]
fn interception_uses_observed_collision_course_without_hidden_missile_target() {
    let mut fixture = Fixture::new(true);
    let mut track = fixture
        .world
        .get::<Group>(fixture.group)
        .unwrap()
        .snapshot
        .tracks[&fixture.contact.track]
        .as_ref()
        .clone();
    track.tags.insert(osg_model::Tag::Kind("missile".into()));
    track.pose.velocity = [-100., 0., 0.];
    let hull = protected_hull(&fixture.world, fixture.ship).unwrap();
    assert_eq!(incoming_missile(&track, &hull), Some(10.));

    track.pose.velocity = [100., 0., 0.];
    assert_eq!(incoming_missile(&track, &hull), None);
    track.pose.velocity = [-100., 100., 0.];
    assert_eq!(incoming_missile(&track, &hull), None);
    track.pose.velocity = [-100., 0., 0.];
    track.tags.clear();
    assert_eq!(
        incoming_missile(&track, &hull),
        None,
        "unknown ships are not classified as incoming weapons"
    );

    track.tags.insert(osg_model::Tag::Kind("missile".into()));
    Arc::make_mut(
        &mut fixture
            .world
            .get_mut::<Group>(fixture.group)
            .unwrap()
            .snapshot,
    )
    .put(track);
    update(&mut fixture.world);
    assert_eq!(fixture.firing_requests(), 1);
    assert!(
        fixture
            .world
            .get::<super::super::missiles::Missile>(fixture.attacker)
            .is_none()
    );
}

#[test]
fn elapsed_threat_memory_and_revoked_officer_stop_existing_fire() {
    for revoke in [false, true] {
        let mut fixture = Fixture::new(true);
        fixture.establish_firing();
        if revoke {
            let duty = fixture
                .world
                .get::<DefenseDuty>(fixture.ship)
                .unwrap()
                .clone();
            fixture
                .world
                .resource_mut::<Directory>()
                .0
                .organizations
                .get_mut(&duty.organization)
                .unwrap()
                .officers
                .remove(&duty.account);
        } else {
            fixture.world.resource_mut::<SimulationCounters>().ticks = THREAT_TICKS + 1;
            fixture.observe();
        }

        update(&mut fixture.world);
        assert!(
            fixture
                .world
                .get::<ShipSoftware>(fixture.ship)
                .unwrap()
                .inbox
                .iter()
                .any(|request| matches!(request.command, Command::StopFiring))
        );
    }
}

#[test]
fn suspended_new_operator_request_is_not_revoked_on_capture() {
    let mut fixture = Fixture::new(true);
    fixture.establish_firing();
    fixture
        .world
        .entity_mut(fixture.ship)
        .insert(AssetOwner(Principal::Player(Id::new())));
    let mut software = fixture.world.get_mut::<ShipSoftware>(fixture.ship).unwrap();
    software.command(Command::MarkTarget {
        contact: 999,
        maximum_flight_time_s: 2.,
    });
    software.inbox.clear();

    update(&mut fixture.world);
    assert!(
        fixture
            .world
            .get::<ShipSoftware>(fixture.ship)
            .unwrap()
            .inbox
            .is_empty()
    );
}

#[test]
fn lease_cancellation_waits_for_both_command_slots_and_retries() {
    for pending_lease_request in [false, true] {
        let mut fixture = Fixture::new(true);
        fixture.establish_firing();
        fixture
            .world
            .entity_mut(fixture.ship)
            .insert(AssetOwner(Principal::Player(Id::new())));
        let lease_request = fixture
            .world
            .get::<DefenseState>(fixture.ship)
            .unwrap()
            .lease
            .as_ref()
            .unwrap()
            .last_request;
        {
            let mut software = fixture.world.get_mut::<ShipSoftware>(fixture.ship).unwrap();
            for _ in 0..254 {
                software.command(Command::HoldAttitude);
            }
            if pending_lease_request {
                software.inbox.push(osg_ship_wasm::Request {
                    id: lease_request,
                    command: Command::StartFiring,
                });
            }
        }

        update(&mut fixture.world);
        assert!(fixture.world.get::<DefenseDuty>(fixture.ship).is_some());
        assert!(
            fixture
                .world
                .get::<DefenseState>(fixture.ship)
                .unwrap()
                .lease
                .is_some()
        );
        let software = fixture.world.get::<ShipSoftware>(fixture.ship).unwrap();
        assert_eq!(software.inbox.len(), 254);
        assert!(
            software
                .inbox
                .iter()
                .all(|request| matches!(request.command, Command::HoldAttitude))
        );

        fixture
            .world
            .get_mut::<ShipSoftware>(fixture.ship)
            .unwrap()
            .inbox
            .remove(0);
        update(&mut fixture.world);
        assert!(fixture.world.get::<DefenseDuty>(fixture.ship).is_none());
        let software = fixture.world.get::<ShipSoftware>(fixture.ship).unwrap();
        assert_eq!(software.inbox.len(), 255);
        assert!(matches!(software.inbox[253].command, Command::StopFiring));
        assert!(matches!(software.inbox[254].command, Command::UnmarkTarget));
    }
}
