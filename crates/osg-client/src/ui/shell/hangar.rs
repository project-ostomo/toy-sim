use super::*;

#[derive(Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Storage,
    Ships,
}

#[derive(Default, Resource)]
pub struct State {
    location: Option<(Id, Id)>,
    pub after: Option<Id>,
    previous: Vec<Option<Id>>,
    tab: Tab,
    cargo: cargo::PaneState,
}

impl State {
    #[cfg(test)]
    pub fn show_ships(&mut self) {
        self.tab = Tab::Ships;
    }

    fn sync(&mut self, model: &FrameModel) {
        let location = model.ship.map(|ship| {
            let host = match ship.presence {
                travel::Presence::Docked { host, .. } => host,
                _ => ship.ship,
            };
            (ship.ship, host)
        });
        if self.location != location {
            self.location = location;
            self.after = None;
            self.previous.clear();
            self.cargo = cargo::PaneState::default();
        }
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    state.sync(model);
    if model.ship.is_none() {
        ui.weak("Select a ship to use its hangar.");
        return;
    }
    if render_query(ui, &model.industry.hangar, |ui, hangar| {
        draw_hangar(ui, state, hangar, model, transfers, intents);
    }) {
        intents.push(Intent::RetryQueries);
    }
}

fn draw_hangar(
    ui: &mut egui::Ui,
    state: &mut State,
    hangar: &industry_model::HangarView,
    model: &FrameModel,
    transfers: &mut cargo::Transfers,
    intents: &mut Vec<Intent>,
) {
    if state.location != Some((hangar.ship, hangar.host)) {
        return;
    }
    ui.strong(&hangar.host_name);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.tab, Tab::Storage, "Station storage");
        ui.selectable_value(&mut state.tab, Tab::Ships, "Docked ships");
    });
    ui.separator();
    match state.tab {
        Tab::Storage => {
            if let Some(host) = &hangar.host_inventory {
                cargo::draw(ui, &mut state.cargo, host.entity, model, transfers, intents);
            } else {
                ui.weak("You do not have access to this station's storage.");
            }
        }
        Tab::Ships => {
            let ready = model.connected
                && model.industry.hangar_query.as_ref().is_some_and(|query| {
                    Some(query.ship) == model.ship.map(|ship| ship.ship)
                        && query.after == state.after
                });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        ready && !state.previous.is_empty(),
                        egui::Button::new("Previous"),
                    )
                    .clicked()
                {
                    state.after = state.previous.pop().flatten();
                }
                ui.label(format!("Page {}", state.previous.len() + 1));
                if ui
                    .add_enabled(ready && hangar.next.is_some(), egui::Button::new("Next"))
                    .clicked()
                {
                    state.previous.push(state.after);
                    state.after = hangar.next;
                }
                if !ready {
                    ui.spinner();
                }
            });
            let height = ui.available_height().max(0.0);
            egui::ScrollArea::vertical()
                .id_salt("hangar_ships")
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .max_height(height)
                .show(ui, |ui| {
                    if hangar.ships.is_empty() {
                        ui.weak("No accessible ships on this hangar page.");
                    }
                    for entry in &hangar.ships {
                        ui.push_id(entry.inventory.entity, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(Icon::Ship.text(24.0).color(ACCENT));
                                ui.vertical(|ui| {
                                    ui.strong(&entry.inventory.name);
                                    ui.small(society::name(
                                        &model.society.directory,
                                        entry.inventory.owner,
                                    ));
                                });
                            });
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        entry.can_open_inventory && ready,
                                        egui::Button::new("Open cargo"),
                                    )
                                    .clicked()
                                {
                                    intents.push(Intent::InspectInventory(entry.inventory.entity));
                                }
                                if model
                                    .ship
                                    .is_some_and(|ship| ship.ship == entry.inventory.entity)
                                {
                                    ui.weak("Active ship");
                                } else if ui
                                    .add_enabled(
                                        entry.can_focus && ready,
                                        egui::Button::new(if entry.can_control {
                                            "Activate"
                                        } else {
                                            "View ship"
                                        }),
                                    )
                                    .clicked()
                                {
                                    intents.push(Intent::FocusShip(entry.inventory.entity));
                                }
                            });
                            ui.separator();
                        });
                    }
                });
        }
    }
}
