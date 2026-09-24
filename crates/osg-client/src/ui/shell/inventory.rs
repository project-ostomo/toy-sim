use super::*;
use industry_model::{CargoItem, IndustryCommand};

#[derive(Default)]
pub(super) struct State {
    consumables: bool,
    cargo: cargo::PaneState,
}

impl State {
    #[cfg(test)]
    pub(super) fn show_consumables(&mut self) {
        self.consumables = true;
    }

    pub fn focus(&mut self) {
        *self = Self::default();
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    let Some(ship) = model.ship else {
        ui.weak("Select a ship to inspect its inventory.");
        return;
    };

    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.consumables, false, "Cargo hold");
        ui.selectable_value(&mut state.consumables, true, "Consumables");
    });
    if state.consumables {
        consumables(ui, model, transfers, intents);
    } else {
        cargo::draw(ui, &mut state.cargo, ship.ship, model, transfers, intents);
    }
}

fn consumables(
    ui: &mut egui::Ui,
    model: &FrameModel,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    let (Some(ship), Some(details)) = (model.ship, model.details) else {
        ui.weak("Waiting for installed tank telemetry…");
        return;
    };

    let height = ui.available_height().max(0.0);
    egui::ScrollArea::vertical()
        .id_salt("consumables")
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_height(height)
        .show(ui, |ui| {
            consumables_body(ui, model, ship, details, transfers, intents);
        });
}

fn consumables_body(
    ui: &mut egui::Ui,
    model: &FrameModel,
    ship: &ShipTelemetry,
    details: &ShipPresentation,
    transfers: &mut cargo::Transfers,
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
            reserve_color(fraction),
        );
    }

    ui.separator();
    ui.weak("Drop matching cargo onto a tank to refill it. Installed consumables cannot be transferred out.");
    cargo::feedback(ui, transfers, ship.ship);

    let reserve_capacity = details
        .health
        .as_ref()
        .map_or(0.0, |health| health.shield_reserve_capacity_kg);
    if reserve_capacity > 0.0 {
        tank(
            ui,
            "Shield reserve",
            "shield_coolant",
            ship.coolant_reserve_kg,
            reserve_capacity,
            &format!(
                "{:.1} / {:.1} kg",
                ship.coolant_reserve_kg, reserve_capacity
            ),
            model,
            ship.ship,
            transfers,
            intents,
        );
    }

    for resource in details
        .inventory
        .iter()
        .filter(|resource| resource.capacity_kg > 0.0 && !exportable_product(&resource.resource))
    {
        tank(
            ui,
            &resource.name,
            &resource.resource,
            resource.amount_kg,
            resource.capacity_kg,
            &format!(
                "{} units · {:.1} / {:.1} kg",
                resource.quantity, resource.amount_kg, resource.capacity_kg
            ),
            model,
            ship.ship,
            transfers,
            intents,
        );
    }
}

fn reserve_color(fraction: f64) -> egui::Color32 {
    if fraction <= 0.1 {
        THREAT
    } else if fraction <= 0.35 {
        WARNING
    } else {
        POSITIVE
    }
}

fn tank(
    ui: &mut egui::Ui,
    name: &str,
    resource: &str,
    amount: f64,
    capacity: f64,
    detail: &str,
    model: &FrameModel,
    ship: Id,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    ui.push_id(resource, |ui| {
        let response = ui.scope(|ui| {
            meter(
                ui,
                name,
                amount,
                capacity,
                detail,
                reserve_color(amount / capacity.max(f64::MIN_POSITIVE)),
            );
            refill_from_own_cargo(ui, model, ship, resource, intents);
        });
        let drop = ui.interact(
            response.response.rect.intersect(ui.clip_rect()),
            ui.id().with("tank_drop"),
            egui::Sense::hover(),
        );
        cargo::tank_drop(ui, drop, ship, resource, model, transfers, intents);
    });
    ui.add_space(5.0);
}

fn exportable_product(resource: &str) -> bool {
    static CATALOGUE: std::sync::OnceLock<osg_ships::Catalogue> = std::sync::OnceLock::new();
    CATALOGUE
        .get_or_init(osg_ships::Catalogue::builtin)
        .resources
        .iter()
        .any(|definition| definition.id == resource && definition.exportable_product)
}

fn refill_from_own_cargo(
    ui: &mut egui::Ui,
    model: &FrameModel,
    ship: Id,
    resource: &str,
    intents: &mut Vec<Intent>,
) {
    let item = CargoItem::Resource(resource.into());
    let inventory = cargo::facility(model, ship);
    let stack =
        inventory.and_then(|inventory| inventory.items.iter().find(|stack| stack.item == item));
    let quantity = stack
        .map_or(0, cargo::available)
        .min(cargo::tank_remaining(model, ship, resource).unwrap_or(0));
    let permitted = inventory.is_some_and(|inventory| inventory.can_transfer);

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                model.connected && permitted && quantity > 0,
                egui::Button::new("Fill from own cargo"),
            )
            .clicked()
        {
            intents.push(Intent::Industry(
                IndustryCommand::Refill {
                    source: ship,
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
            "Cargo transfer permission required".into()
        } else {
            let stack = stack.unwrap();
            format!(
                "Load {}",
                cargo::quantity_label(&item, quantity, stack.unit_mass_kg)
            )
        };
        ui.small(message);
    });
}
