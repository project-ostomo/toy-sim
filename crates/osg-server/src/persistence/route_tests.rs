use super::*;
use osg_model::travel::{Directive, FuelBudget, FuelRequirement, ItineraryEntry, PlanMarker};

#[test]
fn restored_itinerary_preserves_intent_budgets_and_failure_but_replans_status() {
    let account = Id::new();
    let mut app = crate::scenario(&[account], Some(account), None).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let ship_id = id(world, ship).unwrap();
    let station = world
        .query::<(&identity::Identity, &travel::DockingBays)>()
        .iter(world)
        .next()
        .map(|(identity, _)| identity.0)
        .unwrap();
    let position = pose(world, ship).unwrap().position;

    for (enabled, failure) in [
        (true, None),
        (false, None),
        (false, Some("Capture beacon lost".to_string())),
    ] {
        let ship = identity::lookup(world, ship_id).unwrap();
        let original = AutopilotState {
            enabled,
            failure,
            directive_revision: 27,
            itinerary: vec![ItineraryEntry {
                directive: Directive::DockAt(station),
                label: "Destination station".into(),
                max_loss_ppm: 8.5,
                fuel_allowance_kg: 168.,
                estimated_duration_ticks: Some(1200),
            }],
            risk_budget: osg_model::travel::RiskBudget {
                max_log_loss: osg_model::travel::slip::log_loss_from_ppm(12.5),
                spent_log_loss: osg_model::travel::slip::log_loss_from_ppm(4.),
            },
            fuel_budget: Some(FuelBudget {
                complete: true,
                resources: vec![FuelRequirement {
                    resource: "water".into(),
                    required_kg: 168.,
                    available_kg: 1000.,
                }],
            }),
            status: FirmwareStatus {
                phase: FirmwarePhase::Maneuvering,
                spent_loss_ppm: 4.,
                spent_exotic_fuel_kg: 12.,
                estimated_arrival_tick: Some(12345),
                markers: vec![PlanMarker {
                    position,
                    label: "Private maneuver".into(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        world
            .entity_mut(ship)
            .insert(travel::Travel(original.clone()));
        let checkpoint = capture(world).unwrap();
        restore(world, &checkpoint).unwrap();

        let ship = identity::lookup(world, ship_id).unwrap();
        let restored = &world.get::<travel::Travel>(ship).unwrap().0;
        let mut expected = original;
        expected.status = FirmwareStatus {
            spent_loss_ppm: 4.,
            spent_exotic_fuel_kg: 12.,
            phase: if enabled {
                FirmwarePhase::Planning
            } else {
                FirmwarePhase::Idle
            },
            ..Default::default()
        };
        assert_eq!(restored, &expected);
        assert!(
            world
                .get::<vessel::ShipSoftware>(ship)
                .unwrap()
                .controller
                .is_booting()
        );
    }
}
