use super::*;
use industry_model::{CargoItem, CargoStack, FacilityView, IndustryCommand};

#[derive(Default)]
pub(super) struct State {
    consumables: bool,
    source: Option<Id>,
    target: Option<Id>,
    selected: Option<(Id, CargoItem)>,
    quantity: u64,
    search: String,
}

#[derive(Clone)]
struct DraggedCargo {
    source: Id,
    item: CargoItem,
}

impl State {
    pub fn focus(&mut self, inventory: Id) {
        self.source = Some(inventory);
        self.selected = None;
        self.consumables = false;
    }

    pub fn inventories(&self) -> impl Iterator<Item = Id> {
        [self.source, self.target].into_iter().flatten()
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    state.source = state.source.or(model.ship.map(|ship| ship.ship));
    if let Some(error) = &model.industry.error {
        ui.colored_label(THREAT, error);
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.consumables, false, "Cargo holds");
        ui.selectable_value(&mut state.consumables, true, "Consumables");
    });
    if state.consumables {
        consumables(ui, state, model, intents);
        return;
    }

    egui::Panel::bottom(ui.id().with("cargo_actions"))
        .frame(egui::Frame::NONE)
        .show_separator_line(false)
        .show(ui, |ui| transfer_controls(ui, state, model, intents));
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Filter resources and part kits…")
            .desired_width(f32::INFINITY),
    );
    ui.columns(2, |columns| {
        for (index, ui) in columns.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                let selected = if index == 0 {
                    &mut state.source
                } else {
                    &mut state.target
                };
                picker(
                    ui,
                    selected,
                    model,
                    if index == 0 {
                        "Source inventory"
                    } else {
                        "Destination inventory"
                    },
                );
                let selected = *selected;
                let facility = selected.and_then(|id| facility(model, id));
                let Some(facility) = facility else {
                    ui.weak(
                        if selected
                            .is_some_and(|id| model.industry.omitted_inventories.contains(&id))
                        {
                            "Inventory exceeds the publication limit; select fewer inventories."
                        } else if selected.is_some() {
                            "Loading inventory…"
                        } else {
                            "Select an inventory"
                        },
                    );
                    return;
                };
                ui.small(format!(
                    "{:.1} / {:.1} m³",
                    facility.cargo_used_m3, facility.cargo_capacity_m3
                ));
                if !facility.can_transfer {
                    ui.colored_label(MUTED, "VIEW ONLY");
                }
                let search = state.search.to_lowercase();
                let height = ui.available_height().max(0.0);
                let response = egui::ScrollArea::vertical()
                    .id_salt("cargo_items")
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.0)
                    .max_height(height)
                    .show(ui, |ui| {
                        let stacks: Vec<_> = facility
                            .items
                            .iter()
                            .filter(|stack| {
                                stack.quantity > 0 && stack.name.to_lowercase().contains(&search)
                            })
                            .collect();
                        if stacks.is_empty() {
                            ui.weak("Cargo hold empty");
                        }
                        let columns = (ui.available_width() / 96.0).floor().max(1.0) as usize;
                        egui::Grid::new("cargo_grid")
                            .spacing(egui::vec2(6.0, 6.0))
                            .show(ui, |ui| {
                                for (index, stack) in stacks.iter().enumerate() {
                                    cargo_tile(ui, state, facility, stack);
                                    if (index + 1) % columns == 0 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
                let drop_response = ui.interact(
                    response.inner_rect,
                    ui.id().with("cargo_drop"),
                    egui::Sense::hover(),
                );
                if let Some(dragged) = drop_response.dnd_release_payload::<DraggedCargo>() {
                    state.selected = Some((dragged.source, dragged.item.clone()));
                    state.target = Some(facility.entity);
                    state.source = Some(dragged.source);
                    state.quantity =
                        available_stack(model, dragged.source, &dragged.item).map_or(0, available);
                }
            });
        }
    });
}

pub(super) fn facility<'a>(model: &'a FrameModel<'_>, id: Id) -> Option<&'a FacilityView> {
    model
        .industry
        .facilities
        .iter()
        .find(|facility| facility.entity == id)
}

fn available_stack<'a>(
    model: &'a FrameModel,
    source: Id,
    item: &CargoItem,
) -> Option<&'a CargoStack> {
    facility(model, source)?
        .items
        .iter()
        .find(|stack| &stack.item == item)
}

pub(super) fn available(stack: &CargoStack) -> u64 {
    stack.quantity.saturating_sub(stack.reserved)
}

pub(super) fn quantity_label(item: &CargoItem, quantity: u64, unit_mass_kg: f64) -> String {
    if matches!(item, CargoItem::Resource(_)) && unit_mass_kg < 1e-5 {
        toy_sim_ui::units::mass(quantity as f64 * unit_mass_kg)
    } else {
        quantity.to_string()
    }
}

pub(super) fn colocated(source: &FacilityView, target: &FacilityView) -> bool {
    source.entity != target.entity
        && ((source.location.is_some() && source.location == target.location)
            || source.location == Some(target.entity)
            || target.location == Some(source.entity))
}

pub(super) fn transfer_error(
    source: &FacilityView,
    target: &FacilityView,
    stack: &CargoStack,
    quantity: u64,
) -> Option<&'static str> {
    if !source.can_transfer || !target.can_transfer {
        Some("Cargo transfer permission is required on both inventories")
    } else if !colocated(source, target) {
        Some("Inventories must be physically colocated at the same station")
    } else if quantity == 0 || quantity > available(stack) {
        Some("Choose an available quantity; reserved items cannot be moved")
    } else if quantity as f64 * stack.unit_volume_m3
        > (target.cargo_capacity_m3 - target.cargo_used_m3).max(0.0) + 1e-9
    {
        Some("The destination does not have enough cargo space")
    } else {
        None
    }
}

fn transfer_controls(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    ui.separator();
    let Some((source, item)) = state.selected.as_ref() else {
        ui.weak("Select or drag cargo to the other inventory, then confirm the quantity.");
        return;
    };
    let source_id = *source;
    let Some(stack) = available_stack(model, source_id, item) else {
        ui.weak("Loading selected cargo…");
        return;
    };
    let maximum = available(stack);
    state.quantity = state.quantity.min(maximum);
    ui.horizontal(|ui| {
        ui.label(&stack.name);
        if matches!(stack.item, CargoItem::Resource(_)) && stack.unit_mass_kg < 1e-5 {
            let mut mass_kg = state.quantity as f64 * stack.unit_mass_kg;
            if ui
                .add(
                    egui::DragValue::new(&mut mass_kg)
                        .range(0.0..=maximum as f64 * stack.unit_mass_kg)
                        .speed(0.1)
                        .max_decimals(6)
                        .suffix(" kg"),
                )
                .changed()
            {
                state.quantity = quantity_from_mass(mass_kg, stack.unit_mass_kg, maximum);
            }
        } else {
            ui.add(
                egui::DragValue::new(&mut state.quantity)
                    .range(0..=maximum)
                    .suffix(" units"),
            );
        }
        if ui.small_button("All").clicked() {
            state.quantity = maximum;
        }
    });
    let pair = facility(model, source_id).zip(state.target.and_then(|id| facility(model, id)));
    let error = pair.map_or(
        Some("Select a destination inventory"),
        |(source, target)| transfer_error(source, target, stack, state.quantity),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                model.connected && error.is_none(),
                egui::Button::new("Transfer cargo"),
            )
            .clicked()
        {
            intents.push(Intent::Industry(
                IndustryCommand::Transfer {
                    source: source_id,
                    target: state.target.unwrap(),
                    item: item.clone(),
                    quantity: state.quantity,
                },
                "Transfer cargo",
            ));
        }
        if let Some(error) = error {
            ui.weak(error);
        }
    });
}

pub(super) fn quantity_from_mass(mass_kg: f64, unit_mass_kg: f64, maximum: u64) -> u64 {
    if !mass_kg.is_finite() || !unit_mass_kg.is_finite() || unit_mass_kg <= 0.0 {
        return 0;
    }
    ((mass_kg.max(0.0) / unit_mass_kg).round() as u64).min(maximum)
}

fn cargo_tile(ui: &mut egui::Ui, state: &mut State, source: &FacilityView, stack: &CargoStack) {
    let selected = state
        .selected
        .as_ref()
        .is_some_and(|(id, item)| *id == source.entity && item == &stack.item);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(88.0, 88.0), egui::Sense::click_and_drag());
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
                BORDER
            },
        ),
        egui::StrokeKind::Inside,
    );
    let icon = match stack.item {
        CargoItem::Resource(_) => Icon::Cargo,
        CargoItem::Part(_) => Icon::Settings,
    };
    painter.text(
        rect.center_top() + egui::vec2(0.0, 27.0),
        egui::Align2::CENTER_CENTER,
        icon.glyph(),
        Icon::font(30.0),
        ACCENT,
    );
    painter.text(
        rect.right_top() + egui::vec2(-4.0, 4.0),
        egui::Align2::RIGHT_TOP,
        quantity_label(&stack.item, stack.quantity, stack.unit_mass_kg),
        egui::FontId::monospace(11.0),
        TEXT,
    );
    let label = painter.layout(
        stack.name.clone(),
        egui::FontId::proportional(11.0),
        TEXT,
        78.0,
    );
    painter.galley(
        rect.center_bottom() + egui::vec2(-label.size().x / 2.0, -label.size().y - 5.0),
        label,
        TEXT,
    );
    if response.clicked() {
        state.selected = Some((source.entity, stack.item.clone()));
        state.quantity = available(stack);
    }
    if source.can_transfer && available(stack) > 0 {
        response.dnd_set_drag_payload(DraggedCargo {
            source: source.entity,
            item: stack.item.clone(),
        });
    }
    response.on_hover_text(format!(
        "{}\n{} available · {} reserved for jobs\n{:.1} kg · {:.3} m³",
        stack.name,
        available(stack),
        stack.reserved,
        stack.quantity as f64 * stack.unit_mass_kg,
        stack.quantity as f64 * stack.unit_volume_m3
    ));
}

pub(super) fn picker(ui: &mut egui::Ui, selected: &mut Option<Id>, model: &FrameModel, hint: &str) {
    let mut options: std::collections::BTreeMap<Id, String> = model
        .industry
        .directory
        .iter()
        .map(|facility| (facility.entity, facility.name.clone()))
        .collect();
    options.extend(
        model
            .industry
            .facilities
            .iter()
            .map(|facility| (facility.entity, facility.name.clone())),
    );
    options.extend(model.ships.iter().map(|ship| (ship.ship, ship_name(ship))));
    egui::ComboBox::from_id_salt(hint)
        .selected_text(
            selected
                .and_then(|id| options.get(&id))
                .map_or(hint, String::as_str),
        )
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (id, name) in options {
                ui.selectable_value(selected, Some(id), name);
            }
        });
}

fn consumables(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let (Some(ship), Some(details)) = (model.ship, model.details) else {
        ui.weak("Select a controlled ship to inspect its installed tanks.");
        return;
    };
    let height = ui.available_height().max(0.0);
    egui::ScrollArea::vertical()
        .id_salt("consumables")
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_height(height)
        .show(ui, |ui| {
            consumables_body(ui, state, model, ship, details, intents);
        });
}

fn consumables_body(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    ship: &ShipTelemetry,
    details: &ShipPresentation,
    intents: &mut Vec<Intent>,
) {
    ui.strong(ship_name(ship));
    ui.small(format!(
        "Flight computer: {}",
        computer_status(&details.computer)
    ));

    let mut iff = ship.iff.enabled;
    if ui
        .checkbox(&mut iff, "Broadcast IFF / transponder")
        .changed()
    {
        intents.push(Intent::Command(
            ShipCommand::SetTransponderEnabled(iff),
            "Transponder",
        ));
    }

    if matches!(ship.presence, travel::Presence::Docked { .. }) {
        let mut power = ship.dock_services.power;
        if ui
            .checkbox(&mut power, "Request dock power / charge batteries")
            .changed()
        {
            intents.push(Intent::Command(
                ShipCommand::SetDockServices {
                    cargo: ship.dock_services.cargo,
                    power,
                },
                "Dock power",
            ));
        }
    }

    if details.battery_capacity_j > 0 {
        let fraction = ship.battery_j as f64 / details.battery_capacity_j as f64;
        meter(
            ui,
            "Battery",
            ship.battery_j as f64,
            details.battery_capacity_j as f64,
            &format!("{} / {} J", ship.battery_j, details.battery_capacity_j),
            toy_sim_ui::gauges::Tone::Reserve.color(fraction),
        );
    }

    ui.separator();
    ui.weak(
        "Refill from authorized colocated cargo. Installed consumables cannot be transferred out.",
    );
    picker(ui, &mut state.source, model, "Refill source");

    let source = state.source.and_then(|id| facility(model, id));
    let target = facility(model, ship.ship);
    let reserve_capacity = details
        .health
        .as_ref()
        .map_or(0.0, |health| health.shield_reserve_capacity_kg);
    if reserve_capacity > 0.0 {
        meter(
            ui,
            "Shield reserve",
            ship.coolant_reserve_kg,
            reserve_capacity,
            &format!(
                "{:.1} / {:.1} kg",
                ship.coolant_reserve_kg, reserve_capacity
            ),
            toy_sim_ui::gauges::Tone::Reserve.color(ship.coolant_reserve_kg / reserve_capacity),
        );
        refill_control(
            ui,
            model.connected,
            source,
            target,
            ship.ship,
            "shield_coolant",
            reserve_capacity - ship.coolant_reserve_kg,
            1.0,
            intents,
        );
    }

    for resource in details
        .inventory
        .iter()
        .filter(|resource| resource.capacity_kg > 0.0)
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
            toy_sim_ui::gauges::Tone::Reserve.color(resource.amount_kg / resource.capacity_kg),
        );
        ui.push_id(&resource.resource, |ui| {
            refill_control(
                ui,
                model.connected,
                source,
                target,
                ship.ship,
                &resource.resource,
                resource.capacity_kg - resource.amount_kg,
                resource.unit_mass_kg,
                intents,
            );
        });
        ui.add_space(5.0);
    }
}

fn refill_control(
    ui: &mut egui::Ui,
    connected: bool,
    source: Option<&FacilityView>,
    target: Option<&FacilityView>,
    ship: Id,
    resource: &str,
    missing_kg: f64,
    unit_mass_kg: f64,
    intents: &mut Vec<Intent>,
) {
    let item = CargoItem::Resource(resource.into());
    let stack = source.and_then(|source| source.items.iter().find(|stack| stack.item == item));
    let missing = if unit_mass_kg > 0.0 {
        (missing_kg.max(0.0) / unit_mass_kg).floor() as u64
    } else {
        0
    };
    let quantity = stack.map_or(0, available).min(missing);

    let permitted = source.zip(target).is_some_and(|(source, target)| {
        source.can_transfer
            && target.can_transfer
            && (source.entity == target.entity || colocated(source, target))
    });

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                connected && permitted && quantity > 0,
                egui::Button::new("Fill tank"),
            )
            .clicked()
        {
            intents.push(Intent::Industry(
                IndustryCommand::Refill {
                    source: source.unwrap().entity,
                    ship,
                    resource: resource.into(),
                    quantity,
                },
                "Refill tank",
            ));
        }

        let message = if quantity == 0 {
            "No available cargo or tank already full".into()
        } else if !permitted {
            "Source must be authorized and physically colocated".into()
        } else {
            format!("Load {}", quantity_label(&item, quantity, unit_mass_kg))
        };
        ui.small(message);
    });
}

pub(super) fn hangar_selector(ui: &mut egui::Ui, model: &FrameModel, intents: &mut Vec<Intent>) {
    let Some(ship) = model.ship else {
        return;
    };
    let travel::Presence::Docked { host, .. } = ship.presence else {
        return;
    };
    egui::ComboBox::from_id_salt("hangar_ship")
        .selected_text(ship_name(ship))
        .show_ui(ui, |ui| {
            let hulls = model.ships.iter().filter(|hull| {
                matches!(hull.presence, travel::Presence::Docked { host: other, .. } if other == host)
            });
            for hull in hulls {
                if ui.selectable_label(hull.ship == ship.ship, ship_name(hull)).clicked() {
                    intents.push(Intent::FocusShip(hull.ship));
                }
            }
        });
}
