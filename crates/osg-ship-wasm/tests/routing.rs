use glam::DVec3;
use osg_model::{
    GalacticPosition, Id, LocalObstacle, Pose, ProgramAction, ProgramQuery, ProgramReply,
    SlipProbeResult,
    location::LocationContext,
    travel::{
        AutopilotState, Axes, CelestialRef, Destination, Directive, FirmwarePhase, ItineraryEntry,
        Presence, Reference, Target,
    },
};
use osg_ship_api::abi;
use osg_ship_wasm::{
    ControllerRuntime, FUEL_PER_TICK, Input, Observation, ScanSource, SensorContact,
};
use osg_ships::{Catalogue, EXAMPLE_CONTROLLER, ShipState, expedition_patrol};
use std::sync::atomic::{AtomicU64, Ordering};

// A ship's hardware clock starts when built, independently of simulation time.
const WORLD_EPOCH_TICK: u64 = 10_000;

fn id(value: u32) -> Id {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&value.to_be_bytes());
    Id(bytes)
}

struct World {
    tick: AtomicU64,
    searches: AtomicU64,
    first_clearance_search: AtomicU64,
    enabled: bool,
    arrived: bool,
    blocked: bool,
    dock: bool,
    known_station: bool,
    clearance: bool,
    capture_hill_radius: f64,
    inner_body: bool,
}

impl World {
    fn body(&self, after: f64) -> Pose {
        let seconds = (self.tick.load(Ordering::Relaxed) + WORLD_EPOCH_TICK) as f64
            * osg_model::TICK_SECONDS
            + after;
        Pose {
            position: GalacticPosition::from_meters(DVec3::new(1e16, 30000. * seconds, 0.)),
            velocity: [0., 30000., 0.],
            ..Default::default()
        }
    }
}

impl ScanSource for World {
    fn scan(&self, _: f64, _: usize) -> Vec<SensorContact> {
        Vec::new()
    }

    fn query(
        &self,
        query: ProgramQuery,
        _: bool,
        _: osg_model::wasm_world::ReplyCapacity,
    ) -> anyhow::Result<ProgramReply> {
        Ok(match query {
            ProgramQuery::Travel => ProgramReply::Travel {
                state: AutopilotState {
                    enabled: self.enabled,
                    directive_revision: 7,
                    itinerary: vec![ItineraryEntry {
                        directive: if self.dock {
                            Directive::DockAt(id(9))
                        } else {
                            Directive::SlipToSystem(id(2))
                        },
                        label: "Test destination".into(),
                    }],
                    ..Default::default()
                },
                pose: Pose::default(),
                presence: Presence::Space,
                location: LocationContext {
                    system: Some(if self.arrived { id(2) } else { id(1) }),
                    ..Default::default()
                },
                tick: self.tick.load(Ordering::Relaxed) + WORLD_EPOCH_TICK,
                exotic_fuel_kg: 1e9,
                slip_ready: true,
                slip_axis: [0., 0., -1.],
            },
            ProgramQuery::Orrery { .. } => ProgramReply::Orrery(
                if self.clearance && self.tick.load(Ordering::Relaxed) < 300 {
                    vec![LocalObstacle {
                        reference: Target::Destination(Destination::Relative {
                            reference: Reference::Celestial(CelestialRef {
                                system: id(1),
                                body: id(4),
                            }),
                            offset: GalacticPosition::ZERO,
                            axes: Axes::Galactic,
                        }),
                        pose: Pose {
                            position: GalacticPosition::from_meters(-DVec3::X * 1e9),
                            ..Default::default()
                        },
                        radius_m: 1e7,
                        slip_exclusion_m: 1e9 + 1000.,
                        hill_radius_m: 0.,
                    }]
                } else if self.capture_hill_radius > 1e16 {
                    let mut bodies = vec![LocalObstacle {
                        reference: Target::Destination(Destination::Relative {
                            reference: Reference::Celestial(CelestialRef {
                                system: id(2),
                                body: id(3),
                            }),
                            offset: GalacticPosition::ZERO,
                            axes: Axes::Galactic,
                        }),
                        pose: self.body(0.),
                        radius_m: 1e7,
                        slip_exclusion_m: 1e10,
                        hill_radius_m: self.capture_hill_radius,
                    }];
                    if self.inner_body {
                        let mut child = bodies[0].clone();
                        child.reference = Target::Destination(Destination::Relative {
                            reference: Reference::Celestial(CelestialRef {
                                system: id(2),
                                body: id(4),
                            }),
                            offset: GalacticPosition::ZERO,
                            axes: Axes::Galactic,
                        });
                        child.pose = Pose {
                            position: GalacticPosition::from_meters(DVec3::Y * 1e11),
                            ..Default::default()
                        };
                        child.hill_radius_m = 1e12;
                        bodies.push(child);
                    }
                    bodies
                } else {
                    Vec::new()
                },
            ),
            ProgramQuery::OrrerySystem {
                system,
                after_seconds,
            } => {
                assert_eq!(system, id(if self.dock { 1 } else { 2 }));
                if after_seconds == 0. {
                    self.searches.fetch_add(1, Ordering::Relaxed);
                }
                if self.known_station {
                    return Ok(ProgramReply::Orrery(Vec::new()));
                }
                ProgramReply::Orrery(vec![LocalObstacle {
                    reference: Target::Destination(Destination::Relative {
                        reference: Reference::Celestial(CelestialRef {
                            system,
                            body: id(3),
                        }),
                        offset: GalacticPosition::ZERO,
                        axes: Axes::Galactic,
                    }),
                    pose: self.body(after_seconds),
                    radius_m: 1e7,
                    slip_exclusion_m: 1e10,
                    hill_radius_m: self.capture_hill_radius,
                }])
            }
            ProgramQuery::Resolve { after_seconds, .. } => {
                ProgramReply::Pose(self.body(after_seconds))
            }
            ProgramQuery::Beacons { .. } | ProgramQuery::Beacon(_) => {
                ProgramReply::Beacons(if self.known_station {
                    vec![osg_model::Beacon {
                        entity: id(9),
                        system: Some(id(1)),
                        radius_m: 100.,
                        pose: Pose {
                            position: GalacticPosition::from_meters(DVec3::X * 1e8),
                            ..Default::default()
                        },
                        iff: osg_model::IffIdentity {
                            owner: id(8),
                            faction: None,
                            labels: Default::default(),
                            enabled: true,
                        },
                        bays: [(0, Pose::default())].into_iter().collect(),
                    }]
                } else {
                    Vec::new()
                })
            }
            ProgramQuery::SlipEligibilityBatch(probes) => ProgramReply::SlipEligibilityBatch(
                probes
                    .iter()
                    .map(|probe| {
                        if self.clearance
                            && probe.origin.relative_to(GalacticPosition::ZERO).x > 1000.
                        {
                            let _ = self.first_clearance_search.compare_exchange(
                                0,
                                self.searches.load(Ordering::Relaxed),
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            );
                        }
                        SlipProbeResult {
                            ready: !self.blocked
                                && (!self.clearance
                                    || self.tick.load(Ordering::Relaxed) >= 300
                                    || probe.origin.relative_to(GalacticPosition::ZERO).x > 1000.),
                            preparation_s: 10.,
                            duration_s: osg_model::travel::slip::flight_seconds(
                                probe.destination.relative_to(probe.origin).length(),
                                false,
                            ),
                            error: None,
                        }
                    })
                    .collect(),
            ),
            other => anyhow::bail!("unexpected firmware query: {other:?}"),
        })
    }
}

fn run(source: &World, ticks: u64) -> Vec<ProgramAction> {
    let catalogue = Catalogue::builtin();
    let design = expedition_patrol().compile(&catalogue).unwrap();
    let state = ShipState::new(&design, &catalogue);
    let (mass_kg, inertia) = state.mass_properties(&design, &catalogue);
    let mut runtime = ControllerRuntime::new().unwrap();
    let mut computer = runtime.instantiate(EXAMPLE_CONTROLLER).unwrap();
    computer.configure_hardware(&design, &catalogue);
    let mut actions = Vec::new();
    for tick in 0..ticks {
        source.tick.store(tick, Ordering::Relaxed);
        let slice = computer
            .run_slice(
                Input {
                    tick,
                    observation: Observation {
                        time_s: tick as f64 * osg_model::TICK_SECONDS,
                        flight: abi::FlightState {
                            rotation: [0., 0., 0., 1.],
                            mass_kg,
                            inertia: inertia.to_cols_array(),
                            radius_m: design.radius,
                            ..Default::default()
                        },
                        inventory: state.inventory.quantities.clone(),
                        resources: abi::ShipResources {
                            energy_j: design.battery_j,
                            hull_hp: design.hull,
                            hull_max_hp: design.hull,
                            ..Default::default()
                        },
                    },
                    devices: state.snapshot(&design),
                    ..Default::default()
                },
                Some(source),
                FUEL_PER_TICK,
                FUEL_PER_TICK,
            )
            .unwrap();
        assert!(
            computer.fault.is_none(),
            "tick {tick}: {:?}",
            computer.fault
        );
        actions.extend(slice.output.world_actions);
    }
    actions
}

fn world() -> World {
    World {
        tick: AtomicU64::new(0),
        searches: AtomicU64::new(0),
        first_clearance_search: AtomicU64::new(0),
        enabled: true,
        arrived: false,
        blocked: false,
        dock: false,
        known_station: false,
        clearance: false,
        capture_hill_radius: 1e12,
        inner_body: false,
    }
}

#[test]
fn disengaged_firmware_preserves_intent_and_arrival_completes_with_generation() {
    let mut source = world();
    source.enabled = false;
    assert!(run(&source, 80).is_empty());
    source.enabled = true;
    source.arrived = true;
    let actions = run(&source, 80);
    assert!(actions.iter().any(|action| matches!(
        action,
        ProgramAction::Complete {
            directive_revision: 7
        }
    )));
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, ProgramAction::Slip { .. }))
    );
}

#[test]
fn unknown_docking_destination_fails_without_consuming_itinerary() {
    let mut source = world();
    source.dock = true;
    let actions = run(&source, 80);
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::Fail { directive_revision: 7, reason }
            if reason.contains("not known in the current system"))));
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, ProgramAction::Complete { .. }))
    );
}

#[test]
fn obstruction_keeps_searching_without_claiming_exhaustion() {
    let mut source = world();
    source.blocked = true;
    let actions = run(&source, 600);
    assert!(
        source.searches.load(Ordering::Relaxed) >= 2,
        "waiting never retried its search"
    );
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::PublishStatus { status, .. }
            if matches!(status.phase, FirmwarePhase::Planning))));
    assert!(!actions.iter().any(|action| matches!(action,
        ProgramAction::PublishStatus { status, .. }
            if matches!(status.phase, FirmwarePhase::Waiting { .. }))));
    assert!(!actions.iter().any(|action| matches!(
        action,
        ProgramAction::Fail { .. } | ProgramAction::Complete { .. } | ProgramAction::Slip { .. }
    )));
}

#[test]
fn firmware_selects_a_capture_and_publishes_its_private_plan() {
    let actions = run(&world(), 600);
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::Slip { destination, not_before_tick: Some(_), .. }
            if destination.relative_to(GalacticPosition::ZERO).x > 9e15)));
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::PublishStatus { status, .. }
            if status.capture_body == Some(CelestialRef { system: id(2), body: id(3) })
                && !status.markers.is_empty())));
}

#[test]
fn firmware_cannot_capture_the_body_containing_its_departure() {
    let mut source = world();
    source.capture_hill_radius = 2e16;
    let actions = run(&source, 600);
    assert!(source.searches.load(Ordering::Relaxed) > 1);
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, ProgramAction::Slip { .. }))
    );
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::PublishStatus { status, .. } if status.phase == FirmwarePhase::Planning)));
}

#[test]
fn firmware_can_capture_parent_when_departing_from_child_hill_sphere() {
    let mut source = world();
    source.capture_hill_radius = 2e16;
    source.inner_body = true;
    assert!(
        run(&source, 600)
            .iter()
            .any(|action| matches!(action, ProgramAction::Slip { .. }))
    );
}

#[test]
fn clearance_precedes_capture_and_is_retired_when_geometry_clears() {
    let mut source = world();
    source.clearance = true;
    let actions = run(&source, 600);
    assert_eq!(
        source.first_clearance_search.load(Ordering::Relaxed),
        2,
        "clearance must be considered immediately after direct departure"
    );
    let statuses: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            ProgramAction::PublishStatus { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    let clearance = statuses
        .iter()
        .position(|status| {
            status.markers.len() == 2
                && status.markers[0].label == "Departure clearance"
                && status.markers[1].label == "Planned capture"
        })
        .expect("clearance must be published before capture");
    assert!(
        statuses[clearance + 1..].iter().any(|status| {
            status.markers.len() == 1
                && status.markers[0].label == "Planned capture"
                && !matches!(status.phase, FirmwarePhase::Maneuvering)
        }),
        "cleared geometry must release the departure maneuver"
    );
}

#[test]
fn docking_guidance_runs_while_slip_alternatives_are_continually_refined() {
    let mut source = world();
    source.dock = true;
    source.known_station = true;
    let actions = run(&source, 600);
    assert!(source.searches.load(Ordering::Relaxed) > 1);
    assert!(
        source.searches.load(Ordering::Relaxed) <= 600,
        "search must evaluate at most one capture catalogue per callback"
    );
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::ReserveBay { station, .. } if *station == id(9))));
    assert!(actions.iter().any(|action| matches!(action,
        ProgramAction::PublishStatus { status, .. } if matches!(status.phase, FirmwarePhase::Docking))));
}
