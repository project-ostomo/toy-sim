use super::*;

#[derive(Default)]
pub(super) struct State {
    consumables: bool,
    selected: Option<String>,
    target: Option<Id>,
    quantity: u64,
    search: String,
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let (Some(ship), Some(details)) = (model.ship, model.details) else {
        ui.weak("Waiting for inventory…");
        return;
    };
    ui.horizontal(|ui| {
        ui.label(Icon::Ship.text(20.).color(ACCENT));
        ui.strong(ship_name(ship));
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.consumables, false, "Cargo hold");
        ui.selectable_value(&mut state.consumables, true, "Consumables");
    });
    ui.separator();
    if state.consumables {
        ui.weak("Installed tanks · unavailable for cargo transfer");
        egui::ScrollArea::vertical()
            .id_salt("consumables")
            .show(ui, |ui| {
                for resource in details
                    .inventory
                    .iter()
                    .filter(|r| r.capacity_kg > 0.0 || r.quantity > 0)
                {
                    meter(
                        ui,
                        &resource.name,
                        resource.amount_kg,
                        resource.capacity_kg,
                        &format!(
                            "{} units · {:.1} / {:.1} kg",
                            resource.quantity, resource.amount_kg, resource.capacity_kg
                        ),
                        ACCENT,
                    );
                }
            });
        return;
    }
    meter(
        ui,
        "Cargo capacity",
        details.cargo_used_m3,
        details.cargo_capacity_m3,
        &format!(
            "{:.2} / {:.2} m³",
            details.cargo_used_m3, details.cargo_capacity_m3
        ),
        ACCENT,
    );
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Filter cargo…")
            .desired_width(f32::INFINITY),
    );
    let search = state.search.to_lowercase();
    let stacks: Vec<_> = details
        .inventory
        .iter()
        .filter(|r| r.cargo_quantity > 0 && r.name.to_lowercase().contains(&search))
        .collect();
    let columns = ((ui.available_width() / 98.0) as usize).max(1);
    egui::ScrollArea::vertical()
        .id_salt("cargo")
        .max_height((ui.available_height() - 120.0).max(90.0))
        .show(ui, |ui| {
            if stacks.is_empty() {
                ui.add_space(20.0);
                ui.weak(if search.is_empty() {
                    "Cargo hold empty"
                } else {
                    "No matching cargo"
                });
            }
            egui::Grid::new("cargo_stacks")
                .num_columns(columns)
                .spacing(egui::vec2(8.0, 8.0))
                .show(ui, |ui| {
                    for (index, resource) in stacks.iter().enumerate() {
                        let selected = state.selected.as_deref() == Some(&resource.resource);
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(88.0, 88.0), egui::Sense::click());
                        let painter = ui.painter();
                        painter.rect_filled(
                            rect,
                            3.0,
                            if selected {
                                egui::Color32::from_rgb(32, 58, 76)
                            } else {
                                SURFACE
                            },
                        );
                        painter.rect_stroke(
                            rect,
                            3.0,
                            egui::Stroke::new(
                                1.0,
                                if selected || response.hovered() {
                                    ACCENT
                                } else {
                                    egui::Color32::from_rgb(43, 52, 65)
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        painter.text(
                            rect.center_top() + egui::vec2(0.0, 25.0),
                            egui::Align2::CENTER_CENTER,
                            Icon::Cargo.glyph(),
                            Icon::font(34.0),
                            ACCENT,
                        );
                        painter.text(
                            rect.right_top() + egui::vec2(-5.0, 4.0),
                            egui::Align2::RIGHT_TOP,
                            resource.cargo_quantity.to_string(),
                            egui::FontId::monospace(12.0),
                            TEXT,
                        );
                        let label = painter.layout(
                            resource.name.clone(),
                            egui::FontId::proportional(11.0),
                            TEXT,
                            78.0,
                        );
                        painter.galley(
                            rect.center_bottom()
                                + egui::vec2(-label.size().x / 2.0, -label.size().y - 5.0),
                            label,
                            TEXT,
                        );
                        if response.clicked() {
                            state.selected = Some(resource.resource.clone());
                            state.quantity = resource.cargo_quantity;
                        }
                        response.on_hover_text(format!(
                            "{}\n{} units · {:.1} kg · {:.3} m³",
                            resource.name,
                            resource.cargo_quantity,
                            resource.cargo_quantity as f64 * resource.unit_mass_kg,
                            resource.cargo_quantity as f64 * resource.unit_volume_m3
                        ));
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
    ui.separator();
    let host = |s: &ShipTelemetry| match s.presence {
        travel::Presence::Docked { host, .. } => Some(host),
        _ => None,
    };
    let targets: Vec<_> = model
        .ships
        .iter()
        .copied()
        .filter(|s| {
            s.ship != ship.ship
                && (host(ship) == Some(s.ship)
                    || host(s) == Some(ship.ship)
                    || (host(ship).is_some() && host(ship) == host(s)))
        })
        .collect();
    if targets.is_empty() {
        ui.weak("Dock alongside another controlled ship to transfer cargo.");
        return;
    }
    if !targets.iter().any(|s| Some(s.ship) == state.target) {
        state.target = Some(targets[0].ship);
    }
    egui::ComboBox::from_id_salt("cargo_destination")
        .selected_text(
            targets
                .iter()
                .find(|s| Some(s.ship) == state.target)
                .map_or_else(String::new, |s| ship_name(s)),
        )
        .show_ui(ui, |ui| {
            for target in &targets {
                ui.selectable_value(&mut state.target, Some(target.ship), ship_name(target));
            }
        });
    if let Some(resource) = details
        .inventory
        .iter()
        .find(|r| Some(&r.resource) == state.selected.as_ref() && r.cargo_quantity > 0)
    {
        state.quantity = state.quantity.clamp(1, resource.cargo_quantity);
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut state.quantity).range(1..=resource.cargo_quantity));
            if ui
                .add_enabled(model.connected, egui::Button::new("Transfer cargo"))
                .clicked()
            {
                intents.push(Intent::Command(
                    ShipCommand::TransferCargo {
                        target: state.target.unwrap(),
                        resource: resource.resource.clone(),
                        quantity: state.quantity,
                    },
                    "Transfer cargo",
                ));
            }
        });
    }
}
