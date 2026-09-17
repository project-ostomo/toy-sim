use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use toy_sim_model::{Action, DebugCapability, DebugCommand, UniverseCatalogue};

use super::{FocusRequest, SelectedTarget, Selection};
use crate::assets::{self, StarCatalogue};
use crate::state::{Outgoing, OwnedShip, SessionInfo, SessionReset};

#[derive(Resource, Default)]
pub(super) struct Browser {
    expected: Option<[u8; 32]>,
    catalogue: Option<Handle<StarCatalogue>>,
    search: String,
    filtered: Option<String>,
    matches: Vec<usize>,
    page: usize,
}

impl Browser {
    fn filter(&mut self, catalogue: &UniverseCatalogue) {
        let search = self.search.trim().to_lowercase();
        if self.filtered.as_ref() == Some(&search) {
            return;
        }

        self.matches.clear();
        {
            self.matches.extend(
                catalogue
                    .systems
                    .iter()
                    .enumerate()
                    .filter(|(_, system)| {
                        search.is_empty()
                            || system.name.to_lowercase().contains(&search)
                            || system
                                .bodies
                                .iter()
                                .any(|body| body.name.to_lowercase().contains(&search))
                    })
                    .map(|(index, _)| index),
            );
        }

        self.filtered = Some(search);
        self.page = 0;
    }
}

pub(super) fn reset(_: On<SessionReset>, mut browser: ResMut<Browser>) {
    *browser = Browser::default();
}

pub(super) fn synchronize(
    mut browser: ResMut<Browser>,
    session: Res<SessionInfo>,
    server: Res<AssetServer>,
) {
    let expected = session.universe.as_ref().map(|status| status.catalogue);
    if browser.expected != expected {
        browser.expected = expected;
        browser.catalogue = expected.map(|hash| server.load(assets::path(hash)));
        browser.filtered = None;
        browser.matches.clear();
        browser.page = 0;
    }
}

pub(super) fn windows(
    mut contexts: EguiContexts,
    mut browser: ResMut<Browser>,
    mut outgoing: ResMut<Outgoing>,
    selection: Res<Selection>,
    session: Res<SessionInfo>,
    ships: Query<&OwnedShip>,
    assets: Res<AssetServer>,
    catalogues: Res<Assets<StarCatalogue>>,
    mut focus: MessageWriter<FocusRequest>,
) -> Result {
    let Some(status) = &session.universe else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let catalogue = browser
        .catalogue
        .as_ref()
        .and_then(|handle| catalogues.get(handle))
        .map(|asset| &asset.0);
    if let Some(catalogue) = catalogue {
        browser.filter(catalogue);
    }
    let can_inspect = session.capabilities.contains(&DebugCapability::Inspect);
    let can_relocate = session.capabilities.contains(&DebugCapability::Relocate);
    let focused_ship = selection
        .ship
        .filter(|id| ships.iter().any(|ship| ship.0.ship == *id));

    egui::Window::new("Universe")
        .default_pos(egui::pos2(10., 370.))
        .default_width(320.)
        .show(ctx, |ui| {
            let Some(catalogue) = catalogue else {
                if let Some(handle) = &browser.catalogue {
                    if let bevy::asset::LoadState::Failed(error) = assets.load_state(handle.id()) {
                        ui.colored_label(egui::Color32::LIGHT_RED, error.to_string());
                        if ui.button("Retry catalogue").clicked() {
                            assets.reload(assets::path(browser.expected.unwrap()));
                        }
                        return;
                    }
                }
                ui.label("Loading universe catalogue…");
                return;
            };

            ui.add(
                egui::TextEdit::singleline(&mut browser.search)
                    .hint_text("Search systems or bodies"),
            );
            browser.filter(catalogue);
            ui.label(format!(
                "{} systems · {} active · {} matching",
                catalogue.systems.len(),
                status.active_systems.len(),
                browser.matches.len()
            ));

            if status.inspected_body.is_some()
                && can_inspect
                && ui.button("End inspection").clicked()
            {
                outgoing.push(Action::Debug(DebugCommand::InspectBody { body: None }));
            }

            let pages = browser.matches.len().div_ceil(20).max(1);
            browser.page = browser.page.min(pages - 1);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(browser.page > 0, egui::Button::new("Previous"))
                    .clicked()
                {
                    browser.page -= 1;
                }
                ui.label(format!("{} / {}", browser.page + 1, pages));
                if ui
                    .add_enabled(browser.page + 1 < pages, egui::Button::new("Next"))
                    .clicked()
                {
                    browser.page += 1;
                }
            });

            egui::ScrollArea::vertical()
                .max_height(350.)
                .show(ui, |ui| {
                    for &index in browser.matches.iter().skip(browser.page * 20).take(20) {
                        let system = &catalogue.systems[index];
                        let reason = status
                            .active_systems
                            .iter()
                            .find(|active| active.system == system.id)
                            .map_or("inactive", |active| active.reason.as_str());

                        egui::CollapsingHeader::new(format!("{} ({reason})", system.name))
                            .id_salt(system.id)
                            .show(ui, |ui| {
                                ui.small(format!(
                                    "Influence: {:.0} AU",
                                    system.influence_radius_m / 1.495978707e11
                                ));
                                for body in &system.bodies {
                                    ui.horizontal(|ui| {
                                        let label = egui::Button::selectable(
                                            status.inspected_body == Some(body.id),
                                            &body.name,
                                        );
                                        if ui.add_enabled(can_inspect, label).clicked() {
                                            focus.write(FocusRequest {
                                                view: selection.view,
                                                target: SelectedTarget::Celestial(body.id),
                                            });
                                            outgoing.push(Action::Debug(
                                                DebugCommand::InspectBody {
                                                    body: Some(body.id),
                                                },
                                            ));
                                        }

                                        if body.kind != "star"
                                            && can_relocate
                                            && let Some(ship) = focused_ship
                                            && ui.small_button("Relocate ship").clicked()
                                        {
                                            outgoing.push(Action::Debug(
                                                DebugCommand::RelocateToBody {
                                                    ship,
                                                    body: body.id,
                                                },
                                            ));
                                        }
                                    });
                                }
                            });
                    }
                });
        });
    Ok(())
}
