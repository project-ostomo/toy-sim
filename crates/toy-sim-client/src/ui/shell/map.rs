use super::*;
use std::collections::BTreeSet;
use toy_sim_model::ownership::Bloc;

mod canvas;
mod layout;
use layout::{ActiveRoute, Cache};

#[derive(Default)]
pub(super) struct State {
    selected: Option<Id>,
    pan: egui::Vec2,
    zoom: f32,
    preference: Option<travel::PlanningPreferences>,
    search: String,
    sovereignty: Option<Id>,
    cache: Cache,
    catalogue_hash: Option<[u8; 32]>,
    active: ActiveRoute,
    preview_key: Option<(Id, Id)>,
    preview: Option<Vec<Id>>,
    preview_gates: BTreeSet<Id>,
}

impl State {
    fn preview(&mut self, origin: Option<Id>) {
        let key = origin.zip(self.selected.or(origin));
        if self.preview_key != key {
            self.preview_key = key;
            self.preview = key.and_then(|(start, end)| self.cache.network.route(start, end));
            self.preview_gates = self.preview.iter().flatten().copied().collect();
        }
    }
}

fn polity_color(bloc: Option<Bloc>) -> egui::Color32 {
    match bloc {
        Some(Bloc::Union) => egui::Color32::from_rgb(119, 172, 239),
        Some(Bloc::League) => egui::Color32::from_rgb(107, 210, 165),
        Some(Bloc::NonAligned) => egui::Color32::from_rgb(223, 178, 105),
        None => MUTED,
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    if state.catalogue_hash != model.navigation_hash {
        state.catalogue_hash = model.navigation_hash;
        state.cache = Cache::default();
        state.active = ActiveRoute::default();
        state.preview_key = None;
        state.preview = None;
        state.preview_gates.clear();
        state.selected = None;
        state.pan = egui::Vec2::ZERO;
        state.zoom = 0.;
    }
    match model.navigation_status {
        NavigationStatus::Unavailable => {
            ui.weak("Waiting for the galactic catalogue.");
            return;
        }
        NavigationStatus::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Downloading galactic catalogue…");
            });
            return;
        }
        NavigationStatus::Failed(error) => {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "Galactic catalogue could not be loaded.",
            );
            ui.weak(error);
            if ui.button("Retry download").clicked() {
                intents.push(Intent::RetryNavigation);
            }
            return;
        }
        NavigationStatus::Ready => {}
    }
    let catalogue = model.navigation;
    if state.cache.update(catalogue) {
        state.preview_key = None;
        state.active = ActiveRoute::default();
        state.selected = state
            .selected
            .filter(|id| state.cache.systems.contains_key(id));
    }
    let preference = preferences(ui, state, model);
    instruments::fuel_budget(ui, model);
    ui.separator();

    let origin = model
        .ship
        .and_then(|ship| ship.pose.as_ref())
        .and_then(|pose| state.cache.network.nearest(pose.position))
        .or_else(|| {
            let travel::Presence::Docked { host, .. } = &model.ship?.presence else {
                return None;
            };
            state
                .cache
                .beacons
                .get(host)
                .map(|&index| catalogue.beacons[index].system)
        });
    let orders = model.ship.map_or(&[][..], |ship| {
        &ship.travel.orders[ship.travel.order.min(ship.travel.orders.len())..]
    });
    state.active.update(
        &state.cache,
        catalogue,
        &model.celestial_systems,
        origin,
        orders,
    );

    let mut focus = None;
    let mut fit = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("WORMHOLE NETWORK")
                .strong()
                .color(ACCENT),
        );
        ui.weak(format!("{} systems", catalogue.systems.len()));
        if ui.small_button("Fit").clicked() {
            fit = true;
        }
        if ui
            .add_enabled(origin.is_some(), egui::Button::new("My ship").small())
            .clicked()
        {
            focus = origin;
        }
    });
    search(ui, state, model, &mut focus);
    state.preview(origin);
    egui::Panel::bottom(ui.id().with("map_footer"))
        .frame(egui::Frame::NONE)
        .show_separator_line(false)
        .show(ui, |ui| {
            canvas::legend(ui);
            selected_system(ui, state, model, origin, preference, intents);
        });
    canvas::draw(ui, state, model, origin, focus, fit);
    state.preview(origin);
}

fn preferences(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
) -> travel::PlanningPreferences {
    let mut preference = state.preference.unwrap_or_else(|| {
        model
            .ship
            .map_or_else(Default::default, |ship| ship.travel.preferences)
    });
    ui.horizontal(|ui| {
        ui.label("Fuel priority");
        let response = ui.add(
            egui::Slider::new(&mut preference.fuel_priority, 0.1..=1000.)
                .logarithmic(true)
                .suffix("×"),
        );
        if response.changed() {
            state.preference = Some(preference);
        }
        response.on_hover_text("Higher priority spends longer coasting to save propulsion fuel. Used when you set a destination or add a waypoint.");
    });
    if let (Some(ship), Some(details)) = (model.ship, model.details) {
        let seconds = preference.cost(details.mass_kg).seconds_per_kg * 1000.;
        ui.small(format!(
            "Saving 1 t of propellant is worth {:.1} minutes",
            seconds / 60.
        ));
        if preference != ship.travel.preferences && !ship.travel.orders.is_empty() {
            ui.weak("Set destination again to replan with this preference.");
        }
    }
    preference
}

fn search(ui: &mut egui::Ui, state: &mut State, model: &FrameModel, focus: &mut Option<Id>) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text("Find a star system…")
                .desired_width(220.),
        );
        egui::ComboBox::from_id_salt("map-sovereignty")
            .selected_text(
                state
                    .sovereignty
                    .and_then(|id| model.society.directory.sovereignties.get(&id))
                    .map_or("All sovereignties", |s| s.name.as_str()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.sovereignty, None, "All sovereignties");
                for sovereignty in model.society.directory.sovereignties.values() {
                    ui.selectable_value(
                        &mut state.sovereignty,
                        Some(sovereignty.id),
                        &sovereignty.name,
                    );
                }
            });
        if !state.search.is_empty() && ui.small_button("Clear").clicked() {
            state.search.clear();
        }
    });
    if state.search.trim().is_empty() {
        return;
    }

    let matches = state
        .cache
        .search(model.navigation, &state.search, state.sovereignty);
    ui.weak(format!("{} matches", matches.len()));
    egui::ScrollArea::vertical()
        .id_salt("system-search-results")
        .max_height(108.)
        .show_rows(ui, 24., matches.len(), |ui, rows| {
            for row in rows {
                let system = &model.navigation.systems[matches[row]];
                let sovereignty = system
                    .sovereignty
                    .and_then(|id| model.society.directory.sovereignties.get(&id));
                let label = format!(
                    "{}  ·  {}",
                    system.name,
                    sovereignty.map_or("Unclaimed", |s| s.name.as_str())
                );
                if ui
                    .selectable_label(state.selected == Some(system.id), label)
                    .clicked()
                {
                    state.selected = Some(system.id);
                    *focus = Some(system.id);
                }
            }
        });
}

fn selected_system(
    ui: &mut egui::Ui,
    state: &State,
    model: &FrameModel,
    origin: Option<Id>,
    preference: travel::PlanningPreferences,
    intents: &mut Vec<Intent>,
) {
    let catalogue = model.navigation;
    let Some(system) = state
        .selected
        .or(origin)
        .and_then(|id| state.cache.systems.get(&id))
        .map(|&index| &catalogue.systems[index])
    else {
        ui.weak("Select a system or search by name to plan a route.");
        return;
    };
    let sovereignty = system
        .sovereignty
        .and_then(|id| model.society.directory.sovereignties.get(&id));
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&system.name).strong().size(16.));
        ui.colored_label(
            polity_color(sovereignty.map(|s| s.bloc)),
            sovereignty.map_or("Unclaimed", |s| s.name.as_str()),
        );
        ui.weak(format!("Population {}", population(system.population)));
    });
    ui.weak(state.preview.as_ref().map_or_else(
        || "No gate connection".into(),
        |route| {
            format!(
                "{} gate hops · computer compares fuel, time and slip",
                route.len()
            )
        },
    ));

    let destination = catalogue
        .beacons
        .iter()
        .find(|beacon| beacon.system == system.id && beacon.gate_exit.is_some())
        .map(|beacon| travel::Order::TravelTo(travel::Destination::Beacon(beacon.id)));
    ui.horizontal(|ui| {
        let available = model.connected && model.ship.is_some() && destination.is_some();
        if ui
            .add_enabled(available, egui::Button::new("Set destination"))
            .clicked()
        {
            intents.push(Intent::PlanRoute(
                destination.clone().into_iter().collect(),
                false,
                preference,
            ));
        }
        if ui
            .add_enabled(available, egui::Button::new("Add waypoint"))
            .clicked()
        {
            intents.push(Intent::PlanRoute(
                destination.clone().into_iter().collect(),
                true,
                preference,
            ));
        }
        let stations: Vec<_> = catalogue
            .beacons
            .iter()
            .filter(|beacon| beacon.system == system.id && beacon.docking)
            .collect();
        if !stations.is_empty() {
            ui.add_enabled_ui(available, |ui| {
                ui.menu_button("Dock at…", |ui| {
                    for station in stations {
                        if ui.button(&station.name).clicked() {
                            intents.push(Intent::PlanRoute(
                                vec![travel::Order::Dock(station.id)],
                                ui.input(|i| i.modifiers.shift),
                                preference,
                            ));
                            ui.close();
                        }
                    }
                });
            });
        }
    });
}

fn population(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.1} billion", value as f64 / 1e9)
    } else if value >= 1_000_000 {
        format!("{:.1} million", value as f64 / 1e6)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests;
