use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use toy_sim_model::{Action, ShipCommand};

use crate::state::{Outgoing, OwnedShip, ScreenPublication};

#[derive(Resource, Default)]
pub(super) struct MfdWindows {
    pub closed: std::collections::BTreeSet<(toy_sim_model::Id, u8)>,
}

pub(super) fn windows(
    mut contexts: EguiContexts,
    mut state: ResMut<MfdWindows>,
    mut outgoing: ResMut<Outgoing>,
    ships: Query<&OwnedShip>,
    screens: Query<&ScreenPublication>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    state.closed.retain(|(ship, slot)| {
        screens
            .iter()
            .any(|screen| screen.update.ship == *ship && screen.update.slot == *slot)
    });

    for publication in screens.iter() {
        let screen = &publication.update;
        if state.closed.contains(&(screen.ship, screen.slot)) {
            continue;
        }

        let Some(ship) = ships.iter().find(|ship| ship.0.ship == screen.ship) else {
            continue;
        };
        let definition = publication.definition.as_ref();
        let title = definition
            .map(|definition| definition.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| format!("MFD {}", screen.slot + 1));
        let mut open = true;

        egui::Window::new(format!("{title} · {}", screen.ship))
            .id(egui::Id::new(("mfd", screen.ship, screen.slot)))
            .default_width(512.)
            .open(&mut open)
            .show(ctx, |ui| {
                if let Some(definition) = definition {
                    let events = toy_sim_ship_view::screens::show_remote(
                        ui,
                        definition,
                        screen.frame.as_ref(),
                    );

                    for event in events {
                        outgoing.ship(
                            &ship.0,
                            ShipCommand::ScreenInput {
                                slot: screen.slot,
                                revision: screen.revision,
                                kind: event.kind as u8,
                                code: event.code,
                                modifiers: event.modifiers,
                                xy: [event.x, event.y],
                                text: event.text.as_str().unwrap_or_default().to_owned(),
                            },
                        );
                    }
                } else {
                    ui.label("Waiting for display definition…");
                }

                if let Some(error) = &screen.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            });

        if !open {
            state.closed.insert((screen.ship, screen.slot));
            outgoing.push(Action::ScreenUnsubscribe {
                ship: screen.ship,
                slot: screen.slot,
            });
        }
    }
    Ok(())
}
