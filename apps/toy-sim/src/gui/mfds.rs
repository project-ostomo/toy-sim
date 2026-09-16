use crate::vessel::{ControlledVessel, ShipSoftware};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

pub fn window(
    mut contexts: EguiContexts,
    mut ship: Single<(Entity, &mut ShipSoftware), With<ControlledVessel>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let (entity, software) = &mut *ship;
    software.screen_requests.clear();
    let definitions = software.controller.screens.clone();
    let mut events = Vec::new();
    for definition in definitions {
        egui::Window::new(definition.title.as_str().unwrap_or("Custom screen"))
            .id(egui::Id::new(("custom-screen", *entity, definition.id)))
            .default_width(512.)
            .show(ctx, |ui| {
                software.screen_requests.push(definition.id as u8);
                if software.controller.is_booting() {
                    ui.label("Flight computer rebooting");
                    return;
                }
                events.extend(toy_sim_ship_view::screens::show(
                    ui,
                    &definition,
                    software.mfds.get(&definition.id),
                ));
            });
    }
    for event in events {
        software.screen_event(event);
    }
    Ok(())
}
