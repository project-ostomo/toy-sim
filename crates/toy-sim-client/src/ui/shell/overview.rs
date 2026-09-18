use super::*;

pub(super) fn selected_item(
    ui: &mut egui::Ui,
    row: Option<&Row>,
    can_control: bool,
    stand_off: &mut f64,
    intents: &mut Vec<Intent>,
) {
    ui.horizontal(|ui| {
        ui.label(row.map_or(Icon::Target, Row::icon).text(24.).color(ACCENT));
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
    ui.separator();
    let target = row.map(|row| row.target);
    let contact = target.and_then(|target| match target {
        SelectedTarget::Contact(reference) => Some(reference),
        _ => None,
    });
    ui.horizontal(|ui| {
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
            target.is_some(),
            "Center the camera on this object",
        )
        .clicked()
        {
            intents.push(Intent::Look(target));
        }
        if action_button(
            ui,
            Icon::Target,
            "Engage",
            can_control && contact.is_some(),
            "Order the ship's weapons to engage this contact",
        )
        .clicked()
        {
            intents.push(Intent::Engage(contact.unwrap()));
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
                    vec![travel::Order::TravelTo(travel::Destination::Beacon(id))],
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
            .add_enabled(can_control, egui::Button::new("Hold fire").small())
            .clicked()
        {
            intents.push(Intent::Command(ShipCommand::HoldFire, "Hold fire"));
        }
        if ui
            .add_enabled(can_control, egui::Button::new("Stop").small())
            .on_hover_text("Stop automatic guidance; the ship retains its velocity")
            .clicked()
        {
            intents.push(Intent::Command(ShipCommand::PauseTravel, "Stop guidance"));
        }
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
        ACCENT,
    );
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
        let galley =
            ui.painter()
                .layout(text, egui::FontId::proportional(11.), TEXT, f32::INFINITY);
        ui.painter()
            .with_clip_rect(cell.intersect(ui.clip_rect()))
            .galley(
                cell.left_center() - egui::vec2(0., galley.size().y / 2.),
                galley,
                TEXT,
            );
    }
    response.on_hover_text(format!(
        "{}\n{} · {}\n{}\nRelative speed: {:.1} m/s",
        row.name,
        row.kind,
        distance(row.distance),
        row.detail,
        row.speed
    ))
}
