use super::*;

fn row(id: u8, x: f64) -> Row {
    Row {
        target: SelectedTarget::Contact(ContactRef {
            group: Id([7; 16]),
            track: Id([id; 16]),
        }),
        name: "Same name".into(),
        kind: "Ship".into(),
        offset: glam::DVec3::new(x, 0., 0.),
        distance: x,
        speed: 0.,
        radius: 50.,
        detail: String::new(),
        own: false,
    }
}

#[test]
fn default_desktop_stays_stable_without_overlapping_the_selected_item() {
    let ctx = egui::Context::default();
    toy_sim_ui::theme::install(&ctx);
    let mut shell = Shell::default();
    let selection = Selection {
        target: Some(SelectedTarget::Beacon(Id([9; 16]))),
        ..Default::default()
    };
    let navigation = NavigationCatalogue::default();
    let model = FrameModel {
        navigation: &navigation,
        ships: vec![],
        rows: vec![Row {
            target: SelectedTarget::Beacon(Id([9; 16])),
            name: "Sol gate".into(),
            kind: "Stargate".into(),
            ..row(2, 1000.)
        }],
        ship: None,
        details: None,
        system: "Helion".into(),
        vicinity: "Neris".into(),
        connected: true,
        status: "",
        time_ns: 0,
        diagnostics: Default::default(),
        orbits: true,
    };
    let mut previous = None;
    let size = egui::vec2(1600., 900.);
    for frame in 0..10 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(frame as f64 / 60.),
                ..Default::default()
            },
            |ui| {
                panels::draw(
                    ui.ctx(),
                    &mut shell,
                    &model,
                    &selection,
                    &[],
                    &mut Vec::new(),
                );
            },
        );
        output.textures_delta.clear();
        if frame > 3 {
            let selected = shell.desktop.rect(SELECTED).unwrap();
            let overview = shell.desktop.rect(OVERVIEW).unwrap();
            assert!(
                selected.bottom() <= overview.top(),
                "selected {selected:?} overlaps {overview:?}"
            );
            assert!(
                toy_sim_ui::desktop::workspace_in(&ctx)
                    .expand(0.5)
                    .contains_rect(overview),
                "overview {overview:?} outside screen {size:?}"
            );
            if let Some(previous) = previous {
                assert_eq!(overview, previous);
            }
            previous = Some(overview);
        }
    }
}

#[test]
fn commands_use_target_identity_safe_range_and_normalized_galactic_direction() {
    let ship = super::super::tests::ship(Id([1; 16])).0;
    let rows = [row(2, 1000.)];
    let target = rows[0].target;
    let SelectedTarget::Contact(reference) = target else {
        unreachable!()
    };
    let (commands, _) = commands_for(Intent::Align(target), &ship, &rows).unwrap();
    assert!(
        matches!(&commands[0], ShipCommand::SetTravel { orders, .. } if matches!(&orders[0], travel::Order::Guidance(g) if g.mode == travel::GuidanceMode::Align && g.target == travel::Target::Contact(reference)))
    );
    let (commands, _) = commands_for(Intent::Approach(reference, 1.), &ship, &rows).unwrap();
    assert!(
        matches!(&commands[0], ShipCommand::SetTravel { orders, .. } if matches!(&orders[0], travel::Order::Guidance(g) if g.range_m == 150. && g.mode == travel::GuidanceMode::Approach))
    );
    assert!(commands_for(Intent::Align(target), &ship, &[]).is_none());
    assert!(commands_for(Intent::Align(target), &ship, &[row(2, 0.)]).is_none());
    let mut docked = ship;
    docked.presence = travel::Presence::Docked {
        host: Id([3; 16]),
        bay: 0,
    };
    assert!(commands_for(Intent::Approach(reference, 1000.), &docked, &rows).is_none());
}

#[test]
fn overview_ties_are_stable_and_filters_preserve_selection_identity() {
    let rows = [row(3, 500.), row(2, 500.)];
    let mut shell = Shell::default();
    assert_eq!(sorted_rows(&rows, &shell)[0].key(), rows[1].key());
    shell.descending = true;
    assert_eq!(sorted_rows(&rows, &shell)[0].key(), rows[1].key());
    shell.search = "same NAME".into();
    assert_eq!(sorted_rows(&rows, &shell).len(), 2);
    shell.filter = Filter::Celestials;
    assert!(sorted_rows(&rows, &shell).is_empty());
}

#[test]
fn command_feedback_waits_for_every_reply_and_retains_errors() {
    let first = Id([1; 16]);
    let second = Id([2; 16]);
    let mut feedback = Feedback {
        pending: vec![first, second],
        label: "Approach".into(),
        last_tick: 0,
        error: None,
    };
    feedback.receive(&[CommandResult {
        id: first,
        effective_tick: 10,
        error: Some("Target expired".into()),
        reply: None,
    }]);
    assert_eq!(feedback.pending, [second]);
    feedback.receive(&[CommandResult {
        id: second,
        effective_tick: 11,
        error: None,
        reply: None,
    }]);
    feedback.receive(&[]);
    assert!(feedback.pending.is_empty());
    assert_eq!(feedback.error.as_deref(), Some("Target expired"));
}

#[test]
fn itinerary_draws_gate_transfer_and_slip_orders_with_destination_system_names() {
    let ctx = egui::Context::default();
    toy_sim_ui::theme::install(&ctx);
    let origin = GalacticPosition::default();
    let destination = origin.offset_by(glam::DVec3::X * 1e16);
    let sol = Id([1; 16]);
    let terminus = Id([2; 16]);
    let entry = Id([3; 16]);
    let exit = Id([4; 16]);
    let pose = super::super::tests::ship(Id([9; 16])).0.pose.unwrap();
    let navigation = NavigationCatalogue {
        systems: vec![
            NavigationSystem {
                id: sol,
                name: "Sol".into(),
                position: origin,
            },
            NavigationSystem {
                id: terminus,
                name: "Terminus".into(),
                position: destination,
            },
        ],
        beacons: vec![
            NavigationBeacon {
                id: entry,
                system: terminus,
                name: "Sol gate".into(),
                pose: pose.clone(),
                radius_m: 220.,
                gate_exit: Some(exit),
                docking: false,
            },
            NavigationBeacon {
                id: exit,
                system: sol,
                name: "Helion gate".into(),
                pose,
                radius_m: 220.,
                gate_exit: Some(entry),
                docking: false,
            },
        ],
    };
    let mut state = travel::TravelState {
        autopilot_enabled: true,
        status: travel::Status::Active,
        estimated_arrival_tick: Some(600),
        orders: vec![
            travel::Order::Jump(entry),
            travel::Order::Sublight(travel::Destination::Galactic(origin)),
            travel::Order::Slip { destination },
            travel::Order::Sublight(travel::Destination::Galactic(destination)),
        ]
        .into_iter()
        .map(|order| travel::QueuedOrder::estimated(order, 60.))
        .collect(),
        ..Default::default()
    };
    state.orders.extend((0..8).map(|_| {
        travel::QueuedOrder::estimated(
            travel::Order::Sublight(travel::Destination::Galactic(destination)),
            60.,
        )
    }));
    let mut text = Vec::new();
    fn collect(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(shape) => text.push(shape.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, text)),
            _ => {}
        }
    }
    for frame in 0..4 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 1000.),
                )),
                time: Some(frame as f64 / 60.),
                ..Default::default()
            },
            |ui| instruments::itinerary(ui, &state, &navigation, 0),
        );
        output.textures_delta.clear();
        text.clear();
        for shape in &output.shapes {
            collect(&shape.shape, &mut text);
            if let egui::Shape::Text(label) = &shape.shape {
                if label.galley.job.text == "12  Transfer · Terminus" {
                    let bounds = label.galley.rect.translate(label.pos.to_vec2());
                    assert!(
                        bounds.min.y > 220.,
                        "route should extend below the former scroll limit"
                    );
                    assert!(
                        shape.clip_rect.contains_rect(bounds),
                        "last stage must remain visible"
                    );
                }
            }
        }
    }
    for expected in [
        "1  Gate · Sol",
        "2  Transfer · Sol",
        "3  Slip · Terminus",
        "4  Transfer · Terminus",
        "12  Transfer · Terminus",
        "ETA ~01:00",
        "ETA ~12:00",
    ] {
        assert!(
            text.iter().any(|label| label == expected),
            "missing {expected}: {text:?}"
        );
    }
}

#[test]
fn planner_warns_when_one_required_tank_is_short_even_with_other_fuel_aboard() {
    let ctx = egui::Context::default();
    toy_sim_ui::theme::install(&ctx);
    let mut ship = super::super::tests::ship(Id([9; 16])).0;
    ship.travel.orders = vec![travel::Order::WaitUntil(100).into()];
    ship.travel.fuel_budget = Some(travel::FuelBudget {
        resources: vec![
            travel::FuelRequirement {
                resource: "water".into(),
                required_kg: 10.,
                available_kg: 9.,
            },
            travel::FuelRequirement {
                resource: "hydrogen".into(),
                required_kg: 10.,
                available_kg: 1000.,
            },
        ],
        complete: false,
    });
    let navigation = NavigationCatalogue::default();
    let model = FrameModel {
        navigation: &navigation,
        ships: vec![&ship],
        rows: vec![],
        ship: Some(&ship),
        details: None,
        system: "Helion".into(),
        vicinity: String::new(),
        connected: true,
        status: "",
        time_ns: 0,
        diagnostics: Default::default(),
        orbits: true,
    };
    let mut text = String::new();
    for _ in 0..3 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                ..Default::default()
            },
            |ui| instruments::fuel_budget(ui, &model),
        );
        output.textures_delta.clear();
        text.clear();
        for shape in output.shapes {
            if let egui::Shape::Text(shape) = shape.shape {
                text.push_str(&shape.galley.job.text);
                text.push('\n');
            }
        }
    }
    assert!(
        text.contains("FUEL EXHAUSTION · water · estimated shortfall 1.0 kg"),
        "{text}"
    );
    assert!(text.contains("Partial estimate"));
    assert!(!text.contains("FUEL EXHAUSTION · hydrogen"));
}
