use super::*;
use osg_model::ProgramAction;

fn fixture() -> (World, Entity) {
    let mut world = World::new();
    world.init_resource::<identity::IdentityIndex>();
    let entry = ItineraryEntry {
        label: "Next system".into(),
        directive: Directive::SlipToSystem(Id::new()),
        max_loss_ppm: 50.0,
        fuel_allowance_kg: 10.0,
        estimated_duration_ticks: None,
    };
    let ship = world
        .spawn(Travel(AutopilotState {
            enabled: true,
            directive_revision: 7,
            itinerary: vec![entry.clone(), entry],
            ..Default::default()
        }))
        .id();
    (world, ship)
}

#[test]
fn stale_completion_cannot_pop_a_new_directive() {
    let (mut world, ship) = fixture();
    dispatch(
        &mut world,
        ship,
        ProgramAction::Complete {
            directive_revision: 7,
        },
    )
    .unwrap();
    assert_eq!(world.get::<Travel>(ship).unwrap().0.itinerary.len(), 1);
    assert!(
        dispatch(
            &mut world,
            ship,
            ProgramAction::Complete {
                directive_revision: 7
            }
        )
        .is_err()
    );
    assert_eq!(world.get::<Travel>(ship).unwrap().0.itinerary.len(), 1);
    dispatch(
        &mut world,
        ship,
        ProgramAction::Complete {
            directive_revision: 8,
        },
    )
    .unwrap();
    let state = &world.get::<Travel>(ship).unwrap().0;
    assert!(state.itinerary.is_empty());
    assert!(!state.enabled);
    assert_eq!(state.status.phase, FirmwarePhase::Completed);
}

#[test]
fn failure_keeps_intent_and_rejects_late_publications() {
    let (mut world, ship) = fixture();
    let itinerary = world.get::<Travel>(ship).unwrap().0.itinerary.clone();
    dispatch(
        &mut world,
        ship,
        ProgramAction::Fail {
            directive_revision: 7,
            reason: "Insufficient fuel".into(),
        },
    )
    .unwrap();
    let state = &world.get::<Travel>(ship).unwrap().0;
    assert_eq!(state.itinerary, itinerary);
    assert_eq!(state.failure.as_deref(), Some("Insufficient fuel"));
    assert!(!state.enabled);
    assert!(
        dispatch(
            &mut world,
            ship,
            ProgramAction::PublishStatus {
                directive_revision: 7,
                status: FirmwareStatus::default()
            }
        )
        .is_err()
    );
}

#[test]
fn waiting_is_display_state_and_has_no_server_deadline() {
    let (mut world, ship) = fixture();
    world.init_resource::<SimulationCounters>();
    let status = FirmwareStatus {
        phase: FirmwarePhase::Waiting {
            until: Some(1),
            why: "Departure obstruction".into(),
        },
        ..Default::default()
    };
    dispatch(
        &mut world,
        ship,
        ProgramAction::PublishStatus {
            directive_revision: 7,
            status: status.clone(),
        },
    )
    .unwrap();
    world.resource_mut::<SimulationCounters>().ticks = 10_000_000;
    let state = &world.get::<Travel>(ship).unwrap().0;
    assert!(state.enabled);
    assert_eq!(state.status, status);
    assert!(state.failure.is_none());
}

#[test]
fn unavailable_docking_target_fails_on_activation() {
    let (mut world, ship) = fixture();
    world.get_mut::<Travel>(ship).unwrap().0.itinerary[0].directive = Directive::DockAt(Id::new());
    validate_active_directive(&mut world, ship);
    let state = &world.get::<Travel>(ship).unwrap().0;
    assert!(!state.enabled);
    assert!(state.failure.is_some());
    assert_eq!(state.itinerary.len(), 2);
}

#[test]
fn stale_route_cannot_replace_existing_intent() {
    let (mut world, ship) = fixture();
    let original = world.get::<Travel>(ship).unwrap().0.clone();
    let plan = osg_model::routing::Plan {
        planned_tick: 0,
        directive_revision: 6,
        topology_revision: 0,
        itinerary: original.itinerary.clone(),
        fuel_budget: FuelBudget::default(),
        estimated_loss_ppm: 0.0,
        exotic_fuel_kg: 0.0,
    };
    assert!(
        apply_plan(
            &mut world,
            ship,
            6,
            plan,
            PlanningPreferences::default(),
            true
        )
        .is_err()
    );
    assert_eq!(world.get::<Travel>(ship).unwrap().0, original);
}

#[test]
fn malformed_status_cannot_replace_the_last_display_publication() {
    let (mut world, ship) = fixture();
    let original = world.get::<Travel>(ship).unwrap().0.clone();
    let invalid = [
        FirmwareStatus {
            summary: "界".repeat(86),
            ..Default::default()
        },
        FirmwareStatus {
            phase: FirmwarePhase::Waiting {
                until: None,
                why: "x".repeat(257),
            },
            ..Default::default()
        },
        FirmwareStatus {
            markers: vec![PlanMarker {
                position: GalacticPosition::ZERO,
                label: "x".repeat(65),
            }],
            ..Default::default()
        },
        FirmwareStatus {
            markers: vec![
                PlanMarker {
                    position: GalacticPosition::ZERO,
                    label: String::new()
                };
                9
            ],
            ..Default::default()
        },
        FirmwareStatus {
            aim_offset_m: Some([f64::NAN, 0.0, 0.0]),
            ..Default::default()
        },
        FirmwareStatus {
            planned_loss_ppm: 1_000_001.0,
            ..Default::default()
        },
        FirmwareStatus {
            spent_loss_ppm: 1_000_001.0,
            ..Default::default()
        },
        FirmwareStatus {
            spent_exotic_fuel_kg: f64::INFINITY,
            ..Default::default()
        },
    ];
    for status in invalid {
        assert!(
            dispatch(
                &mut world,
                ship,
                ProgramAction::PublishStatus {
                    directive_revision: 7,
                    status
                }
            )
            .is_err()
        );
        assert_eq!(world.get::<Travel>(ship).unwrap().0, original);
    }
}

#[test]
fn failure_text_fits_the_abi_without_splitting_utf8() {
    let (mut world, ship) = fixture();
    dispatch(
        &mut world,
        ship,
        ProgramAction::Fail {
            directive_revision: 7,
            reason: "界".repeat(256),
        },
    )
    .unwrap();
    assert_eq!(
        world.get::<Travel>(ship).unwrap().0.failure.as_deref(),
        Some("界".repeat(85).as_str())
    );
}
