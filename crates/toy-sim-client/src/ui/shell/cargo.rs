use super::*;
use industry_model::{CargoItem, CargoStack, FacilityView, IndustryCommand};

#[derive(Default)]
pub(super) struct PaneState {
    search: String,
    selected: Option<(Storage, CargoItem)>,
}

#[derive(Clone)]
struct DraggedCargo {
    source: Id,
    storage: Storage,
    item: CargoItem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Storage {
    Cargo,
    Product,
}

struct Transfer {
    cargo: DraggedCargo,
    target: Id,
    tank: Option<String>,
    quantity: u64,
}

#[derive(Default)]
pub(super) struct Transfers {
    pending: Option<Transfer>,
    dragging: Option<Id>,
    feedback: Option<(Id, String)>,
}

impl Transfers {
    pub fn inventories(&self) -> impl Iterator<Item = Id> {
        let endpoints = self
            .pending
            .as_ref()
            .map(|pending| [pending.cargo.source, pending.target]);
        self.dragging
            .into_iter()
            .chain(endpoints.into_iter().flatten())
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut PaneState,
    inventory: Id,
    model: &FrameModel,
    transfers: &mut Transfers,
    intents: &mut Vec<Intent>,
) {
    let Some(facility) = facility(model, inventory) else {
        ui.weak(if model.industry.omitted_inventories.contains(&inventory) {
            "Inventory exceeds the publication limit; close other inventory windows."
        } else if model.industry_ready {
            "Inventory unavailable or access revoked."
        } else {
            "Loading inventory…"
        });
        return;
    };

    ui.strong(&facility.name);
    ui.small(format!(
        "{:.1} / {:.1} m³",
        facility.cargo_used_m3, facility.cargo_capacity_m3
    ));
    if !facility.can_transfer {
        ui.colored_label(MUTED, "VIEW ONLY");
    }
    if let Some(error) = &model.industry.error {
        ui.colored_label(THREAT, error);
    }
    feedback(ui, transfers, inventory);
    ui.weak("Drag between cargo windows · Shift-drag to choose quantity");
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Filter resources and part kits…")
            .desired_width(f32::INFINITY),
    );

    let search = state.search.to_lowercase();
    let bounds = ui.available_rect_before_wrap();
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(bounds), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(bounds));
            egui::ScrollArea::vertical()
                .id_salt(("cargo_items", inventory))
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .max_height(bounds.height().max(0.0))
                .show(ui, |ui| {
                    if facility.items.is_empty() {
                        ui.weak("Cargo hold empty");
                    }
                    cargo_grid(ui, state, facility, Storage::Cargo, &search);

                    if !facility.products.is_empty() {
                        ui.separator();
                        ui.small("REACTOR PRODUCTS · Installed reservoirs");
                        cargo_grid(ui, state, facility, Storage::Product, &search);
                    }
                })
        })
        .inner;
    let drop = ui.interact(
        response.inner_rect,
        ui.id().with("cargo_drop"),
        egui::Sense::hover(),
    );
    drop_target(ui, drop, inventory, None, model, transfers, intents);
}

pub(super) fn feedback(ui: &mut egui::Ui, transfers: &Transfers, inventory: Id) {
    if let Some((target, message)) = &transfers.feedback {
        if *target == inventory {
            ui.colored_label(THREAT, message);
        }
    }
}

pub(super) fn tank_drop(
    ui: &mut egui::Ui,
    response: egui::Response,
    ship: Id,
    resource: &str,
    model: &FrameModel,
    transfers: &mut Transfers,
    intents: &mut Vec<Intent>,
) {
    drop_target(
        ui,
        response,
        ship,
        Some(resource),
        model,
        transfers,
        intents,
    );
}

fn drop_target(
    ui: &mut egui::Ui,
    response: egui::Response,
    target: Id,
    tank: Option<&str>,
    model: &FrameModel,
    transfers: &mut Transfers,
    intents: &mut Vec<Intent>,
) {
    if let Some(cargo) = response.dnd_hover_payload::<DraggedCargo>() {
        let mut pending = Transfer {
            cargo: (*cargo).clone(),
            target,
            tank: tank.map(str::to_owned),
            quantity: 0,
        };
        pending.quantity = maximum(model, &pending);
        let error = transfer_problem(model, &pending);
        ui.painter().rect_stroke(
            response.rect,
            2.0,
            egui::Stroke::new(1.0, if error.is_some() { THREAT } else { ACCENT }),
            egui::StrokeKind::Inside,
        );
        if let Some(error) = error {
            response.clone().on_hover_text(error);
        }
    }

    if let Some(cargo) = response.dnd_release_payload::<DraggedCargo>() {
        let mut pending = Transfer {
            cargo: (*cargo).clone(),
            target,
            tank: tank.map(str::to_owned),
            quantity: 0,
        };
        pending.quantity = maximum(model, &pending);
        transfers.feedback = None;
        if ui.input(|input| input.modifiers.shift) {
            transfers.pending = Some(pending);
        } else if let Some(error) = submit(model, &pending, intents) {
            transfers.feedback = Some((target, error.into()));
        }
    }
}

fn maximum(model: &FrameModel, pending: &Transfer) -> u64 {
    let cargo = &pending.cargo;
    let available =
        available_stack(model, cargo.source, cargo.storage, &cargo.item).map_or(0, available);
    pending.tank.as_deref().map_or(available, |resource| {
        available.min(tank_remaining(model, pending.target, resource).unwrap_or(0))
    })
}

fn transfer_problem(model: &FrameModel, pending: &Transfer) -> Option<&'static str> {
    if !model.connected {
        return Some("Connection unavailable");
    }

    let cargo = &pending.cargo;
    let Some(source) = facility(model, cargo.source) else {
        return Some("Source inventory is no longer available");
    };
    let Some(target) = facility(model, pending.target) else {
        return Some("Destination inventory is no longer available");
    };
    let Some(stack) = available_stack(model, cargo.source, cargo.storage, &cargo.item) else {
        return Some("Selected stock is no longer available");
    };

    if let Some(resource) = &pending.tank {
        if cargo.storage != Storage::Cargo || cargo.item != CargoItem::Resource(resource.clone()) {
            return Some(
                "Drop matching cargo onto this tank; installed consumables cannot be moved",
            );
        }
        if !source.can_transfer || !target.can_transfer {
            return Some("Cargo transfer permission is required on both inventories");
        }
        if source.entity != target.entity && !colocated(source, target) {
            return Some("Inventories must be physically colocated at the same station");
        }
        if pending.quantity == 0 || pending.quantity > available(stack) {
            return Some("Choose an available quantity; reserved items cannot be moved");
        }
        if tank_remaining(model, target.entity, resource).is_none_or(|room| pending.quantity > room)
        {
            return Some("The installed tank does not have enough space");
        }
        None
    } else {
        transfer_error(source, target, stack, pending.quantity, cargo.storage)
    }
}

fn submit(
    model: &FrameModel,
    pending: &Transfer,
    intents: &mut Vec<Intent>,
) -> Option<&'static str> {
    if let Some(error) = transfer_problem(model, pending) {
        return Some(error);
    }

    let cargo = &pending.cargo;
    let (command, label) = if let Some(resource) = &pending.tank {
        (
            IndustryCommand::Refill {
                source: cargo.source,
                ship: pending.target,
                resource: resource.clone(),
                quantity: pending.quantity,
            },
            "Refill tank",
        )
    } else {
        match (&cargo.storage, &cargo.item) {
            (Storage::Cargo, item) => (
                IndustryCommand::Transfer {
                    source: cargo.source,
                    target: pending.target,
                    item: item.clone(),
                    quantity: pending.quantity,
                },
                "Transfer cargo",
            ),
            (Storage::Product, CargoItem::Resource(resource)) => (
                IndustryCommand::UnloadProduct {
                    source: cargo.source,
                    target: pending.target,
                    resource: resource.clone(),
                    quantity: pending.quantity,
                },
                "Unload product",
            ),
            (Storage::Product, CargoItem::Part(_)) => return Some("Invalid product stock"),
        }
    };
    intents.push(Intent::Industry(command, label));
    None
}

pub(super) fn tank_remaining(model: &FrameModel, ship: Id, resource: &str) -> Option<u64> {
    if model.ship?.ship != ship {
        return None;
    }

    let details = model.details?;
    let (missing_kg, unit_mass) = if resource == "shield_coolant" {
        (
            details.health.as_ref()?.shield_reserve_capacity_kg - model.ship?.coolant_reserve_kg,
            1.0,
        )
    } else {
        let resource = details
            .inventory
            .iter()
            .find(|tank| tank.resource == resource)?;
        (
            resource.capacity_kg - resource.amount_kg,
            resource.unit_mass_kg,
        )
    };
    (unit_mass > 0.0).then(|| (missing_kg.max(0.0) / unit_mass).floor() as u64)
}

pub(super) fn draw_dialog(
    ctx: &egui::Context,
    transfers: &mut Transfers,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    transfers.dragging = egui::DragAndDrop::payload::<DraggedCargo>(ctx).map(|cargo| cargo.source);
    let Some(pending) = &mut transfers.pending else {
        return;
    };

    let mut open = true;
    let mut finished = false;
    egui::Window::new("Transfer quantity")
        .id(egui::Id::new("cargo_quantity"))
        .collapsible(false)
        .resizable(false)
        .default_width(330.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let cargo = &pending.cargo;
            if let Some(stack) = available_stack(model, cargo.source, cargo.storage, &cargo.item) {
                ui.strong(&stack.name);
                let source = facility(model, cargo.source)
                    .map_or("Unavailable", |source| source.name.as_str());
                let target = facility(model, pending.target)
                    .map_or("Unavailable", |target| target.name.as_str());
                ui.label(format!("{source} → {target}"));
                let maximum = maximum(model, pending);
                pending.quantity = pending.quantity.min(maximum);
                quantity_editor(ui, &mut pending.quantity, stack, maximum);
            }

            let error = transfer_problem(model, pending);
            if let Some(error) = error {
                ui.colored_label(THREAT, error);
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(error.is_none(), egui::Button::new("Confirm transfer"))
                    .clicked()
                {
                    if let Some(error) = submit(model, pending, intents) {
                        transfers.feedback = Some((pending.target, error.into()));
                    }
                    finished = true;
                }
                finished |= ui.button("Cancel").clicked();
            });
        });
    if !open || finished {
        transfers.pending = None;
    }
}

fn quantity_editor(ui: &mut egui::Ui, quantity: &mut u64, stack: &CargoStack, maximum: u64) {
    ui.horizontal(|ui| {
        if matches!(stack.item, CargoItem::Resource(_)) && stack.unit_mass_kg < 1e-5 {
            let mut mass_kg = *quantity as f64 * stack.unit_mass_kg;
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
                *quantity = quantity_from_mass(mass_kg, stack.unit_mass_kg, maximum);
            }
        } else {
            ui.add(
                egui::DragValue::new(quantity)
                    .range(0..=maximum)
                    .suffix(" units"),
            );
        }
        if ui.small_button("All").clicked() {
            *quantity = maximum;
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
    storage: Storage,
    item: &CargoItem,
) -> Option<&'a CargoStack> {
    stacks(facility(model, source)?, storage)
        .iter()
        .find(|stack| &stack.item == item)
}

fn stacks(facility: &FacilityView, storage: Storage) -> &[CargoStack] {
    match storage {
        Storage::Cargo => &facility.items,
        Storage::Product => &facility.products,
    }
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
    storage: Storage,
) -> Option<&'static str> {
    if !source.can_transfer || !target.can_transfer {
        Some("Cargo transfer permission is required on both inventories")
    } else if !colocated(source, target)
        && !(storage == Storage::Product && source.entity == target.entity)
    {
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

pub(super) fn quantity_from_mass(mass_kg: f64, unit_mass_kg: f64, maximum: u64) -> u64 {
    if !mass_kg.is_finite() || !unit_mass_kg.is_finite() || unit_mass_kg <= 0.0 {
        return 0;
    }
    ((mass_kg.max(0.0) / unit_mass_kg).round() as u64).min(maximum)
}

fn cargo_grid(
    ui: &mut egui::Ui,
    state: &mut PaneState,
    facility: &FacilityView,
    storage: Storage,
    search: &str,
) {
    let columns = (ui.available_width() / 96.0).floor().max(1.0) as usize;
    egui::Grid::new(("cargo_grid", storage))
        .spacing(egui::vec2(6.0, 6.0))
        .show(ui, |ui| {
            let matching = stacks(facility, storage)
                .iter()
                .filter(|stack| stack.quantity > 0 && stack.name.to_lowercase().contains(search));

            for (index, stack) in matching.enumerate() {
                cargo_tile(ui, state, facility, stack, storage);
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}

fn cargo_tile(
    ui: &mut egui::Ui,
    state: &mut PaneState,
    source: &FacilityView,
    stack: &CargoStack,
    storage: Storage,
) {
    let selected = state
        .selected
        .as_ref()
        .is_some_and(|(selected_storage, item)| {
            *selected_storage == storage && item == &stack.item
        });
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
        state.selected = Some((storage, stack.item.clone()));
    }
    if source.can_transfer && available(stack) > 0 {
        response.dnd_set_drag_payload(DraggedCargo {
            source: source.entity,
            storage,
            item: stack.item.clone(),
        });
    }
    let location = match storage {
        Storage::Cargo => "Cargo hold",
        Storage::Product => "Installed product reservoir · unload into cargo",
    };
    response.on_hover_text(format!(
        "{}\n{location}\n{} available · {} reserved for jobs\n{:.1} kg · {:.3} m³",
        stack.name,
        available(stack),
        stack.reserved,
        stack.quantity as f64 * stack.unit_mass_kg,
        stack.quantity as f64 * stack.unit_volume_m3
    ));
}
