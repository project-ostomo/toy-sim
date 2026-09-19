use super::*;

pub(super) fn selected_item(
    ui: &mut egui::Ui,
    row: Option<&Row>,
    can_control: bool,
    weapons: Option<&WeaponsInstrument>,
    rows: &[Row],
    stand_off: &mut f64,
    intents: &mut Vec<Intent>,
) {
    ui.horizontal(|ui| {
        ui.label(
            row.map_or(Icon::Target, Row::icon)
                .text(24.)
                .color(row.map_or(ACCENT, |row| super::super::standing::color(row.standing))),
        );
        ui.vertical(|ui| {
            let name = row.map_or("No object selected", |row| row.name.as_str());
            ui.add(egui::Label::new(egui::RichText::new(name).size(16.).strong()).truncate())
                .on_hover_text(name);
            ui.label(
                egui::RichText::new(row.map_or_else(
                    || "Select in the Overview or on the HUD".into(),
                    |row| format!("{}  ·  {}", row.kind, distance(row.distance)),
                ))
                .size(12.)
                .color(MUTED),
            );
        });
    });
    if let Some(row) = row.filter(|row| matches!(row.target, SelectedTarget::Contact(_))) {
        ui.colored_label(
            super::super::standing::color(row.standing),
            format!(
                "{} {}",
                super::super::standing::symbol(row.standing),
                super::super::standing::label(row.standing)
            ),
        )
        .on_hover_text("Standing follows the identity broadcast by IFF.");
    }
    ui.separator();
    if let Some(principal) = row.and_then(|row| row.affiliation) {
        if ui.small_button("Show affiliation").clicked() {
            intents.push(Intent::InspectAffiliation(principal));
        }
    }
    let target = row.map(|row| row.target);
    let contact = target.and_then(|target| match target {
        SelectedTarget::Contact(reference) => Some(reference),
        _ => None,
    });
    let marked = weapons.and_then(|weapons| weapons.target);
    let firing = weapons.is_some_and(|weapons| weapons.firing);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        if action_button(
            ui,
            Icon::Align,
            "Align",
            can_control && target.is_some(),
            "Point the ship toward the selected object",
        )
        .clicked()
        {
            intents.push(Intent::Align(target.unwrap()));
        }
        if action_button(
            ui,
            Icon::Approach,
            "Approach",
            can_control && contact.is_some(),
            "Approach the contact with a safe stand-off",
        )
        .clicked()
        {
            intents.push(Intent::Approach(contact.unwrap(), 100.));
        }
        if action_button(
            ui,
            Icon::KeepRange,
            "Keep range",
            can_control && contact.is_some(),
            "Navigate to the stand-off range below",
        )
        .clicked()
        {
            intents.push(Intent::KeepRange(contact.unwrap(), *stand_off));
        }
        if action_button(
            ui,
            Icon::Look,
            "Look at",
            row.is_some_and(|row| row.can_look),
            "Center the camera on a visible object within 100 km",
        )
        .clicked()
        {
            intents.push(Intent::Look(target));
        }
        if contact.is_some() && contact == marked {
            if action_button(
                ui,
                Icon::Target,
                "Unmark",
                can_control,
                "Clear the marked target and stop firing",
            )
            .clicked()
            {
                intents.push(Intent::Command(ShipCommand::UnmarkTarget, "Unmark target"));
            }
        } else if action_button(
            ui,
            Icon::Target,
            "Mark",
            can_control && contact.is_some(),
            "Mark the selected contact for weapons tracking; firing starts separately",
        )
        .clicked()
        {
            let reference = contact.unwrap();
            intents.push(Intent::Command(
                ShipCommand::MarkTarget {
                    group: reference.group,
                    track: reference.track,
                    maximum_flight_time_s: 30.,
                },
                "Mark target",
            ));
        }
        if firing {
            if action_button(
                ui,
                Icon::Stop,
                "Hold fire",
                can_control,
                "Stop firing while retaining the marked target",
            )
            .clicked()
            {
                intents.push(Intent::Command(ShipCommand::StopFiring, "Stop firing"));
            }
        } else if action_button(
            ui,
            Icon::Play,
            "Fire",
            can_control && marked.is_some(),
            "Enable automatic fire at the marked target",
        )
        .clicked()
        {
            intents.push(Intent::Command(ShipCommand::StartFiring, "Start firing"));
        }
    });
    if let Some(SelectedTarget::Beacon(id)) = target {
        ui.horizontal(|ui| {
            let append = ui.input(|i| i.modifiers.shift);
            if ui
                .add_enabled(can_control, egui::Button::new("Approach"))
                .clicked()
            {
                intents.push(Intent::Queue(
                    vec![travel::Order::Guidance(travel::Guidance {
                        mode: travel::GuidanceMode::Approach,
                        target: travel::Target::Destination(travel::Destination::Beacon(id)),
                        range_m: row.map_or(100., |row| row.radius + 100.),
                    })],
                    append,
                ));
            }
            let gate = row.is_some_and(|r| r.kind == "Stargate");
            if ui
                .add_enabled(
                    can_control,
                    egui::Button::new(if gate { "Jump" } else { "Dock" }),
                )
                .clicked()
            {
                intents.push(Intent::Queue(
                    vec![if gate {
                        travel::Order::Jump(id)
                    } else {
                        travel::Order::Dock(id)
                    }],
                    append,
                ));
            }
            ui.weak("Shift: add to queue");
        });
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Stand-off").size(11.).color(MUTED));
        ui.add(
            egui::DragValue::new(stand_off)
                .speed(100.)
                .range(100. ..=1e6)
                .suffix(" m"),
        );
        if ui
            .add_enabled(can_control, egui::Button::new("Stop").small())
            .on_hover_text("Stop automatic guidance; the ship retains its velocity")
            .clicked()
        {
            intents.push(Intent::Command(
                ShipCommand::SetAutopilot(false),
                "Stop guidance",
            ));
        }
    });
    ui.separator();
    let marked_name = marked.map(|target| {
        rows.iter()
            .find(|row| row.target == SelectedTarget::Contact(target))
            .map_or("Contact outside this view", |row| row.name.as_str())
    });
    ui.horizontal(|ui| {
        ui.label(
            Icon::Target
                .text(16.)
                .color(if marked.is_some() { THREAT } else { ACCENT }),
        );
        ui.label(
            egui::RichText::new(if weapons.is_none() {
                "AWAITING TELEMETRY"
            } else if firing {
                "FIRING ENABLED"
            } else {
                "FIRE STOPPED"
            })
            .size(11.)
            .color(if firing { ACCENT } else { MUTED }),
        );
        ui.add(
            egui::Label::new(egui::RichText::new(marked_name.unwrap_or(
                if weapons.is_some() {
                    "No marked target"
                } else {
                    "Weapons status unavailable"
                },
            )))
            .truncate(),
        )
        .on_hover_text(weapons.map_or("", |weapons| weapons.reason.as_str()));
    });
}

pub(super) fn overview_header(ui: &mut egui::Ui, sort: &mut Sort, descending: &mut bool) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.), egui::Sense::hover());
    let widths = [0.07, 0.43, 0.61, 0.84, 1.];
    for (i, (value, label)) in [
        (Sort::Name, "Name"),
        (Sort::Kind, "Type"),
        (Sort::Distance, "Distance"),
        (Sort::Speed, "m/s"),
    ]
    .into_iter()
    .enumerate()
    {
        let cell = egui::Rect::from_min_max(
            egui::pos2(rect.left() + widths[i] * rect.width(), rect.top()),
            egui::pos2(rect.left() + widths[i + 1] * rect.width(), rect.bottom()),
        );
        let response = ui.interact(cell, ui.id().with(("sort", i)), egui::Sense::click());
        if response.clicked() {
            if *sort == value {
                *descending = !*descending;
            } else {
                *sort = value;
                *descending = false;
            }
        }
        let label = if *sort == value {
            format!("{label} {}", if *descending { "↓" } else { "↑" })
        } else {
            label.into()
        };
        ui.painter().text(
            cell.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(10.),
            if *sort == value { ACCENT } else { MUTED },
        );
    }
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1., BORDER));
}

pub(super) fn overview_row(
    ui: &mut egui::Ui,
    row: &Row,
    selected: bool,
    targeted: bool,
    index: usize,
) -> egui::Response {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.), egui::Sense::hover());
    let response = ui.interact(rect, ui.id().with(row.key()), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &row.name)
    });
    let fill = if selected {
        egui::Color32::from_rgb(37, 69, 84)
    } else if response.hovered() {
        egui::Color32::from_rgb(31, 46, 60)
    } else if index % 2 == 0 {
        egui::Color32::from_rgb(20, 29, 39)
    } else {
        SURFACE
    };
    ui.painter().rect_filled(rect, 0., fill);
    if selected {
        ui.painter()
            .vline(rect.left(), rect.y_range(), egui::Stroke::new(2., ACCENT));
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(11., 0.),
        egui::Align2::CENTER_CENTER,
        row.icon().glyph(),
        Icon::font(14.),
        super::super::standing::color(row.standing),
    );
    if targeted {
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(
                rect.left_center() + egui::vec2(11., 0.),
                egui::vec2(19., 19.),
            ),
            0.,
            egui::Stroke::new(1., THREAT),
            egui::StrokeKind::Inside,
        );
    }
    for (start, end, text) in [
        (0.07, 0.43, row.name.clone()),
        (0.43, 0.61, row.kind.clone()),
        (0.61, 0.84, distance(row.distance)),
        (0.84, 1., format!("{:.0}", row.speed)),
    ] {
        let cell = egui::Rect::from_min_max(
            egui::pos2(rect.left() + rect.width() * start, rect.top()),
            egui::pos2(rect.left() + rect.width() * end - 5., rect.bottom()),
        );
        let galley = ui.painter().layout(
            text,
            egui::FontId::proportional(11.),
            if start == 0.07 {
                super::super::standing::color(row.standing)
            } else {
                TEXT
            },
            f32::INFINITY,
        );
        ui.painter()
            .with_clip_rect(cell.intersect(ui.clip_rect()))
            .galley(
                cell.left_center() - egui::vec2(0., galley.size().y / 2.),
                galley,
                TEXT,
            );
    }
    response.on_hover_text(format!(
        "{}\n{} · {}\n{} · {}\nRelative speed: {:.1} m/s",
        row.name,
        row.kind,
        distance(row.distance),
        super::super::standing::label(row.standing),
        row.detail,
        row.speed
    ))
}
