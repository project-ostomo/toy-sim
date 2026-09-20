use super::*;
use osg_model::ownership::Bloc;
use std::collections::BTreeSet;

mod camera;
mod canvas;
mod layout;
mod planner;
mod spatial;
use layout::{ActiveRoute, Cache};

#[derive(Default)]
pub(super) struct State {
    selected: Option<Id>,
    camera: camera::Camera,
    preference: Option<travel::PlanningPreferences>,
    search: String,
    search_key: Option<(String, Option<Id>, bool, Option<[u8; 32]>)>,
    search_results: Vec<usize>,
    sovereignty: Option<Id>,
    inhabited_only: bool,
    cache: Cache,
    catalogue_hash: Option<[u8; 32]>,
    active: ActiveRoute,
    pub(super) route: planner::Preview,
    suggested: ActiveRoute,
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
        state.active = ActiveRoute::default();
        state.suggested = ActiveRoute::default();
    }
    match model.navigation_status {
        NavigationStatus::Unavailable => {
            ui.weak("Waiting for the inhabited-system directory.");
        }
        NavigationStatus::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Synchronizing inhabited systems…");
            });
        }
        NavigationStatus::Failed(error) => {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "Inhabited-system directory could not be loaded.",
            );
            ui.weak(error);
            if ui.button("Retry synchronization").clicked() {
                intents.push(Intent::RetryNavigation);
            }
        }
        NavigationStatus::Ready => {}
    }
    ui.set_min_height(680.);

    let catalogue = model.navigation;
    if state.cache.update(catalogue) {
        state.search_key = None;
        state.active = ActiveRoute::default();
        state.suggested = ActiveRoute::default();
        state.selected = state
            .selected
            .filter(|id| state.cache.systems.contains_key(id));
    }
    state.cache.update_inhabited(&model.inhabited);
    let preference = preferences(ui, state, model, intents);
    ui.separator();

    let origin = model
        .ship
        .and_then(|ship| ship.pose.as_ref())
        .and_then(|pose| state.cache.nearest(catalogue, pose.position))
        .or_else(|| {
            let travel::Presence::Docked { host, .. } = &model.ship?.presence else {
                return None;
            };
            state
                .cache
                .beacons
                .get(host)
                .and_then(|&index| catalogue.beacons[index].systems.first().copied())
        });
    let orders = model.ship.map_or(&[][..], |ship| {
        &ship.travel.orders[ship.travel.order.min(ship.travel.orders.len())..]
    });
    state.active.update(&state.cache, catalogue, origin, orders);

    let mut focus = None;
    let mut fit = false;
    let mut fit_route = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("GALACTIC MAP").strong().color(ACCENT));
        ui.weak(format!("{} systems", catalogue.systems.len()));
        ui.checkbox(&mut state.inhabited_only, "Inhabited only");
        if ui.small_button("Fit").clicked() {
            fit = true;
        }
        if ui
            .add_enabled(
                !state.active.systems.is_empty() || state.route.plan().is_some(),
                egui::Button::new("Fit route").small(),
            )
            .clicked()
        {
            fit_route = true;
        }
        if ui
            .add_enabled(origin.is_some(), egui::Button::new("My ship").small())
            .clicked()
        {
            focus = origin;
        }
    });
    search(ui, state, model, &mut focus);
    state.suggested.update(
        &state.cache,
        catalogue,
        origin,
        state
            .route
            .plan()
            .map_or(&[], |plan| plan.orders.as_slice()),
    );
    egui::Panel::bottom(ui.id().with("map_footer"))
        .exact_size(200.)
        .resizable(false)
        .frame(egui::Frame::NONE)
        .show_separator_line(false)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("map_route_details")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    canvas::legend(ui);
                    selected_system(ui, state, model, origin, preference, intents);
                    planner::draw(ui, &state.route, model, intents);
                    instruments::fuel_budget(ui, model);
                });
        });
    canvas::draw(ui, state, model, origin, focus, fit, fit_route);
}

fn preferences(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) -> travel::PlanningPreferences {
    let mut preference = state.preference.unwrap_or_else(|| {
        model
            .ship
            .map_or_else(Default::default, |ship| ship.travel.preferences)
    });
    let previous = preference;
    let mut percentage = preference.fuel_fraction * 100.;
    ui.add(egui::Slider::new(&mut percentage, 1. ..=100.).text("Fuel allowance").suffix("%"))
        .on_hover_text("Maximum estimated fuel use for the complete route, as a percentage of each remaining propulsion resource.");
    preference.fuel_fraction = percentage / 100.;
    ui.horizontal_wrapped(|ui| {
        ui.label("Maximum ship-destruction risk");
        ui.add(
            egui::DragValue::new(&mut preference.max_loss_ppm)
                .range(0. ..=1_000_000.)
                .speed(0.1)
                .max_decimals(6)
                .suffix(" ppm"),
        );
        ui.weak(risk_equivalent(preference.max_loss_ppm));
    });
    ui.add(
        egui::Slider::new(&mut preference.max_loss_ppm, 0. ..=1_000_000.)
            .logarithmic(true)
            .smallest_positive(0.001)
            .show_value(false),
    )
    .on_hover_text("Maximum estimated slip loss across the entire itinerary. Slowing cannot remove the dispersion floor. Beacon-assisted estimates assume guidance remains available.");
    ui.checkbox(&mut preference.allow_slipdrive, "Allow slipdrive");
    if preference != previous {
        state.preference = Some(preference);
        if let Some(action) = state.route.cancel_action() {
            intents.push(Intent::CancelRoute(action));
        }
    }
    preference
}

fn risk_equivalent(ppm: f64) -> String {
    if ppm <= 0. {
        "No modelled loss allowed".into()
    } else if ppm >= 1_000_000. {
        "No probability limit".into()
    } else {
        format!("1 in {:.0}", 1_000_000. / ppm)
    }
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
                    .and_then(|id| model.inhabited.sovereignties.get(&id))
                    .map_or("All sovereignties", |s| s.name.as_str()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.sovereignty, None, "All sovereignties");
                for sovereignty in model.inhabited.sovereignties.values() {
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

    let key = (
        state.search.trim().to_lowercase(),
        state.sovereignty,
        state.inhabited_only,
        model.navigation_hash,
    );
    if state.search_key.as_ref() != Some(&key) {
        state.search_results = state
            .cache
            .search(model.navigation, &key.0, state.sovereignty);
        if state.inhabited_only {
            state.search_results.retain(|&index| {
                model
                    .inhabited
                    .systems
                    .binary_search(&model.navigation.systems[index].id)
                    .is_ok()
            });
        }
        state.search_key = Some(key);
    }
    ui.weak(format!("{} matches", state.search_results.len()));
    egui::ScrollArea::vertical()
        .id_salt("system-search-results")
        .max_height(108.)
        .show_rows(ui, 24., state.search_results.len(), |ui, rows| {
            for row in rows {
                let system = &model.navigation.systems[state.search_results[row]];
                let sovereignty = system
                    .sovereignty
                    .and_then(|id| model.inhabited.sovereignties.get(&id));
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
        .and_then(|id| model.inhabited.sovereignties.get(&id));
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&system.name).strong().size(16.));
        ui.weak(
            if model.inhabited.systems.binary_search(&system.id).is_ok() {
                "Inhabited"
            } else {
                "No public directory transmitter"
            },
        );
        ui.colored_label(
            polity_color(sovereignty.map(|s| s.bloc)),
            sovereignty.map_or("Unclaimed", |s| s.name.as_str()),
        );
    });
    let destination = Some(travel::Order::TravelToSystem(system.id));
    ui.horizontal(|ui| {
        let available = model.connected && model.ship.is_some() && destination.is_some();
        if ui
            .add_enabled(available, egui::Button::new("Plan destination"))
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
            .filter(|beacon| beacon.systems.contains(&system.id) && beacon.docking)
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

#[cfg(test)]
mod tests;
