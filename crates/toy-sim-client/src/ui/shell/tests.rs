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
    let selection = Selection::default();
    let navigation = NavigationCatalogue::default();
    let model = FrameModel {
        navigation: &navigation,
        ships: vec![],
        rows: vec![row(2, 1000.)],
        ship: None,
        details: None,
        system: "Helion".into(),
        vicinity: "Neris".into(),
        connected: true,
        status: "",
        time_ns: 0,
        orbits: true,
    };
    let mut previous = None;
    for frame in 0..60 {
        let size = if frame < 30 {
            egui::vec2(1150., 646.)
        } else {
            egui::vec2(958., 598.)
        };
        if frame == 30 {
            previous = None;
        }
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
        if frame % 30 > 3 {
            let selected = shell.desktop.rect(SELECTED).unwrap();
            let overview = shell.desktop.rect(OVERVIEW).unwrap();
            assert!(
                selected.bottom() <= overview.top(),
                "selected {selected:?} overlaps {overview:?}"
            );
            assert!(
                workspace(egui::Rect::from_min_size(egui::Pos2::ZERO, size))
                    .contains_rect(overview)
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
